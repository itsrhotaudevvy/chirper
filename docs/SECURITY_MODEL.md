# Security and privacy model

This describes what the code in this repository does, and where it stops. Each claim points at the
code it comes from. Known gaps are listed in section 9.

---

## 1. Where data goes

- Chirper has no server. The repository has no code that sends data to a server run by Chirper.
- The network drivers send traffic only to the networks the user logs in to: each network's own
  web, API or app protocol endpoints, reached with an unofficial client.
- The app has no telemetry, crash reporting or analytics. No analytics or crash-reporting crate is
  in `Cargo.lock`, and the UI's dependencies are `@tauri-apps/api`, `@fontsource-variable/inter` (the
  font is bundled) and `qrcode-generator`.
- The webview's Content Security Policy allows `https:` for images and connections (section 5). The
  UI does not load remote resources, but the policy does not block them.

## 2. Logging

- `crates/chirper-bridge/src/redact.rs` removes three shapes of secret from text that a driver
  reports, before the text reaches a log line or an error the UI shows: the value after a marker
  word such as `token=`, `"password":`, `Bearer ` or `cookie:`; a Discord-shaped token; and any long
  run that mixes letters and digits.
- `src-tauri/src/logging.rs`: debug builds log everything; release builds log warnings and errors.
  The module states that nothing in the shell logs message content, ids, tokens or keys.

## 3. Keys and secrets

### Database key

The database key is 32 random bytes (`src-tauri/src/keystore.rs`, `random_key`). The desktop app
keeps it in the OS credential store under service `app.chirper`, user `db-key`. Android and iOS keep
it through the `chirper-native` plugin: the Android Keystore wraps it (`DbKeyWrap.kt`), and the iOS
Keychain entry is `ThisDeviceOnly`.

SQLCipher receives the key as a raw key, `PRAGMA key = x'<64 hex>'`
(`crates/chirper-core/src/store/mod.rs`). No passphrase is derived from the key. Plain SQLite is
used only when the `sqlite-plain` feature is selected, which is the default for `chirper-core` and
the development configuration; the app builds with `sqlcipher`.

### Per-login secrets

`crates/chirper-core/src/secure.rs`, `FileSecrets`, stores each login's secrets in `credentials/`
under the data directory. When the database key is present, each value is sealed with AES-256-GCM
under that key, with a random nonce per value. The directory and files are created with owner-only
permissions. Drivers reach these values only through `LoginCtx` and the `Secrets` trait.

Secret values come back as `Zeroizing` buffers. The secret-bearing types the drivers define print
`[redacted]` in `Debug`.

### Media

Media files are sealed in the `OCV1` format (`secure::vault`): AES-256-GCM in 64 KiB chunks, with a
fresh key per file. The format is compatible with the files the Go build wrote.

### Preferences

`prefs.json` in the Tauri app data directory is written with mode 0600 (`src-tauri/src/prefs.rs`).
It holds settings such as the lock timeout and the notification choices, and no secrets.

### Core settings and the Telegram API credentials

`settings.json` in the data directory holds the core settings. It is written as JSON with owner-only
permissions (`secure::write_private`). It is not encrypted. If the user enters a Telegram API id and
hash in Settings → Advanced, both are stored there as `telegram.apiId` and `telegram.apiHash`. An API
hash is an application credential, not a login, but it is private, so it is a known gap (section 9).
The compiled-in pair (`CHIRPER_TELEGRAM_API_ID` and `CHIRPER_TELEGRAM_API_HASH`) is not stored in files.

### Sync bundle

Settings → Sync exports a bundle for another device (`ui/src/screens/settings/sync.ts`). The
bundle holds the UI and app preferences, the core settings, each account's network, login id, name
and avatar URL, and the pin, archive and mute state of chats. It holds no login credentials. The
bundle is encrypted in the webview with AES-GCM; the key comes from PBKDF2 (100,000 iterations) over
the passphrase or PIN the user enters, with a random salt and IV. It is shown as text, a QR code or
a download.

