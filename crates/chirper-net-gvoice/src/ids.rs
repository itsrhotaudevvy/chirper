//! Remote id formats, the same strings the Go connector writes (`pkg/connector/chatinfo.go`,
//! `handlegvoice.go` and `login.go` of mautrix-gvoice, AGPL-3.0). Databases keyed by them keep
//! resolving only while these stay byte-identical.

use chirper_bridge::ids::hash_hex;

/// The network id the login ids are derived from.
const NETWORK: &str = "gvoice";

/// The portal (chat) id of a Google Voice thread.
///
/// A one-to-one call thread that is not a text thread is bridged into the `t.<number>` portal of
/// its peer, as the Go connector does (`getCallThreadPeer`); every other thread keeps its own id.
pub fn portal_id(thread_id: &str, phones: &[String], is_text: bool) -> String {
    if !is_text
        && !thread_id.starts_with("t.")
        && let [peer] = phones
        && peer.starts_with('+')
    {
        return format!("t.{peer}");
    }
    thread_id.to_owned()
}

/// The part id of attachment `index` of a multi-attachment message. The first part is the whole
/// message and has no part id.
pub fn part_id(index: usize) -> String {
    if index == 0 {
        String::new()
    } else {
        index.to_string()
    }
}

/// The remote user id of a phone number: the login's prefix, a dot, then the number in E.164.
///
/// The Go connector takes the prefix from a per-login database sequence; this port uses the login
/// id, which is unique per login in the same way.
pub fn user_id(login_id: &str, e164: &str) -> String {
    format!("{login_id}.{e164}")
}

/// The login id of a Google account, from its voice number. Only a hash of the number is stored in
/// the id, so the id never shows the number.
pub fn login_id(e164: &str) -> String {
    format!("{NETWORK}_{}", hash_hex(&[NETWORK, e164]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phones(list: &[&str]) -> Vec<String> {
        list.iter().map(|phone| (*phone).to_owned()).collect()
    }

    #[test]
    fn a_group_or_text_thread_keeps_its_id() {
        assert_eq!(
            portal_id("1234567", &phones(&["+15550001", "+15550002"]), false),
            "1234567"
        );
        assert_eq!(
            portal_id("1234567", &phones(&["+15550001"]), true),
            "1234567"
        );
    }

    #[test]
    fn a_call_thread_joins_the_peer_portal() {
        assert_eq!(
            portal_id("1234567", &phones(&["+15550001"]), false),
            "t.+15550001"
        );
        // Already a call thread, or a number without a plus sign: the id stays.
        assert_eq!(
            portal_id("t.+15550001", &phones(&["+15550001"]), false),
            "t.+15550001"
        );
        assert_eq!(
            portal_id("1234567", &phones(&["15550001"]), false),
            "1234567"
        );
        assert_eq!(portal_id("1234567", &[], false), "1234567");
    }

    #[test]
    fn part_ids_follow_the_attachment_index() {
        assert_eq!(part_id(0), "");
        assert_eq!(part_id(1), "1");
        assert_eq!(part_id(12), "12");
    }

    #[test]
    fn user_ids_are_login_prefix_and_number() {
        assert_eq!(user_id("gvoice_ab", "+15550001"), "gvoice_ab.+15550001");
    }

    #[test]
    fn login_ids_hide_the_number_and_are_stable() {
        let id = login_id("+15550001");
        assert!(id.starts_with("gvoice_"));
        assert!(!id.contains("15550001"));
        assert_eq!(id, login_id("+15550001"));
        assert_ne!(id, login_id("+15550002"));
        assert_eq!(id.len(), "gvoice_".len() + 32);
    }
}
