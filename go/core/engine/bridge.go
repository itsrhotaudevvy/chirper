package engine

import (
	"context"
	"fmt"
	"path/filepath"
	"sync"

	"go.mau.fi/util/dbutil"
	// Registers the "sqlite3-fk-wal" driver that the bridge databases open. The engine does not
	// rely on the store to link it in.
	_ "go.mau.fi/util/dbutil/litestream"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/bridgeconfig"
	"maunium.net/go/mautrix/bridgev2/commands"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/event"

	"chirper.dev/core/localmx"
	"chirper.dev/core/store"
)

// internalCommandPrefix is the bridgev2 command prefix. Chirper has no command rooms, so the
// prefix is set to something no chat message starts with; an empty prefix would match everything.
const internalCommandPrefix = "!ochat-cmd"

// bridgeInstance is one bridgev2.Bridge with its own bridge database and localmx connector.
type bridgeInstance struct {
	spec    networkSpec
	network bridgev2.NetworkConnector
	br      *bridgev2.Bridge
	db      *dbutil.Database

	startMu sync.Mutex
	started bool
}

// newBridgeInstance opens bridge-<network>.db and creates the bridge. Nothing is started yet.
func (e *Engine) newBridgeInstance(spec networkSpec) (*bridgeInstance, error) {
	s := e.currentSettings()
	if reason := spec.unavailableReason(s); reason != "" {
		return nil, fmt.Errorf("%s: %s", spec.id, reason)
	}
	network, err := connectorWithConfig(spec, s)
	if err != nil {
		return nil, fmt.Errorf("%s: %w", spec.id, err)
	}
	bridgeID := networkid.BridgeID(spec.id)
	log := e.log.With().Str("network", spec.id).Logger()
	path := filepath.Join(e.dataDir, "bridge-"+spec.id+".db")
	db, err := dbutil.NewFromConfig(spec.id, dbutil.Config{
		PoolConfig: dbutil.PoolConfig{
			Type:         "sqlite3-fk-wal",
			URI:          path + "?_txlock=immediate",
			MaxOpenConns: 1,
			MaxIdleConns: 1,
		},
	}, dbutil.ZeroLogger(log))
	if err != nil {
		return nil, fmt.Errorf("failed to open %s bridge database: %w", spec.id, err)
	}
	matrix := localmx.New(bridgeID, e.st, e.mediaDir, &bridgeSink{e: e, bridgeID: bridgeID})
	br := bridgev2.NewBridge(bridgeID, db, log, bridgeConfigFor(spec), matrix, network, commands.NewProcessor)
	return &bridgeInstance{spec: spec, network: network, br: br, db: db}, nil
}

// bridgeConfigFor is the bridgev2 configuration of every Chirper bridge.
func bridgeConfigFor(spec networkSpec) *bridgeconfig.BridgeConfig {
	block := bridgeconfig.PermissionLevelBlock
	admin := bridgeconfig.PermissionLevelAdmin
	return &bridgeconfig.BridgeConfig{
		CommandPrefix:         internalCommandPrefix,
		PrivateChatPortalMeta: true,
		PortalEventBuffer:     64,
		TagOnlyOnCreate:       true,
		OnlyBridgeTags:        []event.RoomTag{event.RoomTagFavourite, event.RoomTagLowPriority},
		MuteOnlyOnCreate:      true,
		Permissions: bridgeconfig.PermissionConfig{
			"*":              &block,
			store.ServerName: &admin,
		},
		Backfill: bridgeconfig.BackfillConfig{
			Enabled:              true,
			MaxInitialMessages:   200,
			MaxCatchupMessages:   1000,
			UnreadHoursThreshold: 720,
			Threads:              bridgeconfig.BackfillThreadsConfig{MaxInitialMessages: 50},
			Queue: bridgeconfig.BackfillQueueConfig{
				Enabled:    true,
				Manual:     true,
				BatchSize:  100,
				BatchDelay: 20,
				MaxBatches: 10,
			},
		},
	}
}

// start starts the bridge once: its connectors and the logins stored in its database.
func (i *bridgeInstance) start(ctx context.Context) error {
	i.startMu.Lock()
	defer i.startMu.Unlock()
	if i.started {
		return nil
	}
	if err := i.br.Start(ctx); err != nil {
		return fmt.Errorf("failed to start %s: %w", i.spec.id, err)
	}
	i.started = true
	return nil
}

// isStarted reports whether start has succeeded.
func (i *bridgeInstance) isStarted() bool {
	i.startMu.Lock()
	defer i.startMu.Unlock()
	return i.started
}

// hasLogins reports whether the bridge database holds at least one login.
func (i *bridgeInstance) hasLogins(ctx context.Context) (bool, error) {
	if err := i.br.DB.Upgrade(ctx); err != nil {
		return false, fmt.Errorf("failed to upgrade %s bridge database: %w", i.spec.id, err)
	}
	users, err := i.br.DB.UserLogin.GetAllUserIDsWithLogins(ctx)
	if err != nil {
		return false, fmt.Errorf("failed to read %s logins: %w", i.spec.id, err)
	}
	return len(users) > 0, nil
}

// close stops the bridge and closes its database.
func (i *bridgeInstance) close() {
	i.startMu.Lock()
	started := i.started
	i.startMu.Unlock()
	if started {
		i.br.Stop()
	}
	_ = i.db.Close()
}
