# Contributing to Chirper

Thanks for helping. Chirper is an on-device messenger, so privacy and correctness come before
features. Please read this file, [SECURITY.md](SECURITY.md) and the [Code of Conduct](CODE_OF_CONDUCT.md)
before you open a pull request.

## Development setup

See [docs/BUILDING.md](docs/BUILDING.md) for the toolchains and the platform packages. In short:

```sh
pnpm -C ui install
cargo tauri dev
```

Optional, for reading the mautrix reference sources while writing a network driver:

```sh
scripts/fetch-third-party.sh     # clones the pinned commits from third_party/pins.txt
```

Toolchain versions: Rust stable (from `rust-toolchain.toml`), Node.js 22 or newer, and pnpm 12. The
Tauri CLI is 2.12.5.

## Repository layout

| Path | Contents |
|---|---|
| `ui/` | Svelte 5 and TypeScript front end (Vite) |
| `src-tauri/` | Tauri application crate `chirper-app`: commands, events, platform code |
| `crates/chirper-core/` | Store, engine, facade, media vault, secrets, JSON model |
| `crates/chirper-bridge/` | The unified bridge runtime and the traits a driver implements |
| `crates/chirper-net-*/` | One protocol client per network |
| `crates/chirper-signal-protocol/` | Signal protocol primitives, used by the WhatsApp driver |
| `plugins/chirper-native/` | Kotlin and Swift code for mobile-only platform features |
| `scripts/` | `build-android.sh`, `fetch-third-party.sh` |
| `third_party/` | `pins.txt` (committed) and the reference clones it names (not committed) |
| `docs/` | Architecture, build, contract, networks, security and plan documents |
| `docs/specs/` | Briefs for driver work |
| `assets/brand/` | Icon and splash sources |
| `.github/` | CI, CodeQL, release and browser-build workflows, Dependabot, issue and PR templates |

Changes to the JSON model must update [docs/CONTRACT.md](docs/CONTRACT.md), the golden fixtures in
`crates/chirper-core/tests/golden/`, and the TypeScript mirror `ui/src/lib/core/types.ts`, in the
same pull request.

## Code style

- **Rust:** `cargo fmt`, and `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
  A change to a gated network must also keep `cargo check -p chirper-core --features more-networks`
  passing (see [docs/BUILDING.md](docs/BUILDING.md) section 4). Do not use `unwrap()` or `expect()` on data that comes from the network, the
  webview or the disk; return an error. New crates use `#![forbid(unsafe_code)]`. Keep functions
  small.
- **TypeScript and Svelte:** `strict` mode, no `any`. Use the tokens in `ui/src/styles/` rather
  than hard-coded colours or sizes. `pnpm -C ui check`, `pnpm -C ui test` and `pnpm -C ui build` must
  pass.
- **All languages:** no dead code, no commented-out code, and no `TODO` stubs that pretend to work.
  A feature a platform or network does not support returns a clear "not supported" error, as in
  `Error::Unsupported`.
- **Editor settings:** `.editorconfig` is authoritative. Files use LF line endings.

### Privacy rules for code

- Never log message bodies, tokens, cookies, phone numbers or keys, even at debug level. Log
  identifiers only when they are needed and cannot identify a person.
- No telemetry, analytics, crash reporters, remote fonts or CDNs.
- Network traffic goes only to the messaging networks themselves.
- Keys live in the OS keystore or the mobile keystore, never in files, environment variables or logs.
  Per-login secrets go through `LoginCtx`, not through files of the driver's own.

## Commit style

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(core): add unread counts to listChats
fix(ui): keep composer focus after send
ci: pin codeql-action to a commit SHA
```

Common types: `feat`, `fix`, `perf`, `refactor`, `test`, `docs`, `build`, `ci`, `chore`. Use the
scope for the area you changed (`core`, `ui`, `app`, `bridge`, `net-<network>`, `ci`). Keep the
subject under 72 characters and write the body in the imperative mood.

## Pull requests

1. Branch from `main`. Keep one logical change per pull request.
2. Run the checks for the areas you touched (see [docs/BUILDING.md](docs/BUILDING.md), section 8).
3. Fill in the pull request template. Its checklist is part of the review.
4. Add an entry under `## [Unreleased]` in [CHANGELOG.md](CHANGELOG.md) for user-visible changes.
5. A maintainer reviews the change. CI, CodeQL and `cargo deny` must pass before merge.

### Checklist (also in the pull request template)

- [ ] Commit messages follow Conventional Commits.
- [ ] Tests cover the logic I changed and pass locally.
- [ ] No message content in logs, error strings or crash output. No tokens, cookies, phone numbers or keys in logs.
- [ ] No new network calls except to the messaging networks themselves.
- [ ] New dependencies are pinned, licensed under AGPL-compatible terms, and pass `cargo deny check`.
- [ ] Docs and the changelog are updated where behaviour changed, including
      [docs/NETWORKS.md](docs/NETWORKS.md) when a driver's status, login flows or capabilities change.
- [ ] A network that moves into or out of the default build updates `engine/networks.rs`, the feature
      list in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) section 2 and the README.

## Reference sources and third-party code

`third_party/pins.txt` pins the mautrix bridge repositories by commit. The clones are reference
material for writing the network drivers. Nothing in the build uses them, and their contents are
not committed. To change a pin, update `pins.txt` in a pull request, explain the upstream change,
and check that `scripts/fetch-third-party.sh` still fetches the exact commit. Do not edit files
under `third_party/` in place.

Code or data copied from a reference source needs an attribution comment that names the file and
its licence, as the driver briefs require.

## Licence

By contributing you agree that your contribution is licensed under the GNU Affero General Public
License v3.0 or later, the licence of this project (see [LICENSE](LICENSE)).
