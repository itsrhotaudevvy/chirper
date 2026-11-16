# Chirper — Tauri rewrite plan

Chirper is an on-device universal messenger (AGPL-3.0). It is a rewrite of the Openchirp Android
app (Kotlin/Compose + Go core) as one Tauri v2 app for **iOS, Android, macOS, Windows and Linux**.
There is still no Chirper server: the mautrix bridgev2 connectors run in-process and talk only to
the networks.

## Goals (from the product owner)

| # | Goal | How |
|---|---|---|
| G1 | Consistent, enhanced design, independent of Android widgets | Own component set in Svelte; one token file; banner-first colour coding (`docs/DESIGN.md`) |
| G2 | One interface on 2 mobile + 3 desktop OSs | Tauri v2; adaptive layout: stack on phones, split view ≥ 900 px |
| G3 | Little friction: show only what is needed, when it is needed | Progressive disclosure rules in `docs/DESIGN.md` §6 |
| G4 | Deep system integration on all 5 platforms | `src-tauri/src/platform/*` + `plugins/chirper-native` (Kotlin/Swift); matrix in §4 |
| G5 | Performance + hardened security/privacy, Go code included | §5 |
| G6 | Better icon and animations | `assets/brand/`, motion tokens in `docs/DESIGN.md` §5 |
| G7 | GitHub ready | README, LICENSE, community files, CI + release workflows, Dependabot |

## 1. Architecture

```
┌──────────── Webview (ui/, Svelte 5 + TypeScript + Vite) ────────────┐
│ screens ── stores ── CoreApi (TauriCore | FakeCore for browser dev)  │
└───────────────┬────────────────────────────────▲─────────────────────┘
        invoke(cmd, args)                  event "core-batch" (16 ms batches)
┌───────────────▼────────────────────────────────┴─────────────────────┐
│ src-tauri/ (crate chirper-app): commands, validation, event batching, │
│   platform/{macos,windows,linux,android,ios}.rs, tray, lock, badge    │
│ plugins/chirper-native/: Kotlin + Swift (haptics, keystore,           │
│   foreground service, share target, secure screen, notification reply)│
│ crates/chirper-core/: safe Rust wrapper over the Go C ABI, keystore   │
└───────────────┬──────────────────────────────────────────────────────┘
                │ C ABI (5 functions, JSON envelopes) — docs/CONTRACT.md
┌───────────────▼──────────── go/core (module chirper.dev/core) ───────┐
│ capi/ (cgo exports) → facade/ → engine/ → localmx/ → store/          │
│ connectors: third_party/mautrix-* + go/connectors/{discord,fluxer}   │
└──────────────────────────────────────────────────────────────────────┘
```

Go linking: `c-archive` (static) on macOS/iOS, `c-shared` loaded with `libloading` on
Windows/Linux/Android. Built by `scripts/build-go.sh <rust-target-triple>`.

## 2. Repository layout

```
ui/                    Svelte app (pnpm)          src-tauri/      Tauri app crate
crates/chirper-core/   Rust FFI + keystore        plugins/chirper-native/  mobile native code
go/core, go/connectors Go (from Openchirp)        third_party/    pinned mautrix clones (gitignored)
scripts/               build/fetch scripts        assets/brand/   icon sources
docs/                  PLAN, CONTRACT, DESIGN, AGENTS, SECURITY model
.github/               workflows, templates
```

## 3. Waves (Haiku 5.5 implementation agents, lead reviews each)

| Wave | Task | Owns |
|---|---|---|
| 1 | W1a Go C ABI + cross-target build script | `go/core/capi`, `scripts/build-go.sh` |
| 1 | W1b Rust core wrapper + keystore | `crates/chirper-core` |
| 1 | W1c UI foundation: tokens, components, CoreApi, FakeCore | `ui/` (foundation dirs) |
| 1 | W1d GitHub readiness | root community files, `.github/` |
| 1 | W1e Brand: icon + splash sources | `assets/brand/` |
| 2 | W2a Tauri shell: commands, events, config, capabilities, CSP | `src-tauri/` |
| 2 | W2b Inbox + adaptive shell | `ui/src/screens/inbox`, `ui/src/App.svelte` |
| 2 | W2c Chat | `ui/src/screens/chat` |
| 2 | W2d Accounts/login, settings, onboarding, lock | `ui/src/screens/{settings,login,onboarding,lock}` |
| 2 | W2e Go hardening | `go/core/{engine,store,localmx,media,security,facade}`, `go/connectors` |
| 3 | W3a Desktop integration | `src-tauri/src/platform/{macos,windows,linux}.rs` |
| 3 | W3b Mobile integration + `tauri android/ios init` | `plugins/chirper-native`, `src-tauri/gen` |
| 4 | Lead: integration, builds per platform, review, docs | all |

