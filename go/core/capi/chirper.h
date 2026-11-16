/*
 * Chirper core C ABI. Hand-maintained; must match go/core/capi/capi.go and docs/CONTRACT.md §B.
 * Strings are NUL-terminated UTF-8. Every call that returns an envelope returns a malloc'd string
 * that must be released with chirper_free.
 */
#ifndef CHIRPER_H
#define CHIRPER_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Event callback. kind and payload are valid only during the call. Called from one Go goroutine;
 * it must not block (copy the strings and enqueue them). */
typedef void (*chirper_event_cb)(void *ctx, const char *kind, const char *payload);

/* Opens the core under data_dir. key may be NULL/0 (no at-rest encryption) or exactly 32 bytes;
 * the key is copied. Returns a handle > 0, or 0 and *err_out set (free with chirper_free). */
int64_t chirper_open(const char *data_dir, const uint8_t *key, int32_t key_len,
                     chirper_event_cb cb, void *ctx, char **err_out);

/* Calls a facade method with a JSON object of arguments (at most 1 MiB). Returns a malloc'd
 * envelope: {"ok":true,"value":<json>} or {"ok":false,"error":"<message>"}. May block. Safe from
 * any thread. */
char *chirper_call(int64_t handle, const char *method, const char *args_json);

/* Frees a string returned by chirper_open or chirper_call. NULL is allowed. */
void chirper_free(char *p);

/* Stops the core and releases the handle. Idempotent. */
void chirper_close(int64_t handle);

/* Static "chirper-core/<version>" string. Do not free. */
const char *chirper_version(void);

#ifdef __cplusplus
}
#endif

#endif /* CHIRPER_H */
