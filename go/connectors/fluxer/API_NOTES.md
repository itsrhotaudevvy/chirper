# Fluxer API notes (for the Chirper bridgev2 connector)

All paths below are relative to `third_party/fluxer/` unless stated otherwise.
Doc root: `fluxer_docs/src/content/docs/` (abbreviated `DOCS/` here).
Source references use `fluxer_api/`, `fluxer_app/`, `fluxer_rexctl/`, `packages/`.

Anything not listed here is NOT used by the connector. Where the docs were ambiguous
the Fluxer source was consulted and noted.

## 1. Discovery and base URLs

- `GET {origin}/.well-known/fluxer` (no auth). Returns `endpoints` with absolute URLs:
  `api_public` (bots/third-party clients; REST base, then `/v1/...`), `api_client`, `gateway`
  (WebSocket `ws`/`wss`), `media`, `static_cdn`, `webapp`. Do not build URLs from the origin.
  - `DOCS/index.md` "Endpoint discovery"
  - `DOCS/http-api/instance.mdx` "Get instance discovery" (route `/.well-known/fluxer`) and
    "Instance endpoints object" (`api`, `api_client`, `api_public`, `gateway`, `media`, ...).
- Default instance: `https://fluxer.app`. Login accepts a custom instance URL (self-hosted).
- Discovery also has `captcha.provider` (`altcha` | `none`), `features`, `limits`
  (`max_message_length`, `max_attachments_per_message`, `max_attachment_file_size`,
  `max_bot_attachment_file_size`), `account_identity`.
  - `DOCS/http-api/instance.mdx` "Instance discovery object", "CAPTCHA configuration object",
    "Limit keys" (`max_message_length` default 4000 in the example; bots always get at least 4000).

## 2. Authentication headers

- User session token: `Authorization: <token>` (raw, no prefix). Also accepted as `Bearer <token>`.
- Bot token: `Authorization: Bot <token>`. Bot token format `<application_id>.<secret>`.
  - `DOCS/authentication.md` "Authorization schemes", "Token formats".
- Bots cannot call user-only routes (403 `ACCESS_DENIED`). Routes marked `bot` in the docs are
  bot-callable; the mark is `<RouteHeader ... bot />` in each `.mdx` file.
- Optional headers used: `User-Agent` (we send `Chirper (https://github.com/chirper, 0.1)`),
  `Accept-Language`, `X-Captcha-Token`, `X-Audit-Log-Reason` (not used),
  `X-Request-ID` (not used). Source: `DOCS/http-api/index.md` "Standard request headers".

## 3. Snowflakes and IDs

- All snowflakes are JSON strings of decimal digits; parse as uint64 (`DOCS/snowflakes.md`).
- Timestamp = `(id >> 22) + 1420070400000` (epoch 2015-01-01). Not needed by the connector
  except for cursor construction (not used).

## 4. Errors and rate limits

- Error body: `{"code": "...", "message": "...", "errors": [...]}` (`DOCS/http-api/index.md`
  "Error response object"; full code list in `DOCS/http-api/errors.md`).
- 429 `RATE_LIMITED` body has `retry_after` (seconds, decimal) and `global` (bool).
  Headers: `Retry-After`, `X-RateLimit-Scope`, `X-RateLimit-Remaining`, `X-RateLimit-Reset-After`.
  Use `retry_after` from the body, and pause all requests with that token when `global` is true
  (`DOCS/topics/rate-limits.md` "Rate limit response object", "Handling a 429").
- Global bucket is 50 req/s per caller; a global denial REVOKES a user session token
  (`DOCS/topics/rate-limits.md` caution box). The client must never retry a global-denied user
  token in a loop.
- CAPTCHA-gated errors: 400 `CAPTCHA_REQUIRED` / `INVALID_CAPTCHA` with `captcha_provider` and
  `altcha_challenge` (`DOCS/topics/captcha.md`).

## 5. Login (password + optional TOTP)

