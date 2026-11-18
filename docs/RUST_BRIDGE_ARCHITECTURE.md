# Chirper Rust core and unified bridge runtime

Status: **design, agreed direction**. Supersedes the "bridges stay Go" scope in
`docs/rust-core-port.md`. Written after reading the Go core (`go/core`, 22.8k LOC) and the
vendored bridgev2 bridges (`third_party/*`).

The goal is one thing: **no Go in the shipped binary, one bridge runtime, one storage engine, one
identity model, and a wire contract that does not change.** The UI (`ui/`) and the Tauri layer
(`src-tauri/`) must keep working against the same JSON envelopes.

---

## 1. What the Go stack actually buys us

bridgev2 is Matrix-shaped, but its useful ideas are transport-independent. Porting must keep every
one of them and drop only the homeserver:

| bridgev2 gives us | what it really is | Rust replacement |
|---|---|---|
| Deterministic portal IDs (`!bridge.<hash>:chirper.local`) | stable local key for a remote chat | `ChatId::derive(network, login, portal)` — same hash, same string |
| Deterministic event IDs (`$bridge.<hash>`) | idempotent write key for a remote message | `EventId::derive(chat, message, part)` |
| Ghost users (`@bridge_<escaped>:chirper.local`) | remote sender as a local identity | `SenderId::derive(network, external)` (same escaping rules) |
| Double puppeting via `@me` | "this message is mine" outbound intent | `Outbound::Owned { login }` on the bridge command |
| `MatrixAPI` / `Intent` | outbound transport abstraction | `NetworkDriver` methods; there is no second transport |
| Per-login DB (`bridge-*.db`) with portals/users/messages | remote-entity bookkeeping | one `chirper.db`, `network`/`login` columns (already in the schema) |
| `BridgeState` state machine | connection lifecycle + reason codes | `SessionState` (same wire values) |
| `MessageStatus` (pending/failed/retry) | optimistic writes + undo | `PendingOp` in the store, same settle path |
| Backfill cursors + batch send | history paging, delivery batching | `BackfillCursor`, `SendQueue` in the runtime |
| Capability flags per network | feature negotiation for the UI | `Capabilities` (unchanged JSON) |

What we deliberately **do not** keep: homeserver URLs, access tokens, `mxc://`, room versions,
encryption of a fake Matrix room, power levels, `event.StateKey`, `unsigned`, the bridgev2 SQL
layer, `maunium.net/go/mautrix`, and the `bridge-*.db` files.

The three services that remain are: **timeline store**, **bridge runtime**, **app facade**. Today
they are `store` + `localmx` + `engine`/`facade`, glued by a C ABI. In Rust they are three crates
in one process with no FFI at all.

---

## 2. Crate graph

```
ui/ (Svelte 5, unchanged)
  │ invoke(command) / core-batch events   ← docs/CONTRACT.md C, unchanged
src-tauri/ (chirper-app)
  │ plain Rust calls, no C ABI, no dlopen, no c-archive
  ├── chirper-core      facade + engine: chats, messages, logins, outbound, settings
  │     ├── chirper-store    rusqlite + SQLCipher, schema v1..v5 + migrations
  │     ├── chirper-bridge   the unified runtime (this document)
  │     ├── chirper-media    metadata strip, blurhash, vault, MIME sniff
  │     └── chirper-secure   keystore, key vault, log redaction, zeroizing secrets
  ├── chirper-net-discord
  ├── chirper-net-fluxer
  ├── chirper-net-testnet    in-process demo network, e2e + UI dev, wasm-clean
  └── (later) chirper-net-<network> …  one crate per network, feature-gated
```

Rules:

- `chirper-core` is the only crate `src-tauri` depends on. Networks are behind features
  (`net-discord`, `net-fluxer`, `net-testnet`) so mobile builds can drop what they do not ship.
