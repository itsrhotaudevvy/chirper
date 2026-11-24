# Chirper Build Guide

This guide covers the toolchains, the desktop and Android builds, the iOS path (CI only), the
environment variables, and the checks CI runs. The core is plain Rust: there is no Go, cgo or C ABI
step, and `scripts/build-go.sh` and `scripts/build-sqlcipher.sh` no longer exist.

---

## 1. Prerequisites

| Tool | Version | Notes |
|---|---|---|
| Rust | stable, through rustup | `rust-toolchain.toml` selects `stable`, adds `rustfmt` and `clippy`, and lists the targets in section 2. `src-tauri` declares `rust-version = 1.90`; the workspace declares 1.88. |
| Node.js | 22 or newer | CI uses 22. |
| pnpm | 12 | CI uses pnpm 12. `pnpm -C ui install --frozen-lockfile` uses `ui/pnpm-lock.yaml`. |
| Tauri CLI | 2.12.5 | The root scripts call `cargo tauri`, which needs the cargo subcommand: `cargo install tauri-cli --version 2.12.5 --locked`. CI installs the npm package `@tauri-apps/cli@2.12.5`, which provides `tauri`. |
| C toolchain and perl | any recent | The `sqlcipher` feature of `chirper-core` builds vendored OpenSSL, which needs a C compiler, `make` and perl. |

### Platform packages

- **macOS:** Xcode Command Line Tools (`xcode-select --install`).
- **Linux** (the list CI installs):
  `build-essential curl file patchelf libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev libxdo-dev libsecret-1-dev`.
- **Windows:** Visual Studio Build Tools with the C++ workload. Release builds run on the GitHub
  Windows runners.
- **Android:** Android SDK, NDK `28.2.13676358`, and JDK 21 (CI uses Temurin 21).
- **iOS:** full Xcode on macOS. There is no documented local iOS flow; see section 5.

### Homebrew `clippy-driver` pitfall

If `cargo clippy` picks up Homebrew's `clippy-driver`, it is incompatible with rustup's toolchain.
Put rustup's toolchain bin directory first on `PATH` (adjust the host triple):

```sh
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
```

---

## 2. Targets

`rust-toolchain.toml` lists these triples. rustup installs them with the toolchain.

| Platform | Triples |
|---|---|
| macOS | `aarch64-apple-darwin`, `x86_64-apple-darwin` |
| iOS | `aarch64-apple-ios`, `aarch64-apple-ios-sim` |
| Windows | `x86_64-pc-windows-msvc` |
| Linux | `x86_64-unknown-linux-gnu` |
| Android | `aarch64-linux-android`, `armv7-linux-androideabi`, `i686-linux-android`, `x86_64-linux-android` |

The release matrix in `.github/workflows/release.yml` also builds `universal-apple-darwin`,
`i686-pc-windows-msvc`, `aarch64-pc-windows-msvc` and `aarch64-unknown-linux-gnu`. Those are not in
`rust-toolchain.toml`; `rustup target add <triple>` adds one locally.

---

## 3. Install and run

```sh
pnpm -C ui install
cargo tauri dev            # or: pnpm dev, which runs the same command from the repo root
```

`cargo tauri dev` runs `pnpm --dir ui dev` (set in `src-tauri/tauri.conf.json`) and opens the
desktop window.

- **Demo network in the app:** `cargo tauri dev --features mock`. The `mock` feature of `chirper-app`
  adds the demo network and opens the database without a key. Development only.
- **UI in a browser:** `pnpm -C ui dev`. The UI detects that it is not inside Tauri and uses the
  in-memory `FakeCore` (`ui/src/lib/core/index.ts`). It makes no network calls.
- **Browser bundle for GitHub Pages:** `CHIRPER_WEB=1 pnpm -C ui build`. `CHIRPER_WEB_BASE` sets
  the base path (`ui/vite.config.ts`).

---

## 4. Desktop bundles

```sh
cargo tauri build                                    # the current machine
cargo tauri build --target <triple>                  # a listed triple, for example x86_64-pc-windows-msvc
```

The bundle targets are set in `src-tauri/tauri.conf.json`: `app`, `dmg`, `deb`, `rpm`, `appimage`,
`nsis` and `msi`. Without `--target` the output is under `target/release/bundle/`; with `--target`
it is under `target/<triple>/release/bundle/`. The release workflow uploads
`target/<triple>/release/bundle/**/*.{dmg,exe,msi,AppImage,deb,rpm}`.

