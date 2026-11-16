# Security policy

## Reporting a vulnerability

Please report security issues privately. Do not open a public issue, pull request or discussion.

Use GitHub Security Advisories: <https://github.com/chirper-app/chirper/security/advisories/new>

Include:

- the affected version or commit and platform,
- steps to reproduce or a proof of concept,
- the impact you expect,
- whether you plan to publish the issue, and when.

Do not include real message content, real phone numbers, or other people's accounts or tokens in the
report. Use test accounts where possible.

We aim to acknowledge reports within 7 days and to agree a fix and disclosure date with you. This is a
volunteer project, so these are targets, not guarantees. We credit reporters in the advisory unless
you ask us not to.

## Supported versions

Chirper is pre-1.0. Only the latest release and the `main` branch receive security fixes.

## Scope

In scope:

- The Chirper application: Svelte UI, Tauri commands and events, Rust crates, Go core, and the Chirper
  connectors in `go/connectors/`.
- The C ABI between Rust and Go, and the Tauri capability and CSP configuration.
- Local storage: database, media vault, key handling, file permissions and the app lock.
- Build and release automation in `.github/` and `scripts/`.
- Pinned third-party connector code, when the issue is in how Chirper uses it.

Out of scope:

- Vulnerabilities in the messaging networks themselves, or in their servers and protocols. Report
  those to the network vendor.
- Vulnerabilities in the upstream mautrix connectors that do not affect Chirper's use of them. Report
  those upstream. We still want to know about them.
- Denial of service that needs an already-authenticated local user to cause.
- Social engineering, physical attacks that need the device unlocked by its owner, and spam from
  networks.

## Threat model summary

Chirper has no server. The threats we design against are:

1. **Local attacker with device access.** Someone who can read the app's data directory, or who has
   the device while it is locked. Mitigations: database key in the OS keystore (Keychain, Credential
   Manager, Secret Service, Android Keystore, iOS Keychain `ThisDeviceOnly`), files created with `0700`
   and `0600`, media encrypted with AES-GCM, app lock (biometrics or passphrase), screen privacy
   (`FLAG_SECURE`, `NSWindow.sharingType`, `SetWindowDisplayAffinity`, app-switcher blur).
   Limit: an attacker with the unlocked device and the running app can read what the app shows.

2. **Network attacker.** Someone on the path between the device and a messaging network. Mitigations:
   TLS for all connector traffic, certificate validation on by default, no plaintext fallback, no
   traffic to any Chirper-operated host. Limit: metadata visible to the network you use, as with any
   client of that network.

3. **Malicious network payloads.** A message, media file, profile or event from a remote party, or a
   compromised network endpoint. Mitigations: all FFI inputs validated in Rust and re-validated in Go,
   argument length limits (1 MiB for JSON args), panics recovered on both sides of the boundary, media
   served only from paths inside the data directory, strict CSP and scoped Tauri capabilities, no
   message content rendered as HTML without sanitising. Limit: parser bugs in third-party libraries are
   handled through dependency updates and `cargo deny`, `govulncheck` and CodeQL.

Out of scope for the threat model: a compromised operating system or kernel, a jailbroken or rooted
device, malicious firmware, a malicious build toolchain, and a compromised developer machine that
signs releases. Chirper cannot protect secrets against an attacker who controls the OS.

## Safe handling in logs

Logs must never contain message bodies, tokens, cookies, phone numbers or keys. Bug reports and
screenshots should be redacted the same way.
