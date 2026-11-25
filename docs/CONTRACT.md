# Chirper — wire contract

Three boundaries: the JSON model, the core facade, and the Tauri commands and events the webview
uses. The JSON shapes below are pinned by the golden fixtures in `crates/chirper-core/tests/golden/`
(checked by `crates/chirper-core/tests/golden.rs`). A change to a shape must change the fixtures and
the TypeScript mirror in the same pull request.

## A. JSON model (camelCase)

The Rust types are in `crates/chirper-core/src/model.rs`; the TypeScript mirror is
`ui/src/lib/core/types.ts`. Summary:

```jsonc
Network  {"id","name","color","beta","available","reason","loginFlows":[{"id","name","description"}],
          "capabilities":{"edits","reactions","replies","redactions","typing","readReceipts","stickers","voice"}}
Account  {"network","loginId","name","avatarUrl","state":"CONNECTED|CONNECTING|BAD_CREDENTIALS|TRANSIENT_DISCONNECT|UNKNOWN_ERROR|LOGGED_OUT","error","message"}
LoginState {"processId","step":{"type":"user_input|cookies|display_and_wait|complete","stepId","instructions",
            "userInput":{"fields":[{"id","name","type","description","options","pattern"}]},
            "cookies":{"url","userAgent","fields":[...],"extractJs"},
            "displayAndWait":{"type":"qr|emoji|code|nothing","data","imageUrl"},
            "complete":{"loginId"}}}
Chat     {"roomId","network","loginId","name","avatarUrl","topic","isDm","isGroup","isSpace","memberCount",
          "lastActivity","preview":{"sender","body","type"},"unreadCount","mentionCount","pinned","archived","mutedUntil","typing":[]}
Message  {"eventId","roomId","order","sender","senderName","senderAvatar","fromMe","ts","type","body","formattedBody",
          "media":{"url","mime","size","width","height","durationMs","fileName","thumbnailUrl","blurhash"}|null,
          "replyTo":{"eventId","senderName","body"}|null,"edited","redacted",
          "reactions":[{"key","count","fromMe","senders"}],"status":"sending|sent|delivered|read|failed","error","live"}
```

Event kinds (`crates/chirper-core/src/model.rs`): `chat`, `chat_removed`, `message`,
`message_removed`, `typing`, `account`, `core_error`.

## B. Core facade

The webview reaches these through the Tauri commands in section C. Each one is a method of
`chirper_core::Core` (`crates/chirper-core/src/engine/`). Arguments are camelCase in JSON; the
Rust method takes the same values.

| method | args | value |
|---|---|---|
| `start` | `{}` | `null` |
| `networks` | `{}` | `Network[]` |
| `accounts` | `{}` | `Account[]` |
| `startLogin` | `{"network","flowId"}` | `LoginState` |
| `submitLoginInput` | `{"processId","values":{field:value}}` | `LoginState` |
| `submitLoginCookies` | `{"processId","cookies":{field:value}}` | `LoginState` |
| `waitLoginStep` | `{"processId"}` | `LoginState` |
| `cancelLogin` | `{"processId"}` | `null` |
| `logout` | `{"network","loginId"}` | `null` |
| `listChats` | `{"filter":{"network","archived","unreadOnly","query"}}` | `Chat[]` |
| `getChat` | `{"roomId"}` | `Chat` or `null` |
| `listMessages` | `{"roomId","beforeOrder","limit"}` | `Message[]` (newest last) |
| `loadOlder` | `{"roomId"}` | `{"hasMore":bool}` |
| `syncHistory` | `{"network"}` | `null` |
| `markUnread` | `{"roomId","unread"}` | `null` |
| `sendText` | `{"roomId","text","replyTo","txnId"}` | `{"eventId"}` |
| `sendMedia` | `{"roomId","path","mime","fileName","caption","kind","replyTo","txnId"}` | `{"eventId"}` |
| `edit` | `{"roomId","eventId","text"}` | `null` |
| `redact` | `{"roomId","eventId"}` | `null` |
| `react` / `unreact` | `{"roomId","eventId","key"}` | `null` |
| `markRead` | `{"roomId","eventId"}` | `null` |
| `setTyping` | `{"roomId","typing"}` | `null` |
| `setChatFlags` | `{"roomId","flags":{"pinned","archived","mutedUntil"}}` | `null` |
| `mediaFile` | `{"url"}` | `{"path"}` |
| `search` | `{"query","limit"}` | `Message[]` |
| `settings` | `{}` | `CoreSettings` |
| `setSettings` | `{"settings":CoreSettings}` | `null` |

`CoreSettings` includes `telegram: {"apiId","apiHash"}`, which defaults to `{"apiId":0,"apiHash":""}`.
`setSettings` rejects an `apiId` that is not a whole number from 0 to 2147483647, and an `apiHash`
that is neither empty nor 32 hexadecimal characters. The Telegram driver reads the pair when the core
opens, so a change applies after a restart.

Input is checked in `crates/chirper-core/src/engine/validate.rs` before it reaches the store or a
driver. Text is capped at 64 KiB and a list page at 500 messages.

