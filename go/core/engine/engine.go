// Package engine runs one bridgev2 bridge per network inside the app process and implements the
// Chirper operations (chats, messages, logins and outbound actions) on top of the local store.
package engine

import (
	"context"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sync"

	"github.com/rs/zerolog"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/bridgev2/status"
	"maunium.net/go/mautrix/id"

	"chirper.dev/core/security"
	"chirper.dev/core/store"
)

// DBKeyBytes is the length of the database key: 32 random bytes from the OS keystore.
const DBKeyBytes = 32

// Options configure an Engine.
type Options struct {
	// DataDir holds the databases, media, logs and settings. It is created with owner-only access.
	DataDir string
	// DBKey encrypts the databases at rest when the build has SQLCipher. It is nil for no
	// encryption, or exactly DBKeyBytes long. The engine keeps its own copy and zeroes it in Stop.
	DBKey []byte
}

// ErrNotRunning is returned for operations on a network whose bridge is not started.
var ErrNotRunning = errors.New("network is not connected")

// Engine is the Chirper core. Use New, then Start, and Stop when done.
type Engine struct {
	dataDir  string
	mediaDir string
	st       *store.Store
	log      zerolog.Logger
	logFile  io.Closer
	disp     *dispatcher
	ctx      context.Context
	cancel   context.CancelFunc

	settingsPath string
	settingsMu   sync.Mutex
	settings     Settings

	dbKey []byte

	mu        sync.Mutex
	instances map[networkid.BridgeID]*bridgeInstance
	states    map[accountKey]status.BridgeState
	typing    map[id.RoomID][]id.UserID
	// liveEvents are the events whose next message payload is live (see queueMessage).
	liveEvents map[id.EventID]bool

	pending   pendingOps
	logins    loginRegistry
	startOnce sync.Once
	stopOnce  sync.Once
}

type accountKey struct {
	network string
	loginID string
}

// New opens the store, the media directory and the settings under dataDir without encryption.
// Bridges are not started until Start.
func New(dataDir string, sink Listener) (*Engine, error) {
	return NewWithOptions(Options{DataDir: dataDir}, sink)
}

// NewWithOptions opens the engine described by opts. Bridges are not started until Start.
func NewWithOptions(opts Options, sink Listener) (*Engine, error) {
	if sink == nil {
		return nil, errors.New("engine: nil listener")
	}
	if opts.DBKey != nil && len(opts.DBKey) != DBKeyBytes {
		return nil, fmt.Errorf("engine: database key must be %d bytes or nil", DBKeyBytes)
	}
	dataDir := opts.DataDir
	if err := os.MkdirAll(dataDir, privateDirMode); err != nil {
		return nil, fmt.Errorf("failed to create data directory: %w", err)
	}
	log, logFile, err := openLog(dataDir)
	if err != nil {
		return nil, fmt.Errorf("failed to open log: %w", err)
	}
	if err := setupDatabaseKey(log, opts.DBKey); err != nil {
		_ = logFile.Close()
		return nil, err
	}
	mediaDir := filepath.Join(dataDir, "media")
	if err := os.MkdirAll(mediaDir, privateDirMode); err != nil {
		_ = logFile.Close()
		return nil, fmt.Errorf("failed to create media directory: %w", err)
	}
	settingsPath := filepath.Join(dataDir, "settings.json")
	settings, err := loadSettings(settingsPath)
	if err != nil {
		_ = logFile.Close()
		return nil, err
	}
	ctx, cancel := context.WithCancel(context.Background())
	st, err := store.Open(ctx, filepath.Join(dataDir, "chirper.db"))
	if err != nil {
		cancel()
		_ = logFile.Close()
		return nil, err
	}
	if err := tightenTree(dataDir); err != nil {
		cancel()
		_ = st.Close()
		_ = logFile.Close()
		return nil, fmt.Errorf("failed to secure data directory: %w", err)
	}
	e := &Engine{
		dataDir:      dataDir,
		mediaDir:     mediaDir,
		st:           st,
		log:          log,
		logFile:      logFile,
		disp:         newDispatcher(sink),
		ctx:          ctx,
		cancel:       cancel,
		settingsPath: settingsPath,
		settings:     settings,
		dbKey:        append([]byte(nil), opts.DBKey...),
		instances:    make(map[networkid.BridgeID]*bridgeInstance),
		states:       make(map[accountKey]status.BridgeState),
		typing:       make(map[id.RoomID][]id.UserID),
		liveEvents:   make(map[id.EventID]bool),
		pending:      pendingOps{ops: make(map[id.EventID]*undoOp)},
		logins:       loginRegistry{procs: make(map[string]*loginProcess)},
	}
	e.disp.onFail = e.reportError
	return e, nil
}

