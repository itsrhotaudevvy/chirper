# Chirper Build & Packaging Guide

This guide details how to set up the development environment, build the native Go core and SQLCipher dependencies, and package Chirper across desktop (macOS, Windows, Linux) and mobile (Android, iOS).

---

## 1. Prerequisites

### Universal Tooling
| Tool | Minimum Version | Description |
|---|---|---|
| **Rust** | 1.85+ (Edition 2024 supported) | Rust compiler & Cargo (`rust-toolchain.toml`) |
| **Go** | 1.27+ | Compiles the bridge engine and connector cores |
| **Node.js** | 22+ | Frontend runtime for Svelte 5 |
| **pnpm** | 9+ (or 12) | Node package manager |
| **Tauri CLI** | v2.12+ | CLI orchestrator (`cargo install tauri-cli`) |

### Platform-Specific Tooling
- **macOS**:
  - Xcode Command Line Tools: `xcode-select --install`
- **Android**:
  - Android SDK (API Level 34 or 36)
  - Android NDK: `28.2.13676358`
  - Java JDK: OpenJDK 17 or 21 (`export JAVA_HOME=/path/to/jdk`)
  - Android Debug Bridge (`adb`)
- **Linux**:
  - `build-essential`, `patchelf`, `pkg-config`
  - Libraries: `libwebkit2gtk-4.1-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev`, `libssl-dev`, `libsecret-1-dev`
- **Windows**:
  - Visual Studio Build Tools (C++ development workload)
  - MinGW-w64 (MSYS2) for CGO cross-compilation

---

## 2. Source Preparation

### 2.1 Clone and Fetch Connectors
Chirper pins upstream mautrix bridgev2 connectors in `third_party/`. Run the fetch script to populate connector sources:
```bash
# Fetch pinned third-party connector repositories
./scripts/fetch-third-party.sh
```

### 2.2 Install Frontend Dependencies
```bash
# Install Svelte 5 and Vite dependencies
pnpm -C ui install
```

---

## 3. Building the Go Core & SQLCipher

The Go core (`go/core`) must be compiled for the target architecture before compiling the Tauri application.

### Available Target Triples
- **macOS**:
  - `universal-apple-darwin` (Universal binary: Apple Silicon + Intel)
  - `aarch64-apple-darwin` (Apple Silicon 64-bit ARM)
  - `x86_64-apple-darwin` (Intel 64-bit x86_64)
- **Windows**:
  - `x86_64-pc-windows-msvc` (Windows 64-bit x86_64)
  - `aarch64-pc-windows-msvc` (Windows 11 on ARM64)
  - `i686-pc-windows-msvc` (Windows 32-bit x86)
- **Linux**:
  - `x86_64-unknown-linux-gnu` (Linux 64-bit x86_64)
  - `aarch64-unknown-linux-gnu` (Linux 64-bit ARM64 / aarch64)
  - `armv7-unknown-linux-gnueabihf` (Linux 32-bit ARMv7)
- **Android**:
  - `aarch64-linux-android` (arm64-v8a)
  - `armv7-linux-androideabi` (armeabi-v7a)
  - `x86_64-linux-android` (x86_64)
  - `i686-linux-android` (x86 32-bit)
- **iOS**:
  - `aarch64-apple-ios` (Physical iPhone / iPad devices)
  - `aarch64-apple-ios-sim` (Apple Silicon iOS Simulator)
  - `x86_64-apple-ios` (Intel iOS Simulator)

### Build Command:
```bash
# 1. Build SQLCipher for target
./scripts/build-sqlcipher.sh <target-triple>

# 2. Compile Go core (C-archive or C-shared)
./scripts/build-go.sh <target-triple> --release
```
*Compiled artifacts are placed into `go/out/<target-triple>/`.*

---

## 4. Building Desktop Apps

### Development Mode
Runs the Vite development server and launches the desktop webview with hot reloading:
```bash
cargo tauri dev
```

### Release Bundles

