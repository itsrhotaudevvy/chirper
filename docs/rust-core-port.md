# Rust core port (branch `rust-core-port`)

The Go core has been removed. The core, the bridge runtime and every network driver are Rust.

## Decided

- The core (store, engine, vault, secrets, facade) and the bridge runtime are Rust. There is no Go
  runtime, no cgo and no C ABI. The UI talks to `chirper-core` through the same camelCase JSON
  contract as before ([CONTRACT.md](CONTRACT.md)), pinned by the golden fixtures in
  `crates/chirper-core/tests/golden/`.
- A driver speaks its network's protocol and reports domain events (`chirper_bridge::Event`) with
  remote ids. The runtime (`chirper-bridge/src/runtime.rs`) owns supervision, backoff, rate-limit
  waits, command pacing, capability gates and the mapping to local ids. Architecture:
  [ARCHITECTURE.md](ARCHITECTURE.md).
- On-disk compatibility: the `chirper.db` schema of the Go core (versions 1 to 5, same names), plus
  version 6 for remote ids, logins and backfill; the same derived ids; and the `OCV1` media vault.
- Our own protocol clients. No network client or bridge library is a dependency. Cryptography uses
  RustCrypto primitives and the `curve25519`, `ed25519` and `x25519` crates. Public schemas and tables
  are vendored as data with attribution.
- The default build ships five networks: WhatsApp, Telegram, Discord, Fluxer and SMS. The other eight
  drivers are behind `net-*` features of `chirper-core`, and `more-networks` turns all of them on. This
  is a product decision for now: the drivers stay in the repository and keep compiling.
- Drivers that need the operating system are provided by the host through `Options::host_networks`,
  which replaces the built-in entry with the same id. The Android SMS driver works this way.
- Networks with no working driver are listed as unavailable with a reason. They are not removed.
- `#![forbid(unsafe_code)]` in `chirper-core` and in most driver crates. Three crates do not set it
  yet (see Remaining work).

## Current state

The five networks of the default build report available. None has been tested against a live server.
The drivers' tests use fake servers and fixed vectors.

| Area | State |
|---|---|
| `crates/chirper-bridge` | Runtime, ids, domain model, driver traits, redaction, shared HTTP client. Done. Does not yet set `forbid(unsafe_code)`. |
| `crates/chirper-core` | Store (schema v1 to v6, SQLCipher or plain SQLite), content codec with a reader for the Go core's rows, engine and facade, `OCV1` vault, sealed per-login secrets, JSON model, network registry with the `net-*` and `more-networks` features. Tests: golden envelopes, end-to-end demo. Example: `smoke`. Done. |
| `crates/chirper-net-whatsapp` | Multi-device client: Noise XX handshake, binary nodes, protobuf, QR and pairing-code linking, receiving, sending, media, app-state sync, history sync. Uses `chirper-signal-protocol`. Reports available. Not tested live. |
| `crates/chirper-net-telegram` | MTProto client: TL codegen, obfuscated transport, auth keys, SRP two-step password, update stream, dialogs, messages, media, backfill. Logins `phone`, `qr` and `bot`. Credentials from the build or from Settings → Advanced (applies after a restart). Reports available when credentials are valid. Not tested live. |
| `crates/chirper-net-discord` | REST v10 and gateway with resume and rate-limit buckets. Logins `browser`, `qr`, `token` and `bot`. Reports available. Not tested live. |
| `crates/chirper-net-fluxer` | Instance discovery, REST with the 429 policy, gateway, ALTCHA proof of work. Logins `token`, `bot` and `password`. Reports available. Not tested live. Does not yet set `forbid(unsafe_code)`. |
| `crates/chirper-net-sms` | Android only, through the host platform. Reads, receives and sends plain text. Not verified on a device. |
| `crates/chirper-net-testnet` | Demo network for tests and demo mode. Done. Does not yet set `forbid(unsafe_code)`. |
| `crates/chirper-signal-protocol` | Signal protocol primitives: X3DH and PQXDH, Double Ratchet, sender keys, sealed sender v1, storage traits with an in-memory store. Interop vectors against the Go libsignal. Used by the WhatsApp driver. |
| Gated: `chirper-net-linkedin`, `chirper-net-gvoice`, `chirper-net-slack`, `chirper-net-bluesky` | Each reports available on this branch with its login flows (LinkedIn: cookies; Google Voice: cookies; Slack: email, token, app; Bluesky: password). Not in the default build. Not tested live. |
| Gated: `chirper-net-twitter` | Placeholder on this branch: unavailable, no login flow. On `wip/more-networks` it has cookies and password flows and reports available. |
| Gated: `chirper-net-signal`, `chirper-net-meta`, `chirper-net-gmessages` | Placeholders: unavailable, no login flow, on this branch and on `wip/more-networks`. |
| `src-tauri` (`chirper-app`) | Opens the core with the desktop key from the OS credential store, or with the key from the mobile plugin. Commands, `core-batch` events, app lock, preferences, notifications, tray, deep links, `chirper-media://`. Not verified on devices. |
| `plugins/chirper-native` | Kotlin and Swift code: key wrap, biometrics, secure-screen cover, connection service, SMS platform. Not verified on devices. |
| iMessage | Listed by the registry with a reason, in `more-networks` builds only. No driver. |

## Remaining work

1. Exercise the five networks of the default build against real accounts, and fix what the real
   services disagree with: WhatsApp (QR and pairing code, receive, send, media), Telegram (each login
   flow, then receive and send), Discord (each login flow, then send, edit, redact, reaction,
   backfill), Fluxer (each login flow), and SMS on an Android device. Until a driver has been run this
   way, its "available" status is unverified.
2. Telegram credentials from Settings → Advanced apply only after a restart. Decide whether the
   registry should rebuild on `set_settings`. The settings also hold the API hash in plain JSON in
   `settings.json` (owner-only file). Decide whether it moves to the secret store.
3. Gated networks: finish Signal, Messenger, Instagram and Google Messages, which are placeholders
   on both branches. Review the parked X and LinkedIn password drivers on `wip/more-networks`, and
   bring them to a state that can be tested live. Then decide whether they go into the default build.
4. Set `#![forbid(unsafe_code)]` in `chirper-bridge`, `chirper-net-testnet` and `chirper-net-fluxer`.
5. Decide whether drivers stay always compiled into `chirper-core` (as the gated features now allow
   for the eight), or whether the five become features too.
6. Decide on the webview policy. The Content Security Policy allows `https:` for images, media and
   connections ([SECURITY_MODEL.md](SECURITY_MODEL.md) section 5).
7. Decide whether iMessage gets a driver, or stays a listed entry with its reason.
8. Keep the docs in step with the code. The next change to a driver's status, login flows or
   capabilities should update [NETWORKS.md](NETWORKS.md) and the table in [README.md](../README.md)
   in the same pull request.
