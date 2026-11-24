# Task `slack`: Slack in `crates/chirper-net-slack`

**Owned:** `crates/chirper-net-slack/**`. Read `README.md` and `_driver-common.md` first.
**Reference:** `/Volumes/SSD/Code/Chirper/third_party/slack/pkg/` (`connector/`, `msgconv/`
incl. `mrkdwn/` and `matrixfmt/`, `emoji/`, `slackid/`). The Go code uses `slack-go`; reimplement
the Web API calls and the RTM/websocket client yourself.

- Login flows with the reference ids: `token` (cookie `d` + `xoxc-` token from the web client,
  via `LoginStep::Cookies` on `https://app.slack.com/` using the reference's extract JS, and a
  manual `user_input` fallback), `app` (bot/user `xoxb-`/`xoxp-` token), and `email` (port
  `login-email*.go`; the captcha runs in the app webview through a `Cookies` step with
  `login-email-captcha.js` as `extract_js`). Validate with `auth.test`; a login is one
  workspace (team); `login_id` per `slackid`.
- Realtime: `rtm.connect` → websocket (`wss://wss-primary.slack.com/...` from the response),
  ping every 30 s, reconnect on `goodbye`. Events: `message` (+ subtypes `message_changed`,
  `message_deleted`, `bot_message`, `file_share`, `thread_broadcast`, `me_message`),
  `reaction_added/removed`, `user_typing`, `channel_marked`/`im_marked`/`group_marked`,
  `channel_joined/left`, `member_joined_channel`.
- Sync: `client.userBoot` or `conversations.list` (types public, private, mpim, im) + `users.info`
  batch caching; threads are their own portals exactly as the reference does (check
  `slackid` for thread portal ids).
- Conversion: mrkdwn and blocks/rich_text → HTML subset; `<@U123>` mentions → display names;
  custom emoji `:name:` keep as text; files: download with the token+cookie
  (`url_private_download`), cap 100 MiB.
- Commands: `chat.postMessage` (thread_ts for thread portals and replies), files upload v2
  (`files.getUploadURLExternal` + upload + `files.completeUploadExternal`), `chat.update`,
  `chat.delete`, `reactions.add/remove` (emoji name ↔ unicode via the `emoji` table — copy the
  table with attribution), `conversations.mark`, typing over the websocket.
- Backfill: `conversations.history` / `conversations.replies` with `latest` cursor.
- Rate limits: honour `Retry-After` on 429.
