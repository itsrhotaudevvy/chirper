## Summary

<!-- What changes and why. Link the issue if there is one: Fixes #123 -->

## Type of change

- [ ] Bug fix
- [ ] Feature
- [ ] Refactor or performance
- [ ] Docs, CI or build

## Checklist

- [ ] Commit messages follow Conventional Commits (`feat(scope): ...`, `fix(scope): ...`).
- [ ] Tests added or updated for the logic I changed, and they pass locally.
- [ ] `gofmt` and `go vet` are clean for changed Go modules.
- [ ] `cargo fmt --check` and `cargo clippy -- -D warnings` are clean for changed Rust crates.
- [ ] `pnpm -C ui check`, `test` and `build` pass if the UI changed.
- [ ] No message content in logs, error strings or crash output. No tokens, cookies, phone numbers or keys in logs.
- [ ] No new network calls except to the messaging networks themselves. No telemetry, analytics, remote fonts or CDNs.
- [ ] New dependencies are pinned, have an AGPL-compatible licence, and pass `cargo deny check`.
- [ ] Secrets, keys and key material are stored only in the OS keystore, never in files.
- [ ] Docs (README, PRIVACY.md, CHANGELOG.md under Unreleased) updated where behaviour changed.

## Screenshots or recordings (UI changes)

<!-- Use light and dark themes. Redact any real messages or contacts. -->
