/* Smoke test for the Go core C ABI. Opens a core in a temp dir, calls networks, frees, closes. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <unistd.h>

#include "chirper.h"

static int events = 0;

static void on_event(void *ctx, const char *kind, const char *payload) {
	(void)ctx;
	(void)payload;
	events++;
	printf("event: %s\n", kind);
}

int main(void) {
	char dir[] = "/tmp/chirper-smoke-XXXXXX";
	if (mkdtemp(dir) == NULL) {
		perror("mkdtemp");
		return 1;
	}

	char *err = NULL;
	int64_t h = chirper_open(dir, NULL, 0, on_event, NULL, &err);
	if (h == 0) {
		fprintf(stderr, "chirper_open failed: %s\n", err ? err : "(no message)");
		chirper_free(err);
		return 1;
	}
	printf("version: %s\n", chirper_version());

	char *env = chirper_call(h, "networks", "{}");
	if (env == NULL) {
		fprintf(stderr, "chirper_call returned NULL\n");
		chirper_close(h);
		return 1;
	}
	printf("networks envelope length: %zu\n", strlen(env));
	printf("networks envelope starts with: %.40s\n", env);

	chirper_free(env);

	char *bad = chirper_call(h, "noSuchMethod", "{}");
	printf("unknown method envelope: %s\n", bad);
	chirper_free(bad);

	chirper_close(h);
	chirper_close(h); /* idempotent */
	printf("events seen: %d\n", events);
	return 0;
}