- `POST /v1/auth/login` (unauthenticated; `DOCS/http-api/authentication.mdx` "Log in with a password").
  - JSON body: `{"login": "<email or username>", "password": "...", "invite_code"?: ...}`.
    (`email` is accepted as an alias on older apps.) Password 8-256 chars.
  - Header `X-Captcha-Token` required when `captcha.provider == altcha`.
  - 200 response: either the authentication token response
    `{"token", "user_id", "user", "device_proof"?, "entry"?}` (`token` is the session token,
    "returned only here"), or the MFA challenge `{"mfa": true, "ticket", "allowed_methods",
    "totp", "webauthn", "backup_codes"}`.
  - 403 `IP_AUTHORIZATION_REQUIRED` (new IP, sends email, needs polling) -> NOT supported by
    this connector; returned as a clear error.
  - Rate limit: 10 req/10s; `DOCS/topics/rate-limits.md`.
- `POST /v1/auth/login/mfa/totp` (`DOCS/http-api/authentication.mdx` "Complete login with TOTP"):
  body `{"code": "<6-digit or backup code>", "ticket": "<ticket>"}` -> authentication token
  response. Ticket valid 5 minutes.
- WebAuthn MFA (`/v1/auth/login/mfa/webauthn*`) is documented but NOT supported (needs a
  browser/passkey); the connector reports "not supported yet" if the challenge offers only WebAuthn.
- ALTCHA (`DOCS/topics/captcha.md`, "Solve a challenge", "Token format"):
  - Challenge fields in `altcha_challenge.parameters`: `algorithm` (`PBKDF2/SHA-256`), `nonce`
    (hex), `salt` (hex), `cost` (PBKDF2 iterations), `keyLength`, `keyPrefix` (hex), plus
    `keySignature`, `expiresAt`; `altcha_challenge.signature` (hex).
  - Solver: `password = nonce_bytes || uint32_BE(counter)` for counter 0,1,2,...;
    `derivedKey = PBKDF2-HMAC-SHA256(password, salt_bytes, cost, keyLength)`; stop when
    `derivedKey` starts with `keyPrefix` bytes. Reference implementation:
    `fluxer_rexctl/internal/setup/captcha.go` (`searchAltcha`, `solveAltcha`).
  - Server verification: `fluxer_api/pkgs/captcha/src/providers/AltchaProvider.ts`
    (`AltchaPayloadSchema`: `challenge{parameters,signature}`, `solution{counter,derivedKey,time?}`).
  - Token = base64(StdEncoding) of JSON `{"challenge": <altcha_challenge as received>,
    "solution": {"counter": n, "derivedKey": "<hex>"}}`, at most 4096 chars.
  - Retry at most 3 times on `INVALID_CAPTCHA` (reference client does the same).
- Logout: `POST /v1/auth/logout` (bot-callable; revokes the session). Used on logout for user tokens.

## 6. Current user, guilds, channels

- `GET /v1/users/@me` (bot-callable). User object: `id`, `username`, `discriminator`,
  `global_name`, `avatar`, `bot?`, `email?`, `verified`, ... (`DOCS/http-api/users.mdx`
  "User object"; `DOCS/http-api/users/current-user.mdx` "Get current user").
- `GET /v1/users/@me/guilds` (bot-callable). Array of guild objects, ascending id, `limit` up to 200
  (`DOCS/http-api/guilds.mdx`).
- `GET /v1/guilds/{guild_id}` (bot-callable), used for guild (space) portals.
- `GET /v1/guilds/{guild_id}/channels` (bot-callable). Array of channel objects
  (`DOCS/http-api/guild-channels.mdx`).
- `GET /v1/users/@me/channels` (bot-callable). Open DM and group DM channels, unpaginated
  (`DOCS/http-api/users/private-channels.mdx`).
- `POST /v1/users/@me/channels` (bot-callable). Body `{"recipient_id": "<id>"}` opens a DM
  (returns the same channel when it exists). Captcha is required only for group DMs.