- No crate depends on a Matrix type or a homeserver concept. `#![forbid(unsafe_code)]` everywhere;
  the C ABI, `libloading`, and the Go runtime all disappear, so there is no `unsafe` at all.
- `chirper-bridge` must not depend on `chirper-store`. It emits domain events; the engine persists
  them. This keeps drivers testable against an in-memory sink.

---

## 3. The unified bridge runtime (`chirper-bridge`)

### 3.1 Two traits, one runtime

```rust
/// Static description of a network: name, capabilities, login flows. No I/O.
pub trait Network: Send + Sync + 'static {
    fn id(&self) -> NetworkId;             // "discord"
    fn name(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
    fn login_flows(&self) -> Vec<LoginFlow>;
    fn connect(&self, ctx: LoginContext) -> Pin<Box<dyn Future<Output = Result<Box<dyn Session>>>>;
}

/// One live login. Owned by the runtime, driven on a dedicated task.
pub trait Session: Send + 'static {
    /// Stream of normalized remote changes until the connection dies.
    fn events(self: Box<Self>) -> EventStream;         // mpsc::Receiver<RawEvent>
    /// Commands from the app (send, edit, react, typing, read, backfill).
    fn command(&mut self, cmd: Command) -> BoxFuture<'_, Result<CommandResult>>;
    fn backfill(&mut self, chat: ChatId, cursor: Option<Cursor>) -> BoxFuture<'_, Result<Page>>;
    fn logout_remote(&mut self) -> BoxFuture<'_, Result<()>>;
}
```

`Network::connect` receives `LoginContext { login_id, secrets: SecretStore, data_dir,
http: HttpClient, sink: EventSink }`. `EventSink` is the **only** way a driver reports anything:

```rust
pub enum BridgeEvent {
    ChatUpsert(ChatMeta),          // portal metadata changed
    ChatRemoved(ChatId),
    Message(MessageEvent),         // upsert/redact with deterministic ids
    Typing { chat: ChatId, senders: Vec<SenderId> },
    Metadata(MetadataEvent),       // members, avatars, read receipts, receipts
    Session(SessionState),         // connected/backfilling/running/needs_reauth/…
    Delivery { chat: ChatId, event: EventId, status: DeliveryStatus },
    Error(BridgeError),
}
```

That enum replaces `bridgev2`'s `MatrixAPI`/`MatrixConnector`/`Intent` surface (about 4k lines of
Go in `localmx`). Drivers never see SQL, never see IDs, never see the app.

### 3.2 Runtime responsibilities (what Go got for free from bridgev2)

- **Supervision.** One task per login: `connect` → `events` → on close, backoff (1s, ×2, jitter,
  cap 2min) → reconnect. `SessionState` follows the same wire values as `BridgeState`.
- **Idempotent writes.** The store upserts on the derived event id, so a reconnect or replay
  cannot duplicate a message. Deterministic ids are the mechanism, as today.
- **Ordering.** Per-chat monotonic `order` comes from the store, not the network, so unsynced
  history and live messages interleave correctly.
- **Capability gates.** Commands whose capability is off fail with `Unsupported` before reaching
  the driver, so a driver never has to answer "can you edit".
- **Rate limiting and retry.** Per-network token bucket + per-host concurrency limit; a 429 or
  network error becomes `RetryAfter` and the runtime retries, not the driver.
- **Command ordering per chat.** A per-chat `mpsc` so edit-after-send cannot reorder.
- **Secrets.** Tokens live in `SecretStore` (zeroizing, never `Debug`, redacted in logs) and are
  handed to drivers only as `SecretString`.

### 3.3 Identity without Matrix

Newtypes with the exact on-disk strings we already have, because the UI treats them as opaque and
the database must not be rewritten for cosmetics:

