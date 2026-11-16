# Contributing to Chirper

Thanks for helping. Chirper is an on-device messenger, so privacy and correctness come before
features. Please read this file, [SECURITY.md](SECURITY.md) and the [Code of Conduct](CODE_OF_CONDUCT.md)
before you open a pull request.

## Development setup

See the **Build from source** section of the [README](README.md#build-from-source). In short:

```sh
scripts/fetch-third-party.sh     # pinned connector sources into third_party/
pnpm -C ui install
scripts/build-go.sh <rust-triple>
cargo tauri dev
```

Toolchain versions: Rust (stable, from `rust-toolchain.toml`), Go 1.27, Node.js 22+ with pnpm.

## Repository layout

| Path | Contents |
|---|---|
| `ui/` | Svelte 5 + TypeScript front end (Vite) |
| `src-tauri/` | Tauri application crate: commands, events, platform glue |
| `crates/chirper-core/` | Safe Rust wrapper over the Go C ABI, keystore access |
| `plugins/chirper-native/` | Kotlin and Swift code for mobile-only platform features |
| `go/core/` | Go core: C ABI (`capi`), facade, engine, local Matrix store |
| `go/connectors/` | Chirper-owned connectors (Discord, Fluxer) |
| `third_party/` | Pinned mautrix connector clones, fetched, never committed (`pins.txt` is) |
| `scripts/` | `fetch-third-party.sh`, `build-go.sh` |
| `docs/` | `PLAN.md`, `CONTRACT.md`, `DESIGN.md`, `AGENTS.md` (binding design documents) |
| `assets/brand/` | Icon and splash sources |
| `.github/` | CI, CodeQL, release workflows, Dependabot, issue and PR templates |

Changes to the JSON model or the C ABI must update `docs/CONTRACT.md` and every mirror of the
contract in the same pull request.

## Code style

- **Rust:** `cargo fmt`, and `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
  Do not use `unwrap()` or `expect()` on data that comes from the network, the webview or the disk.
  Return errors instead. Keep functions small.
- **Go:** `gofmt` and `go vet` must be clean. Run tests with `-race`. Recover panics at the C ABI
  boundary and return them as error envelopes. Keep the package boundaries in `go/core` intact.
- **TypeScript and Svelte:** `strict` mode, no `any`. Use the design tokens in `ui/src/styles/`
  rather than hard-coded colours or sizes. `pnpm -C ui check`, `test` and `build` must pass.
- **All languages:** no dead code, no commented-out code, no `TODO` stubs that pretend to work. If a
  platform does not support a feature, return a clear "not supported on <platform>" error.
- **Editor settings:** `.editorconfig` is authoritative. Files use LF line endings.

### Privacy rules for code

- Never log message bodies, tokens, cookies, phone numbers or keys, even at debug level. Log
  identifiers only when they are needed and cannot identify a person.
- No telemetry, analytics, crash reporters, remote fonts or CDNs.
- The only network traffic is to the messaging networks themselves.
- Keys live in the OS keystore, never in files, environment variables or logs.

## Commit style

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(core): add unread counts to listChats
fix(ui): keep composer focus after send
ci: pin codeql-action to a commit SHA
```

Common types: `feat`, `fix`, `perf`, `refactor`, `test`, `docs`, `build`, `ci`, `chore`. Use the
scope for the area you changed (`core`, `ui`, `app`, `crypto`, `connectors`, `ci`). Keep the subject
under 72 characters and write the body in the imperative mood.

## Pull requests

1. Branch from `main`. Keep one logical change per pull request.
2. Run the checks for the areas you touched (see the README).
3. Fill in the pull request template. Its checklist is part of the review.
4. Add an entry under `## [Unreleased]` in [CHANGELOG.md](CHANGELOG.md) for user-visible changes.
5. A maintainer reviews the change. CI, CodeQL and `cargo deny` must pass before merge.

### Checklist (also in the pull request template)

- [ ] Commit messages follow Conventional Commits.
- [ ] Tests cover the logic I changed and pass locally.
- [ ] No message content in logs, error strings or crash output. No tokens, cookies, phone numbers or keys in logs.
- [ ] No new network calls except to the messaging networks themselves.
- [ ] New dependencies are pinned, licensed under AGPL-compatible terms, and pass `cargo deny check`.
- [ ] Docs and the changelog are updated where behaviour changed.

## Adding or updating third-party connectors

Connector sources are pinned by commit in `third_party/pins.txt`. To change a pin, update that file
in a pull request, explain the upstream change, and make sure `scripts/fetch-third-party.sh` still
fetches the exact commit. Do not edit files under `third_party/` in place.

## Licence

By contributing you agree that your contribution is licensed under the GNU Affero General Public
License v3.0 or later, the licence of this project (see [LICENSE](LICENSE)).