## 4. System integration matrix

| Capability | macOS | Windows | Linux | Android | iOS |
|---|---|---|---|---|---|
| Haptics | NSHapticFeedbackManager (trackpad) | — (no-op) | — | VibrationEffect / HapticFeedbackConstants | UIImpact/Selection/NotificationFeedbackGenerator |
| Notifications + inline reply | UNUserNotification | Toast | libnotify | NotificationCompat + RemoteInput | UNNotification (foreground/refresh only) |
| Unread badge | Dock badge | Taskbar overlay icon | Unity launcher entry (best effort) | notification badge | app badge |
| App lock | Touch ID (LocalAuthentication) | Windows Hello | — (passphrase) | BiometricPrompt | Face/Touch ID |
| Secret storage (DB key) | Keychain | Credential Manager | Secret Service | Android Keystore (AES-GCM wrap) | Keychain (ThisDeviceOnly) |
| Screen privacy | `NSWindow.sharingType = none` | `SetWindowDisplayAffinity` | — | `FLAG_SECURE` | blur on app-switcher snapshot |
| Background | tray, launch at login | tray, autostart | tray, autostart | foreground service | BGAppRefreshTask (no push: no server) |
| Window material | vibrancy, overlay titlebar | Mica | — | edge-to-edge | safe areas |
| Deep links | `chirper://` | `chirper://` | `chirper://` | `chirper://` | `chirper://` |
| Share into Chirper | — | — | — | share target | share extension (later) |
| Single instance | yes | yes | yes | n/a | n/a |

## 5. Performance and security

Performance
- One generic FFI entry point; events batched every 16 ms into one IPC message.
- Virtualised lists (inbox and timeline), stable keys, no layout thrash; images lazy-decoded,
  served via the `chirper-media://` custom protocol (no base64 over IPC).
- Svelte 5 runes, no runtime CSS-in-JS; JS budget < 150 kB gzip; release `opt-level=3`, LTO, strip.
- Go: `-trimpath -ldflags "-s -w"`, connectors started lazily per account.

Security and privacy
- No telemetry, no crash reporting, no remote fonts or CDNs; strict CSP; Tauri capabilities scoped
  per window to the exact commands used; `withGlobalTauri` off.
- DB key: 32 random bytes in the OS keystore, never in files or logs. SQLCipher where built,
  media encrypted with the existing AES-GCM vault.
- All FFI inputs validated in Rust (ids, lengths, paths canonicalised inside the data dir); Go
  re-validates; panics recovered on both sides of the boundary.
- Files `0700`/`0600`; no message bodies, tokens, cookies or phone numbers in logs.
- CI: `cargo audit`, `cargo deny`, `govulncheck`, `pnpm audit`, CodeQL.

## 6. Status (2026-11-16)

All waves above are implemented. Verified on the lead's Mac: Go vet/tests (plain and
`sqlcipher` tags), Rust fmt/clippy `-D warnings`/tests for the workspace and the Android target,
UI check/92 tests/build (68 kB gzip JS), screenshot review of every screen at 390×844 and
1280×800 in dark and light, a real-core Rust→Go smoke run (`cargo run -p chirper-core --example
smoke --features static-link`: demo login, send, events, encrypted reopen, no plaintext header),
the macOS `.app` with the encrypted Go core linked, and a debug arm64 APK.

Not yet verified — needs hardware or CI:
- Runtime on Android/iOS devices (keystore wrap, foreground service, notification reply,
  FLAG_SECURE, app-switcher blur, BGAppRefreshTask). iOS has not been compiled (no Xcode here).
- Windows and Linux builds (type-checked only), Windows Hello, taskbar overlay, Unity badge.
- Touch ID prompt, real network logins (all connectors were exercised only via the demo network).
- SQLCipher for Windows/Linux (`scripts/build-sqlcipher.sh` paths unexecuted); Windows ARM64
  has no OpenSSL 3.5.9 mingw target.

Open decisions: Signal is opt-in (`CHIRPER_LIBSIGNAL_DIR`) until libsignal is built in CI; iOS
export-compliance key (`ITSAppUsesNonExemptEncryption`); CODEOWNERS / code-of-conduct contact
placeholders; voice-note recording (mic hidden until built).