```rust
pub struct ChatId(String);    // "!discord.<32 hex>:chirper.local"   (derived: network+login+portal)
pub struct EventId(String);   // "$discord.<32 hex>"                 (derived: chat+message+part)
pub struct SenderId(String);  // "@discord_<escaped>:chirper.local"  (derived: network+external)
pub struct LoginId(String);   // opaque, chosen by the driver
```

`chirper-bridge::ids` is ~120 lines of SHA-256 + escaping, ported once, tested against the Go
vectors so existing databases keep resolving. **Decision D3**: keep the legacy string format for
data compatibility; the `chirper.local` suffix is an implementation detail of the id derivation and
carries no Matrix semantics.

### 3.4 Login state machine

One `LoginProcess` per `startLogin`, driven by the same four wire steps the UI already renders
(`user_input`, `cookies`, `display_and_wait`, `complete`). A driver implements
`LoginFlow::next(step) -> Result<Step>`; the runtime owns process ids, cancellation, timeouts and
`waitLoginStep` fans out to whatever the flow parked. This is `go/core/engine/login.go` with the
bridgev2 process registry replaced by a plain `HashMap` + `oneshot`.

---

## 4. Storage: one database, one migration chain

- `chirper.db` only. `bridge-*.db` disappears; `room`/`event`/`media` already carry `bridge_id`
  and `login_id`, so the split existed only because bridgev2 needed a per-bridge DB.
- Schema v1..v5 is ported **verbatim** (column names and indices included) so an existing install
  opens with no rewrite. Migration v6 (new) is "import legacy `bridge-*.db` rows into the unified
  tables, then delete the file". Until then, opening a legacy install shows the same data.
- `rusqlite` with `bundled-sqlcipher` (or the `native/sqlcipher` build we already produce for
  Android), `MaxOpen=1` as today, same pragmas (`WAL`, `secure_delete`, `temp_store=MEMORY`).
- `_txlock=immediate` is replaced by an explicit `IMMEDIATE` transaction per write, which is what
  the Go layer emulated.
- Full-text search stays `LIKE`-based for now (same behaviour, same limits); FTS5 is a later
  optimisation behind a migration, listed as future work, not smuggled into this port.

---

## 5. Security model (why this is the point)

1. **No cgo, no Go runtime, no `unsafe`.** The whole core is memory-safe Rust with
   `#![forbid(unsafe_code)]`; the two FFI boundaries (Go C ABI on one side, `libloading` on the
   other) and the per-platform link-model hacks disappear. Fewer moving parts, smaller binary,
   no `dlopen` of a core library on Linux/Android.
2. **Same at-rest guarantees, one key.** SQLCipher for the DB, AES-256-GCM `OCV1` vault for media
   (**byte-compatible** format in `crates/chirper-core/src/vault` so existing media decrypts), key
   from the OS keystore via `chirper-secure`.
3. **Secrets discipline.** `SecretString`/`Zeroizing` for tokens, keys and cookies; a
   `redact` module that strips token-shaped strings from every log line (ported from
   `go/core/security/logredact.go`); `Debug` on secret types prints `[redacted]`.
4. **Network hardening kept and improved.** `rustls` (no OpenSSL, no system CA drift), TLS 1.2+,
   the existing per-request size caps and scheme checks in one shared `HttpClient`, plus
   per-network allowlists. `go/core/connectors/*/httpsec` exists twice today; in Rust it is one
   crate.
5. **Input validation at the boundary.** `crates/chirper-core/src/validate.rs` (already written,
   274 lines) becomes the single gate; the Go facade's `maxLen` checks are ported next to it and
   covered by table tests.

---

## 6. Compatibility and performance

