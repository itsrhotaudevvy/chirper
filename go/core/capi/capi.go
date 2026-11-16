// Package main is the C ABI of the Chirper core: cgo exports over the facade. The contract is
// docs/CONTRACT.md section B, and the hand-written header is chirper.h next to this file.
package main

/*
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef void (*chirper_event_cb)(void *ctx, const char *kind, const char *payload);

// Go cannot call a C function pointer directly, so the callback goes through this helper.
static inline void chirper_emit(chirper_event_cb cb, void *ctx, const char *kind, const char *payload) {
	if (cb != NULL) {
		cb(ctx, kind, payload);
	}
}
*/
import "C"

import (
	"errors"
	"fmt"
	"sync"
	"unsafe"

	"chirper.dev/core/facade"
)

// coreVersion must match the version in the root Cargo.toml.
const coreVersion = "0.1.0"

// dbKeyLen is the only accepted key length.
const dbKeyLen = 32

// methodNameMax bounds the method string read from C.
const methodNameMax = 128

// eventRelay forwards facade events to the C callback. Each event is copied to C strings that are
// freed when the call returns. After close, events are dropped.
type eventRelay struct {
	mu     sync.Mutex
	cb     C.chirper_event_cb
	ctx    unsafe.Pointer
	closed bool
}

// OnEvent implements facade.EventListener.
func (r *eventRelay) OnEvent(kind, payload string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.closed {
		return
	}
	ckind := C.CString(kind)
	defer C.free(unsafe.Pointer(ckind))
	cpayload := C.CString(payload)
	defer C.free(unsafe.Pointer(cpayload))
	C.chirper_emit(r.cb, r.ctx, ckind, cpayload)
}

// close stops further events. It waits for an event that is being delivered.
func (r *eventRelay) close() {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.closed = true
}

// chirper_open opens the core under data_dir. See chirper.h.
//
//export chirper_open
func chirper_open(dataDir *C.char, key *C.uint8_t, keyLen C.int32_t, cb C.chirper_event_cb, ctx unsafe.Pointer, errOut **C.char) (handle C.int64_t) {
	defer func() {
		if r := recover(); r != nil {
			handle = 0
			setErr(errOut, fmt.Errorf("internal error: %v", r))
		}
	}()
	h, err := openCore(dataDir, key, keyLen, cb, ctx)
	if err != nil {
		setErr(errOut, err)
		return 0
	}
	return C.int64_t(h)
}

// openCore creates the facade core and registers it.
func openCore(dataDir *C.char, key *C.uint8_t, keyLen C.int32_t, cb C.chirper_event_cb, ctx unsafe.Pointer) (int64, error) {
	if dataDir == nil {
		return 0, errors.New("data_dir is NULL")
	}
	dbKey, err := copyKey(key, keyLen)
	if err != nil {
		return 0, err
	}
	// The engine keeps (and later zeroes) its own copy; ours is cleared as soon as it is handed over.
	defer clear(dbKey)
	relay := &eventRelay{cb: cb, ctx: ctx}
	core, err := facade.NewCoreWithKey(C.GoString(dataDir), dbKey, relay)
	if err != nil {
		return 0, err
	}
	return insert(&entry{core: core, listener: relay}), nil
}

// copyKey returns a Go copy of the 32-byte key, or nil when there is no key.
func copyKey(key *C.uint8_t, keyLen C.int32_t) ([]byte, error) {
	if err := checkKeyLen(int(keyLen)); err != nil {
		return nil, err
	}
	if keyLen == 0 {
		return nil, nil
	}
	if key == nil {
		return nil, errors.New("key is NULL but key_len is 32")
	}
	return C.GoBytes(unsafe.Pointer(key), dbKeyLen), nil
}

// checkKeyLen accepts 0 (no key) or exactly 32 bytes.
func checkKeyLen(n int) error {
	if n != 0 && n != dbKeyLen {
		return fmt.Errorf("key must be %d bytes, got %d", dbKeyLen, n)
	}
	return nil
}

// chirper_call runs a facade method and returns a malloc'd envelope. See chirper.h.
//
//export chirper_call
func chirper_call(handle C.int64_t, method *C.char, argsJSON *C.char) (out *C.char) {
	defer func() {
		if r := recover(); r != nil {
			out = C.CString(string(errorEnvelope(fmt.Errorf("internal error: %v", r))))
		}
	}()
	return C.CString(string(call(int64(handle), method, argsJSON)))
}

// call returns the envelope of one method call.
func call(handle int64, method *C.char, argsJSON *C.char) []byte {
	e, ok := lookup(handle)
	if !ok {
		return errorEnvelope(errors.New("unknown or closed handle"))
	}
	name, err := boundedBytes(method, methodNameMax)
	if err != nil {
		return errorEnvelope(fmt.Errorf("method: %w", err))
	}
	args, err := boundedBytes(argsJSON, maxArgsBytes)
	if err != nil {
		return errorEnvelope(fmt.Errorf("args_json: %w", err))
	}
	value, err := dispatch(e.core, string(name), args)
	if err != nil {
		return errorEnvelope(err)
	}
	return okEnvelope(value)
}

// boundedBytes copies a NUL-terminated C string of at most limit bytes. A NULL pointer is empty.
func boundedBytes(p *C.char, limit int) ([]byte, error) {
	if p == nil {
		return nil, nil
	}
	n := int(C.strnlen(p, C.size_t(limit+1)))
	if n > limit {
		return nil, fmt.Errorf("exceeds %d bytes", limit)
	}
	return C.GoBytes(unsafe.Pointer(p), C.int(n)), nil
}

// setErr stores err as a malloc'd message in errOut when errOut is set.
func setErr(errOut **C.char, err error) {
	if errOut != nil {
		*errOut = C.CString(err.Error())
	}
}

// chirper_free frees a string returned by chirper_call or chirper_open.
//
//export chirper_free
func chirper_free(p *C.char) {
	C.free(unsafe.Pointer(p))
}

// chirper_close stops the core and releases it. It is idempotent.
//
//export chirper_close
func chirper_close(handle C.int64_t) {
	defer func() { _ = recover() }()
	if e, ok := remove(int64(handle)); ok {
		closeCore(e)
	}
}

// closeCore stops the facade (which zeroes the engine's key copy) and silences the callback.
func closeCore(e *entry) {
	defer e.listener.close()
	e.core.Stop()
}

var (
	versionOnce sync.Once
	versionC    *C.char
)

// chirper_version returns a static "chirper-core/<version>" string. Do not free it.
//
//export chirper_version
func chirper_version() (out *C.char) {
	defer func() {
		if r := recover(); r != nil {
			out = nil
		}
	}()
	versionOnce.Do(func() { versionC = C.CString("chirper-core/" + coreVersion) })
	return versionC
}

// main is required for c-archive and c-shared builds.
func main() {}
