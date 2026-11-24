# Task `whatsapp`: WhatsApp phase 2 in `crates/chirper-net-whatsapp`

**Owned:** `crates/chirper-net-whatsapp/**`. Read `README.md` and `_driver-common.md` first.
**Uses:** `chirper-signal-protocol` (path dependency `../chirper-signal-protocol`) for every Signal
protocol operation: sessions, prekeys, sender keys. Do not re-implement any of it here; if that
crate lacks something, stop and report exactly what (function, behaviour), do not edit it.
**Reference:** whatsmeow, `~/go/pkg/mod/go.mau.fi/whatsmeow@v0.0.0-20261116172437-19d61352436f/`
(`message.go`, `send.go`, `receipt.go`, `prekeys.go`, `notification.go`, `appstate/`,
`appstate.go`, `historysync.go`, `download.go`, `upload.go`, `mediaconn.go`, `group.go`,
`user.go`, `presence.go`, `pair-code.go`, `retry.go`, `proto/`), and the mautrix bridge
`/Volumes/SSD/Code/Chirper/third_party/whatsapp/pkg/connector/` + `pkg/msgconv/` for behaviour and ids
(`pkg/waid/`). Port the protobuf messages you need (`waE2E.Message` subset, `waHistorySync`,
`waServerSync`, `waSyncAction`, `waMmsRetry`, `waWeb.WebMessageInfo`) as hand-written `prost`
structs in `proto.rs`/`proto/` with attribution, keeping field numbers.

## Starting point (phase 1, keep it)

Noise XX handshake, binary node codec + token tables, JIDs, QR pairing (`pair.rs`), registration
and login payloads, the `Connection` socket, device keys. The QR login works up to `success`.

## What to build

1. **Store** (`store.rs`): implement the `chirper-signal-protocol` store traits over
   `LoginCtx` secrets (one secret per record: `identity`, `prekey/<id>`, `signed-prekey`,
   `session/<jid>.<device>`, `sender-key/<group>/<sender>`, `identity-key/<jid>`), plus
   app-state sync keys (`appstate-key/<id>`), app-state versions/hashes (`appstate/<name>`), and the
   device record (`device`, already written by the pairing). Writes after every mutation.
2. **Login flows**: `qr` (existing) and `phone` (pairing code: `pair-code.go` — link-code
   companion registration, show the 8-character code as `display_and_wait` kind `code`, finish
   on `pair-success` exactly like QR). `login_id` as today (phone number user part) — check
   `waid` and keep it identical to the Go bridge's `UserLoginID`.
3. **Connect**: `Connection` with the login payload; on `success`: upload prekeys when the server
   count is low (`prekeys.go`: 812 → refill to 812, 30 per batch as whatsmeow), set passive false,
   send presence `available` only if the reference does, resync app state, then `Running`.
   Keep-alive pings (`keepalive.go` intervals), handle `stream:error` codes 515 (reconnect),
   401 / `conflict` / `device_removed` (→ `Error::Auth`, LoggedOut).
4. **Receiving**: `<message>` nodes → decrypt each `<enc>` (`pkmsg`, `msg`, `skmsg`) with the
   protocol crate; unpad (`unpadRandomMax16`), decode `waE2E.Message`; send `receipt`s
   (delivery, and `retry` receipts on decryption failure per `retry.go`, with a bounded retry
   counter), process `senderKeyDistributionMessage`, `protocolMessage` (revoke → `Event::Redact`,
   edit → edit, history sync notification → download + inflate + import, app-state key share →
   store keys), reactions → `Event::Reactions` (keep a per-message reaction map in memory, bounded,
   since WhatsApp sends one reaction per user), `<receipt>` (read → `Event::ReadReceipt`, delivery →
   `Event::Delivery`), `<chatstate>` → `Typing`, `<notification>` (devices, groups, picture,
   encrypt/prekey count), `<call>` → a `Notice` "Missed call".
5. **Content**: text (`conversation`, `extendedTextMessage` with WhatsApp formatting `*b*` `_i_`
   `~s~` ```` ```code``` ```` → HTML subset, mentions), quoted replies (`contextInfo.stanzaId`),
   images/video/audio/voice (`ptt`)/documents/stickers: download from the media hosts
   (`mediaconn`), decrypt (HKDF info `WhatsApp Image Keys` etc., AES-256-CBC, HMAC-SHA256 10-byte
   MAC check, SHA-256 of the file checked against `fileEncSha256`/`fileSha256`), location,
   contacts (vCard → `Unsupported` with the name), polls/view-once/ephemeral wrappers unwrapped
   where safe (`ephemeralMessage`, `viewOnceMessage*`, `documentWithCaptionMessage`,
   `editedMessage`, `deviceSentMessage` → `from_me`).
6. **Sending**: text, replies, media (encrypt + upload to the media host, thumbnails optional),
   edits, revokes, reactions, read receipts (`read` receipts per sender), typing (`chatstate`).
   1:1: fetch the recipient's and our own other devices (`usync` devices query), establish
   sessions from prekey bundles (`encrypt` iq `get` key), encrypt per device, `<participants>`
   fan-out, `deviceSentMessage` wrapper for our own devices. Groups: sender-key encrypt, send
   the distribution message to devices that lack it (`participants` with `skmsg` +
   per-device `pkmsg`), as `send.go`. Message ids as whatsmeow `GenerateMessageID`.
7. **Chats**: groups (`w:g2` `participating` query) and the chat list from history sync /
   app state (`archive`, `pin`, `mute`, contact names from `contact` actions and push names).
   Avatars: `profile picture` iq (url only, the engine downloads).
8. **Backfill**: WhatsApp has no server-side history paging; history arrives through history
   sync. Buffer the history-sync messages not yet delivered **through the secrets store** (they
   hold message bodies, so never as plain files): `history/<chat>/<page>` blobs of up to 50
   encoded `WebMessageInfo`, deleted once delivered. Serve `backfill` from that buffer;
   `has_more` is false when it is exhausted. On-demand history (`peerDataOperation`
   `HISTORY_SYNC_ON_DEMAND` to the primary phone) when the local buffer is empty, as whatsmeow
   `BuildHistorySyncRequest`.
9. Capabilities: all (match the Go build), `voice` true.

## Tests

Node fixtures for every receive path (build the nodes with the codec), decrypt with sessions set
up between two in-memory stores (simulate the phone with the protocol crate), media decrypt with
a fixed vector (encrypt with your own encrypt function, plus a known-answer vector from
whatsmeow's tests if present), app-state patch decoding with LTHash checks (`appstate/lthash`),
history-sync parsing from a small handcrafted protobuf, the 1:1 and group send fan-out (assert
the produced `<message>` node), pairing-code generation with fixed randomness.

## Done means

The crate passes tests/clippy/fmt; `unavailable()` returns `None`; `examples/qr_probe.rs` still
builds; add `examples/session_probe.rs` (connect a stored login from a temp data dir, print chat
count and incoming message count only).
