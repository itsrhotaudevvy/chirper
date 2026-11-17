# Rust core port (branch rust-core-port)

## Decided
- **Scope:** the core (engine, store, media, security, facade and the C ABI) moves to Rust.
  The protocol bridges (third_party/*, connectors/discord, connectors/fluxer) stay Go.
- **Boundary:** Rust hosts the app. The bridges are built as a Go c-archive/c-shared library and
  linked into the Rust binary, which works on iOS and Android (no sidecar processes).
  Go calls back into Rust through function pointers registered at start-up for storage and events.

## Current state
- `crates/chirper-core` is a thin FFI wrapper that loads the Go C ABI. It flips direction in this port:
  the Rust core becomes the owner and the Go side becomes the bridge library.
- The Go core (`go/core`, about 22.8k lines) is still the implementation. Nothing has been ported yet.

## Phases
1. Rust store: SQLCipher through rusqlite, with the same schema and migrations as `go/core/store`.
2. Rust security and keystore: `go/core/security`, `chirper-core/src/keystore.rs`.
3. Rust media: `go/core/media`.
4. Rust engine and facade: `go/core/engine`, `go/core/facade`, and the dispatch table in `go/core/capi`.
5. Go bridge library: `go/core/capi` is reduced to the bridge side, calling back into Rust for store and events.
6. Remove the Go core modules that are no longer used. Keep the bridges until each one is ported.
