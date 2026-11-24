# Task `docs-refresh`: documentation for the all-Rust core

**Owned:** `README.md`, `CONTRIBUTING.md`, `docs/ARCHITECTURE.md`, `docs/BUILDING.md`,
`docs/CONTRACT.md`, `docs/PLAN.md`, `docs/NETWORKS.md`, `docs/rust-core-port.md`,
`docs/GETTING_STARTED.md`, `docs/SECURITY_MODEL.md`. Nothing else (not `docs/specs/`, not
`docs/RUST_BRIDGE_ARCHITECTURE.md`, not `docs/DESIGN.md`, no code).

The Go core (`go/`, cgo, C ABI, `scripts/build-go.sh`, `scripts/build-sqlcipher.sh`, the mautrix
bridges as dependencies) is gone. The app is Svelte UI → Tauri (`src-tauri`) → `chirper-core`
(store, engine, media vault, secrets) → `chirper-bridge` (unified runtime) → one
`chirper-net-<network>` crate per network, each our own protocol client. Source of truth: read
`docs/RUST_BRIDGE_ARCHITECTURE.md`, `docs/specs/README.md` (parity table), `Cargo.toml`,
`crates/*/Cargo.toml`, `crates/*/src/lib.rs` doc comments, `src-tauri/Cargo.toml`,
`scripts/build-android.sh`, `.github/workflows/*.yml`, and `git log --oneline -40` (read-only).

Write plainly; no marketing tone; no claims you cannot point to in the code.

1. `README.md`: what Chirper is, the networks (from each driver's `unavailable()`: usable vs.
   listed-as-unavailable — check the code, do not guess), build quick start, links.
2. `docs/ARCHITECTURE.md`: the crate graph and data flow (commands in, `core-batch` events out),
   storage (one `chirper.db`, SQLCipher, OCV1 media vault, sealed per-login secrets), the bridge
   runtime responsibilities, how a driver plugs in, host-provided drivers (Android SMS).
3. `docs/BUILDING.md`: prerequisites (Rust stable via rustup with the targets in
   `rust-toolchain.toml`, Node 22+ / pnpm 12, Tauri CLI 2.12.5, Android SDK + NDK 28.2.13676358 +
   JDK 21, Xcode for iOS), `cargo tauri dev`, desktop builds, `scripts/build-android.sh`, iOS via
   CI, the `CHIRPER_TELEGRAM_API_ID/HASH` build variables, the Homebrew `clippy-driver` pitfall
   (put `~/.rustup/toolchains/<toolchain>/bin` first on PATH), the test commands CI runs, and
   `scripts/fetch-third-party.sh` as optional reference sources. Remove every Go/cgo/MinGW step.
4. `docs/CONTRACT.md`: delete the C ABI section and any Go-specific wording; keep the JSON
   envelopes unchanged; mention that `crates/chirper-core/tests/golden/` pins them.
5. `docs/PLAN.md`: replace the Go architecture diagram and Go-specific rows with the Rust ones;
   update §6 status to what is verified now (state what is not verified: live network logins,
   device runtime, Windows/Linux runtime).
6. `docs/NETWORKS.md`: one row per network: driver crate, login flows (from `login_flows()`),
   capabilities (from `capabilities()`), available or the reason, protocol notes. Read the code.
7. `docs/rust-core-port.md`: rewrite "Current state" and "Remaining work" from the code.
8. Other owned files: remove Go references, keep the rest.

Check every relative link you write resolves (`ls` the target). End with the final report from
`docs/specs/README.md` (files changed, what you verified).