---

## 5. Android

Set the SDK and NDK paths first. `scripts/build-android.sh` defaults to
`$HOME/Library/Android/sdk` and to NDK `28.2.13676358` under it.

```sh
export JAVA_HOME=/path/to/jdk-21
export ANDROID_HOME=$HOME/Library/Android/sdk
export NDK_HOME=$ANDROID_HOME/ndk/28.2.13676358

scripts/build-android.sh --apk                         # universal APK
scripts/build-android.sh --apk --split-per-abi         # one APK per ABI
scripts/build-android.sh --debug --target aarch64 --apk
```

`scripts/build-android.sh` takes the same arguments as `tauri android build`. It points the
`<triple>-ar` and `<triple>-ranlib` variables at the NDK's LLVM tools, which the vendored OpenSSL
build of SQLCipher needs. The release workflow builds APKs only; it does not build an AAB.

APKs are written under `src-tauri/gen/android/app/build/outputs/apk/`. If
`src-tauri/gen/android/keystore.properties` exists, the release build is signed with it. The
release workflow writes that file from the signing secrets (section 6).

---

## 6. iOS

iOS builds run in CI only, on `macos-latest`, in the `iOS` job of `release.yml`:

1. `tauri ios init --ci` generates the Xcode project under `src-tauri/gen/apple` when it is missing.
2. `tauri ios build --export-method app-store-connect` builds the signed IPA. It uses the
   `IOS_CERTIFICATE_*` and `IOS_PROVISIONING_PROFILE_*` secrets and `APPLE_DEVELOPMENT_TEAM`.
3. `tauri ios build --target aarch64-sim` builds an unsigned simulator bundle.

The `chirper-native` plugin's Swift package and `src-tauri/Info.ios.plist` are merged in by the
Tauri CLI. Nothing in this repository documents a local iOS build.

---

## 7. Environment variables

| Variable | Read by | Purpose |
|---|---|---|
| `CHIRPER_TELEGRAM_API_ID`, `CHIRPER_TELEGRAM_API_HASH` | `chirper-net-telegram`, at compile time (`option_env!`) | Telegram API credentials. Without both, Telegram is listed as unavailable. |
| `CHIRPER_TG_API_ID`, `CHIRPER_TG_API_HASH` | `release.yml` (GitHub secrets) | Mapped to the two variables above for release builds. |
| `ANDROID_HOME`, `ANDROID_SDK_ROOT`, `NDK_HOME`, `ANDROID_NDK_HOME` | `scripts/build-android.sh` | SDK and NDK locations. |
| `CHIRPER_WEB`, `CHIRPER_WEB_BASE` | `ui/vite.config.ts` | Browser build with the strict no-network policy, and its base path. |
| `ANDROID_KEYSTORE_B64`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD` | `release.yml` | Android release signing. Without them the build is unsigned. |
| `APPLE_DEVELOPMENT_TEAM`, `IOS_CERTIFICATE_*`, `IOS_PROVISIONING_PROFILE_*` | `release.yml` | iOS signing. |

---

## 8. Checks

These are the commands CI runs (`.github/workflows/ci.yml`):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --features chirper-core/mock -- -D warnings
cargo test --workspace --features chirper-core/mock
cargo deny check

pnpm -C ui install --frozen-lockfile
pnpm -C ui check
pnpm -C ui test
pnpm -C ui build
```

`cargo deny` reads `deny.toml`. Two more commands are useful locally:

```sh
cargo run -p chirper-core --example smoke   # demo login, send, events, reopen the encrypted database
cargo test -p <crate>                       # one crate, for example chirper-net-discord
```

The live probes in `crates/chirper-net-telegram/examples/` and `crates/chirper-net-whatsapp/examples/`
open real connections. They are never run by `cargo test`.

CodeQL (`.github/workflows/codeql.yml`) analyses JavaScript and TypeScript, and Rust.

---

## 9. Reference sources (optional)

`scripts/fetch-third-party.sh` clones the mautrix bridge repositories at the commits pinned in
`third_party/pins.txt` into `third_party/`. Those clones are read-only reference material for
writing the network drivers. Nothing in the build uses them, and `third_party/*` is gitignored
except for `pins.txt`. The script needs git and network access.
