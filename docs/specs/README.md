# Driver and protocol specs (rust-core-port)

Each file in this directory is the complete brief for one implementation task. An implementation
agent reads **this file first, then its own spec**, and nothing it does may contradict either.

## Rules for every task

1. **Read before writing.** `docs/RUST_BRIDGE_ARCHITECTURE.md`, `docs/rust-core-port.md`,
   `crates/chirper-bridge/src/{network.rs,model.rs}` (the only API a driver talks to) and the
   reference driver `crates/chirper-net-discord/src/` (layout, error mapping, login flow, session
   loop, backfill, tests). Match their style.
2. **Ownership.** Create or modify files only inside the paths your spec lists as *owned*. Read
   anything. Never edit another crate, the workspace `Cargo.toml`, `crates/chirper-core`,
   `src-tauri`, `ui`, `docs/` (except your own spec's "Report" section is *not* yours either —
   report in your final message), or `third_party/`.
3. **Our own protocol client.** No client, SDK or bridge library for the network (no `grammers`,
   `whatsapp-rust`, `libsignal`, `atrium`, `slack-morphism`, …). Allowed: vetted primitives for
   cryptography (RustCrypto `aes`, `aes-gcm`, `cbc`, `ctr`, `sha1`, `sha2`, `hmac`, `hkdf`, `pbkdf2`,
   `curve25519-dalek`, `x25519-dalek`, `ed25519-dalek`, `num-bigint`), codecs (`serde`,
   `serde_json`, `prost` derive, `flate2`, `base64`, `hex`), transport (`reqwest` with rustls,
   `tokio`, `tokio-tungstenite` with rustls). Never hand-roll a cipher, hash or MAC.
4. **Reference sources** (read-only, for protocol knowledge; data tables may be copied *with an
   attribution comment naming the file and licence*):
   - mautrix bridges (Go, AGPL-3.0): `/Volumes/SSD/Code/Chirper/third_party/<network>/`
   - Go module cache: `~/go/pkg/mod/go.mau.fi/{whatsmeow,libsignal}@*/`
   - Our Go connectors: `/Volumes/SSD/Code/Chirper/go/connectors/{discord,fluxer}/`
   Port behaviour, not Go idioms: no global state, no goroutine-per-thing, no `interface{}` maps.
5. **Dependencies.** Pin exact versions (`"=x.y.z"`). Prefer versions already in
   `Cargo.lock` (run `grep -A1 'name = "<crate>"' Cargo.lock`). Add a new crate only if nothing in
   the lockfile does the job, and say so in your report.
6. **Build in your own target directory** so parallel agents do not block each other:
   `export CARGO_TARGET_DIR=/Volumes/SSD/Code/Chirper-rust/target/agents/<your-task-id>`.
   Also put rustup's toolchain first on PATH (Homebrew ships a second, incompatible
   `clippy-driver`): `export PATH=$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH`.
   Set both in **every** shell command (the shell does not keep exports between calls).
   Only build/test **your own crate**: `cargo test -p <crate>`,
   `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>`.
   Never run workspace-wide commands; other crates are being edited at the same time.
7. **Quality bar.** `#![forbid(unsafe_code)]`. Clippy clean with `-D warnings`. No `unwrap()`,
   `expect()` or indexing that can panic on data from the network; every parse returns
   `chirper_bridge::Error::Protocol`. Small functions, doc comments on public items, no dead code,
   no `todo!()`/`unimplemented!()`, no stubs that pretend to work. Something genuinely not done
   returns `Error::Unsupported("<what>")`, and `Network::unavailable()` stays `Some(reason)` until
   login **and** receiving **and** sending text work end to end in your tests.
8. **Privacy.** Never log message bodies, tokens, cookies, keys, phone numbers or e-mail
   addresses. Use `log::debug!` for protocol milestones only. `Debug` on any type that holds a
   secret prints `[redacted]`. Secrets go through `LoginCtx::{get,set,delete}_secret`, never to
   files of your own.
9. **Tests without the network.** Every codec, crypto step and state machine is tested with fixed
   vectors (copied from the reference implementation's tests or computed once and pinned) and
   with fake transports (a `tokio::io::duplex`, a local `TcpListener`, or a trait the session is
   generic over). Tests never touch the internet. A live probe, if useful, is an `examples/`
   binary that reads credentials from environment variables and is never run by `cargo test`.
10. **Mapping to the bridge model.** Remote ids are **exactly the strings the Go bridge used**:
    the reference bridge's `PortalID`, `MessageID` (incl. part ids), `UserID`, `UserLoginID` and
    `PortalKey.Receiver` rules (its `ids`/`*id` package or `ids.go`; cite the file in a comment).
    Databases written by the Go build are keyed by them, so a different string orphans the user's
    history. Add a unit test per id kind pinning the format. Messages
    the local user sent from another device set `from_me: true`. History pages set `live: false`.
    Media is downloaded by the driver and handed over in `Content::Media { data, .. }`, capped at
    100 MiB; larger files become `Content::Unsupported` with a description.
11. **Git.** Do not run git commands that change anything (no add/commit/stash/checkout/reset).
12. **Final report** (your last message): files created/changed; what is complete and tested;
    what is NOT complete or uncertain (be blunt); exact commands you ran and their results
    (test counts); any new dependency and why; questions about the bridge API.

## Task index

| id | spec | crate(s) owned | depends on |
| --- | --- | --- | --- |
| signal-protocol | `signal-protocol.md` | `crates/chirper-signal-protocol` | — |
| telegram | `telegram.md` | `crates/chirper-net-telegram` | — |
| bluesky | `bluesky.md` | `crates/chirper-net-bluesky` | — |
| slack | `slack.md` | `crates/chirper-net-slack` | — |
| linkedin | `linkedin.md` | `crates/chirper-net-linkedin` | — |
| gvoice | `gvoice.md` | `crates/chirper-net-gvoice` | — |
| whatsapp | `whatsapp.md` | `crates/chirper-net-whatsapp` | signal-protocol |
| gmessages | `gmessages.md` | `crates/chirper-net-gmessages` | — |
| twitter | `twitter.md` | `crates/chirper-net-twitter` | — |
| signal | `signal.md` | `crates/chirper-net-signal` | signal-protocol |
| meta | `meta.md` | `crates/chirper-net-meta` | signal-protocol (E2EE only) |

## Parity targets from the Go build (pinned by `crates/chirper-core/tests/golden.rs`)

Name, colour and **every login flow id** below must be kept once a driver is available.

| id | colour | login flows | capabilities in the Go build |
| --- | --- | --- | --- |
| whatsapp | #25D366 | qr, phone | all |
| telegram | #26A5E4 | (phone, qr, bot in mautrix) | all |
| signal | #3A76F0 | (qr link in mautrix) | — |
| instagram | #E1306C | android, instagram, instagram-password | all but voice |
| messenger | #FF2542 | facebook, messenger, messenger-lite, messenger-lite-android | all but voice |
| twitter | #7C5CFF | cookies, password | edits, reactions, readReceipts, redactions, replies, typing |
| linkedin | #FF2D7A | cookies, password | edits, reactions, readReceipts, redactions, replies, typing |
| gmessages | #1A73E8 | google (and qr) | all |
| gvoice | #34A853 | cookies | readReceipts, replies |
| slack | #4A154B | email, token, app | all |
| bluesky | #1185FE | password | readReceipts, replies |
| discord | #5865F2 | browser, qr, token, bot | all |
| fluxer | #FF77A9 | token, bot, password | all |

"Beyond parity" is welcome (more capabilities than the Go build had) when it is real and tested.