- Channel object (`DOCS/http-api/channels.mdx` "Channel object", "Channel types"):
  - `id`, `type`, `guild_id?`, `name?`, `topic?`, `position?`, `parent_id?`,
    `recipients?` (partial users, caller excluded), `owner_id?`, `icon?`, `nsfw?`,
    `permission_overwrites?`, `last_message_id?`.
  - Types used: `0 GUILD_TEXT`, `1 DM`, `2 GUILD_VOICE`, `3 GROUP_DM`, `4 GUILD_CATEGORY`,
    `5 GUILD_ANNOUNCEMENT`, `10/11/12` threads (ignored), `15 GUILD_FORUM`, `16 GUILD_MEDIA`.
    Forum/media/threads have no messages in the plain message API (`INVALID_CHANNEL_TYPE`).
- Guild object (`DOCS/http-api/guilds.mdx` "Guild object"): `id`, `name`, `icon?`, `owner_id`,
  `member_count?`, `features`. Fields for guild icon: `icon` hash (`DOCS/media-proxy/routes.mdx`
  "Get guild icon", path `/icons/{guild_id}/{hash}.{ext}`).
- Avatar URL: `{endpoints.media}/avatars/{user_id}/{hash}.{ext}` (animated hashes start with `a_`,
  use `gif`; otherwise `webp`/`png`), from `DOCS/media-proxy/routes.mdx` "Get user avatar".

## 7. Messages

Partial user object (`DOCS/http-api/users.mdx` "Partial user object"): `id`, `username`,
`discriminator`, `global_name?`, `avatar?`, `bot?`, `system?`, `flags`.

Message object (`DOCS/http-api/messages.mdx` "Message object"):
`id`, `channel_id`, `author`, `webhook_id?`, `type`, `flags`, `content`, `timestamp` (ISO8601),
`edited_timestamp?`, `pinned`, `mentions[]`, `mention_roles[]`, `mention_channels?`, `users?`,
`embeds[]`, `attachments[]`, `stickers[]`, `reactions?`, `message_reference?`,
`message_snapshots?`, `nonce?`, `call?`, `referenced_message?`, `thread?`.

Message types used: `0 DEFAULT`, `1 RECIPIENT_ADD`, `2 RECIPIENT_REMOVE`, `3 CALL`,
`4 CHANNEL_NAME_CHANGE`, `5 CHANNEL_ICON_CHANGE`, `6 CHANNEL_PINNED_MESSAGE`, `7 USER_JOIN`,
`12 CHANNEL_FOLLOW_ADD`, `18 THREAD_CREATED`, `19 REPLY`, `21 THREAD_STARTER_MESSAGE`.
Only `0` and `19` are rendered as text; system types are rendered as `state`/`unsupported`.

Attachment object (`DOCS/http-api/messages.mdx` "Message attachment object"): `id`, `filename`,
`title?`, `description?`, `content_type`, `size`, `url` (nullable, signed, valid 24h via `is`),
`proxy_url`, `width?`, `height?`, `duration?`, `waveform?`, `expires_at`, `expired?`.
Signed URLs keep `ex`, `is`, `hm` query params (`DOCS/http-api/messages.mdx` "Signed attachment URLs").
When `url` is null or expired, refetch the message (`GET .../messages/{id}`) or use
`POST /v1/attachments/refresh-urls` (bot-callable; `DOCS/http-api/messages.mdx` "Refresh attachment URLs").

Reaction object: `emoji {name, id?, animated?}`, `count`, `me?`. Emoji path value: URL-encoded
`name` for Unicode, `name:id` for custom emoji (`DOCS/http-api/messages.mdx` "Reaction emoji path value").

Mentions in content: `<@id>` or `<@!id>` for users, `<@&id>` roles, `<#id>` channels
(`packages/markdown_parser/rust/src/links.rs`, the `b'@'` and `b'#'` arms).

Message reference input: `{"message_id": "...", "channel_id"?, "guild_id"?, "type"?}`
(`DOCS/http-api/messages.mdx` "Message reference input object"; reply = type 0 default).

