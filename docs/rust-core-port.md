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
  RustCrypto primitives and `curve25519`/`ed25519`/`x25519` crates. Public schemas and tables are
  vendored as data with attribution.
- Drivers that need the operating system are provided by the host through `Options::host_networks`,
  which replaces the built-in entry with the same id. The Android SMS driver works this way.
- Networks with no working driver are listed as unavailable with a reason. They are not removed.
- Priority networks: SMS, WhatsApp, Telegram, Discord, Fluxer.
- `#![forbid(unsafe_code)]` in `chirper-core` and in most driver crates. Three crates do not set it
  yet (see Remaining work).

## Current state

| Area | State |
|---|---|
| `crates/chirper-bridge` | Runtime, ids, domain model, driver traits, redaction, shared HTTP client. Done. Does not yet set `forbid(unsafe_code)`. |
| `crates/chirper-core` | Store (schema v1 to v6, SQLCipher or plain SQLite), content codec with a reader for the Go core's rows, engine and facade, `OCV1` vault, sealed per-login secrets, JSON model. Tests: golden envelopes, end-to-end demo. Example: `smoke`. Done. |
| `crates/chirper-net-testnet` | Demo network for tests and the `mock` build. Done. Does not yet set `forbid(unsafe_code)`. |
| `crates/chirper-net-discord` | REST v10 and gateway with resume and rate-limit buckets. Token and QR login. Not yet exercised against a live account. |
| `crates/chirper-net-fluxer` | Instance discovery, REST with the 429 policy, gateway, ALTCHA proof of work. Session token, bot token, and password login. Not yet exercised against a live account. Does not yet set `forbid(unsafe_code)`. |
| `crates/chirper-net-telegram` | Work in progress. MTProto client, TL code generation, obfuscated transport, auth key creation, encrypted session. Login flows `phone`, `qr` and `bot`. Reported as available only in builds with `CHIRPER_TELEGRAM_API_ID` and `CHIRPER_TELEGRAM_API_HASH`. Not yet verified against a live account. |
| `crates/chirper-net-whatsapp` | Phase one: Noise XX handshake over WebSocket, binary node codec, QR device linking. Reported unavailable: messaging is not implemented. |
| `crates/chirper-signal-protocol` | Signal protocol primitives: X3DH and PQXDH, Double Ratchet, sender keys, sealed sender v1, storage traits with an in-memory store. No driver uses it yet. |
| `crates/chirper-net-slack` | Login by e-mail, token or app. Web API and RTM websocket. Not yet verified live. |
| `crates/chirper-net-linkedin` | Cookie login, web messaging API and realtime stream. Not yet verified live. |
| `crates/chirper-net-gvoice` | Cookie login, thread list, realtime channel, text and image messages. Not yet verified live. |
| `crates/chirper-net-bluesky` | Password login, AT Protocol sessions, `chat.bsky` through the PDS proxy, polled event log. Not yet verified live. |
| `crates/chirper-net-sms` | Android only, through the host platform. Reads, receives and sends plain text. Not verified on a device. |
| `crates/chirper-net-signal`, `chirper-net-meta`, `chirper-net-twitter`, `chirper-net-gmessages` | Placeholders. Reported unavailable, with no login flow. |
| `src-tauri` (`chirper-app`) | Opens the core with the desktop key from the OS credential store, or with the key from the mobile plugin. Commands, `core-batch` events, app lock, preferences, notifications, tray, deep links, `chirper-media://`. Not verified on devices. |
| `plugins/chirper-native` | Kotlin and Swift code: key wrap, biometrics, secure-screen cover, connection service, SMS platform. Not verified on devices. |
| iMessage | Listed by the registry with a reason. No driver. |

## Remaining work

1. Exercise the enabled drivers against real accounts, and fix what the real services disagree
   with: Discord first (login, send, edit, redact, reaction, backfill), then Fluxer, Slack, Bluesky,
   LinkedIn and Google Voice. Until a driver has been run this way, its "available" status is
   unverified.
2. Finish Telegram: the login, update and messaging paths. Until they work end to end it should
   keep reporting unavailable. Then run its live probe, `crates/chirper-net-telegram/examples/authkey_probe.rs`.
3. WhatsApp phase two: Signal sessions and prekeys on `chirper-signal-protocol`, messaging, groups
   (sender keys), media, app-state and history sync. Until then it stays unavailable.
4. Write the drivers for Signal, Messenger, Instagram, X and Google Messages. Each stays unavailable
   until its login, receive and send paths work.
5. Close the login-flow gaps against the Go build's flows (see [NETWORKS.md](NETWORKS.md) section 4):
   Discord's browser and bot flows, WhatsApp's phone flow, LinkedIn's password flow, and Google
   Messages' Google flow.
6. Set `#![forbid(unsafe_code)]` in `chirper-bridge`, `chirper-net-testnet` and `chirper-net-fluxer`.
7. Decide whether drivers stay always compiled into `chirper-core` (as now) or become feature-gated,
   as the earlier design proposed. No per-network feature flags exist.
8. Decide on the webview policy. The Content Security Policy allows `https:` for images, media and
   connections ([SECURITY_MODEL.md](SECURITY_MODEL.md) section 5).
9. Decide whether iMessage gets a driver, or stays a listed entry with its reason.
10. Keep the docs in step with the code. The docs were refreshed for the all-Rust layout; the next
    change to a driver's status, login flows or capabilities should update [NETWORKS.md](NETWORKS.md)
    and the table in [README.md](../README.md) in the same pull request.
