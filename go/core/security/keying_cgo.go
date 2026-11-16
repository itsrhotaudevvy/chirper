//go:build sqlcipher

package security

/*
#define SQLITE_HAS_CODEC 1
#include <string.h>
#include <sqlite3.h>

// Open-time keying. SQLite runs the auto-extensions inside sqlite3_open_v2, before any statement, so
// the key is set before mattn/go-sqlite3 runs its own PRAGMAs. Those PRAGMAs read the schema, so a
// key set later, in a connect hook, comes too late: an existing encrypted file fails with "file is
// not a database". The open mode is thread-local. The Go side locks the OS thread around the open,
// so the open runs with the mode it set and no other connection on any thread sees it.
#define CHIRPER_KEY_BYTES 32
#define CHIRPER_DEFAULT_MODE 0
#define CHIRPER_PLAIN_MODE 1
#define CHIRPER_KEYED_MODE 2

static unsigned char chirper_default_key[CHIRPER_KEY_BYTES];
static int chirper_default_keyed = 0;
static __thread int chirper_open_mode = CHIRPER_DEFAULT_MODE;
static __thread unsigned char chirper_open_key[CHIRPER_KEY_BYTES];

static int chirper_autokey(sqlite3 *db, char **pzErr, const sqlite3_api_routines *api) {
	(void)pzErr;
	(void)api;
	switch (chirper_open_mode) {
	case CHIRPER_PLAIN_MODE:
		return SQLITE_OK;
	case CHIRPER_KEYED_MODE:
		return sqlite3_key(db, chirper_open_key, CHIRPER_KEY_BYTES);
	}
	if (chirper_default_keyed) {
		return sqlite3_key(db, chirper_default_key, CHIRPER_KEY_BYTES);
	}
	return SQLITE_OK;
}

static void chirper_register_autokey(void) {
	sqlite3_auto_extension((void (*)(void))chirper_autokey);
}

static void chirper_set_default_key(const unsigned char *key) {
	memcpy(chirper_default_key, key, CHIRPER_KEY_BYTES);
	chirper_default_keyed = 1;
}

static void chirper_set_open_mode(int mode, const unsigned char *key) {
	chirper_open_mode = mode;
	if (key != NULL) {
		memcpy(chirper_open_key, key, CHIRPER_KEY_BYTES);
	}
}
*/
import "C"

import (
	"database/sql/driver"
	"errors"
	"runtime"
	"unsafe"

	"github.com/mattn/go-sqlite3"
)

// The auto-extension is registered once per process. It does nothing until a key is set.
func init() {
	C.chirper_register_autokey()
}

// setDefaultKey sets the key that every connection opened without an explicit mode gets. key must
// be keyBytes long.
func setDefaultKey(key []byte) {
	C.chirper_set_default_key((*C.uchar)(unsafe.Pointer(&key[0])))
}

// openWith opens dsn through drv. plain opens the file without a key. A non-nil key opens it with
// that key. Otherwise the process default key applies.
func openWith(drv *sqlite3.SQLiteDriver, dsn string, plain bool, key []byte) (driver.Conn, error) {
	if !plain && key == nil {
		return drv.Open(dsn)
	}
	if !plain && len(key) != keyBytes {
		return nil, errors.New("security: database key must be 32 bytes")
	}
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	if plain {
		C.chirper_set_open_mode(C.CHIRPER_PLAIN_MODE, nil)
	} else {
		C.chirper_set_open_mode(C.CHIRPER_KEYED_MODE, (*C.uchar)(unsafe.Pointer(&key[0])))
	}
	defer C.chirper_set_open_mode(C.CHIRPER_DEFAULT_MODE, nil)
	return drv.Open(dsn)
}
