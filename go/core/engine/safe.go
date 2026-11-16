package engine

import "fmt"

// goSafe runs fn in a new goroutine. A panic in fn is reported as a core_error event instead of
// ending the host process, which is a Rust app and cannot survive a Go panic.
func (e *Engine) goSafe(fn func()) {
	go func() {
		defer e.recoverReport()
		fn()
	}()
}

// recoverReport must be deferred directly by the goroutine that may panic.
func (e *Engine) recoverReport() {
	if v := recover(); v != nil {
		e.reportError(fmt.Errorf("internal error: %v", v))
	}
}
