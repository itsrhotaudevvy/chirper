# Chirper — binding contracts

Three boundaries. Each side must match exactly. Changing anything here needs the lead.

## A. JSON model (unchanged from Openchirp, camelCase)

The JSON shapes `Network`, `Account`, `LoginState`, `Chat`, `Message`, `CoreSettings` and the core
event kinds are exactly those of `go/core/facade` / `go/core/engine/types.go`. TypeScript mirrors
live in `ui/src/lib/core/types.ts`; Rust mirrors in `crates/chirper-core/src/model.rs`.
Summary (see engine/types.go for authority):

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
Event kinds: `chat`, `chat_removed`, `message`, `message_removed`, `typing`, `account`, `core_error`.

## B. Go C ABI (`go/core/capi`, header `chirper.h`)

```c
// Callback: kind and payload are NUL-terminated UTF-8, valid only during the call. Called from a
// single Go goroutine; must not block (Rust copies and enqueues).
typedef void (*chirper_event_cb)(void *ctx, const char *kind, const char *payload);

// Opens the core. key may be NULL/0 (no at-rest DB encryption) or exactly 32 bytes.
// Returns a handle > 0, or 0 with *err_out set (free with chirper_free).
int64_t chirper_open(const char *data_dir, const uint8_t *key, int32_t key_len,
                     chirper_event_cb cb, void *ctx, char **err_out);

// Calls a facade method. Returns a malloc'd envelope (free with chirper_free):
//   {"ok":true,"value":<json>}  or  {"ok":false,"error":"<message>"}
// May block (network I/O, waitLoginStep). Safe from any thread.
char *chirper_call(int64_t handle, const char *method, const char *args_json);

void chirper_free(char *p);
void chirper_close(int64_t handle);          // Stop() + release; idempotent
const char *chirper_version(void);           // static string, do not free
```

Rules: every export recovers panics into an error envelope; `args_json` max 1 MiB; unknown
method → error; handle table guarded by a mutex; `key` bytes are copied and the copy zeroed on close.

### Methods (`method` string → `args_json` object → `value`)

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

## C. Rust ⇄ webview (Tauri)

Commands are `snake_case` versions of the methods above with the same args (Tauri converts
camelCase arg keys), e.g. `invoke('list_messages', {roomId, beforeOrder, limit})`. All return the
`value` or reject with `{ "code": string, "message": string }`.

Extra app commands:

| command | args | notes |
|---|---|---|
| `app_info` | `{}` | `{"version","platform":"macos|windows|linux|android|ios","formFactor":"phone|tablet|desktop"}` |
| `haptic` | `{"kind":"selection|light|medium|heavy|success|warning|error"}` | no-op where unsupported |
| `set_badge` | `{"count"}` | dock/taskbar/app badge |
| `notify_message` | `{"roomId","title","body","avatarPath"}` | respects privacy setting (hide content) |
| `lock_status` / `unlock` | `{}` | `unlock` runs biometrics/OS auth; returns `{"unlocked":bool}` |
| `set_secure_screen` | `{"enabled"}` | screenshot/screen-share protection |
| `open_external` | `{"url"}` | http(s) only, after user tap |
| `set_app_prefs` | `{"prefs":{"hideNotificationContent","notifications","appLock","lockAfterSecs","secureScreen","closeToTray","launchAtLogin","haptics"}}` | persisted by Rust in `<appData>/prefs.json` (0600); Rust owns notification + lock decisions so they work while the webview is hidden |
| `get_app_prefs` | `{}` | the same object (defaults: notifications true, hideNotificationContent false, appLock false, lockAfterSecs 60, secureScreen true on mobile/false on desktop, closeToTray true, launchAtLogin false, haptics true) |
| `pick_files` | `{"kind":"media|file"}` | native file dialog → `[{"path","name","mime","size"}]`; paths returned here are the only ones `send_media` accepts |

Events to the webview:
- `core-batch`: `[{ "kind": string, "payload": object }]`, flushed every 16 ms or at 256 items.
  Chat events for the same `roomId` inside one batch are coalesced (last wins).
- `app-lock`: `{ "locked": bool }`.
- `deep-link`: `{ "url": string }` (`chirper://chat/<roomId>`).
- `notification-reply`: `{ "roomId", "text" }`.

Media in the webview: `chirper-media://localhost/<urlencoded mxc uri>` resolved by Rust via
`mediaFile`, served with the right MIME, only for paths inside the data dir.

### Settled answers (lead, after W1c)
- `lock_status` / `unlock` return `{"locked": bool}` / `{"unlocked": bool}`.
- `set_app_prefs` applies its side effects itself (secure screen, autostart). `set_secure_screen`
  stays for temporary per-screen overrides (e.g. forcing protection while a QR login is shown).
- Typing payload: `{"roomId","names":[]}`. CookieField follows engine/types.go.
- Webview subscriptions: `onEvents` (core-batch), `onAppLock`, `onDeepLink`, `onNotificationReply`.
- Text on colour banners uses `--on-banner` (near-black) over the banner, which passes 4.5:1 on all
  14 spectrum colours in every theme; `--on` is for page content.

## D. UI screen entry points (ui/src/screens)

| file | props | owner |
|---|---|---|
| `inbox/InboxScreen.svelte` | `{}` (selection via `ui` store) | W2b |
| `chat/ChatScreen.svelte` | `{ roomId: string; onback?: () => void }` | W2c |
| `chat/EmptyChat.svelte` | `{}` (split-view placeholder) | W2c |
| `settings/SettingsScreen.svelte` | `{ onclose: () => void }` | W2d |
| `login/AddNetwork.svelte` | `{ network?: string; onclose: () => void }` | W2d |
| `onboarding/Onboarding.svelte` | `{ ondone: () => void }` | W2d |
| `lock/LockScreen.svelte` | `{}` (renders only while locked) | W2d |

Navigation lives in `ui/src/lib/router.svelte.ts` (W2b): `router.view` = `'inbox' | 'chat' |
'settings' | 'add-network'`, `router.go(view, params?)`, `router.back()`; Escape and the Android
back button call `router.back()`. Screens never import each other; they call the router.