## C. Tauri commands and events

Commands are `snake_case` versions of the methods above with the same args (Tauri converts
camelCase argument keys), e.g. `invoke('list_messages', {roomId, beforeOrder, limit})`. Each
command returns its value or rejects with `{ "code": string, "message": string }`. Commands fail
with `locked` while the app is locked and `not_ready` until the core has opened.

Every command registered in `src-tauri/src/lib.rs`:

| command | args | notes |
|---|---|---|
| `start`, `networks`, `accounts`, `start_login`, `submit_login_input`, `submit_login_cookies`, `wait_login_step`, `cancel_login`, `logout` | as in section B | |
| `list_chats`, `get_chat`, `list_messages`, `load_older`, `sync_history`, `mark_unread`, `send_text`, `send_media`, `edit`, `redact`, `react`, `unreact`, `mark_read`, `set_typing`, `set_chat_flags`, `media_file`, `search`, `settings`, `set_settings` | as in section B | |
| `app_info` | `{}` | `{"version","platform":"macos|windows|linux|android|ios","formFactor":"phone|tablet|desktop"}` |
| `haptic` | `{"kind":"selection|light|medium|heavy|success|warning|error"}` | no-op where unsupported |
| `set_badge` | `{"count"}` | dock, taskbar or app badge |
| `notify_message` | `{"roomId","title","body","avatarPath"}` | respects the hide-content preference |
| `lock_status` / `unlock` | `{}` | `lock_status` returns `{"locked":bool}`; `unlock` runs the OS authentication and returns `{"unlocked":bool}` |
| `set_secure_screen` | `{"enabled"}` | screenshot and screen-share protection for a temporary override, for example while a QR login is shown |
| `open_external` | `{"url"}` | http(s) only, after a user tap |
| `open_login_webview` | `{"url","extractJs","waitForPattern","userAgent","title"}` | opens a sign-in window for cookie login steps; returns `{"token","cancelled"}` |
| `set_app_prefs` | `{"prefs":{"hideNotificationContent","notifications","appLock","lockAfterSecs","secureScreen","closeToTray","launchAtLogin","haptics","mutedNetworks"}}` | stored by Rust in `<appData>/prefs.json` (mode 0600). Rust owns the notification and lock decisions so they work while the webview is hidden. It applies the side effects itself (secure screen, autostart). |
| `get_app_prefs` | `{}` | the same object. Defaults: notifications true, hideNotificationContent false, appLock false, lockAfterSecs 60, secureScreen true on Android and iOS and false on desktop, closeToTray true, launchAtLogin false, haptics true, mutedNetworks empty. |
| `pick_files` | `{"kind":"media|file"}` | native file dialog, returns `[{"path","name","mime","size"}]`. Only paths returned here are accepted by `send_media`. |

Events to the webview:

- `core-batch`: `[{ "kind": string, "payload": object }]`, flushed every 16 ms or at 256 items.
  Chat events for the same `roomId` in one batch are coalesced, the last one wins.
- `core-ready` and `core-failed`: emitted once the core has opened or failed to open. A failure
  message never contains user data.
- `app-lock`: `{ "locked": bool }`.
- `deep-link`: `{ "url": string }`, for `chirper://chat/<roomId>`.
- `notification-reply`: `{ "roomId", "text" }`.

Media in the webview: `chirper-media://localhost/<urlencoded mxc uri>` is resolved by Rust through
`mediaFile` and served with the right MIME type. Only paths inside the data directory are served.

### Settled answers

- `set_secure_screen` stays for temporary per-screen overrides. Persistent state goes through
  `set_app_prefs`.
- Typing event payload: `{"roomId","names":[]}`.
- Cookie fields follow the Rust model in `crates/chirper-core/src/model.rs`.
- Webview subscriptions: `onEvents` (`core-batch`), `onAppLock`, `onDeepLink`, `onNotificationReply`.
- Text on colour banners uses `--on-banner` (near-black), which passes 4.5:1 on all 14 spectrum
  colours in every theme. `--on` is for text on page content.

## D. UI screen entry points (`ui/src/screens`)

| file | props |
|---|---|
| `inbox/InboxScreen.svelte` | `{}` (selection in the `ui` store) |
| `chat/ChatScreen.svelte` | `{ roomId: string; onback?: () => void }` |
| `chat/EmptyChat.svelte` | `{}` (split-view placeholder) |
| `settings/SettingsScreen.svelte` | `{ onclose: () => void }` |
| `login/AddNetwork.svelte` | `{ network?: string; onclose: () => void }` |
| `onboarding/Onboarding.svelte` | `{ ondone: () => void }` |
| `lock/LockScreen.svelte` | `{}` (renders only while locked) |

Navigation lives in `ui/src/lib/router.svelte.ts`: `router.view` is `'inbox' | 'chat' | 'settings' |
'add-network'`, with `router.go(view, params?)` and `router.back()`. Escape and the Android back
button call `router.back()`. Screens do not import each other; they call the router.
