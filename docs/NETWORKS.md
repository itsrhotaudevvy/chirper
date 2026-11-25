# Supported networks

This build ships five networks, plus the demo network when demo mode is on. Every fact here comes
from the driver code: the `unavailable()`, `login_flows()`, `capabilities()` and `color()` methods
of each `chirper-net-*` crate, the crate documentation, and the registry in
`crates/chirper-core/src/engine/networks.rs`.

None of these drivers has been tested against live servers. The drivers' own tests use fake
transports and fixed vectors, and those are the only exercise they have had. "Available" means that
`unavailable()` returns no reason. It does not mean that login, receiving and sending work on the
real network.

Capability names are the ones the UI reads (`Capabilities` in `crates/chirper-bridge/src/model.rs`):
`edits`, `reactions`, `replies`, `redactions`, `typing`, `readReceipts`, `stickers`, `voice`. The
runtime refuses a command whose capability is off, before the driver sees it.

---

## 1. Networks in this build

| Network | Id | Driver crate | Status | Login flows (id) | Capabilities |
|---|---|---|---|---|---|
| WhatsApp | `whatsapp` | `chirper-net-whatsapp` | Available | `qr`, `phone` | all |
| Telegram | `telegram` | `chirper-net-telegram` | Available when an API id and hash are set; otherwise unavailable | `phone`, `qr`, `bot` | all |
| Discord | `discord` | `chirper-net-discord` | Available | `browser`, `qr`, `token`, `bot` | all but `readReceipts`, `voice` |
| Fluxer | `fluxer` | `chirper-net-fluxer` | Available | `token`, `bot`, `password` | all but `stickers`, `voice` |
| SMS | `sms` | `chirper-net-sms` | Available on Android; unavailable elsewhere | `device` | none |
| Demo | `testnet` | `chirper-net-testnet` | Only in demo mode | `demo` | all |

"None" means `Capabilities::default()`: every capability is off.

### WhatsApp

- **Login:** `qr` links Chirper as a companion device: the user scans the code in WhatsApp under
  Settings → Linked devices. `phone` links with a pairing code that the user types on the phone, under
  Link with phone number.
- **Protocol:** the crate's own multi-device client. It runs the Noise XX handshake over WebSocket,
  the binary node codec and the protobuf messages. It uses `chirper-signal-protocol` for the
  end-to-end sessions. It covers receiving, sending, media, app-state sync and history sync. Remote
  ids follow the Go bridge (`ids`).

### Telegram

- **Credentials:** the driver needs a Telegram API id and hash. It takes them from two places:
  - the user's settings, entered in **Settings → Advanced** and stored as `telegram.apiId` and
    `telegram.apiHash` in the core settings. A valid pair (an id above zero, and a 32-character hex
    hash) takes precedence;
  - the build, from `CHIRPER_TELEGRAM_API_ID` and `CHIRPER_TELEGRAM_API_HASH`, read at compile time.

  If neither is valid, the network is unavailable with the reason "Telegram needs an API id. Enter
  yours in Settings → Advanced". The core reads the settings when it opens, so a change takes effect
  after a restart, and the Settings screen says so.
- **Login:** `phone` (login code, then the two-step password when the account has one), `qr`, and `bot`.
- **Protocol:** MTProto 2.0 over TCP with the obfuscated transport, a TL schema code generator (schema
  in `tl/`), SRP for the two-step password, the update stream, dialogs, messages, media and backfill
  (`src/client`, `src/login`, `src/session`, `src/backfill.rs`, `src/media.rs`).

### Discord

- **Login:** `browser` opens Discord's own sign-in page in a web view, and Chirper reads the token the
  web app keeps. `qr` scans a QR code with the Discord mobile app, for a user account. `token` takes a
  user token. `bot` takes a bot token. Each flow ends by checking the token against the API, and the
  account type must match the flow.
- **Protocol:** the v10 REST API and the gateway over WebSocket, with resume (`api`, `gateway`,
  `remoteauth`, `login`).
- **Capabilities:** edits, reactions, replies, redactions, typing, stickers. The driver reports no
  read receipts and no voice.

### Fluxer

- **Login:** `token` takes a session token; `bot` takes a bot token; `password` takes an e-mail or
  username and a password, plus an authenticator code when the account asks for one. The flow's
  `instance` field takes the origin of the instance; the default is `https://fluxer.app`.
- **Protocol:** instance discovery (`discovery`), REST with the 429 policy (`api`), the ALTCHA
  proof-of-work that instances use (`captcha`), and the gateway with resume (`gateway`).
