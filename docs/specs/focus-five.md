# Task `focus-five`: ship only WhatsApp, Fluxer, Discord, Telegram and SMS

The product owner wants the app, for now, to support exactly five networks: **WhatsApp, Fluxer,
Discord, Telegram, SMS** (plus the demo `testnet` network when demo mode is on). The other driver
crates stay in the repository and keep compiling, but they are not part of the app build and are
not listed in the UI.

**Owned:** `crates/chirper-core/Cargo.toml`, `crates/chirper-core/src/engine/networks.rs`,
`crates/chirper-core/tests/**`, `src-tauri/Cargo.toml`, `.github/workflows/ci.yml`,
`package.json`, `ui/src/lib/core/fake.ts` (and its test file, if any).

1. In `chirper-core/Cargo.toml`, make every driver dependency except discord, fluxer, telegram,
   whatsapp, sms and testnet `optional = true`, each behind a feature named `net-<crate suffix>`
   (`net-signal`, `net-meta`, `net-twitter`, `net-linkedin`, `net-gmessages`, `net-gvoice`,
   `net-slack`, `net-bluesky`), plus `more-networks = [all eight]`. Default features stay as
   they are otherwise (`sqlite-plain`).
2. `engine/networks.rs`: register those eight only under their `#[cfg(feature = "...")]`. The
   `UNPORTED` list (iMessage) is not shown in this build: gate it behind `more-networks` as well,
   so the network list is exactly the five (+ `testnet` in demo mode). Keep the registry order:
   the five in the order WhatsApp, Telegram, Discord, Fluxer, SMS.
3. `tests/golden.rs` `network_rows_match_the_go_core`: check the Go rows only for networks
   compiled into this build, and assert that the build lists exactly `whatsapp`, `telegram`,
   `discord`, `fluxer`, `sms` (+ `testnet`) by default. Adjust `tests/demo.rs` if it relies on
   networks that are now gated off.
4. CI (`ci.yml`): keep `cargo test --workspace` and clippy over the workspace (this still compiles
   every driver crate as its own package), and add one `cargo check -p chirper-core --features
   more-networks` step so the gated registry keeps compiling.
5. `ui/src/lib/core/fake.ts` (the browser demo core): offer only the same five networks (+ demo),
   keeping its data valid for the UI tests; run `pnpm -C ui check` and `pnpm -C ui test`.
6. `src-tauri/Cargo.toml`: nothing to enable; make sure it does not turn `more-networks` on.

Verify: `cargo test -p chirper-core`, `cargo check -p chirper-core --features more-networks`,
`cargo clippy -p chirper-core --all-targets -- -D warnings`, `cargo test -p chirper-core --test
golden`, `pnpm -C ui check`, `pnpm -C ui test`. Note: the telegram, whatsapp and discord crates are
being edited by other agents; if chirper-core fails to build because of an error *inside* one of
them, wait a few minutes and retry rather than touching their code, and report it if it persists.
