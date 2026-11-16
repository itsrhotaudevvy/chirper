# Chirper Architecture

Chirper is an on-device, zero-server universal messaging client built with **Tauri v2**, **Svelte 5**, **Rust**, and **Go**. It connects directly to multiple messaging protocols in-process without relying on third-party servers or intermediaries.

---

## 1. High-Level System Overview

```
┌────────────────────────────────────────────────────────────────────────┐
│                        Webview Presentation Layer                      │
│                  (Svelte 5 Runes + TypeScript + Vite)                  │
│                                                                        │
│   Screens: Inbox, Chat Timeline, Accounts, Settings, App Lock          │
│   State: Svelte 5 reactive stores ($state, $derived)                   │
│   CoreApi abstraction: TauriCore (production) | FakeCore (browser dev) │
└───────────────────────────────────┬────────────────────────────────────┘
          invoke("core_command", payload) │ ▲ event: "core-batch" (16ms throttle)
                                          │ │
┌───────────────────────────────────▼─┴──────────────────────────────────┐
│                           Tauri v2 Host (Rust)                         │
│                           crate: `chirper-app`                         │
│                                                                        │
│  - IPC Dispatcher & Schema Validator                                   │
│  - Event Aggregator (batches Go events every 16ms to avoid IPC flood)  │
│  - Custom Protocol Handler: `chirper-media://` (zero-copy disk stream) │
│  - System Tray, Dock/Taskbar Badges, Notifications                     │
│  - Platform Drivers: macOS, Windows, Linux, Android, iOS               │
└──────────────────┬─────────────────────────────────┬───────────────────┘
                   │                                 │
┌──────────────────▼──────────────────┐   ┌──────────▼───────────────────┐
│     Platform Native Integration     │   │      `crates/chirper-core`   │
│     `plugins/chirper-native`        │   │      (Safe Rust FFI Wrapper) │
│                                     │   │                              │
│  Android (Kotlin):                  │   │  - Dynamic / Static C ABI    │
│   • BiometricPrompt                 │   │  - Keystore abstraction      │
│   • Foreground Sync Service         │   │  - JSON envelope marshaling  │
│   • FLAG_SECURE window flag         │   │  - Thread-safe event queue   │
│   • RemoteInput notification reply  │   └──────────────┬───────────────┘
│  Apple (Swift/Obj-C):               │                  │
│   • LocalAuthentication (Touch/Face)│                  │ C ABI (5 symbols)
│   • Keychain Access                 │                  │ JSON envelopes
│   • NSWindow sharingType privacy    │                  │
└─────────────────────────────────────┘   ┌──────────────▼───────────────┐
                                          │     Go Core (`go/core`)      │
                                          │     module: chirper.dev/core │
                                          │                              │
                                          │  - CGO Entrypoints (`capi/`) │
                                          │  - Core Facade & Engine      │
                                          │  - In-memory event dispatch  │
                                          │  - Local Matrix emulation    │
                                          │  - Encrypted SQLCipher Store │
                                          └──────────────┬───────────────┘
                                                         │
                                          ┌──────────────▼───────────────┐
                                          │       Network Connectors     │
                                          │                              │
                                          │  Mautrix Bridgev2:           │
                                          │   • WhatsApp, Signal         │
                                          │   • Telegram, Discord        │
                                          │   • Slack, Messenger, IG     │
                                          │   • Bluesky, Twitter/X       │
                                          │   • Google Messages & Voice  │
                                          │   • LinkedIn, Fluxer         │
                                          └──────────────────────────────┘
```

---

## 2. Component Breakdown

### 2.1 Frontend (`ui/`)
- **Framework**: Svelte 5 using the modern runes syntax (`$state`, `$derived`, `$effect`).
- **Styling**: Single semantic design system defined in `tokens.css` with a color-coded rainbow hierarchy. Each network has a distinct hue across the visual spectrum for instant chat identification.
- **Adaptive Shell**:
  - Small screens (<900px, phones): Stacked single-pane navigation (Inbox view transitions to full-screen Chat).
  - Large screens (≥900px, desktop/tablets): Persistent master-detail split view with resizable panels.
- **Media Pipeline**: Images, avatars, and audio are loaded via the custom `chirper-media://` protocol. This bypasses base64 encoding over IPC, avoiding memory bloat.

### 2.2 Tauri Rust Layer (`src-tauri/`)
- Acts as the orchestration layer between the Webview and native platform APIs.
- **Batching & Throttling**: Event traffic from bridge connectors can be heavy during initial sync. Rust aggregates incoming Go events and flushes them to the Webview every 16ms, aligning with 60 FPS display frames.
- **Window Management**: Window state persistence, system tray icon with unread badges, and hardware acceleration controls.

### 2.3 Rust Core Wrapper (`crates/chirper-core/`)
- Enforces memory safety over raw C pointers exposed by the Go runtime.
- Handles differences in Go linking models:
  - **macOS / iOS**: Static archive (`c-archive`, linked directly into binary).
  - **Android / Linux / Windows**: Shared library (`c-shared`, dynamically loaded via `libloading`).
- Exposes high-level async Rust functions matching the contract documented in `docs/CONTRACT.md`.

### 2.4 Go Core Engine (`go/core/`)
- Embeds connector bridges without needing external Matrix homeservers or external bridge daemons.
- Runs an in-memory local Matrix state emulator (`localmx/`) that translates Mautrix bridgev2 events into normalized Chirper events:
  - Account state transitions (`connected`, `connecting`, `needs_reauth`, `disconnected`).
  - Message lifecycle (`received`, `sent`, `delivered`, `read`).
  - Typings, presence, and read receipts.
- **Storage**: User databases, chat history, and cached credentials are encrypted on disk using SQLCipher. The database encryption key is protected by the host OS secure enclave / keystore.

### 2.5 Native Plugins (`plugins/chirper-native/`)
- **Android**:
  - `ForegroundSyncService`: Keeps socket connections alive when configured by the user.
  - `FLAG_SECURE`: Prevents sensitive chat contents from appearing in the Android Recents switcher or screenshots.
  - `BiometricPrompt`: Hardware-backed biometric authentication.
  - `RemoteInput`: Inline reply directly from system notifications.
- **macOS / iOS**:
  - `LocalAuthentication`: Touch ID / Face ID integration for instant app unlocking.
  - Secure screen capture prevention (`NSWindow.sharingType = .none`).
