# Task `telegram-credentials`: Telegram API id from settings, as the Go build allowed

The Go build let the user enter a Telegram API id/hash in Settings → Advanced
(`ui/src/screens/settings/AdvancedSection.svelte`, stored as `settings.telegram = { apiId,
apiHash }`; the default `{ "apiId": 0, "apiHash": "" }` is pinned by the golden test). The Rust
Telegram driver only reads `CHIRPER_TELEGRAM_API_ID/HASH` at compile time, so a build without them
lists Telegram as unavailable even when the user entered credentials.

**Owned:** `crates/chirper-net-telegram/src/lib.rs` (and new files in that crate if needed),
`crates/chirper-core/src/engine/networks.rs`, `crates/chirper-core/src/engine/mod.rs`,
`crates/chirper-core/src/engine/settings.rs`, `crates/chirper-core/tests/**`.

1. `chirper_net_telegram::Telegram::with_credentials(api_id: i32, api_hash: String)`; `new()`
   keeps using the compile-time values. Precedence: valid user settings (`apiId > 0` and a
   32-hex-character `apiHash`) override the compiled-in pair; otherwise the compiled-in pair;
   otherwise unavailable with the existing reason, reworded to say the id can be entered in
   Settings → Advanced. Every place in the crate that calls `api_credentials()` must use the
   credentials of the instance instead (thread them through the login and the session).
2. The registry (`networks::available`) takes the Telegram credentials from the settings that
   `Core::open` already reads (it reads `demo_network` the same way). A settings change applies at
   the next start, as in the Go build (`SetSettings` doc: "applies at the next Start"); if the
   engine can rebuild the registry on `set_settings` cheaply and safely, do that instead and test it.
3. Validation: `set_settings` rejects an `apiId` that is negative or not an integer and an
   `apiHash` that is neither empty nor 32 hex characters, with a clear error.
4. Tests: precedence (settings over compiled, compiled when settings are empty, unavailable when
   neither); the network row turns available after valid settings and a restart (or immediately,
   if 2 rebuilds); invalid values are rejected; the golden settings test still passes.

Verify: `cargo test -p chirper-net-telegram` (with and without `CHIRPER_TELEGRAM_API_ID=1
CHIRPER_TELEGRAM_API_HASH=test`), `cargo test -p chirper-core`, clippy `-D warnings` and fmt for
both crates.