| | Go core today | Rust core |
|---|---|---|
| Platforms | c-archive on Apple, c-shared + `libloading` elsewhere; NDK/gomobile plumbing | plain `cargo build --target`; no NDK, no cgo, no linker tricks |
| iOS | static archive, bitcode rules, `xcodebuild` wrapper | staticlib/cdylib, same as any Tauri app |
| Startup | Go runtime init + SQLCipher driver + per-bridge DB open | tokio runtime + one DB open |
| Memory | Go runtime + GC headroom + per-bridge DBs | no GC, one allocator, one DB |
| Latency | cgo call per command, JSON over the C ABI, then JSON to IPC | one JSON decode per command, no cgo round-trip |
| Web demo | impossible | `chirper-net-testnet` builds for wasm |
| Binary | Go archive folded into the Rust binary | one Rust binary |

The wire contract (`docs/CONTRACT.md`) is **unchanged**; the C ABI section is retired, and
`src-tauri` calls `chirper-core` directly. `CoreApi` in the UI does not notice.

---

## 7. Network drivers

"One unified system" is the framework; each network is then a driver, not a fork of the runtime.

| network | plan | state |
|---|---|---|
| discord | own gateway client: HTTP + `tungstenite` WS, `discord::proto` models ported from `go/connectors/discord` | ported |
| fluxer | own HTTP/WS client, from `go/connectors/fluxer` | ported |
| testnet | in-process demo network (`go/core/internal/testnet`), also UI dev + wasm | ported |
| telegram | `grammers` (MTProto in pure Rust) | next |
| signal | `presage`/`libsignal` bindings | next |
| whatsapp | `whatsapp-rust`-class crate or a new one | research |
| slack, twitter/x, bluesky (`atrium`), linkedin, meta, gmessages, gvoice | per-network HTTP/WS, same trait | pending |

Until a driver exists, the network stays **listed** with `available:false` and a `reason`
("Rust port in progress"), which the UI already renders. That is honest and needs no UI change.

---

## 8. Port order (each step compiles and is tested)

1. **Skeleton**: workspace crates, `chirper-bridge` traits + runtime, `ids`, `model`, `validate`.
2. **Store**: schema v1..v5 + queries, migration tests built from the Go schema, `pending` ops.
3. **Secure + media**: keystore, `OCV1` vault (format-compat vectors), strip, blurhash, MIME.
4. **Engine**: chats, messages, outbound, logins, settings, dispatcher (coalescing, live flag).
5. **Facade**: the `CoreApi` surface as native Rust, same JSON; `src-tauri` rewritten to call it.
6. **Drivers**: testnet (tests), fluxer, discord.
7. **Delete Go**: `go/`, `third_party/pins.txt`, `scripts/build-go.sh`, `scripts/fetch-third-party.sh`,
   the `go` CI job, the cgo bits in the release workflow, the `static-link`/`dynamic-link` features
   and `build.rs` of `chirper-core`.
8. **Docs and CI**: this file, `ARCHITECTURE.md`, `CONTRACT.md`, `BUILDING.md`, `NETWORKS.md`.

Golden fixtures: before deleting the Go implementation, capture envelopes from `go/core/e2e_test.go`
and the facade tests into `crates/chirper-core/tests/golden/*.json`. The Rust port asserts against
those strings, so "the UI does not change" is a test, not a promise. The most important ones
(network row, chat, message, login step) are in this commit's follow-up.

---

## 9. Decision log

- **D1** One bridge runtime crate, one crate per network. No shared "bridge SDK" clone of
  bridgev2's class hierarchy: traits + a runtime, nothing else.
- **D2** Domain events, not Matrix events. The engine persists; drivers never touch SQL.
- **D3** Keep the legacy id strings on disk for compatibility; no Matrix types in code.
- **D4** One database. Per-bridge DBs are dead; legacy files are imported by migration v6.
- **D5** No C ABI. `src-tauri` links `chirper-core` directly.
- **D6** `OCV1` vault format is frozen; media written by the Go build stays readable.
- **D7** Networks that are not ported are listed as unavailable with a reason, not removed.
- **D8** `#![forbid(unsafe_code)]` in every crate; if a network SDK needs FFI it gets its own
  crate with a documented, encapsulated `unsafe` block.