- **Capabilities:** edits, reactions, replies, redactions, typing, read receipts. Stickers and voice are off.

### SMS

- **Availability:** the driver needs a platform. On Android the app passes one (`AndroidSms` in
  `src-tauri/src/platform/android.rs`, built on the `chirper-native` plugin). Elsewhere the network is
  unavailable, with the reason "SMS works in the Android app, which reads and sends through the
  phone's own SMS service."
- **Login:** `device` (the "This phone" flow). It asks for the SMS permissions in a `permissions` step.
- **Behaviour:** Chirper is not the default SMS app. The driver reads, receives and sends plain text.
  It cannot mark messages read, delete them or write to the provider. Its capabilities are all off, so
  the UI offers no edits, reactions, replies or receipts.

### Demo

- An in-process network with two chats and no I/O. It is registered only when the core is opened
  in demo mode, which the app's `mock` feature turns on. It is the reference driver for the runtime
  and the UI in development.

---

## 2. Not in this build

These networks are compiled only with their Cargo feature in `chirper-core` (see
[ARCHITECTURE.md](ARCHITECTURE.md) section 2). They are not registered in the default build and are
not listed in the app. Their unfinished work is on the branch `wip/more-networks`. The table gives the
state of each crate on the current branch and on that branch.

| Network | Id | Feature | On this branch | On `wip/more-networks` |
|---|---|---|---|---|
| Signal | `signal` | `net-signal` | Placeholder: unavailable, no login flow | Same |
| Messenger | `messenger` | `net-meta` | Placeholder: unavailable, no login flow | Same |
| Instagram | `instagram` | `net-meta` | Placeholder: unavailable, no login flow | Same |
| X (Twitter) | `twitter` | `net-twitter` | Placeholder: unavailable, no login flow | Cookies and password flows; reports available |
| LinkedIn | `linkedin` | `net-linkedin` | Cookies flow; reports available | Cookies and password flows |
| Google Messages | `gmessages` | `net-gmessages` | Placeholder: unavailable, no login flow | Same |
| Google Voice | `gvoice` | `net-gvoice` | Cookies flow; reports available | Same |
| Slack | `slack` | `net-slack` | E-mail, token and app flows; reports available | Same |
| Bluesky | `bluesky` | `net-bluesky` | Password flow; reports available | Same |
| iMessage | `imessage` | `more-networks` | Listed with a reason; no driver | Same: no crate; listed with a reason |

- `more-networks` turns on all eight features above, and also lists iMessage.
- `net-meta` covers two networks, Messenger and Instagram.
- A feature-gated driver that reports available has not been tested against live servers either.
- "Placeholder" means the crate has the network description and the `unavailable()` reason, and no login flow.

---

## 3. Colours

Each driver's `color()` is the colour the UI uses for the network. These are the values in the code:

| Network | Colour | | Network | Colour |
|---|---|---|---|---|
| WhatsApp | `#25D366` | | Slack | `#4A154B` |
| Telegram | `#26A5E4` | | Bluesky | `#1185FE` |
| Discord | `#5865F2` | | Fluxer | `#FF77A9` |
| SMS | `#8E8E93` | | Demo | `#8E8E93` |
| Signal | `#3A76F0` | | Messenger | `#FF2542` |
| Instagram | `#E1306C` | | X (Twitter) | `#7C5CFF` |
| LinkedIn | `#FF2D7A` | | Google Messages | `#1A73E8` |
| Google Voice | `#34A853` | | iMessage | `#34C759` |

---

## 4. Login flows against the Go build

The Go build's login flows are in the parity table in `docs/specs/README.md`. The five networks in
this build offer the flow ids of that table: WhatsApp (`qr`, `phone`), Telegram (`phone`, `qr`,
`bot`), Discord (`browser`, `qr`, `token`, `bot`) and Fluxer (`token`, `bot`, `password`). The golden
tests pin the network ids, colours and flow ids that the UI depends on.

The gated networks do not yet offer every Go flow. Signal, Messenger, Instagram and Google Messages
have no flows, and X has its flows only on `wip/more-networks`.

---

## 5. Registration

`crates/chirper-core/src/engine/networks.rs` builds the list. The usable networks come first, in this
order: WhatsApp, Telegram, Discord, Fluxer, SMS, then the demo network in demo mode. The gated
networks are added only under their features. A host can replace the SMS entry on Android. In a
`more-networks` build, iMessage is listed last, with its reason.
