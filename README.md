<p align="center">
  <img src="assets/brand/icon.svg" width="128" height="128" alt="Chirper icon">
</p>

<h1 align="center">Chirper</h1>

<p align="center">One on-device inbox for the chat networks you already use.</p>

<p align="center">
  <a href="https://github.com/itsrhotaudevvy/chirper/releases/latest"><img src="https://img.shields.io/github/v/release/itsrhotaudevvy/chirper?color=blue&label=release" alt="Latest Release"></a>
  <a href="https://github.com/itsrhotaudevvy/chirper/actions/workflows/ci.yml"><img src="https://github.com/itsrhotaudevvy/chirper/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/itsrhotaudevvy/chirper/actions/workflows/codeql.yml"><img src="https://github.com/itsrhotaudevvy/chirper/actions/workflows/codeql.yml/badge.svg" alt="CodeQL"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0-green.svg" alt="License: AGPL-3.0"></a>
  <a href="https://v2.tauri.app"><img src="https://img.shields.io/badge/Tauri-v2-orange.svg" alt="Tauri v2"></a>
</p>

Chirper signs in to chat networks and shows their chats in one app. The app is built with Tauri v2,
Svelte 5 and Rust. The whole core, including every network client, is Rust: there is no Go runtime
and no Matrix server. Each network is an unofficial client that talks to that network directly from
the app process.

---

## Downloads

Release packages are on the [GitHub Releases](https://github.com/itsrhotaudevvy/chirper/releases/latest)
page. The release workflow (`.github/workflows/release.yml`) builds desktop bundles for macOS,
Windows and Linux, an Android APK set, and an iOS build for App Store Connect and the simulator.

---

## Documentation

- [Getting started](docs/GETTING_STARTED.md): installing, adding accounts, the inbox.
- [Architecture](docs/ARCHITECTURE.md): crates, data flow, storage, how a network driver plugs in.
- [Building](docs/BUILDING.md): toolchains, desktop and Android builds, iOS in CI, tests.
- [Supported networks](docs/NETWORKS.md): per-network driver, login flows, capabilities and status.
- [Security and privacy model](docs/SECURITY_MODEL.md): keys, storage, webview policy, screen privacy.
- [Wire contract](docs/CONTRACT.md): the JSON model and the commands and events the UI uses.
- [Plan and status](docs/PLAN.md): goals, layout, what is verified and what is not.
- [Rust core port](docs/rust-core-port.md): decisions, current state and remaining work.
- [Design system](docs/DESIGN.md) and [privacy notes](PRIVACY.md).

## Why Chirper

- **No Chirper server.** The app has no backend. Network traffic goes only to the networks you log in to.
- **Data stays on the device.** Chat history is in one SQLCipher database. Media is sealed with the
  `OCV1` vault format. Per-login credentials are sealed with the database key.
- **One colour per network.** Each network has a fixed colour from the design system, so chats can
  be told apart at a glance. See [docs/DESIGN.md](docs/DESIGN.md).

## Supported platforms

| Platform | Packaging in the release workflow | App lock | Screen privacy |
|---|---|---|---|
| macOS | `.app`, `.dmg`; universal, arm64 and x64 | Touch ID or password | Excluded from capture (`set_content_protected`) |
| Windows | `.msi`, NSIS `.exe`; x64, x86, arm64 | Windows Hello | Excluded from capture |
| Linux | `.AppImage`, `.deb`, `.rpm`; x64, arm64 | Not available | Not available |
| Android | APK (universal and per-ABI) | BiometricPrompt, through `chirper-native` | `FLAG_SECURE` |
| iOS | App Store Connect build and simulator build, in CI only | Face ID or Touch ID, through `chirper-native` | Cover in the app switcher |

## Supported networks

The default build ships five networks, and the demo network in demo mode. The status below is what
each driver's `unavailable()` reports in this repository. None of these networks has been tested
against live servers: the drivers' tests use fake servers, so login, receiving and sending on the
real networks are unverified (see [docs/PLAN.md](docs/PLAN.md) section 5).

| Network | Status in this build |
|---|---|
| WhatsApp | Available. Scan a QR code, or enter a pairing code on the phone. |
| Telegram | Available once an API id and hash are set, at build time or in Settings → Advanced. Phone number with login code, QR code, or bot token. Otherwise listed as unavailable. |
| Discord | Available. Browser sign-in, QR code, user token, or bot token. |
| Fluxer | Available. Session token, bot token, or e-mail or username with password. |
| SMS | Available on Android only, through the phone's own SMS service. Listed as unavailable elsewhere. |

Nine more networks (Signal, Messenger, Instagram, X, LinkedIn, Google Messages, Google Voice, Slack
and Bluesky) and iMessage are not in the default build. The nine are compiled by eight Cargo features
of `chirper-core`; `net-meta` covers Messenger and Instagram. They are behind Cargo features of
`chirper-core`, and their unfinished work is on the branch `wip/more-networks`. See
[docs/NETWORKS.md](docs/NETWORKS.md) section 2 for their state and [docs/BUILDING.md](docs/BUILDING.md)
section 4 for building them.

Each network's login flows, capabilities and protocol notes are in [docs/NETWORKS.md](docs/NETWORKS.md).

## Architecture

```
Svelte UI (ui/)
   │ invoke(command)                 ▲ "core-batch" events (batched every 16 ms)
src-tauri/ (chirper-app)              │ commands, lock, notifications, media protocol
   │ plain Rust calls                 │
chirper-core  (store, engine, vault, secrets, facade)
   │ Command / Event
chirper-bridge  (runtime: supervision, backoff, ids, capability gates)
   │ Session / LoginProcess
chirper-net-<network>  (one protocol client per network)
```

The full picture, including the data flow and storage, is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Build from source

### Prerequisites

- **Rust**, stable channel, through rustup. `rust-toolchain.toml` selects the channel and the targets.
  `src-tauri` declares `rust-version = 1.90`.
- **Node.js 22 or newer** and **pnpm** (CI uses pnpm 12).
- **Tauri CLI 2.12.5**. See [docs/BUILDING.md](docs/BUILDING.md) for how to install it.
- **Platform packages** for Tauri, listed in [docs/BUILDING.md](docs/BUILDING.md).
- **Android:** SDK, NDK `28.2.13676358` and JDK 21. **iOS:** full Xcode on macOS.

### Quick start

```sh
pnpm -C ui install
cargo tauri dev          # the desktop app, with the UI dev server
```

`pnpm -C ui dev` runs the UI alone in a browser, with an in-memory demo core in place of the Rust core.
For the demo network inside the desktop app, add `--features mock`.

Release bundles, Android builds and the iOS path are in [docs/BUILDING.md](docs/BUILDING.md).

### Tests and checks

These are the commands CI runs (see [docs/BUILDING.md](docs/BUILDING.md) section 9):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p chirper-core --features more-networks
cargo deny check
pnpm -C ui install --frozen-lockfile && pnpm -C ui check && pnpm -C ui test && pnpm -C ui build
```

## Privacy

Chirper has no telemetry, crash reporting or analytics, and no analytics crate is in the lock file.
Read [PRIVACY.md](PRIVACY.md) for what is stored and where.

## Security

Report vulnerabilities privately through GitHub Security Advisories. See [SECURITY.md](SECURITY.md).
The model is described in [docs/SECURITY_MODEL.md](docs/SECURITY_MODEL.md).

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) for the development setup, code style and the pull request
checklist. This project follows the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

Chirper is free software licensed under the [GNU Affero General Public License v3.0 or later](LICENSE).
