# Task `bluesky`: Bluesky DMs in `crates/chirper-net-bluesky`

**Owned:** `crates/chirper-net-bluesky/**`. Read `README.md` and `_driver-common.md` first.
**Reference:** `/Volumes/SSD/Code/Chirper/third_party/bluesky/pkg/connector/` (all files; it uses
the `indigo` XRPC client — reimplement the few XRPC calls it makes with `reqwest` + serde; no
`atrium`/`indigo`-like crate).

- Login flow `password` with fields `domain` (default `bsky.social`), `username`, `password`
  (app passwords recommended in the instructions). Resolve the PDS: handle → DID via
  `com.atproto.identity.resolveHandle`, DID document (`plc.directory` for `did:plc`,
  `/.well-known/did.json` for `did:web`) → PDS endpoint. `com.atproto.server.createSession`;
  `2FA` (`AuthFactorTokenRequired`) → extra `user_input` step `code`. Store access + refresh JWT
  and PDS URL; refresh with `com.atproto.server.refreshSession` on `ExpiredToken`.
- Chat calls go to the PDS with header `atproto-proxy: did:web:api.bsky.chat#bsky_chat`:
  `chat.bsky.convo.listConvos` (paged), `getLog` (cursor-based event log: polling loop as in the
  reference, default 5 s, back off to 30 s when idle), `getMessages` (backfill),
  `sendMessage` (with facets for links/mentions), `deleteMessageForSelf`, `addReaction`,
  `removeReaction`, `updateRead`. Profiles via `app.bsky.actor.getProfiles` (batch 25).
- Ids exactly as `ids.go`. Capabilities: reactions, redactions (for self only: say so in the
  capability doc comment), read receipts, replies off unless the API supports them.
- Rich text: facets (byte offsets in UTF-8!) ↔ `formatted` links; test with emoji.
