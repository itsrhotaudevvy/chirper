//! Chirper's core: one database, one bridge runtime, one event stream.
//!
//! The shell (Tauri) calls [`Core`], which stores everything locally and talks to the network
//! drivers through [`chirper_bridge::Runtime`]. No Matrix, no Go runtime, no C ABI, and no
//! `unsafe` in this crate or the drivers.
//!
//! * [`model`] – the camelCase JSON the UI already uses (`docs/CONTRACT.md`).
//! * [`store`] – the SQLite/SQLCipher timeline, schema-compatible with installs of the Go core.
//! * [`secure`] – the `OCV1` media vault, sealed per-login secrets and log redaction.
//! * [`content`] – versioned, Matrix-free event content with a reader for legacy rows.
//! * [`engine`] – the facade: chats, messages, logins, settings and event routing.

#![forbid(unsafe_code)]

pub mod content;
pub mod engine;
pub mod ids;
pub mod model;
pub mod secure;
pub mod store;

pub use engine::{
    Core, Error, EventStream, MediaUpload, Options, Result, SETTINGS_FILE, networks::HostNetworks,
    validate,
};

/// The crate's place in the version line, for the shell's diagnostics.
pub const CORE_ABI: u32 = 2;
