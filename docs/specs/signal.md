# Task `signal`: Signal in `crates/chirper-net-signal`

**Owned:** `crates/chirper-net-signal/**`. Read `README.md` and `_driver-common.md` first.
**Uses:** `chirper-signal-protocol` (path dependency) for sessions, prekeys, sender keys and
sealed sender. If it lacks something, stop and report exactly what; never edit it.
**Reference:** `/Volumes/SSD/Code/Chirper/third_party/signal/pkg/` — `signalmeow/` is the protocol
client (provisioning, websocket, sending/receiving, attachments, profiles, groups, storage
service, contact sync), `connector/` the behaviour, `msgconv/` conversion, `signalid/` the ids.
The libsignal v0.106.1 Rust sources are at
`/Volumes/SSD/Code/Chirper/third_party/signal/pkg/libsignalgo/libsignal/rust/` (`zkgroup`,
`zkcredential`, `poksho`, `account-keys`, `protocol`, `net`): port what you need from there
(AGPL-3.0, attribute each file), never depend on it.

The Go build listed Signal as unavailable, so everything here is **beyond parity**; it still must
be complete and tested before `unavailable()` returns `None`.

1. **Linking** (flow id `qr`): provisioning websocket (`/v1/websocket/provisioning/`), show
   `sgnl://linkdevice?uuid=<id>&pub_key=<base64>` as `display_and_wait` kind `qr`, decrypt the
   `ProvisionEnvelope` (`provisioning_cipher.go`), register the linked device
   (`PUT /v1/devices/link` with signed EC and Kyber prekeys, registration id, the device name
   encrypted as `devicename.go`), store ACI/PNI identities, password, device id, profile key,
   master key / account entropy pool. `login_id` per `signalid`.
2. **Keys**: upload one-time EC and Kyber prekeys, signed prekeys, last-resort Kyber key for ACI
   and PNI; refill when the server count drops (as `keys.go`).
3. **Chat connection**: authenticated websocket (`/v1/websocket/`) with the reference's headers,
   keep-alive, request/response multiplexing (`wspb` WebSocketMessage), queue-empty marker.
   TLS with rustls against Signal's own CA when the reference pins it (port the pin).
4. **Receiving**: envelopes (ciphertext, prekey, sealed sender, plaintext content), decrypt,
   `Content` → `DataMessage` (text with body ranges → HTML subset, quotes, attachments, reactions,
   deletes, stickers as media), `EditMessage`, `TypingMessage`, `ReceiptMessage`, `SyncMessage`
   (sent transcripts → `from_me`, read sync, contacts, blocked), decryption-error retry
   (`retry.go`), server delivery receipts. Acknowledge each request.
5. **Sending**: 1:1 and groups (sender keys + sealed sender; multi-recipient sealed sender v2 if
   the reference uses it, else per-device v1), mismatched/stale device handling (409/410),
   unidentified access keys from profile keys, sync transcripts to our own devices; text,
   replies, attachments (upload v4 via the attachment form, AES-256-CBC + HMAC-SHA256, digest),
   edits, deletes, reactions, typing, read receipts.
6. **Groups v2**: master key → `GroupSecretParams` (zkgroup), auth credentials
   (`/v1/certificate/auth/group`), presentation, fetch group state, decrypt title/description/
   avatar/members (AES-GCM-SIV blobs: use `aes-gcm-siv` if you must add one crate; it is RustCrypto).
   Port zkgroup's needed parts into `src/zkgroup/` with libsignal's own test vectors.
7. **Profiles**: fetch and decrypt names/avatars with the profile key; contacts from the storage
   service or contact sync.
8. **Backfill**: Signal has no server history; `backfill` returns an empty page with
   `has_more: false`. Capabilities: all except `stickers` sending if not done.

Tests: provisioning decrypt vector (encrypt with your own implementation using fixed keys, plus a
vector from the Go tests if present), envelope decrypt through two in-memory protocol stores,
zkgroup vectors from libsignal's tests, attachment encrypt/decrypt with digest, the websocket
request/response multiplexer over a duplex stream, a fake server end-to-end (link → connect →
receive → send).