#### macOS (`.dmg` & `.app`)
```bash
# Single architecture (current machine)
cargo tauri build

# Universal binary (Apple Silicon + Intel)
cargo tauri build --target universal-apple-darwin
```
Outputs:
- Bundle: `target/release/bundle/macos/Chirper.app`
- Installer: `target/release/bundle/dmg/Chirper_<version>_<arch>.dmg`

#### Linux (`.AppImage` & `.deb`)
```bash
# x86_64
cargo tauri build --target x86_64-unknown-linux-gnu

# ARM64 (aarch64)
cargo tauri build --target aarch64-unknown-linux-gnu
```
Outputs:
- `target/<triple>/release/bundle/appimage/chirper_<version>_<arch>.AppImage`
- `target/<triple>/release/bundle/deb/chirper_<version>_<arch>.deb`

#### Windows (`.msi` & `.exe`)
```bash
# x64 (64-bit Intel/AMD)
cargo tauri build --target x86_64-pc-windows-msvc

# ARM64 (Windows on ARM)
cargo tauri build --target aarch64-pc-windows-msvc

# x86 (32-bit)
cargo tauri build --target i686-pc-windows-msvc
```
Outputs:
- `target/<triple>/release/bundle/msi/Chirper_<version>_<arch>_en-US.msi`
- `target/<triple>/release/bundle/nsis/Chirper_<version>_<arch>-setup.exe`

---

## 5. Building Mobile Apps

### Android (`.apk` & `.aab`)

Ensure `JAVA_HOME` and `ANDROID_HOME` are exported:
```bash
export JAVA_HOME=/opt/homebrew/opt/openjdk@17 # Or your JDK path
export ANDROID_HOME=$HOME/Library/Android/sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/28.2.13676358
```

#### Build Release APKs:
```bash
# Universal release APK (all architectures bundled into one APK)
cargo tauri android build --apk

# Split APKs per target architecture (arm64-v8a, armeabi-v7a, x86_64, x86)
cargo tauri android build --apk --split-per-abi

# Target a specific architecture directly (e.g. aarch64 / arm64-v8a)
cargo tauri android build --apk --target aarch64

# Play Store App Bundle (AAB)
cargo tauri android build --aab
```
Artifact locations:
- Universal: `src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk`
- Per-ABI: `src-tauri/gen/android/app/build/outputs/apk/<abi>/release/app-<abi>-release-unsigned.apk`
- Bundle: `src-tauri/gen/android/app/build/outputs/bundle/release/app-release.aab`

#### Push directly to connected Android device:
```bash
# Sign the APK with debug keystore (or release keystore)
apksigner sign --ks ~/.android/debug.keystore --ks-pass pass:android --key-pass pass:android \
  --out app-release-signed.apk app-universal-release-unsigned.apk

# Install to connected device
adb install -r app-release-signed.apk
```

### iOS (`.ipa` & `.app`)
*iOS release builds require a full Xcode installation and Apple Developer Provisioning Profiles, executed on macOS or in GitHub Actions.*
```bash
# Generate Xcode project (first time)
cargo tauri ios init --ci

# Build unsigned ARM64 simulator bundle
cargo tauri ios build --target aarch64-sim

# Build unsigned x86_64 simulator bundle
cargo tauri ios build --target x86_64

# Build signed App Store distribution package (physical devices)
cargo tauri ios build --target aarch64 --export-method app-store-connect
```

---

## 6. Environment Variables Reference

| Variable | Description |
|---|---|
| `CHIRPER_TG_API_ID` | Telegram API App ID for Telegram connector |
| `CHIRPER_TG_API_HASH` | Telegram API App Hash |
| `ANDROID_KEYSTORE_B64` | Base64-encoded release keystore for Android signing |
| `ANDROID_KEY_ALIAS` | Key alias in Android release keystore |
| `ANDROID_KEY_PASSWORD` | Key password for Android keystore |
| `APPLE_DEVELOPMENT_TEAM` | 10-character Apple Team ID for iOS code signing |
| `TAURI_SIGNING_PRIVATE_KEY` | Tauri updater signing key for automated update bundles |