## 4. Storage and failure

- The database is `chirper.db` in the data directory, with the schema described in
  [ARCHITECTURE.md](ARCHITECTURE.md) section 6.
- If the database cannot be opened, because the file is not a database or the key does not match,
  the startup code moves the whole data directory aside and starts with an empty one. It does not
  delete the old directory, because the key may become available again (`src-tauri/src/startup.rs`).

## 5. Webview

The Content Security Policy in `src-tauri/tauri.conf.json` is:

```
default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline';
img-src 'self' data: blob: https: chirper-media: http://chirper-media.localhost;
media-src 'self' data: blob: https: chirper-media: http://chirper-media.localhost;
font-src 'self'; connect-src ipc: http://ipc.localhost https:;
object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'
```

- `withGlobalTauri` is `false`; `freezePrototype` is `true`.
- Scripts are limited to `'self'`.
- `https:` is allowed for images, media and connections. A script in the page could therefore
  make HTTPS requests. Section 9 lists tightening this as a gap.
- Every command checks two things first. `ensure_unlocked` refuses while the app is locked, and
  `ready_core` refuses until the core is open (`src-tauri/src/commands.rs`). The rule is listed in
  [CONTRACT.md](CONTRACT.md) section C.
- Media reaches the page through `chirper-media://`, which serves only files inside the data
  directory. Any other path returns 404.

## 6. App lock and screen privacy

| Platform | App lock | Screen privacy |
|---|---|---|
| macOS | LocalAuthentication, `DeviceOwnerAuthentication` (Touch ID or the account password) | Window excluded from capture (`set_content_protected`) |
| Windows | Windows Hello (`UserConsentVerifier`) | Window excluded from capture (`set_content_protected`) |
| Linux | Not available | Not available |
| Android | BiometricPrompt, through the plugin | `FLAG_SECURE` on the activity window, through the plugin |
| iOS | Face ID or Touch ID, through the plugin | Cover over the app while it is in the app switcher, through the plugin |

The app starts locked when the app-lock preference is on. The lock timeout is `lockAfterSecs` (60 by
default). Secure screen is on by default on Android and iOS and off on desktop
(`src-tauri/src/prefs.rs`).

## 7. Memory safety

- `chirper-core` has `#![forbid(unsafe_code)]`.
- The driver crates set it, except `chirper-bridge`, `chirper-net-testnet` and `chirper-net-fluxer`,
  which do not set it yet.
- `src-tauri` contains `unsafe` in its macOS and Windows platform code (Objective-C and Win32 calls).
  Each block there has a `SAFETY` comment.
- The shared HTTP client in `chirper-bridge` uses `reqwest` with `rustls`.

## 8. Network drivers

- Remote error text is redacted before the runtime forwards it (section 2).
- A driver's rate-limit wait is honoured and capped at 15 minutes; the runtime does not retry an
  authentication failure (`crates/chirper-bridge/src/runtime.rs`).
- The Discord browser login shows Discord's sign-in page in a window. It reads the token the page
  keeps, checks it against the API, and stores it as a secret (`crates/chirper-net-discord/src/login.rs`).
- The live-probe examples (`crates/chirper-net-telegram/examples`, `crates/chirper-net-whatsapp/examples`)
  connect to real networks. They are never run by `cargo test`.

## 9. Known gaps

- The CSP allows `https:` for images, media and connections (section 5).
- The Telegram API hash is kept in `settings.json` as plain JSON, not in the secret store (section 3).
- `#![forbid(unsafe_code)]` is not set in `chirper-bridge`, `chirper-net-testnet` or `chirper-net-fluxer`.
- No driver has been tested against a live account in this repository, so the behaviour of logins
  and message handling on the real networks is unverified.
- The Linux build has no app lock and no screen-capture exclusion (section 6).