### Endpoints used (all bot-callable unless noted)
| Operation | Route | Doc reference |
|---|---|---|
| List messages | `GET /v1/channels/{channel_id}/messages?limit=1-100&before=&after=&around=` (100 req/10s) | messages.mdx "List channel messages" |
| Get message | `GET /v1/channels/{channel_id}/messages/{message_id}` | messages.mdx "Get message" |
| Create message (JSON) | `POST /v1/channels/{channel_id}/messages` body `{content, nonce, message_reference, allowed_mentions?}`; 20 req/10s | messages.mdx "Create message" |
| Create message (multipart) | same route, `multipart/form-data` with `payload_json` + `files[0]` (attachment metadata `{id:0, filename, content_type, title?, description?}`) | messages.mdx "Multipart body" |
| Modify message | `PATCH /v1/channels/{channel_id}/messages/{message_id}` body `{content, allowed_mentions?}`; 20 req/10s | messages.mdx "Modify message" |
| Delete message | `DELETE /v1/channels/{channel_id}/messages/{message_id}` -> 204 | messages.mdx "Delete message" |
| Bulk delete | not used | |
| Add own reaction | `PUT /v1/channels/{channel_id}/messages/{message_id}/reactions/{emoji}/@me` -> 204 | messages.mdx "Add own reaction" |
| Remove own reaction | `DELETE .../reactions/{emoji}/@me` -> 204 | messages.mdx "Remove own reaction" |
| Remove reactions by emoji | `DELETE .../reactions/{emoji}` (guild only, MANAGE_MESSAGES) | not used |
| Typing | `POST /v1/channels/{channel_id}/typing` -> 204 (20 req/10s) | messages.mdx "Indicate typing" |
| Acknowledge message | `POST /v1/channels/{channel_id}/messages/{message_id}/ack` body `{"mention_count": n, "manual"?: bool}` -> 204. **User sessions only** ("Bots can't use this route") | messages.mdx "Acknowledge message" |
| Get guild | `GET /v1/guilds/{guild_id}` (bot-callable; `DOCS/http-api/guilds.mdx` "Get guild") | guilds.mdx |
| Get attachment file | `GET /v1/channels/{channel_id}/attachments/{attachment_id}/{filename}?mid=` (only when `proxy_attachments_through_api`) | messages.mdx "Get attachment file" |
| Download media | GET the signed `url` (or `proxy_url`) directly, no auth (`DOCS/media-proxy/overview.md`) | |

Multipart create (`fluxerapi.CreateMessage` with an `Upload`): `payload_json` holds the JSON body
with `attachments: [{id: 0, filename, content_type}]`, and the bytes are in `files[0]`. This avoids the
four-step presigned upload (`DOCS/topics/uploads.md` lists both paths as valid).

Create message limits: text up to `max_message_length`; `nonce` (<=32 chars) is remembered for 5 min
per channel, so retries reuse it (messages.mdx "Create message" footnote 1). Create message needs
`SEND_MESSAGES` and `ATTACH_FILES` in guild channels.

## 8. Gateway

Discovery: user clients use `endpoints.gateway` (from `.well-known/fluxer`); bots may call
`GET /v1/gateway/bot` (bot-callable, returns `url`, `shards`, `session_start_limit`)
(`DOCS/http-api/gateway.mdx`). Add query params `v=1&encoding=json` (no compression)
(`DOCS/gateway/overview.md` "Connection parameters"). We do not use zstd.

Frame: every message is a JSON object `{"op": int, "d"?: any, "s"?: int, "t"?: string}`
(`DOCS/gateway/overview.md` "Gateway payload"). Snowflakes are strings.

Opcodes (`DOCS/gateway/opcodes-and-close-codes.md` "Opcodes"):
- 0 Dispatch (server->client, has `t`, `s`)
- 1 Heartbeat (both ways; `d` = last processed `s` or `null`)
- 2 Identify (client->server)
- 6 Resume (client->server)
- 7 Reconnect (server->client; reconnect and resume)
- 9 Invalid Session (server->client, always `d: false`)
- 10 Hello (server->client, `d.heartbeat_interval` ms)
- 11 Heartbeat ACK (server->client)
- 12 Gateway Error (voice only; not used)
- 14 Lazy Request (client->server; guild subscriptions)

