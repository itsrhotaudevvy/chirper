# Supported networks

Every fact here comes from the driver code in this repository: the `unavailable()`, `login_flows()`,
`capabilities()` and `color()` methods of each `chirper-net-*` crate, and the module documentation of
the crate. Where a driver is still being written, the table shows what `unavailable()` reports now.

None of the drivers has been verified against a live account in this repository. "Available" means
that `unavailable()` returns no reason; it does not mean that login, receiving and sending have been
tested end to end. See [PLAN.md](PLAN.md) section 5.

Capability names are the ones the UI reads (`Capabilities` in `crates/chirper-bridge/src/model.rs`):
`edits`, `reactions`, `replies`, `redactions`, `typing`, `readReceipts`, `stickers`, `voice`. The
runtime refuses a command whose capability is off, before the driver sees it.

---

## 1. Summary

| Network | Id | Driver crate | Status | Login flows (id) | Capabilities |
|---|---|---|---|---|---|
| Discord | `discord` | `chirper-net-discord` | Available | `token`, `qr` | all but `readReceipts`, `voice` |
| Fluxer | `fluxer` | `chirper-net-fluxer` | Available | `token`, `bot`, `password` | all but `stickers`, `voice` |
| Telegram | `telegram` | `chirper-net-telegram` | Available only when built with the API id; otherwise unavailable | `phone`, `qr`, `bot` | all (as `Capabilities::ALL`) |
| SMS | `sms` | `chirper-net-sms` | Available on Android; unavailable elsewhere | `device` | none |
| WhatsApp | `whatsapp` | `chirper-net-whatsapp` | Unavailable: "WhatsApp messaging lands in the next phase" | `qr` | none |
| Signal | `signal` | `chirper-net-signal` | Unavailable: "Signal is being ported to the unified Rust runtime." | none | none |
| Messenger | `messenger` | `chirper-net-meta` | Unavailable: "Messenger is being ported to the unified Rust runtime." | none | none |
| Instagram | `instagram` | `chirper-net-meta` | Unavailable: "Instagram is being ported to the unified Rust runtime." | none | none |
| X (Twitter) | `twitter` | `chirper-net-twitter` | Unavailable: "X is being ported to the unified Rust runtime." | none | none |
| Google Messages | `gmessages` | `chirper-net-gmessages` | Unavailable: "Google Messages is being ported to the unified Rust runtime." | none | none |
| LinkedIn | `linkedin` | `chirper-net-linkedin` | Available | `cookies` | all but `stickers`, including `voice` |
| Google Voice | `gvoice` | `chirper-net-gvoice` | Available | `cookies` | `readReceipts` |
| Slack | `slack` | `chirper-net-slack` | Available | `email`, `token`, `app` | all but `stickers`, `voice` |
| Bluesky | `bluesky` | `chirper-net-bluesky` | Available | `password` | `reactions`, `redactions`, `readReceipts` |
| iMessage | `imessage` | none | Listed with a reason; no driver | none | none |
| Demo | `testnet` | `chirper-net-testnet` | Available in builds with the demo network enabled | `demo` | all |

"None" for capabilities means `Capabilities::default()`: every capability is off. The unavailable
drivers report that today; their capabilities will be set when they are finished.

The demo network is registered only when the core is opened with `demo_network` (the `mock` build).
iMessage is listed by `engine/networks.rs` from a fixed table, not by a driver.

---

## 2. Colours

Each driver's `color()` is the colour the UI uses for the network. These are the values in the code:

| Network | Colour |
|---|---|
| Telegram | `#26A5E4` |
| WhatsApp | `#25D366` |
| Signal | `#3A76F0` |
| Messenger | `#FF2542` |
| Instagram | `#E1306C` |
| X (Twitter) | `#7C5CFF` |
| LinkedIn | `#FF2D7A` |
| Google Messages | `#1A73E8` |
| Google Voice | `#34A853` |
| Slack | `#4A154B` |
| Bluesky | `#1185FE` |
| Discord | `#5865F2` |
| Fluxer | `#FF77A9` |
| SMS | `#8E8E93` |
| Demo | `#8E8E93` |

---

## 3. Per network

Notes are taken from each crate's module documentation and its code. Protocol detail is in the
crate's own modules.

### Discord (`discord`)

- **Login:** `token` takes a user token, or a bot token that starts with `Bot `. `qr` uses the
  Discord mobile app's remote-auth flow.
- **Protocol:** REST v10 and the gateway over WebSocket (`api`, `gateway`, `remoteauth`).
- **Capabilities:** edits, reactions, replies, redactions, typing, stickers. Discord has no read
  receipts in this driver, and voice is off.

### Fluxer (`fluxer`)

- **Login:** `token` takes a session token; `bot` takes a bot token; `password` takes an e-mail or
  username with a password, and an authenticator code when the account asks for one.
- **Protocol:** instance discovery (`discovery`), REST with the 429 policy (`api`), the ALTCHA
  proof-of-work used by the instance (`captcha`), and the gateway with resume (`gateway`). The
  driver can talk to any Fluxer instance; the default is set in `discovery`.
- **Capabilities:** edits, reactions, replies, redactions, typing, read receipts. Stickers and voice are off.

### Telegram (`telegram`)

- **Availability:** `unavailable()` returns a reason unless the build has both
  `CHIRPER_TELEGRAM_API_ID` and `CHIRPER_TELEGRAM_API_HASH`. The release workflow supplies them from
  secrets.
