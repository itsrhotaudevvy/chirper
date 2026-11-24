# Task `telegram`: Telegram phase 2 in `crates/chirper-net-telegram`

**Owned:** `crates/chirper-net-telegram/**`.

## Starting point (phase 1, done, keep it)

`tl/` codegen from `tl/api.tl` + `tl/mtproto.tl` restricted by `tl/allowlist.txt` (build.rs; add
the functions you need to the allowlist, the type closure follows), `mtproto/` with transport
(obfuscated abridged/intermediate over TCP), auth-key generation, `Core` (encrypted session state
machine) and `MtSession` (`invoke<R: RemoteCall>`, update frames, `InvokeError::{Migrate, RateLimited, Rpc}`).
`api_credentials()` reads `CHIRPER_TELEGRAM_API_ID`/`CHIRPER_TELEGRAM_API_HASH` at compile time.

## Reference

`/Volumes/SSD/Code/Chirper/third_party/telegram/pkg/connector/` (mautrix-telegram on gotd):
`login*.go`, `client.go`, `handletelegram.go` (updates → events), `tomatrix.go` (message
conversion), `backfill.go`, `chatsync.go`, `reactions.go`, `media/`, `telegramfmt/` (entities →
formatted body). Update-gap handling as in the Telegram docs (`pts`, `qts`, `seq`, `date`,
`updates.getDifference`, `updates.getChannelDifference`).

## What to build

1. **Client layer** (`client/`): one `Client` that owns the `MtSession` for the home DC, wraps every
   call in `invokeWithLayer(initConnection(...))` on first use (device model "Chirper", system
   version from `std::env::consts::OS`, app version from `CARGO_PKG_VERSION`, lang "en"), follows
   `*_MIGRATE_X` by generating/loading the auth key for that DC and retrying once, maps
   `FLOOD_WAIT_X` to `Error::RateLimited`, `AUTH_KEY_UNREGISTERED`/`SESSION_REVOKED`/
   `USER_DEACTIVATED*` to `Error::Auth`, transport failures to `Error::Network`. File downloads from
   other DCs use `auth.exportAuthorization`/`importAuthorization` and a cached per-DC session.
2. **Persistence.** Secrets (via `LoginCtx` secrets): `auth_key/<dc>` (256 bytes), `home_dc`.
   Non-secret state in `ctx.data_dir/state.json` (written atomically: temp file + rename, mode
   0600): server salt, time offset, update state (`pts`, `qts`, `date`, `seq`), per-channel `pts`,
   and a peer cache (user/channel id → access hash, minimal names) bounded to 50 000 entries.
3. **Login flows** (ids exactly `phone`, `qr`, `bot`):
   - `phone`: `user_input` phone (kind `phone`) → `auth.sendCode` → `user_input` code (kind
     `number`) → `auth.signIn`; `SESSION_PASSWORD_NEEDED` → `account.getPassword` → `user_input`
     password (kind `password`) → SRP (`inputCheckPasswordSRP`; algorithm
     `passwordKdfAlgoSHA256SHA256PBKDF2HMACSHA512iter100000SHA256ModPow`, use `pbkdf2` + `hmac` +
     `sha2` + `num-bigint`, validate `p`/`g` with the existing `check_dh_params`) → `auth.checkPassword`.
     Wrong code → the same step again with an error in `instructions`. `PHONE_MIGRATE_X` switches DC.
     Sign-up of new accounts is **not** supported: return `Error::Auth("this phone number has no
     Telegram account")`.
   - `qr`: `auth.exportLoginToken` → `display_and_wait` kind `qr`, data
     `tg://login?token=<base64url>`; refresh on expiry; `updateLoginToken` → export again →
     `loginTokenSuccess` / `loginTokenMigrateTo` (import on the other DC); 2FA → password step.
   - `bot`: `user_input` token → `auth.importBotAuthorization`.
   - `login_id` = the user's numeric id as a string. Profile reported via `Event::State.profile`.
