# Chirper — plan and status

Chirper is an on-device chat client (AGPL-3.0) for **iOS, Android, macOS, Windows and Linux**. It is
one Tauri v2 app: a Svelte 5 UI, a Rust shell, a Rust core, and one Rust protocol client per network.
There is no Chirper server. The network drivers run in the app process and talk only to the
networks.

This document records the goals, the layout and the status. The architecture is in
[ARCHITECTURE.md](ARCHITECTURE.md), the per-network state in [NETWORKS.md](NETWORKS.md), and the
open engineering work in [rust-core-port.md](rust-core-port.md).

## Goals

| # | Goal | How |
|---|---|---|
| G1 | Consistent design, independent of platform widgets | Own component set in Svelte; one token set; banner-first colour coding ([DESIGN.md](DESIGN.md)) |
| G2 | One interface on two mobile and three desktop systems | Tauri v2; stacked navigation on phones, split view from 900 px |
| G3 | Little friction: show only what is needed, when it is needed | Progressive disclosure rules in [DESIGN.md](DESIGN.md) |
| G4 | System integration on all five platforms | `src-tauri/src/platform/*` and `plugins/chirper-native`; see section 4 |
| G5 | Performance and privacy | Batched events, a custom media protocol, SQLCipher, no telemetry; see [SECURITY_MODEL.md](SECURITY_MODEL.md) |
| G6 | Icon and motion | `assets/brand/`; motion tokens in [DESIGN.md](DESIGN.md) |
| G7 | Public project | README, LICENSE, community files, CI, CodeQL, release workflow, Dependabot |

## 1. Layout

```
ui/                        Svelte 5 app (pnpm, Vite)
src-tauri/                 Tauri shell, crate chirper-app
crates/chirper-bridge/     unified bridge runtime and driver traits
crates/chirper-core/       store, engine, vault, secrets, JSON model
crates/chirper-net-*/      one protocol client per network
crates/chirper-signal-protocol/   Signal protocol primitives
plugins/chirper-native/    Android (Kotlin) and iOS (Swift) code
scripts/                   build-android.sh, fetch-third-party.sh
third_party/pins.txt      pinned mautrix commits for reference (clones are gitignored)
assets/brand/              icon and splash sources
docs/                      these documents, DESIGN.md, specs/ (briefs for driver work)
.github/                   CI, CodeQL, release and browser-build workflows, templates
```

## 2. Driver work

Each network is a separate crate that implements the traits in `chirper-bridge`. Their state is
listed in [NETWORKS.md](NETWORKS.md) and [rust-core-port.md](rust-core-port.md). A driver is listed
as unavailable, with its reason, until its login, receive and send paths work end to end.

## 3. Platform integration

What the code implements, per platform (`src-tauri/src/platform/` and `plugins/chirper-native`).
"Plugin" means the Kotlin or Swift code in `chirper-native`.

| Capability | macOS | Windows | Linux | Android | iOS |
|---|---|---|---|---|---|
| App lock | Touch ID or password (LocalAuthentication) | Windows Hello (UserConsentVerifier) | Not available | BiometricPrompt, plugin | Face ID or Touch ID, plugin |
| Database key storage | OS credential store (`keyring`) | OS credential store (`keyring`) | OS credential store (`keyring`) | Android Keystore wrap, plugin | Keychain, plugin |
| Screen privacy | Excluded from capture | Excluded from capture | Not available | `FLAG_SECURE`, plugin | Cover in the app switcher, plugin |
| Unread badge | Dock label | Taskbar overlay icon | Unity launcher over D-Bus, best effort | Yes | Through the plugin |
| Haptics | Trackpad haptics (NSHapticFeedback) | Not verified | No-op | Plugin | Plugin |
| Tray and background | Tray; close to tray | Tray; close to tray | Tray; close to tray | Connection service, plugin | Background refresh, plugin |
| Launch at login | Yes | Yes | Yes | Not applicable | Not applicable |
| Single instance | Yes | Yes | Yes | Not applicable | Not applicable |
| Notifications | Yes | Yes | Yes | Yes; inline reply | Not verified |
| Deep links `chirper://` | Yes | Yes | Yes | Yes | Yes |

Settings that persist are in `prefs.json`; see [CONTRACT.md](CONTRACT.md) section C.

## 4. Performance

- Each command is one call into the core. Events from every login go through one channel and reach
  the webview as `core-batch` messages, flushed every 16 ms or at 256 items (`src-tauri/src/events.rs`).
- Chat events for the same room in one batch are coalesced.
- Images, avatars and audio are served by the `chirper-media://` protocol, not as base64 over IPC.
- The release profile is `opt-level = 3`, fat LTO, one codegen unit, and `strip` (root `Cargo.toml`).
- Each login has its own supervisor task and its own command queue.

## 5. Status

Status for this refresh, on branch `rust-core-port`. The builds were not run for this document; it
describes the code and the checks that CI runs.

What CI checks on every push to `main` and on every pull request (`.github/workflows/ci.yml`):

- `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`, with the
  `chirper-core/mock` feature;
- `cargo test --workspace --features chirper-core/mock`, which includes the golden wire-contract tests, the end-to-end demo tests,
  the bridge runtime tests and the per-driver id and codec tests;
- `cargo deny check`;
- `pnpm -C ui check`, `pnpm -C ui test` and `pnpm -C ui build`.

Not verified in the repository, and only checked by hand or not at all:

- Live logins, receiving and sending on any network. No driver has been run against a real account
  in the repository's checks. Discord is the first to be exercised next (see
  [rust-core-port.md](rust-core-port.md)).
- Android and iOS device runtime: the key wrap, the connection service, notification reply,
  `FLAG_SECURE`, the app-switcher cover and background refresh.
- Windows and Linux runtime. The release workflow builds them; nothing in the repository runs them.
- Touch ID and Windows Hello prompts, the Windows taskbar overlay and the Linux Unity badge.
- The iOS build. It runs only in the `iOS` job of the release workflow.

## 6. Open decisions

- iOS export compliance: `ITSAppUsesNonExemptEncryption` is not set in `src-tauri/Info.ios.plist`.
- Owners and contacts: `.github/CODEOWNERS` names `@chirper-app/maintainers` as a placeholder, and the
  conduct contact in `CODE_OF_CONDUCT.md` is marked "to be set by the maintainers".
- Voice-note recording: the UI has audio playback for voice messages but no recording.
