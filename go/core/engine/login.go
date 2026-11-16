package engine

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"time"

	"github.com/google/uuid"
	"maunium.net/go/mautrix/bridgev2"

	"chirper.dev/core/store"
)

// loginTimeout is how long a login process may wait for the user before it is abandoned.
const loginTimeout = 30 * time.Minute

var (
	errLoginFinished = errors.New("login process is finished")
	errLoginTimedOut = errors.New("login process timed out or was cancelled")
)

// stepFunc advances a login process by one step.
type stepFunc func(context.Context) (*bridgev2.LoginStep, error)

// loginProcess drives one bridgev2.LoginProcess. A step runs in its own goroutine; callers wait
// for it, and a second call while a step runs joins that step instead of starting another, as the
// provisioning API does.
type loginProcess struct {
	e       *Engine
	id      string
	network string
	proc    bridgev2.LoginProcess
	ctx     context.Context
	cancel  context.CancelFunc

	mu       sync.Mutex
	next     *bridgev2.LoginStep
	pending  chan struct{}
	err      error
	finished bool
}

// loginRegistry holds the login processes that are not finished.
type loginRegistry struct {
	mu    sync.Mutex
	procs map[string]*loginProcess
}

func (r *loginRegistry) add(lp *loginProcess) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.procs[lp.id] = lp
}

func (r *loginRegistry) get(id string) (*loginProcess, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	lp, ok := r.procs[id]
	if !ok {
		return nil, fmt.Errorf("unknown login process %q", id)
	}
	return lp, nil
}

func (r *loginRegistry) remove(id string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	delete(r.procs, id)
}

// closeAll cancels every unfinished login process.
func (r *loginRegistry) closeAll() {
	r.mu.Lock()
	procs := make([]*loginProcess, 0, len(r.procs))
	for _, lp := range r.procs {
		procs = append(procs, lp)
	}
	r.mu.Unlock()
	for _, lp := range procs {
		lp.close(true)
	}
}

// newLoginProcess registers a login process that expires after loginTimeout.
func (e *Engine) newLoginProcess(network string, proc bridgev2.LoginProcess) *loginProcess {
	ctx, cancel := context.WithTimeout(e.ctx, loginTimeout)
	lp := &loginProcess{e: e, id: uuid.NewString(), network: network, proc: proc, ctx: ctx, cancel: cancel}
	e.logins.add(lp)
	e.goSafe(func() {
		<-ctx.Done()
		lp.close(true)
	})
	return lp
}

// close ends the process. The bridgev2 process is cancelled unless it completed.
func (lp *loginProcess) close(cancelProc bool) {
	lp.mu.Lock()
	if lp.finished {
		lp.mu.Unlock()
		return
	}
	lp.finished = true
	lp.mu.Unlock()
	if cancelProc {
		lp.proc.Cancel()
	}
	lp.cancel()
	lp.e.logins.remove(lp.id)
}

// submit runs fn as the next step unless one is already running. want, when set, is the step type
// the process must be waiting on. It returns the step that results.
func (lp *loginProcess) submit(want bridgev2.LoginStepType, fn stepFunc) (LoginState, error) {
	lp.mu.Lock()
	if lp.finished {
		lp.mu.Unlock()
		return LoginState{}, errLoginFinished
	}
	if lp.pending == nil {
		if want != "" && (lp.next == nil || lp.next.Type != want) {
			lp.mu.Unlock()
			return LoginState{}, fmt.Errorf("login is not waiting for a %s step", want)
		}
		done := make(chan struct{})
		lp.pending = done
		lp.e.goSafe(func() { lp.execute(done, fn) })
	}
	done := lp.pending
	lp.mu.Unlock()
	select {
	case <-done:
	case <-lp.ctx.Done():
		return LoginState{}, errLoginTimedOut
	}
	lp.mu.Lock()
	defer lp.mu.Unlock()
	if lp.err != nil {
		return LoginState{}, lp.err
	}
	return lp.stateLocked(), nil
}

// execute runs one step and records its outcome. A failed or completed process is closed.
func (lp *loginProcess) execute(done chan struct{}, fn stepFunc) {
	step, err := lp.runStep(fn)
	lp.mu.Lock()
	lp.pending = nil
	lp.err = err
	if err == nil {
		lp.next = step
	}
	lp.mu.Unlock()
	close(done)
	switch {
	case err != nil:
		lp.close(true)
	case step.Type == bridgev2.LoginStepTypeComplete:
		lp.close(false)
		// Pull in the history of every chat of this network so the inbox is not empty after signing in.
		lp.e.goSafe(func() { _ = lp.e.SyncHistory(lp.network) })
	}
}

// runStep calls fn and checks the step it returns. Panics become errors.
func (lp *loginProcess) runStep(fn stepFunc) (step *bridgev2.LoginStep, err error) {
	defer func() {
		if v := recover(); v != nil {
			step, err = nil, fmt.Errorf("login step failed unexpectedly: %v", v)
		}
	}()
	step, err = fn(lp.ctx)
	if err != nil {
		return nil, err
	}
	if step == nil {
		return nil, errors.New("login returned no step")
	}
	switch step.Type {
	case bridgev2.LoginStepTypeUserInput, bridgev2.LoginStepTypeCookies,
		bridgev2.LoginStepTypeDisplayAndWait, bridgev2.LoginStepTypeComplete:
		return step, nil
	}
	return nil, fmt.Errorf("not supported yet: login step %q", step.Type)
}

