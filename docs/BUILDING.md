# Chirper Build Guide

This guide covers the toolchains, the desktop and Android builds, the iOS path (CI only), the
features of the core, the environment variables, and the checks CI runs. The core is plain Rust:
there is no Go, cgo or C ABI step, and `scripts/build-go.sh` and `scripts/build-sqlcipher.sh` no
longer exist.

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
- **iOS:** full Xcode on macOS. There is no documented local iOS flow; see section 6.

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
  turns on demo mode, which adds the demo network and opens the database without a key. Development only.
- **UI in a browser:** `pnpm -C ui dev`. The UI detects that it is not inside Tauri and uses the
  in-memory `FakeCore` (`ui/src/lib/core/index.ts`) in place of the Rust core.
- **Browser bundle for GitHub Pages:** `CHIRPER_WEB=1 pnpm -C ui build`. `CHIRPER_WEB_BASE` sets
  the base path (`ui/vite.config.ts`).

---

## 4. Features and the gated networks

The default build has five networks: WhatsApp, Telegram, Discord, Fluxer and SMS. Nine more networks
are behind eight Cargo features of `chirper-core`, and they are not in the default build (see
[ARCHITECTURE.md](ARCHITECTURE.md) section 2 for the full list):

- `net-signal`, `net-twitter`, `net-linkedin`, `net-gmessages`, `net-gvoice`, `net-slack`,
  `net-bluesky`: one network each. `net-meta` covers Messenger and Instagram.
- `more-networks`: all eight features, and iMessage, which has no driver and is listed with its reason.

CI checks that the gated registry compiles with:

```sh
cargo check -p chirper-core --features more-networks
```

The app crate `chirper-app` does not forward these features. To run the app with the gated networks,
pass the feature to `chirper-core` on the Tauri command line, for example
`cargo tauri dev --features chirper-core/more-networks`. That command is not run in CI, and it has
not been verified here.

LinkedIn, Google Voice, Slack and Bluesky are complete on this branch and report available. Signal,
Messenger, Instagram and Google Messages are placeholders here. The unfinished X driver and the
LinkedIn password flow are on the branch `wip/more-networks`. [NETWORKS.md](NETWORKS.md) section 2
gives the state of each one.

---

## 5. Desktop bundles

```sh
cargo tauri build                                    # the current machine
cargo tauri build --target <triple>                  # a listed triple, for example x86_64-pc-windows-msvc
```

The bundle targets are set in `src-tauri/tauri.conf.json`: `app`, `dmg`, `deb`, `rpm`, `appimage`,
`nsis` and `msi`. Without `--target` the output is under `target/release/bundle/`; with `--target`
it is under `target/<triple>/release/bundle/`. The release workflow uploads
`target/<triple>/release/bundle/**/*.{dmg,exe,msi,AppImage,deb,rpm}`.

---

## 6. Android

Build Android with `scripts/build-android.sh`. It is the script CI uses. It sets the NDK's LLVM
tools for the vendored OpenSSL build of SQLCipher, then runs `tauri android build` (or
`cargo tauri android build` when `tauri` is not on `PATH`) with the arguments you pass. Set the SDK
and NDK locations first. The script defaults to `$HOME/Library/Android/sdk` and to NDK
`28.2.13676358` under it.

```sh
export JAVA_HOME=/path/to/jdk-21
export ANDROID_HOME=$HOME/Library/Android/sdk
export NDK_HOME=$ANDROID_HOME/ndk/28.2.13676358

scripts/build-android.sh --apk                         # universal APK
scripts/build-android.sh --apk --split-per-abi         # one APK per ABI
scripts/build-android.sh --debug --target aarch64 --apk
```

The script is the only build path documented for Android. The Android build uses the default
features, so it has the five networks. Whether `--features` passes through the script to the Tauri
CLI has not been verified.

The release workflow builds APKs only; it does not build an AAB. APKs are written under
`src-tauri/gen/android/app/build/outputs/apk/`. If `src-tauri/gen/android/keystore.properties`
exists, the release build is signed with it. The release workflow writes that file from the signing
secrets (section 8).

