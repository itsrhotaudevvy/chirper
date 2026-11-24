# Task `linkedin`: LinkedIn messaging in `crates/chirper-net-linkedin`

**Owned:** `crates/chirper-net-linkedin/**`. Read `README.md` and `_driver-common.md` first.
**Reference:** `/Volumes/SSD/Code/Chirper/third_party/linkedin/pkg/` (`linkedingo/` is the
protocol client: port its requests, GraphQL query ids, headers, decoration ids and the SSE
realtime client; `connector/` for behaviour; `linkedinfmt/`, `matrixfmt/` for formatting).

- Login flows as in `login.go`/`login_password.go`: cookie login (`LoginStep::Cookies` on
  `https://www.linkedin.com/login`, fields `li_at`, `JSESSIONID` from cookies) and password
  login with its `code`/`action` steps if the reference implements it without a browser
  challenge; if it needs a challenge page, fall back to the cookie step. Store the cookie jar.
- CSRF header from `JSESSIONID`; exact headers/user agent of the reference.
- Realtime: the SSE stream `realtime/connect` (decorations per reference), heartbeat handling,
  reconnect; events for new message, edit, delete, reactions, typing, seen receipts.
- Sync: conversations via the messenger GraphQL endpoints; backfill messages by
  `deliveredAt` cursor.
- Commands: send text (with attributes for formatting/mentions if the reference does), media
  upload (`voyagerVideoDashMediaUploadMetadata` or the reference's flow), edit, delete,
  react/unreact, mark read, typing.