// setupDatabaseKey keys the SQLite driver when the build can encrypt and a key is given. Otherwise
// it logs the one warning of this start that the databases are not encrypted.
func setupDatabaseKey(log zerolog.Logger, key []byte) error {
	switch {
	case !security.EncryptionAvailable:
		log.Warn().Msg("databases are NOT encrypted: this build has no SQLCipher")
	case key == nil:
		log.Warn().Msg("databases are NOT encrypted: no database key was given")
	default:
		return security.RegisterEncryptedDriver(hex.EncodeToString(key))
	}
	return nil
}

// Start begins delivering events and starts the bridges that already have logins. Each bridge
// starts in its own goroutine, so Start returns quickly.
func (e *Engine) Start(ctx context.Context) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	if err := tightenTree(e.dataDir); err != nil {
		return fmt.Errorf("failed to secure data directory: %w", err)
	}
	e.startOnce.Do(func() {
		e.goSafe(e.disp.run)
		s := e.currentSettings()
		for _, spec := range e.enabledSpecs() {
			if spec.unavailableReason(s) != "" {
				continue
			}
			if _, err := os.Stat(e.bridgeDBPath(spec.id)); err != nil {
				continue
			}
			e.goSafe(func() { e.restore(spec) })
		}
	})
	return nil
}

// restore starts the bridge of spec when its database holds logins.
func (e *Engine) restore(spec networkSpec) {
	inst, err := e.instance(spec)
	if err != nil {
		e.reportError(err)
		return
	}
	has, err := inst.hasLogins(e.ctx)
	if err != nil {
		e.reportError(err)
		return
	}
	if !has {
		return
	}
	if err := inst.start(e.ctx); err != nil {
		e.reportError(err)
	}
}

// Stop stops every bridge, closes the databases and ends event delivery. Stop is idempotent.
func (e *Engine) Stop() {
	e.stopOnce.Do(func() {
		e.cancel()
		e.logins.closeAll()
		e.mu.Lock()
		instances := make([]*bridgeInstance, 0, len(e.instances))
		for _, inst := range e.instances {
			instances = append(instances, inst)
		}
		e.mu.Unlock()
		for _, inst := range instances {
			inst.close()
		}
		e.disp.stop()
		_ = e.st.Close()
		_ = e.logFile.Close()
		for i := range e.dbKey {
			e.dbKey[i] = 0
		}
		e.dbKey = nil
	})
}

func (e *Engine) bridgeDBPath(networkID string) string {
	return filepath.Join(e.dataDir, "bridge-"+networkID+".db")
}

// instance returns the bridge of spec, creating it (without starting it) on first use.
func (e *Engine) instance(spec networkSpec) (*bridgeInstance, error) {
	e.mu.Lock()
	defer e.mu.Unlock()
	if e.ctx.Err() != nil {
		// A background task that outlives Stop must not create a database file after it closed.
		return nil, errors.New("engine is stopped")
	}
	if inst, ok := e.instances[networkid.BridgeID(spec.id)]; ok {
		return inst, nil
	}
	inst, err := e.newBridgeInstance(spec)
	if err != nil {
		return nil, err
	}
	e.instances[networkid.BridgeID(spec.id)] = inst
	return inst, nil
}

// runningInstance returns the started bridge for networkID, or ErrNotRunning.
func (e *Engine) runningInstance(networkID string) (*bridgeInstance, error) {
	e.mu.Lock()
	inst, ok := e.instances[networkid.BridgeID(networkID)]
	e.mu.Unlock()
	if !ok || !inst.isStarted() {
		return nil, fmt.Errorf("%w: %s", ErrNotRunning, networkID)
	}
	return inst, nil
}

// availableSpec returns the registry row of networkID when it can be used now.
func (e *Engine) availableSpec(networkID string) (networkSpec, error) {
	s := e.currentSettings()
	for _, spec := range e.enabledSpecs() {
		if spec.id == networkID {
			if reason := spec.unavailableReason(s); reason != "" {
				return spec, errors.New(reason)
			}
			return spec, nil
		}
	}
	return networkSpec{}, fmt.Errorf("unknown or disabled network %q", networkID)
}

// enabledSpecs lists the registry rows that apply to the current settings, including unavailable
// ones (callers check unavailableReason).
func (e *Engine) enabledSpecs() []networkSpec {
	e.settingsMu.Lock()
	demo := e.settings.DemoNetwork
	e.settingsMu.Unlock()
	var out []networkSpec
	for _, spec := range networkSpecs() {
		if spec.demo && !demo {
			continue
		}
		out = append(out, spec)
	}
	return out
}

// Networks lists every network of the current settings. Unavailable networks are listed too, with
// the reason, so that the app can explain what is missing.
func (e *Engine) Networks() []Network {
	s := e.currentSettings()
	out := []Network{}
	for _, spec := range e.enabledSpecs() {
		row, ok := e.networkRow(spec, s)
		if ok {
			out = append(out, row)
		}
	}
	return out
}