---

## 7. iOS

iOS builds run in CI only, on `macos-latest`, in the `iOS` job of `release.yml`:

1. `tauri ios init --ci` generates the Xcode project under `src-tauri/gen/apple` when it is missing.
2. `tauri ios build --export-method app-store-connect` builds the signed IPA. It uses the
   `IOS_CERTIFICATE_*` and `IOS_PROVISIONING_PROFILE_*` secrets and `APPLE_DEVELOPMENT_TEAM`.
3. `tauri ios build --target aarch64-sim` builds an unsigned simulator bundle.

The `chirper-native` plugin's Swift package and `src-tauri/Info.ios.plist` are merged in by the
Tauri CLI. Nothing in this repository documents a local iOS build.

---

## 8. Environment variables

| Variable | Read by | Purpose |
|---|---|---|
| `CHIRPER_TELEGRAM_API_ID`, `CHIRPER_TELEGRAM_API_HASH` | `chirper-net-telegram`, at compile time (`option_env!`) | Telegram API credentials for the build. Without both, Telegram needs credentials from Settings → Advanced. |
| `CHIRPER_TG_API_ID`, `CHIRPER_TG_API_HASH` | `release.yml` (GitHub secrets) | Mapped to the two variables above for release builds. |
| `ANDROID_HOME`, `ANDROID_SDK_ROOT`, `NDK_HOME`, `ANDROID_NDK_HOME` | `scripts/build-android.sh` | SDK and NDK locations. |
| `CHIRPER_WEB`, `CHIRPER_WEB_BASE` | `ui/vite.config.ts` | Browser build with the strict no-network policy, and its base path. |
| `ANDROID_KEYSTORE_B64`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD` | `release.yml` | Android release signing. Without them the build is unsigned. |
| `APPLE_DEVELOPMENT_TEAM`, `IOS_CERTIFICATE_*`, `IOS_PROVISIONING_PROFILE_*` | `release.yml` | iOS signing. |

### Telegram credentials

A user can enter the Telegram API id and hash in **Settings → Advanced** instead of building with
them. The settings are stored in `settings.json`, and the core reads them when it opens, so the
change applies after a restart. A valid user pair takes precedence over the compile-time pair. See
[NETWORKS.md](NETWORKS.md) section 1.

---

## 9. Checks

These are the commands CI runs (`.github/workflows/ci.yml`):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p chirper-core --features more-networks
cargo deny check

pnpm -C ui install --frozen-lockfile
pnpm -C ui check
pnpm -C ui test
pnpm -C ui build
```

The root `package.json` has a `check` script that runs the UI checks and the workspace clippy:

```sh
pnpm check    # pnpm --dir ui check && pnpm --dir ui test && cargo clippy --workspace --all-targets -- -D warnings
```

`cargo deny` reads `deny.toml`. CI runs it through the `cargo-deny-action`. Two more commands are
useful locally:

```sh
cargo run -p chirper-core --example smoke   # demo login, send, events, reopen the encrypted database
cargo test -p <crate>                       # one crate, for example chirper-net-discord
```

The examples that connect to real servers are not run by `cargo test`:
`crates/chirper-net-telegram/examples/` (`authkey_probe`, `login_probe`),
`crates/chirper-net-whatsapp/examples/` (`qr_probe`, `session_probe`) and
`crates/chirper-signal-protocol/examples/interop_reply.rs`.

CodeQL (`.github/workflows/codeql.yml`) analyses JavaScript and TypeScript, and Rust.

---

## 10. Reference sources (optional)

`scripts/fetch-third-party.sh` clones the mautrix bridge repositories at the commits pinned in
`third_party/pins.txt` into `third_party/`. Those clones are read-only reference material for
writing the network drivers. Nothing in the build uses them, and `third_party/*` is gitignored
except for `pins.txt`. The script needs git and network access.
