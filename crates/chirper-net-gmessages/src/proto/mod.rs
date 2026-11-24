//! The protobuf schemas of the Google Messages web protocol, as `prost` structs.
//!
//! [`pairing`] holds the authentication, UKEY2, RPC envelope and Gaia messages; [`chat`] the
//! conversations, requests, responses and update events. Both are plain data: the encoding that
//! goes on the wire is chosen by the caller (binary protobuf or the positional `pblite` JSON of
//! [`crate::pblite`]).

pub mod chat;
pub mod pairing;