- **Login:** `phone` (login code, then the two-step password when the account has one), `qr`, and `bot`.
- **Protocol:** MTProto 2.0 over TCP, with a TL schema code generator (schema in `tl/`), the client in
  `src/client`, the logins in `src/login` and the encrypted session in `src/session`. The driver is
  still being completed; see
  [rust-core-port.md](rust-core-port.md).
- **Capabilities:** the driver returns `Capabilities::ALL`. The runtime enforces the flags, but the
  driver has not yet been verified for each of them.

### SMS (`sms`)

- **Availability:** the driver needs a platform. On Android the app passes a platform built on the
  `chirper-native` plugin (`src-tauri/src/platform/android.rs`). Elsewhere it reports unavailable with
  the reason "SMS works in the Android app, which reads and sends through the phone's own SMS service."
- **Login:** `device` (the "This phone" flow). It asks for the SMS permissions in a `permissions`
  step, then starts the account.
- **Behaviour:** the app is not the default SMS application. The driver reads, receives and sends
  plain text. It cannot mark messages read, delete them or write to the provider. The capabilities
  are all off, so the UI does not offer edits, reactions, replies or receipts.

### WhatsApp (`whatsapp`)

- **Login:** `qr` links the app as a companion device. The flow runs the Noise XX handshake over
  WebSocket, uses the binary node codec, and shows the pairing QR code.
- **Availability:** unavailable. The module documentation says the phase-one work is the transport and
  the linking; messaging, the Signal protocol (through `chirper-signal-protocol`), media and history
  sync come next.
- **Capabilities:** none.

### Signal, Messenger, Instagram, X, Google Messages

These five networks have no working driver in this repository. Each crate's `lib.rs` contains the
network description and the `unavailable()` reason, and no login flow; the crate documentation says
they are work in progress. Some of these crates have modules under construction in the working tree
that the `lib.rs` does not use yet. The Messenger and Instagram drivers share `chirper-net-meta`.

### LinkedIn (`linkedin`)

- **Login:** `cookies`. The UI reads the session cookies from the sign-in page.
- **Protocol:** LinkedIn's own web messaging API, with its realtime stream (server-sent events with a
  heartbeat) (`api`, `realtime`, `session`). The ids match the Go bridge's.
- **Capabilities:** edits, reactions, replies, redactions, typing, read receipts, and voice. Stickers are off.

### Google Voice (`gvoice`)

- **Login:** `cookies`. The sign-in page is voice.google.com; the UI reads the listed cookies.
- **Protocol:** the Voice thread list and the realtime channel, with a pblite codec (`api`,
  `realtime`, `wire`, `schema`). The behaviour follows mautrix-gvoice (AGPL-3.0), named in the module
  documentation. Sends do not use the Electron helper of the reference; they carry the default
  tracking data.
- **Capabilities:** read receipts only. The driver handles text and image messages.

### Slack (`slack`)

- **Login:** `email` (address, a CAPTCHA when Slack asks for one, the confirmation code, a workspace
  choice, and two-factor or browser sign-in where a workspace needs it); `token` (an auth token,
  with a cookie when the token comes from a browser); and `app` (a bot or user token pasted by hand).
- **Protocol:** the Web API, and the real-time websocket (`rtm`). The message conversions follow the
  mautrix Slack bridge (AGPL-3.0), named in the module documentation.
- **Capabilities:** edits, reactions, replies, redactions, typing, read receipts. Slack has no stickers
  in this driver (they arrive as images), and voice is off.

### Bluesky (`bluesky`)

- **Login:** `password`, with a handle and an app password. The server defaults to `bsky.social`.
- **Protocol:** AT Protocol XRPC. The session comes from `com.atproto.server.createSession`, is
  refreshed when it expires, and is deleted on logout. `chat.bsky` calls go through the PDS proxy to
  the chat service. The session polls the event log: it waits 5 seconds between polls and doubles the
  wait after each empty poll, up to a maximum.
- **Capabilities:** reactions, redactions and read receipts. Edits, replies and typing are off.

### Demo (`testnet`)

- An in-process network with two chats and no I/O. It is the reference driver for the runtime and
  the UI in development. It is registered only in the `mock` build.

---

## 4. Differences from the Go build's login flows

The Go build's login flows are in the parity table in `docs/specs/README.md`. Where the Rust driver
exposes a different set, the table below says so. The golden tests pin the network ids, colours
and login flow ids that the UI depends on, so a missing flow is a gap to close, not a rename.

| Network | Go build | Rust driver in this repository |
|---|---|---|
| Discord | `browser`, `qr`, `token`, `bot` | `token` (which also accepts a bot token), `qr` |
| WhatsApp | `qr`, `phone` | `qr` |
| LinkedIn | `cookies`, `password` | `cookies` |
| Google Messages | `google`, `qr` | none (unavailable) |
| Signal | `qr` (link, in mautrix) | none (unavailable) |
| Messenger | `facebook`, `messenger`, `messenger-lite`, `messenger-lite-android` | none (unavailable) |
| Instagram | `android`, `instagram`, `instagram-password` | none (unavailable) |
| X (Twitter) | `cookies`, `password` | none (unavailable) |
| Telegram, Fluxer, Slack, Bluesky, Google Voice | as in the Go build | as in section 1 |

---

## 5. Registration

`crates/chirper-core/src/engine/networks.rs` builds the list. Drivers are added in this order; the
UI shows the available ones first:

Discord, Fluxer, Telegram, WhatsApp, SMS, Signal, Messenger, Instagram, X, LinkedIn, Google Messages,
Google Voice, Slack, Bluesky, and the demo network in the `mock` build. A host can replace the SMS
entry on Android. iMessage is added from a fixed table, with its reason.