Identify (`DOCS/gateway/commands.md` "Identify", "Identify properties object"):
`{"token": "<raw token, no prefix>", "properties": {"os": "Android", "browser": "Chirper", "device": "Chirper", "mobile": true}}`.
`flags` is omitted (0). CHANNEL_THREADS (`1 << 2`) is not requested because threads are not
implemented, so no thread events are sent (`DOCS/gateway/events.md` "Dispatch delivery").
Identify is answered by Ready (`t: "READY"`) or a close.

Resume (`DOCS/gateway/commands.md` "Resume"): `{"token", "session_id", "seq"}`. Responses:
Dispatches above `seq` replayed, then `RESUMED`. Opcode 9 `d:false` -> must Identify again.
Note: a 1000/1001 client close ends the session (`DOCS/gateway/opcodes-and-close-codes.md`);
we close with 4000 (or a network error) when we intend to resume.

Heartbeat: send op 1 every `heartbeat_interval` ms, starting after Hello. `d` is `null` until the
first Dispatch is processed, then the last processed sequence (`DOCS/gateway/commands.md` "Heartbeat").
If no ACK arrives within 45000 ms the server closes with 4009. Server may send op 1 with `d: null`;
respond immediately (`DOCS/gateway/overview.md` "Heartbeats").

Lazy Request (`DOCS/gateway/commands.md` "Lazy Request", "Guild subscription object"): needed for
user sessions in guilds with more than 250 members, since user sessions are passive there and get
only mentions (`DOCS/gateway/event-filtering.md` "Active and passive guilds"). We send
`{"subscriptions": {"<guildId>": {"active": true, "typing": true}}}` after READY with every
available guild in one frame, and again for each GUILD_CREATE. Bots are never passive: skipped.

Sharding: a bot may have at most 2,500 guilds per shard (close 4011). We use one shard (no `shard`
field) and document that bots in >2,500 guilds are unsupported (`DOCS/gateway/overview.md`
"Sharding").

Close codes (`DOCS/gateway/opcodes-and-close-codes.md` "Close codes"):
- 4000 Unknown error (session creation failed / draining / resume unavailable): resumable reconnect.
- 4001 Unknown opcode, 4002 decode error, 4003 not authenticated, 4005 already authenticated,
  4007 invalid sequence, 4009 heartbeat timeout, 4012 invalid API version: protocol faults in
  our client. Reconnect; Resume is attempted for any close except 1000/1001
  (`DOCS/gateway/opcodes-and-close-codes.md` "Close codes").
- 4004 Authentication failed / invalid token: permanent. Mark account BAD_CREDENTIALS.
- 4008 Rate limited / Too many sessions / Identify rate limited / Resume rate limited: backoff and retry.
- 4010 Invalid shard, 4011 Sharding required: permanent configuration error.
- Reasons per code: `DOCS/gateway/opcodes-and-close-codes.md` "Close reasons".
- Resumable window: 60 000 ms after the socket dropped (`DOCS/gateway/overview.md` "Resuming a session").

Ordering: events are ordered within one session; on GUILD_CREATE replace cached roles/channels/
emojis/stickers/voice_states/threads for that guild, members are partial (`DOCS/gateway/events.md`
"GUILD_CREATE", `DOCS/gateway/overview.md` "Ordering").

## 9. Gateway events used

Envelope: `{"op": 0, "t": "<EVENT>", "s": <seq>, "d": {...}}` (`DOCS/gateway/events.md` "Dispatch envelope").

