# Chirper Architecture

Chirper is an on-device chat client built with **Tauri v2**, **Svelte 5** and **Rust**. There is no
Chirper server. Each network is a Rust driver that runs in the app process and talks to that network
directly. The core, the bridge runtime and every driver are Rust; there is no Go code, no cgo and no
C ABI.

---

## 1. Layers

```
Svelte UI (ui/)                         CoreApi: TauriCore in the app, FakeCore in a browser
   │ invoke(command, args)               ▲ "core-batch" event, 16 ms or 256 items per batch
src-tauri/ (crate chirper-app)           │ command handlers, lock, notifications, chirper-media://
   │ direct Rust calls                   │
chirper-core (crate)                     │ CoreEvent stream (EventStream)
   │  engine: facade, dispatch, logins   │
   │  store: chirper.db (SQLCipher)      │
   │  secure: OCV1 vault, FileSecrets    │
   │ RuntimeEvent (local ids)            │
chirper-bridge (crate)                   │
   │  Runtime: supervisors, queues,      │
   │  backoff, capability gates, ids     │
   │ Session / LoginProcess traits       │
chirper-net-<network> (one per network)  │
   │                                     │
network servers (HTTPS, WebSocket, TCP)─┘
```

Rules that the code follows:

- `src-tauri` depends only on `chirper-core` (plus `chirper-bridge` and the SMS driver for the host
  driver). It never talks to a driver directly.
- `chirper-bridge` has no database and no UI type. A driver reports domain events and receives
  commands; it never sees SQL or a local id.
- `chirper-core` persists what the runtime reports and emits the wire events the UI reads.

## 2. Crates

| Crate | Role |
|---|---|
| `chirper-app` (`src-tauri/`) | Tauri shell: commands, `core-batch` batching, app lock, preferences, notifications, `chirper-media://`, platform code. |
| `chirper-core` | Store (SQLCipher or plain SQLite), engine and facade, OCV1 media vault, sealed per-login secrets, log redaction, JSON model. |
| `chirper-bridge` | The unified bridge runtime and the traits a driver implements (`Network`, `Session`, `LoginProcess`, `Secrets`), the domain model, the id derivation and the error type. |
| `chirper-net-testnet` | In-process demo network. Registered in demo mode only. |
| `chirper-net-whatsapp` | WhatsApp: own multi-device client. QR and pairing-code linking, receiving, sending, media, app-state and history sync. |
| `chirper-net-telegram` | Telegram: own MTProto client. Available when an API id and hash are set, in the build or in Settings → Advanced. |
| `chirper-net-discord` | Discord: REST v10 and gateway. Browser, QR, user-token and bot logins. |
| `chirper-net-fluxer` | Fluxer: instance discovery, REST, gateway. Token, bot and password logins. |
| `chirper-net-sms` | SMS driver. It needs a platform: the Android app provides one. Elsewhere it reports unavailable. |
| `chirper-signal-protocol` | Signal protocol primitives (X3DH, Double Ratchet, sender keys, sealed sender). The WhatsApp driver uses it. |
| `chirper-net-signal`, `chirper-net-meta`, `chirper-net-twitter`, `chirper-net-gmessages`, `chirper-net-linkedin`, `chirper-net-gvoice`, `chirper-net-slack`, `chirper-net-bluesky` | Gated networks. Compiled only with their feature; not in the default build. See [NETWORKS.md](NETWORKS.md) section 2. |

### Features of `chirper-core`

| Feature | Effect |
|---|---|
| `sqlite-plain` (default) | Plain SQLite. For development and tests. |
| `sqlcipher` | SQLCipher, with vendored OpenSSL. The app builds with it. |
| `net-signal`, `net-meta`, `net-twitter`, `net-linkedin`, `net-gmessages`, `net-gvoice`, `net-slack`, `net-bluesky` | Each compiles its gated network and registers it. `net-meta` covers Messenger and Instagram. |
| `more-networks` | Turns on all eight `net-*` features, and lists iMessage with its reason. |