// networkRow describes spec. It reports false when the connector of an available network fails to load.
func (e *Engine) networkRow(spec networkSpec, s Settings) (Network, bool) {
	row := Network{
		ID: spec.id, Name: spec.name, Color: spec.color, Beta: !spec.demo,
		LoginFlows: []LoginFlow{}, Capabilities: spec.caps,
	}
	if row.Reason = spec.unavailableReason(s); row.Reason != "" {
		return row, true
	}
	conn, err := connectorWithConfig(spec, s)
	if err != nil {
		e.reportError(err)
		return Network{}, false
	}
	row.Name = conn.GetName().DisplayName
	row.Available = true
	for _, f := range conn.GetLoginFlows() {
		row.LoginFlows = append(row.LoginFlows, LoginFlow{ID: f.ID, Name: f.Name, Description: f.Description})
	}
	return row, true
}

// Accounts lists the logins of every started bridge.
func (e *Engine) Accounts() ([]Account, error) {
	e.mu.Lock()
	all := make([]*bridgeInstance, 0, len(e.instances))
	for _, inst := range e.instances {
		all = append(all, inst)
	}
	e.mu.Unlock()
	instances := make([]*bridgeInstance, 0, len(all))
	for _, inst := range all {
		if inst.isStarted() {
			instances = append(instances, inst)
		}
	}
	out := []Account{}
	for _, inst := range instances {
		user, err := inst.br.GetUserByMXID(e.ctx, store.SelfUserID)
		if err != nil {
			return nil, fmt.Errorf("failed to load user: %w", err)
		}
		for _, login := range user.GetUserLogins() {
			out = append(out, e.accountOf(inst.spec.id, login, string(login.ID), e.lastState(inst.spec.id, string(login.ID))))
		}
	}
	return out, nil
}

// lastState returns the last bridge state reported for a login, or an empty state.
func (e *Engine) lastState(network, loginID string) status.BridgeState {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.states[accountKey{network: network, loginID: loginID}]
}

// accountState maps a bridge state to the account state of ARCHITECTURE.md.
func accountState(ev status.BridgeStateEvent) string {
	switch ev {
	case "":
		return string(status.StateConnecting)
	case status.StateBackfilling, status.StateRunning:
		return string(status.StateConnected)
	case status.StateStarting:
		return string(status.StateConnecting)
	default:
		return string(ev)
	}
}

// Logout logs the login out remotely, deletes it in the bridge, and erases its chats from the store:
// rooms, messages, reactions, members and media, with the media files. The bridge deletes first, and
// blocks until its own cleanup of the login is done.
func (e *Engine) Logout(networkID, loginID string) error {
	inst, err := e.runningInstance(networkID)
	if err != nil {
		return err
	}
	login, err := inst.br.GetExistingUserLoginByID(e.ctx, networkid.UserLoginID(loginID))
	if err != nil {
		return fmt.Errorf("failed to find login: %w", err)
	}
	if login == nil {
		return fmt.Errorf("unknown login %q on %s", loginID, networkID)
	}
	// The bridge forgets the login's portals when it deletes the login, so they are read first.
	portalRooms, err := portalRoomsOf(e.ctx, inst.br, login)
	if err != nil {
		return err
	}
	login.Delete(e.ctx, status.BridgeState{StateEvent: status.StateLoggedOut},
		bridgev2.DeleteOpts{LogoutRemote: true, BlockingCleanup: true})
	purged, err := e.st.PurgeLogin(e.ctx, networkID, loginID, portalRooms)
	for _, roomID := range purged {
		e.queueChat(roomID)
	}
	if err != nil {
		return fmt.Errorf("failed to erase login data: %w", err)
	}
	return nil
}

// portalRoomsOf returns the Matrix rooms of the portals that login is in, read from the bridge database.
func portalRoomsOf(ctx context.Context, br *bridgev2.Bridge, login *bridgev2.UserLogin) ([]id.RoomID, error) {
	links, err := br.DB.UserPortal.GetAllForLogin(ctx, login.UserLogin)
	if err != nil {
		return nil, fmt.Errorf("failed to list login portals: %w", err)
	}
	var rooms []id.RoomID
	for _, link := range links {
		portal, err := br.GetExistingPortalByKey(ctx, link.Portal)
		if err != nil {
			return nil, fmt.Errorf("failed to load portal: %w", err)
		}
		if portal != nil && portal.MXID != "" {
			rooms = append(rooms, portal.MXID)
		}
	}
	return rooms, nil
}

// reportError logs err and sends a core_error event. Errors caused by Stop are not reported.
func (e *Engine) reportError(err error) {
	if e.ctx.Err() != nil {
		return
	}
	e.log.Error().Err(err).Msg("Core error")
	e.disp.emit("core_error", mustJSON(map[string]string{"message": err.Error()}))
}

func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if v != "" {
			return v
		}
	}
	return ""
}
