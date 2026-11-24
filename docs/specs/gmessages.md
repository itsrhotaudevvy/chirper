# Task `gmessages`: Google Messages in `crates/chirper-net-gmessages`

**Owned:** `crates/chirper-net-gmessages/**`. Read `README.md` and `_driver-common.md` first.
**Reference:** `/Volumes/SSD/Code/Chirper/third_party/gmessages/pkg/libgm/` (the whole protocol
client: `crypto/` AES-CTR+HMAC and AES-GCM, `gmproto/` protobufs, `pblitedecode/`, long-poll
`/$rpc/...ReceiveMessages`, pairing, session refresh, `events/`) and `connector/`. Port the
`.proto` files you need as hand-written `prost` structs with attribution.

- Login flows `qr` (pair with the phone: `RegisterPhoneRelay`, QR URL
  `https://support.google.com/messages/?p=web_computer#?c=<base64>`, refresh every 30 s, wait for
  `PairSuccessful` on the long-poll) and `google` (Google account pairing: cookies step on
  `messages.google.com` + emoji confirmation shown as `display_and_wait` kind `emoji`), as the
  reference does. Store the auth data (tachyon token, keys, browser device) as JSON in a secret.
- Session: long-poll receive loop with ack batching, token refresh before expiry, the
  "phone not responding" states mapped to `TransientDisconnect`, conversations list, messages
  (SMS, MMS media download+decrypt, RCS reactions, typing, read receipts), send text/media,
  reactions, delete, mark read. Backfill with `ListMessages` cursors.