| Event | Payload | Use |
|---|---|---|
| `READY` | `session_id`, `user`, `guilds[]` (`{id, properties, channels[], members[], unavailable?}`), `private_channels[]`, `relationships`, `read_states[]`, `users[]` | Account identity, initial portals |
| `RESUMED` | `{_timings_gw?}` | Resume finished |
| `GUILD_CREATE` | guild ready object (`id`, `properties`, `roles`, `channels`, `emojis`, `stickers`, `members` (partial), `member_count`, `unavailable?`) | Guild-as-space portals |
| `GUILD_DELETE` | `{id, unavailable?, unavailable_hidden?}` | Remove guild portals (keep them only when `unavailable`) |
| `CHANNEL_CREATE`, `CHANNEL_UPDATE` | channel object | Create/update portals |
| `CHANNEL_DELETE` | channel object as before | Remove portal |
| `CHANNEL_RECIPIENT_ADD`, `CHANNEL_RECIPIENT_REMOVE` | `{channel_id, user}` (`DOCS/gateway/events.md`) | Group DM membership |
| `MESSAGE_CREATE` | message object + `channel_type`, `guild_id?`, `member?` | New messages |
| `MESSAGE_UPDATE` | full message object | Edits |
| `MESSAGE_DELETE` | `{id, channel_id, guild_id?, author_id?}` | Redactions |
| `MESSAGE_DELETE_BULK` | `{ids[], channel_id, guild_id?}` | Redactions |
| `MESSAGE_ACK` | `{channel_id, message_id, mention_count, manual?, version?, flags?}` | Read state from other sessions |
| `MESSAGE_REACTION_ADD` | `{user_id, channel_id, message_id, emoji{name,id?,animated?}, guild_id?, member?}` | Reaction add |
| `MESSAGE_REACTION_ADD_MANY` | `{channel_id, message_id, reactions[{user_id, emoji}]}` (DM debounce flag only; not requested) | not requested |
| `MESSAGE_REACTION_REMOVE` | `{user_id, channel_id, message_id, emoji}` | Reaction remove |
| `MESSAGE_REACTION_REMOVE_ALL` | `{channel_id, message_id}` | Clear reactions |
| `MESSAGE_REACTION_REMOVE_EMOJI` | `{channel_id, message_id, emoji}` | Clear one emoji |
| `TYPING_START` | `{channel_id, user_id, timestamp, guild_id?, member?}` | Typing indicator (timestamp in Unix seconds) |
| `USER_UPDATE` | user object | Own profile change |
| `RATE_LIMITED` | `{opcode, retry_after, meta}` | Log only (bot full-member-list request) |

Events deliberately NOT handled in v1: thread events, voice, presence, relationships, guild
member add/remove, emoji/sticker/role events, channel pins, calls, webhooks, invites,
audit logs, saved messages, memes.

## 10. Privacy and logging rules applied

- Tokens are never logged (the `Authorization` value, `token` in Identify/Resume, the solved
  ALTCHA token and the `derivedKey`).
- Message bodies, usernames of other people in logs, and phone numbers are never logged.
- Only event names, opcodes, close codes and HTTP status codes are logged.

## 11. Implementation status (what the code does)

Implemented: discovery, token/bot/password(+TOTP) login with ALTCHA solving, Gateway
Identify/Resume/heartbeat/reconnect, guild spaces and channel portals, DMs and group DMs,
text/media/reply/edit/delete/reaction/typing/read-receipt send paths, live message, edit, delete,
reaction, typing and read-state events, and paged backfill.

Not implemented (see the list below): thread events, presence, voice, member events, pins,
mentions rewriting (`<@id>` is sent through unchanged), read receipts for bots (bots cannot ack).

## 12. Not supported in v1 (returns explicit errors)

- Password login that returns `IP_AUTHORIZATION_REQUIRED` (new-IP email authorization).
- WebAuthn second factor.
- Threads, forum and media channels (no thread-aware portals).
- Voice and calls.
- Bots in more than 2,500 guilds (sharding).
- Presigned (multi-part) attachment uploads: attachments are sent via multipart on the create
  message route instead (`DOCS/topics/uploads.md` says both are valid).
