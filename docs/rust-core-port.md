# Rust core port (branch rust-core-port)

## Decided
- The core (store, engine, media, security, facade) and the bridge runtime are **all Rust**.
  There is no Go runtime, no cgo and no C ABI: the UI talks to `chirper-core` through the same
  camelCase JSON contract as before (`docs/CONTRACT.md` is unchanged).
- Drivers speak their own protocol and emit domain events (`chirper_bridge::Event`). The unified
  `chirper_bridge::Runtime` owns reconnection, backoff, session states, id derivation and the
  event stream. See `docs/RUST_BRIDGE_ARCHITECTURE.md`.
- On-disk compatibility is a hard requirement: the same `chirper.db` schema (v1–v5 verbatim, v6
  for remote ids and logins), the same deterministic ids, and the same `OCV1` media vault.
- `#![forbid(unsafe_code)]` in the core and in every driver.

## Current state
| Area | State |
| --- | --- |
| `crates/chirper-bridge` | Runtime, ids, model, network traits, secret/file plumbing. Done. |
| `crates/chirper-core` | Store (v1–v6), content codec (new + legacy Go reader), engine, facade, media vault. Done. |
| `crates/chirper-net-testnet` | In-app demo network used by tests and the UI build. Done. |
| `crates/chirper-net-discord` | REST v10 + gateway driver: login, chats, messages, edits, redactions, reactions, typing, backfill, attachments. Not yet exercised against a live account. |
| `src-tauri` | Migrated to `Core::open(Options)`; desktop DB key lives in the OS keyring (`src/keystore.rs`). |
| Fluxer | Not ported. Listed as unavailable with a reason, like Telegram, Signal, WhatsApp, Facebook, Instagram, iMessage and SMS. |
| Go tree (`go/`) | Still present, no longer linked by the app. Deleted once the golden fixtures are captured from it. |

## Remaining work
1. Capture golden fixtures from the Go implementation (`go/core/e2e_test.go` and the facade tests)
   into `crates/chirper-core/tests/golden/` so the wire contract and the derived ids stay pinned.
2. Exercise the Discord driver against a live account (login, send, edit, redact, reaction, backfill)
   and fix what the real API disagrees with.
3. Port Fluxer, then the remaining networks listed in `engine/networks.rs`.
4. Delete `go/`, `third_party/pins.txt`, `scripts/build-go.sh`, `scripts/fetch-third-party.sh` and the
   Go/cgo CI jobs; drop the `cbindgen`/C-header artefacts.
5. Refresh `ARCHITECTURE.md`, `BUILDING.md` and `NETWORKS.md` for the all-Rust layout.
