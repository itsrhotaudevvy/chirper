# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it reaches 1.0.0.

## [Unreleased]

### Added

- Project governance and community files: licence (AGPL-3.0-or-later), README, contributing guide,
  security policy, code of conduct and privacy statement.
- GitHub support: issue and pull request templates, Dependabot configuration for Cargo, npm, Go
  modules and GitHub Actions, and a CODEOWNERS file.
- Continuous integration for the Go core and connectors, the Rust workspace (including `cargo deny`)
  and the UI, with CodeQL analysis for Go, JavaScript/TypeScript and Rust.
- Release workflow that builds desktop bundles for macOS (universal), Windows and Linux, Android APK and
  AAB, and an iOS build, and drafts a GitHub release with the artifacts attached.
- Repository configuration: `.editorconfig`, `.gitattributes`, `rust-toolchain.toml` and a `deny.toml`
  licence policy compatible with AGPL-3.0.
