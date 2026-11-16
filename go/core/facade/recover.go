package facade

import (
	"encoding/json"
	"errors"
	"fmt"
)

// recoverErr converts a panic into an error. It is deferred by functions that do not own a Core.
func recoverErr(err *error) {
	if v := recover(); v != nil {
		*err = fmt.Errorf("internal error: %v", v)
	}
}

// recoverTo converts a panic into an error and reports it as a core_error event. The methods
// without an error result pass nil and only report.
func (c *Core) recoverTo(err *error) {
	v := recover()
	if v == nil {
		return
	}
	reported := c.report(v)
	if err != nil {
		*err = reported
	}
}

// report sends a core_error event for v and returns it as an error.
func (c *Core) report(v any) error {
	msg := fmt.Sprint(v)
	if e, ok := v.(error); ok {
		msg = e.Error()
	}
	if raw, err := json.Marshal(map[string]string{"message": msg}); err == nil {
		c.listener.OnEvent("core_error", string(raw))
	}
	return errors.New(msg)
}
