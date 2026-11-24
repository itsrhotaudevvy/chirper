# Task `twitter`: X (Twitter) DMs in `crates/chirper-net-twitter`

**Owned:** `crates/chirper-net-twitter/**`. Read `README.md` and `_driver-common.md` first.
**Reference:** `/Volumes/SSD/Code/Chirper/third_party/twitter/pkg/` (`twittermeow/` client:
`methods/`, `data/` types, `cookies/`, `crypto/`; `connector/` behaviour; `juiceboxgo/` is the
Juicebox PIN-key recovery for encrypted XChat). It is the largest reference (36k lines): do the
plain-DM path completely first, XChat second.

- Login flow `cookies` (`LoginStep::Cookies` on `https://x.com/i/flow/login`, cookies `auth_token`,
  `ct0`, plus the reference's extract JS). Skip `password` (castle-token webview) and list it.
- Headers exactly as the reference (bearer constant, `x-csrf-token` = `ct0`, client transaction
  id generation if it computes one — port and test it with fixed inputs).
- Plain DMs: inbox initial state, `user_updates` polling with cursor (reference interval),
  conversation sync, messages, edits, deletes, reactions, read marks, media upload (INIT/APPEND/
  FINALIZE) and download, typing.
- XChat (encrypted DMs): port the key handling and message decryption/encryption only if the
  whole chain (Juicebox recovery → keys → decrypt) can be tested offline with vectors from the
  reference's tests. Otherwise list XChat as not done and report XChat conversations as
  `Unsupported` messages ("Encrypted X chat — open X to read").
