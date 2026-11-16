# Getting Started with Chirper

Welcome to Chirper! This guide will help you install Chirper, set up your messaging accounts, and make the most out of your unified inbox.

---

## 1. Installation

### Android
1. Download the latest `chirper-<version>-universal-release.apk` from the [GitHub Releases](https://github.com/itsrhotaudevvy/chirper/releases) page.
2. If prompted on Android, allow installation from unknown sources or side-load via `adb`:
   ```bash
   adb install -r chirper-universal-release.apk
   ```
3. Launch Chirper. On Android 13+, grant notification permissions to receive message alerts.

### macOS
1. Download `Chirper_<version>_aarch64.dmg` (Apple Silicon) or `Chirper_<version>_x64.dmg` (Intel).
2. Open the `.dmg` disk image and drag **Chirper.app** into your **Applications** folder.
3. Launch Chirper. You may optionally enable Touch ID app lock in Settings.

### Linux
- **AppImage**: Make executable (`chmod +x Chirper.AppImage`) and run.
- **Debian / Ubuntu**: Install the `.deb` package via `sudo dpkg -i chirper_<version>_amd64.deb`.

### Windows
- Run `Chirper_<version>_x64-setup.exe` or install via the `.msi` Windows Installer package.

---

## 2. Initial Setup & Walkthrough

### 2.1 Master Password & Biometrics
Upon first launch, Chirper will prompt you to initialize your local encrypted database vault:
1. Enter a secure master passphrase or activate OS biometrics (Touch ID, Face ID, Android Biometric, or Windows Hello).
2. This passphrase derives the SQLCipher encryption key that seals your chat history and account tokens on disk.

### 2.2 Adding Your First Account
1. Click the **+** (Add Account) icon in the navigation bar or go to **Settings > Accounts**.
2. Select one of the 13 supported networks:
   - **WhatsApp**: Scan the QR code using WhatsApp on your primary phone (Settings > Linked Devices).
   - **Signal**: Scan the QR code using the Signal app (Settings > Linked Devices).
   - **Telegram**: Enter your international phone number and the verification code sent to your Telegram app.
   - **Discord**: Log in via Discord mobile QR code or user token.
   - **Slack / Google Messages / Bluesky**: Follow the on-screen network prompts.
3. Once authenticated, Chirper begins a background initial sync of your recent chats.

---

## 3. Navigating the Unified Inbox

### 3.1 Color-Coded Rainbow Design
Every network is visually identified by its signature color:
- **Badge Indicators**: Unread counters and network pills clearly indicate the source platform.
- **Search & Filters**: Filter your inbox by network (e.g. view only Signal and WhatsApp messages, or view all combined).

### 3.2 Keyboard Shortcuts (Desktop)
- `Cmd/Ctrl + K`: Quick switcher / global search across contacts and chats.
- `Cmd/Ctrl + 1-9`: Switch to pinned channels / accounts.
- `Cmd/Ctrl + L`: Immediately lock Chirper (requires biometrics/password to reopen).
- `Cmd/Ctrl + ,`: Open Settings.
- `Esc`: Clear search / return to inbox from conversation.

---

## 4. Privacy Features

### Screen Privacy Protection
- On Android, enable **Screen Privacy** in Settings to prevent screenshotting and redact chat content when switching apps (`FLAG_SECURE`).
- On macOS, screen recording and screen capture utilities are blocked from capturing window contents when privacy mode is active.

### Zero-Server Guarantee
All bridges run strictly inside the local app process. There are no Chirper servers, telemetry trackers, or third-party relays. Your messages travel directly to and from official protocol endpoints.
