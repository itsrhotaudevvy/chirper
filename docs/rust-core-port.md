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
- **Our own protocol clients.** Every driver speaks its network's protocol with code in this
  repository: no client or bridge library (grammers, whatsapp-rust, libsignal, serenity, …) is a
  dependency. Vetted primitives are used for all cryptography (RustCrypto `aes`/`sha2`/`hkdf`,
  `x25519`/`curve25519`/`ed25519-dalek`, `num-bigint`) and for codecs (`serde`, `prost` derive,
  `flate2`); never a hand-rolled cipher or hash. Public schemas and protocol tables (Telegram TL,
  WhatsApp protobuf field numbers and token dictionaries) are vendored as data with attribution.
- Drivers that need the operating system are provided by the host through
  `Options::host_networks` and replace the built-in entry with the same id (Android SMS).
- Priority networks: SMS, WhatsApp, Telegram, Discord, Fluxer.

## Current state
| Area | State |
| --- | --- |
| `crates/chirper-bridge` | Runtime, ids, model, network traits, secret/file plumbing. Done. |
| `crates/chirper-core` | Store (v1–v6), content codec (new + legacy Go reader), engine, facade, media vault. Done. |
| `crates/chirper-net-testnet` | In-app demo network used by tests and the UI build. Done. |
| `crates/chirper-net-discord` | REST v10 + gateway driver: token and QR login, resume, rate-limit buckets, chats/threads, messages, edits, redactions, reactions, typing, backfill, attachments, embeds, stickers. Not yet exercised against a live account. |
| `crates/chirper-net-fluxer` | Port of our Go Fluxer connector: discovery, REST, gateway with resume, token/bot/password login. |
| `crates/chirper-net-telegram` | Phase 1: own TL codegen, MTProto 2.0 crypto, obfuscated transport, auth-key generation, encrypted session. Phase 2: login (code, 2FA SRP, QR), updates, dialogs, messaging, files. Needs an api_id at build time. |
| `crates/chirper-net-whatsapp` | Phase 1: Noise XX handshake, binary node codec, protobuf, XEdDSA, QR pairing. Phase 2: Signal sessions, prekeys, messaging, groups (sender keys), media, app-state and history sync. |
| `crates/chirper-net-sms` | Android only, through the phone's SMS provider (host driver). Not the default SMS app: reads, receives and sends SMS; MMS is received, not sent. |
| `src-tauri` | Migrated to `Core::open(Options)`; desktop DB key lives in the OS keyring (`src/keystore.rs`). |
| Unported | Signal, Facebook, Instagram, iMessage: listed as unavailable with a reason. |
| Go tree (`go/`) | Still present, no longer linked by the app. Deleted once the golden fixtures are captured from it. |

## Remaining work
1. Capture golden fixtures from the Go implementation (`go/core/e2e_test.go` and the facade tests)
   into `crates/chirper-core/tests/golden/` so the wire contract and the derived ids stay pinned.
2. Exercise the Discord driver against a live account (login, send, edit, redact, reaction, backfill)
   and fix what the real API disagrees with.
3. Telegram phase 2 and WhatsApp phase 2 (see the table above); exercise Fluxer and SMS on real
   accounts/devices.
4. Delete `go/`, `third_party/pins.txt`, `scripts/build-go.sh`, `scripts/fetch-third-party.sh` and the
   Go/cgo CI jobs; drop the `cbindgen`/C-header artefacts.
5. Refresh `ARCHITECTURE.md`, `BUILDING.md` and `NETWORKS.md` for the all-Rust layout.
