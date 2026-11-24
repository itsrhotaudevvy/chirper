# Common requirements for a full network driver

Every network spec refers here. "Port the connector" means: read every file of the reference
bridge's `pkg/connector/` and its protocol library, and reproduce its **behaviour** (login flows
and their step/field ids, chat sync, live events, conversion, backfill, outbound commands, error
handling) in Rust against `chirper-bridge`. Where the reference does something Chirper has no
place for (Matrix-only features, provisioning API, bridge commands, double puppeting, spaces
built only for Matrix clients, push notifications to a homeserver), skip it and list it in your
report.

## Shape of the crate

```
src/lib.rs        Network impl, login flow dispatch, Session impl (thin)
src/api/…         the protocol client: requests, responses (serde/prost types), errors
src/login.rs      LoginProcess implementations
src/convert.rs    remote → Content/RemoteMessage/ChatMeta, and Chirper → remote for sends
src/ids.rs        id formats, identical to the Go bridge (see README rule 10)
src/session.rs    connect, event loop, command handling, backfill
examples/probe.rs optional live probe from environment variables
```

## Required behaviour

1. `login_flows()` lists the reference's flows with the **same flow ids**; each `LoginProcess`
   emits steps with the same `step_id`s and field ids where practical. Cookie/web logins use
   `LoginStep::Cookies` (url, user agent, fields with sources, `extract_js` when the reference
   has one, `wait_for_url_pattern`). The UI shows that web page in a webview and returns the
   cookies.
2. Credentials (tokens, cookies, keys, session blobs) are stored with `ctx.set_secret` under
   short names (`token`, `cookies`, `session`), as JSON where it has structure. `connect` reads
   them; missing or rejected credentials → `Error::Auth`.
3. On connect: report `Event::State { state: Connecting }`, sync the chat list (`Event::Chat`
   for every conversation, newest first, at most 500 unless the network pages cheaply), then
   `Running` with the user's profile. Live events from the network's realtime channel
   (websocket, MQTT, long-poll, SSE — whatever the reference uses) or, when it only polls, a
   poll loop with the reference's interval.
4. Messages: text (with the network's formatting converted to the HTML subset `b i u s code pre
   a blockquote br` in `formatted`), replies (`reply_to`), edits (`edit_of` on a new
   `RemoteMessage` with the target id and the new content), deletions (`Event::Redact`),
   reactions (`Event::Reactions` with the full set for the message), typing, read receipts,
   attachments (download → `Content::Media`), and everything else as `Unsupported` with a short
   description.
5. Commands: implement every `Command` variant the network supports and set `capabilities()`
   to match exactly; unsupported ones return `Error::Unsupported`.
6. Backfill: page backwards with the network's own cursor; oldest-first inside the page;
   `has_more` only when the network says or a full page came back.
7. Errors: HTTP 401/403 or the network's "logged out" signal → `Error::Auth`; 429 or its rate
   limit body → `Error::RateLimited { retry_after_ms }` from the header/body; 5xx and transport
   → `Error::Network`; unparseable → `Error::Protocol`. Never include response bodies that may
   contain tokens in error strings (use `chirper_bridge::redact` if present, else summarise).
8. `unavailable()` returns `None` when the above works in tests end to end (login against a fake
   server → connect → receive a message → send a message → backfill a page).

## Testing pattern

Make the HTTP base URL injectable (`with_base(..)` like Discord) and run a local fake server in
tests: a `tokio::net::TcpListener` speaking minimal HTTP/1.1 (or a websocket via
`tokio_tungstenite::accept_async`), serving canned JSON/protobuf fixtures that you write from the
reference's types and test files. Pin the conversion of every fixture.
