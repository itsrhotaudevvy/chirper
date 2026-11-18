# Rust core port (branch rust-core-port)

## Decided
- **Scope:** everything moves to Rust. The core (store, security, media, engine, facade) *and* the
  network bridges. No Go in the shipped binary, no C ABI, no `libloading`.
- **Bridges:** the Mautrix bridgev2 bridges are replaced by one unified Rust bridge runtime
  (`chirper-bridge`) that keeps bridgev2's useful ideas (deterministic ids, idempotent writes,
  ghost/portal identity, per-login state machine, backfill, capability negotiation) without any
  Matrix concept. Each network becomes a driver crate.
- See `docs/RUST_BRIDGE_ARCHITECTURE.md` for the crate graph, traits, id model, storage layout,
  security model, driver plan and decision log.

## Current state
- The Rust crates are a thin FFI wrapper over the Go C ABI (`crates/chirper-core/src/ffi.rs`,
  `static-link`/`dynamic-link` features). They flip direction in this port: the Rust core becomes
  the implementation and the wrapper disappears.
- The Go core (`go/core`, about 22.8k lines) is still the implementation. Nothing has been ported
  yet.

## Phases
1. `chirper-bridge`: traits, runtime, deterministic ids, domain event model.
2. `chirper-store` (in `chirper-core` today): SQLCipher through rusqlite, schema v1..v5 verbatim,
   plus migration v6 that imports legacy `bridge-*.db` files.
3. `chirper-secure` and `chirper-media`: keystore, `OCV1` vault (format compatible), strip,
   blurhash, MIME.
4. Engine and facade: chats, messages, outbound, logins, settings, dispatcher.
5. Drivers: testnet, fluxer, discord; the rest stay listed as unavailable with a reason.
6. Delete `go/`, `third_party/pins.txt`, `scripts/build-go.sh`, `scripts/fetch-third-party.sh`, the
   Go CI job, and the cgo parts of the release workflow.