The five networks of the default build (WhatsApp, Telegram, Discord, Fluxer, SMS) are always compiled.
The demo network is compiled always and registered only in demo mode. The app crate, `chirper-app`,
has its own `mock` feature, which turns on demo mode (`Options::demo_network`). It does not forward
`more-networks`: to build the app with the gated networks, the feature has to be passed to
`chirper-core` directly (see [BUILDING.md](BUILDING.md) section 4).

Plugins: `plugins/chirper-native` holds the Android (Kotlin) and iOS (Swift) code. It provides the
keystore wrap, biometric prompts, the secure-screen cover, the Android connection service, and the
SMS platform used by the host driver.

## 3. Data flow

### 3.1 Commands in

1. The UI calls `invoke("<command>", args)`. Commands are listed in [CONTRACT.md](CONTRACT.md).
2. The handler in `src-tauri/src/commands.rs` checks that the app is unlocked and that the core is
   open, then calls the matching `chirper-core` method.
3. For a chat action, `chirper-core` checks the input (`engine/validate.rs`) and calls
   `Runtime::command`. The runtime refuses commands the network's capabilities do not allow
   (`Error::Unsupported`), queues the command on that login's queue, and waits for the network's
   command gap (40 ms per network) before the driver runs it.
4. The result goes back to the caller. Message state changes come back as events, below.

### 3.2 Events out

1. A driver's `Session::next` returns an `Event` with remote ids (chat, message, reaction, typing,
   member, read receipt, delivery, session state, error).
2. The runtime turns remote ids into local ids (`ChatId`, `EventId`, `SenderId`) and sends a
   `RoutedEvent` to the engine.
3. The engine's dispatcher (`engine/dispatch.rs`) writes the store and emits `CoreEvent`s: `chat`,
   `chat_removed`, `message`, `message_removed`, `typing`, `account` and `core_error`.
4. The Tauri shell's pump (`src-tauri/src/events.rs`) collects the events for up to 16 ms or 256
   items, keeps only the last `chat` event per room in a batch, and emits one `core-batch`. It
   also posts OS notifications for new messages while the window is not focused, unless the
   preferences hide the content.

Channel sizes are fixed in code: 64 queued operations per login (`OPS_QUEUE`) and 256 queued driver
events (`EVENT_QUEUE`). A full event queue makes the driver wait.

## 4. The bridge runtime

`chirper-bridge::Runtime` does the work that a driver must not repeat:

- **Supervision.** One supervisor task per login. It runs `connect`, then `next` until the session
  ends, then reconnects. The delay starts at 1 s, doubles up to 120 s, and has jitter.
- **Rate limits.** A `RateLimited { retry_after_ms }` error makes the next reconnect wait for the
  requested time, capped at 15 minutes. A command gap of 40 ms per network keeps a burst of user
  actions from becoming a burst of requests. A driver still handles a real HTTP 429 itself.
- **Auth failures.** A failure that is not retryable (for example bad credentials) is not retried.
  The account state goes to the UI instead.
- **Capability gates.** Edit, redact, react, reply, read receipt and typing commands fail with
  `Unsupported` before they reach a driver when the driver's `capabilities()` is off.
- **Secrets.** A driver reads and writes its secrets through `LoginCtx`, which uses the `Secrets`
  trait. Secret values come back as `Zeroizing` buffers. The secret-bearing types the drivers define
  print `[redacted]` in `Debug`; `LoginCtx` and `FileSecrets` do not print their secret values.
- **Errors.** Error text from a driver passes through `redact` before it leaves the runtime. It
  removes values after markers such as `token=` or `Bearer`, Discord-shaped tokens, and long
  mixed letter-and-digit runs.

## 5. Identity

Drivers speak only remote ids. The runtime derives local ids with the same formats the Go core
used, so an existing database keeps resolving (`chirper-bridge/src/ids.rs`):

| Kind | Format | Derived from |
|---|---|---|
| Chat (`ChatId`) | `!<network>.<32 hex>:chirper.local` | network, login and remote portal id |
| Event (`EventId`) | `$<network>.<32 hex>` | chat, remote message id and part |
| Sender (`SenderId`) | `@<network>_<escaped>:chirper.local` | network and remote user id |
| Local event | `$local.<32 hex>` | random, for messages the user sends |
| Local user | `@me:chirper.local` | fixed |

