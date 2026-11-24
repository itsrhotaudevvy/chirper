//! Remote id formats, identical to the Go bridge's `pkg/connector/ids.go` (mautrix-bluesky,
//! AGPL-3.0). Databases written by the Go build are keyed by these strings, so changing one orphans
//! the user's history.
//!
//! * login id: the account's DID (`did:plc:abc`)
//! * user id: the DID's method and specific id joined by `-` (`plc-abc`)
//! * portal id: the conversation id, unchanged
//! * message id: `<conversation>:<message>`

use chirper_bridge::{Error, Result};

/// Split a DID into its method and specific id. Only the shapes Bluesky issues are accepted.
pub fn parse_did(did: &str) -> Option<(&str, &str)> {
    let rest = did.strip_prefix("did:")?;
    let (method, value) = rest.split_once(':')?;
    let method_ok = !method.is_empty()
        && method
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let value_ok = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '%'));
    (method_ok && value_ok).then_some((method, value))
}

/// The login id of an account: its DID.
pub fn login_id(did: &str) -> String {
    did.to_owned()
}

/// The user id of a DID, `<method>-<specific id>`.
pub fn user_id(did: &str) -> Result<String> {
    let (method, value) = parse_did(did)
        .ok_or_else(|| Error::Protocol("a Bluesky account id is not a DID".to_owned()))?;
    Ok(format!("{method}-{value}"))
}

/// The remote id of a message in a conversation.
pub fn message_id(convo: &str, message: &str) -> String {
    format!("{convo}:{message}")
}

/// The message part of a remote message id, once it is checked to belong to `convo`.
pub fn message_in<'a>(convo: &str, target: &'a str) -> Result<&'a str> {
    match target.split_once(':') {
        Some((owner, message)) if owner == convo && !message.is_empty() => Ok(message),
        _ => Err(Error::Protocol(
            "the message does not belong to this chat".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_ids_join_the_method_and_the_specific_id() {
        assert_eq!(user_id("did:plc:abc123").unwrap(), "plc-abc123");
        assert_eq!(user_id("did:web:example.com").unwrap(), "web-example.com");
        assert_eq!(
            user_id("did:web:example.com:users:ada").unwrap(),
            "web-example.com:users:ada"
        );
    }

    #[test]
    fn malformed_dids_are_protocol_errors() {
        for did in [
            "",
            "plc:abc",
            "did:PLC:abc",
            "did:plc:",
            "did:plc:a b",
            "did:::x",
        ] {
            assert!(
                matches!(user_id(did), Err(Error::Protocol(_))),
                "{did:?} must be rejected"
            );
        }
    }

    #[test]
    fn the_login_id_is_the_did_itself() {
        assert_eq!(login_id("did:plc:abc123"), "did:plc:abc123");
    }

    #[test]
    fn message_ids_are_conversation_colon_message() {
        assert_eq!(message_id("3kzconvo", "3kzmsg"), "3kzconvo:3kzmsg");
        assert_eq!(message_in("3kzconvo", "3kzconvo:3kzmsg").unwrap(), "3kzmsg");
        assert!(message_in("other", "3kzconvo:3kzmsg").is_err());
        assert!(message_in("3kzconvo", "3kzconvo:").is_err());
        assert!(message_in("3kzconvo", "3kzmsg").is_err());
    }
}
