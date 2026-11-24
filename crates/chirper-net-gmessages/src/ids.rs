//! Remote ids, in the format of the Go bridge (`connector/id.go`, `connector/stableid.go`).
//!
//! Every chat, message and person id is `<prefix>.<google id>`, where the prefix identifies the
//! paired account. The Go build's prefix comes from its database unless `deterministic_id_prefix`
//! is set; this driver always derives it from the login id that way, so the ids it produces match
//! a Go database only for accounts that were configured that way (see the driver report).

use sha2::{Digest, Sha256};

use chirper_bridge::PortalRef;

/// The account prefix of every id: the first 8 bytes of SHA-256 of the login id, hex encoded.
pub fn account_prefix(login_id: &str) -> String {
    hex::encode(&Sha256::digest(login_id.as_bytes())[..8])
}

/// `<prefix>.<real id>`.
pub fn make(prefix: &str, real: &str) -> String {
    format!("{prefix}.{real}")
}

/// The real id of a `<prefix>.<real id>` string, when the prefix is this account's.
pub fn parse<'a>(prefix: &str, id: &'a str) -> Option<&'a str> {
    let real = id.strip_prefix(prefix)?.strip_prefix('.')?;
    (!real.is_empty()).then_some(real)
}

/// The portal of a conversation: the id plus the login part of the portal key.
pub fn portal(prefix: &str, login_id: &str, conversation_id: &str) -> PortalRef {
    PortalRef {
        id: make(prefix, conversation_id),
        receiver: login_id.to_owned(),
    }
}

/// The message id of a Google message.
pub fn message(prefix: &str, message_id: &str) -> String {
    make(prefix, message_id)
}

/// The user id of a participant.
pub fn user(prefix: &str, participant_id: &str) -> String {
    make(prefix, participant_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prefix_is_sixteen_hex_digits_of_the_login_hash() {
        let prefix = account_prefix("someone@example.com/42");
        assert_eq!(prefix.len(), 16);
        assert!(prefix.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(prefix, account_prefix("someone@example.com/42"));
        assert_ne!(prefix, account_prefix("other@example.com/42"));
    }

    #[test]
    fn portal_message_and_user_ids_keep_the_go_format() {
        let prefix = "0123456789abcdef";
        assert_eq!(portal(prefix, "login/1", "conv-7").id, "0123456789abcdef.conv-7");
        assert_eq!(portal(prefix, "login/1", "conv-7").receiver, "login/1");
        assert_eq!(message(prefix, "m-9"), "0123456789abcdef.m-9");
        assert_eq!(user(prefix, "p-3"), "0123456789abcdef.p-3");
    }

    #[test]
    fn parsing_requires_this_accounts_prefix() {
        let prefix = "0123456789abcdef";
        assert_eq!(parse(prefix, "0123456789abcdef.conv"), Some("conv"));
        assert_eq!(parse(prefix, "fedcba9876543210.conv"), None);
        assert_eq!(parse(prefix, "0123456789abcdef."), None);
        assert_eq!(parse(prefix, "0123456789abcdef"), None);
    }
}
