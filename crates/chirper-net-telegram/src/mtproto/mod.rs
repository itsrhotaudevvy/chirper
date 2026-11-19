//! The MTProto 2.0 client: cryptography, transport, the auth key handshake and the encrypted
//! session.
//!
//! Phase 2 builds on these pieces: [`connect`] opens a data centre, [`generate_auth_key`] creates
//! the key, and [`MtSession::start`] runs encrypted RPC calls over it.

mod authkey;
mod constants;
pub mod crypto;
pub mod plain;
mod session;
pub mod transport;

pub use authkey::{AuthKey, AuthKeyError, AuthKeyResult, generate_auth_key};
pub use session::{InvokeError, MtSession, UpdateFrame};
pub use transport::{
    Dc, Transport, TransportError, connect, connect_with, production_dc, test_dc2,
};
