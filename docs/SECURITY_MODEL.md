# Chirper Security & Privacy Model

Chirper is built from first principles for uncompromising privacy. Unlike traditional cross-network chat clients, Chirper operates entirely on-device with zero intermediary servers.

---

## 1. Zero-Server Architecture

### 1.1 Direct Peer-to-Network Communication
In Chirper:
- There is **no Chirper cloud backend**, no proxy relay, and no sync server.
- The bridges and protocol handlers run as in-process native modules inside the Chirper binary on your local machine or phone.
- Network traffic flows strictly between your local client and the official endpoints of the respective networks (e.g., WhatsApp Web servers, Telegram MTProto endpoints, Signal servers).

### 1.2 No Telemetry or Leakage
- **No Analytics / Telemetry**: Zero metric tracking, telemetry calls, or usage beacons.
- **No Crash Reporting**: Diagnostic stack traces are stored locally if enabled, never transmitted to any third party.
- **No Remote CDNs**: All UI assets, icons, fonts, and scripts are packaged statically into the binary.

---

## 2. Storage & At-Rest Encryption

### 2.1 Database Encryption with SQLCipher
All application data—message history, media metadata, contact cards, and account sessions—are stored in a SQLite database encrypted via **SQLCipher** (AES-256 with 256-bit PBKDF2 key derivation).

### 2.2 Hardware-Backed Keystore Integration
Database encryption keys are never stored in plaintext on the filesystem. They are sealed using the host operating system's native hardware keystores:

| Platform | Keystore Implementation | Security Level |
|---|---|---|
| **Android** | Android Keystore Provider (AES-GCM-256) | Hardware Enclave / StrongBox |
| **macOS** | Apple Keychain Services | Secure Enclave (`kSecAttrAccessibleThisDeviceOnly`) |
| **iOS** | iOS Keychain | Secure Enclave (`kSecAttrAccessibleThisDeviceOnly`) |
| **Windows** | Windows Credential Manager / DPAPI | Windows Security Architecture |
| **Linux** | FreeDesktop Secret Service (`libsecret`) | System Keyring (GNOME Keyring / KWallet) |

---

## 3. Webview Sandboxing & Content Security Policy (CSP)

The user interface runs inside Tauri's sandboxed Webview (Wry/WebKit on macOS/iOS/Linux, WebView2 on Windows, Android System WebView on Android).

### 3.1 Strict CSP
Defined in `src-tauri/tauri.conf.json`:
```http
default-src 'self';
script-src 'self';
style-src 'self' 'unsafe-inline';
img-src 'self' data: blob: chirper-media: http://chirper-media.localhost;
media-src 'self' data: blob: chirper-media: http://chirper-media.localhost;
font-src 'self';
connect-src ipc: http://ipc.localhost;
object-src 'none';
base-uri 'none';
frame-ancestors 'none';
form-action 'none'
```
- Remote script execution is completely blocked.
- IPC is restricted exclusively to authenticated Tauri commands.
- `freezePrototype: true` protects JavaScript native object prototypes from pollution.

### 3.2 IPC Isolation & Media Streaming
- Messages and events do not expose raw tokens or memory buffers to unprivileged contexts.
- Media items are streamed directly via the custom `chirper-media://` protocol handler in Rust. Images and videos never pass as base64-encoded strings across the IPC boundary, preventing heap exposure and memory bloat.

---

## 4. Screen Privacy & App Lock

### 4.1 System Screen Privacy
- **Android**: `FLAG_SECURE` is applied to the main Activity window when enabled. This blocks third-party screen recorders, screenshot captures, and redacts app preview snapshots in the Android recent apps overview.
- **macOS**: `NSWindow.sharingType = .none` prevents screen-recording software and display capture tools from reading window pixels.
- **iOS**: A blur overlay is automatically superimposed whenever the app resigns active state into the app switcher.

### 4.2 Biometric App Lock
- Supports Touch ID, Face ID, Windows Hello, and Android BiometricPrompt.
- When enabled, the local database key is wiped from memory upon locking; biometric authentication is required to decrypt and resume session keys.