4. **Session** (`Session` impl):
   - On connect: `updates.getState` if no stored state, else `updates.getDifference` loop until
     `differenceEmpty`/`difference` (handle `differenceSlice`, `differenceTooLong` → reset to
     fresh state and resync dialogs). Then `messages.getDialogs` paged (all dialogs, folder 0 and
     1) → `Event::Chat` for each, with `is_dm` for users, `member_count` where known.
   - Live updates: `updateNewMessage`, `updateNewChannelMessage`, `updateEditMessage`,
     `updateEditChannelMessage`, `updateDeleteMessages`, `updateDeleteChannelMessages`,
     `updateUserTyping`, `updateChatUserTyping`, `updateChannelUserTyping`,
     `updateMessageReactions`, `updateReadHistoryInbox/Outbox`, `updateReadChannelInbox`,
     `updateShortMessage`, `updateShortChatMessage`, `updateShortSentMessage`, `updates`,
     `updatesCombined`, `updatesTooLong` (→ getDifference), channel `pts` gaps
     (→ `updates.getChannelDifference`). Apply pts ordering: a gap waits 0.5 s then fetches the
     difference, duplicates are dropped.
   - **Ids must equal the Go bridge's** (`pkg/connector/ids/ids.go`), because databases written by the
     Go build are keyed by them: portal `user:<id>`, `chat:<id>`, `channel:<id>`, forum topics
     `channel:<id>:<topic>` (check the `PeerType` string values there); message id `<msg_id>`, or
     `<channel_id>.<msg_id>` in channels; user ids `<id>` and `channel-<id>`; login id `<user_id>`;
     `PortalRef.receiver` = the login id for user and chat portals, empty for channels (check
     `MakePortalKey` and follow it exactly).
   - Content: text with entities → `Content::Text { formatted }` (HTML subset: b, i, u, s, code,
     pre, a, blockquote, spoiler as `<span data-mx-spoiler>`); photos, documents (video, audio,
     voice, sticker, file by attributes), geo → `Location` (`geo:lat,long`), service messages →
     `Notice` with a human sentence; polls, games, invoices → `Unsupported` with a description.
     Media download via `upload.getFile` in 512 KiB chunks, max 100 MiB, from the right DC.
   - Commands: `SendText` (`messages.sendMessage` with `reply_to`, random_id from `getrandom`),
     `SendMedia` (`upload.saveFilePart`/`saveBigFilePart` ≥ 10 MiB, then `messages.sendMedia` with
     `inputMediaUploadedPhoto` for images else `inputMediaUploadedDocument` with filename/mime/
     audio-voice/video attributes), `Edit`, `Redact` (`messages.deleteMessages` revoke=true or
     `channels.deleteMessages`), `React`/`Unreact` (`messages.sendReaction`, emoticon reactions;
     custom emoji keys `custom:<id>`), `Read` (`messages.readHistory`/`channels.readHistory`),
     `Typing` (`messages.setTyping` typing/cancel). The `Ack.remote_event_id` is the new message
     id from the returned updates; `Event::Delivery{Sent}` after the ack.
   - `backfill`: `messages.getHistory` with `offset_id` cursor, `limit` ≤ 100, oldest-first order
     within the page, `has_more` when a full page came back.
   - `logout_remote`: `auth.logOut`.
   - Capabilities: edits, reactions, replies, redactions, typing, read receipts, stickers, voice.
5. `unavailable()` returns `Some(..)` only when `api_credentials()` is `None`.

## Tests (no network)

- SRP: a known-answer test using the example values from Telegram's SRP documentation, or values
  computed once from the Go implementation (`gotd/td/internal/crypto/srp`), pinned.
- TL round trips for every newly allowlisted constructor you construct or parse.
- Update-state machine: pts gap detection, duplicate drop, getDifference application, channel
  gaps — pure functions over a state struct.
- Message conversion: entities → HTML for nested/overlapping entities and UTF-16 offsets
  (Telegram offsets are UTF-16 code units: test with emoji and CJK).
- Login state machines driven against a fake `invoke` (make the login and session generic over
  an `Api` trait with an in-memory implementation that returns canned TL objects).
- Peer-cache bound and atomic state file writes (temp dir).

## Done means

The crate passes tests and clippy; `unavailable()` is `None` with credentials set (verify with
`CHIRPER_TELEGRAM_API_ID=1 CHIRPER_TELEGRAM_API_HASH=x cargo test -p chirper-net-telegram`).
Keep `examples/authkey_probe.rs` working; add `examples/login_probe.rs` (phone login from
environment variables, prints only the user id) for the lead to run live.
