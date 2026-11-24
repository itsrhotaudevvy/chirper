//! Remote ids of the Messenger driver, in the exact formats the Go bridge used.
//!
//! From `pkg/metaid/ids.go` (AGPL-3.0, mautrix-meta): a portal is its thread key in decimal, a user is
//! its id in decimal, and a message is `fb:<message id>`. Messenger has no message parts.

use chirper_bridge::Error;
use chirper_bridge::model::PortalRef;

/// The prefix of every Messenger message id.
pub const MESSAGE_PREFIX: &str = "fb";

/// The portal of a thread key.
pub fn portal(thread_key: i64) -> PortalRef {
    PortalRef {
        id: thread_key.to_string(),
        receiver: String::new(),
    }
}

/// The remote id of a user.
pub fn user(id: i64) -> String {
    id.to_string()
}

/// The remote id of a message, from the id the server uses (`mid.$...`).
pub fn message(message_id: &str) -> String {
    format!("{MESSAGE_PREFIX}:{message_id}")
}

/// The server's message id of a remote message id; ids without the prefix are returned as they are.
pub fn server_message(remote: &str) -> &str {
    remote
        .strip_prefix(MESSAGE_PREFIX)
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or(remote)
}

/// The thread key of a portal id.
pub fn thread_key(portal: &str) -> Result<i64, Error> {
    portal
        .parse::<i64>()
        .map_err(|_| Error::Protocol(format!("{portal:?} is not a Messenger chat id")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portals_are_thread_keys_in_decimal() {
        assert_eq!(portal(146).id, "146");
        assert_eq!(portal(146).receiver, "");
        assert_eq!(thread_key("146").expect("parses"), 146);
        assert!(matches!(thread_key("abc"), Err(Error::Protocol(_))));
    }

    #[test]
    fn users_are_decimal_ids() {
        assert_eq!(user(100_000_123), "100000123");
    }

    #[test]
    fn messages_are_prefixed_and_unprefixed_again() {
        assert_eq!(message("mid.$cAAA"), "fb:mid.$cAAA");
        assert_eq!(server_message("fb:mid.$cAAA"), "mid.$cAAA");
        assert_eq!(server_message("mid.$cAAA"), "mid.$cAAA");
        assert_eq!(server_message("fb:"), "");
    }
}