// stateLocked is the LoginState of the process. The caller must hold lp.mu.
func (lp *loginProcess) stateLocked() LoginState {
	return LoginState{ProcessID: lp.id, Step: stepJSON(lp.next)}
}

// stepJSON converts a bridgev2 login step to the JSON LoginStep.
func stepJSON(s *bridgev2.LoginStep) LoginStep {
	out := LoginStep{Type: string(s.Type), StepID: s.StepID, Instructions: s.Instructions}
	if p := s.UserInputParams; p != nil {
		fields := make([]InputField, 0, len(p.Fields))
		for _, f := range p.Fields {
			fields = append(fields, InputField{
				ID: f.ID, Name: f.Name, Type: string(f.Type), Description: f.Description,
				Options: nonNilStrings(f.Options), Pattern: f.Pattern,
			})
		}
		out.UserInput = &UserInputStep{Fields: fields}
	}
	if p := s.CookiesParams; p != nil {
		fields := make([]CookieField, 0, len(p.Fields))
		for _, f := range p.Fields {
			sources := make([]CookieSource, 0, len(f.Sources))
			for _, src := range f.Sources {
				sources = append(sources, CookieSource{Type: string(src.Type), Name: src.Name, CookieDomain: src.CookieDomain})
			}
			fields = append(fields, CookieField{ID: f.ID, Required: f.Required, Sources: sources})
		}
		out.Cookies = &CookiesStep{URL: p.URL, UserAgent: p.UserAgent, Fields: fields, ExtractJS: p.ExtractJS}
	}
	if p := s.DisplayAndWaitParams; p != nil {
		out.DisplayAndWait = &DisplayStep{Type: string(p.Type), Data: p.Data, ImageURL: p.ImageURL}
	}
	if p := s.CompleteParams; p != nil {
		out.Complete = &CompleteStep{LoginID: string(p.UserLoginID)}
	}
	return out
}

func nonNilStrings(v []string) []string {
	if v == nil {
		return []string{}
	}
	return v
}

// StartLogin starts a login flow of networkID and returns its first step.
func (e *Engine) StartLogin(networkID, flowID string) (LoginState, error) {
	spec, err := e.availableSpec(networkID)
	if err != nil {
		return LoginState{}, err
	}
	inst, err := e.instance(spec)
	if err != nil {
		return LoginState{}, err
	}
	if err := inst.start(e.ctx); err != nil {
		return LoginState{}, err
	}
	user, err := inst.br.GetUserByMXID(e.ctx, store.SelfUserID)
	if err != nil {
		return LoginState{}, fmt.Errorf("failed to load user: %w", err)
	}
	proc, err := inst.network.CreateLogin(e.ctx, user, flowID)
	if err != nil {
		return LoginState{}, fmt.Errorf("failed to create login: %w", err)
	}
	lp := e.newLoginProcess(networkID, proc)
	return lp.submit("", proc.Start)
}

// SubmitLoginInput submits the values of a user input step.
func (e *Engine) SubmitLoginInput(processID string, values map[string]string) (LoginState, error) {
	lp, err := e.logins.get(processID)
	if err != nil {
		return LoginState{}, err
	}
	in, ok := lp.proc.(bridgev2.LoginProcessUserInput)
	if !ok {
		return LoginState{}, errors.New("this login does not take typed input")
	}
	return lp.submit(bridgev2.LoginStepTypeUserInput, func(ctx context.Context) (*bridgev2.LoginStep, error) {
		return in.SubmitUserInput(ctx, values)
	})
}

// SubmitLoginCookies submits the cookies collected from the login web page.
func (e *Engine) SubmitLoginCookies(processID string, cookies map[string]string) (LoginState, error) {
	lp, err := e.logins.get(processID)
	if err != nil {
		return LoginState{}, err
	}
	ck, ok := lp.proc.(bridgev2.LoginProcessCookies)
	if !ok {
		return LoginState{}, errors.New("this login does not take cookies")
	}
	return lp.submit(bridgev2.LoginStepTypeCookies, func(ctx context.Context) (*bridgev2.LoginStep, error) {
		return ck.SubmitCookies(ctx, cookies)
	})
}

// WaitLoginStep blocks until the display-and-wait step of the login (a QR code scan, say) finishes
// and returns the step that follows.
func (e *Engine) WaitLoginStep(processID string) (LoginState, error) {
	lp, err := e.logins.get(processID)
	if err != nil {
		return LoginState{}, err
	}
	w, ok := lp.proc.(bridgev2.LoginProcessDisplayAndWait)
	if !ok {
		return LoginState{}, errors.New("this login has no step to wait for")
	}
	return lp.submit(bridgev2.LoginStepTypeDisplayAndWait, w.Wait)
}

// CancelLogin abandons a login process.
func (e *Engine) CancelLogin(processID string) {
	if lp, err := e.logins.get(processID); err == nil {
		lp.close(true)
	}
}