The hex digest is the first 16 bytes of SHA-256 over the parts, separated by NUL bytes. Each
driver has unit tests that pin its remote id format.

## 6. Storage

Everything is in the app data directory. The core opens these paths (`engine/mod.rs`):

| Path | Contents | Protection |
|---|---|---|
| `chirper.db` | Rooms, messages, events, reactions, logins, remote ids, backfill state. Schema version 6. | SQLCipher with a raw 256-bit key (`PRAGMA key = x'…'`). Plain SQLite only in development builds. |
| `media/` | Media files, sealed one file at a time. | `OCV1` format (`secure::vault`): AES-256-GCM in 64 KiB chunks, a fresh key per file. |
| `credentials/` | Per-login secrets (tokens, cookies, keys). | Sealed with the database key, owner-only permissions. |
| `settings.json` | Core settings (`CoreSettings`), written as JSON. Holds the Telegram API id and hash if the user entered them in Settings → Advanced. | Owner-only file (`secure::write_private`). Not encrypted: the API hash is an app credential, not a login. |
| `prefs.json` | App preferences, in the Tauri app data directory. | Mode 0600. Holds no secrets. |

Schema history: versions 1 to 5 are the schema the Go core wrote, with the same table and column
names. Version 6 adds the remote-id columns (`room.remote_id`, `event.remote_id`) and the login and
backfill tables that the runtime needs. There is one database per install; the per-bridge
databases of the Go build are not created. Message rows written by the Go core are still read
through a compatibility parser.

The database key is 32 random bytes:

- **Desktop** (macOS, Windows, Linux): created on first run and kept in the OS credential store
  under service `app.chirper`, user `db-key` (`src-tauri/src/keystore.rs`).
- **Android and iOS**: wrapped by the `chirper-native` plugin (Android Keystore, iOS Keychain).

If the database cannot be opened because the file is unreadable or the key does not match, the
startup code moves the data directory aside and starts with an empty one. It never deletes the old
directory (`src-tauri/src/startup.rs`).

## 7. How a driver plugs in

A driver is a crate that implements the traits in `chirper-bridge/src/network.rs`:

1. `Network`: `id`, `name`, `color`, `capabilities`, `login_flows`, `unavailable`, `start_login`,
   `connect`. This is the static description; `unavailable` returns the reason the network is not
   usable, or `None`.
2. `LoginProcess`: `next(LoginInput) -> LoginStep` for each step of a login flow, and `cancel`.
3. `Session`: `next` (the event stream), `command`, `backfill`, `logout_remote`.

To register a driver, add it to `chirper-core/src/engine/networks.rs` (`available()`) and to the
`[dependencies]` of `chirper-core`. A driver that is not part of the default build is an optional
dependency behind a `net-*` feature, and is registered under `#[cfg(feature = ...)]`. The network id is
written to the database and to settings, so it must not change once it has shipped.

A driver that needs the operating system is provided by the host instead. The host passes a list
in `Options::host_networks`; an entry with the same id replaces the built-in one. The Android SMS
driver works this way: `host_networks` in `src-tauri/src/platform/mod.rs` builds it on Android, with
a platform (`AndroidSms`, in `platform/android.rs`) that calls the `chirper-native` plugin.

Networks with no driver, such as iMessage, come from a fixed list in `engine/networks.rs`. That list
is compiled only with `more-networks`, and its entries are shown as unavailable with a reason.

## 8. Testing

- `chirper-core/tests/golden.rs` checks the JSON envelopes against fixtures in
  `chirper-core/tests/golden/`. The fixtures were captured from the Go core before it was removed.
- `chirper-core/tests/demo.rs` runs the engine end to end against the demo driver.
- `chirper-core/examples/smoke.rs` logs into the demo network, sends a message, and reopens the
  encrypted database with the same key.
- Each driver tests its codecs, crypto steps and state machines with fixed vectors and fake
  transports. The live probes in the drivers' `examples/` directories connect to real servers; they
  are examples, not tests, and `cargo test` does not run them.

See [BUILDING.md](BUILDING.md) for the commands.
