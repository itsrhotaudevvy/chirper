//! Remote ids, in the exact formats of the Go bridge, so stored history keeps resolving.
//!
//! Ported from `pkg/signalid/ids.go` (AGPL-3.0): a portal is a user's ACI (`<uuid>`, or
//! `PNI:<uuid>` for a phone-number identity) or a group id (44 characters); a message is
//! `<sender ACI>|<timestamp ms>`; a part after the first is its index as text; a user is the
//! ACI, or `pni_<uuid>` for a phone-number identity; a user login is the ACI.

use chirper_bridge::{Error, Result};

/// The prefix of a phone-number identity in a user id.
pub const PNI_USER_PREFIX: &str = "pni_";
/// The prefix of a phone-number identity in a service id.
pub const PNI_SERVICE_PREFIX: &str = "PNI:";

/// A 16-byte UUID as a lowercase hyphenated string.
pub fn uuid_from_bytes(bytes: &[u8]) -> Result<String> {
    if bytes.len() != 16 {
        return Err(Error::Protocol("a UUID is 16 bytes".to_owned()));
    }
    let hex = hex::encode(bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

/// The 16 bytes of a hyphenated UUID string, or `None` if it is not one.
pub fn uuid_to_bytes(text: &str) -> Option<[u8; 16]> {
    let compact: String = text.chars().filter(|c| *c != '-').collect();
    if compact.len() != 32 || text.len() != 36 {
        return None;
    }
    let decoded = hex::decode(compact).ok()?;
    decoded.try_into().ok()
}

/// The portal id of a direct chat with `aci`.
pub fn dm_portal_id(aci: &str) -> String {
    aci.to_owned()
}

/// The portal id of a phone-number identity chat.
pub fn pni_portal_id(pni: &str) -> String {
    format!("{PNI_SERVICE_PREFIX}{pni}")
}

/// The message id: `<sender ACI>|<timestamp ms>`.
pub fn message_id(sender_aci: &str, timestamp_ms: u64) -> String {
    format!("{sender_aci}|{timestamp_ms}")
}

/// The part id of part `index`: empty for the first part, the index as text afterwards.
pub fn part_id(index: usize) -> String {
    if index == 0 {
        String::new()
    } else {
        index.to_string()
    }
}

/// The user id of an ACI.
pub fn user_id(aci: &str) -> String {
    aci.to_owned()
}

/// The user id of a phone-number identity.
pub fn pni_user_id(pni: &str) -> String {
    format!("{PNI_USER_PREFIX}{pni}")
}

/// The user id of a service id as the reference formats it (`PNI:<uuid>` becomes `pni_<uuid>`).
pub fn user_id_from_service_id(service_id: &str) -> String {
    match service_id.strip_prefix(PNI_SERVICE_PREFIX) {
        Some(pni) => pni_user_id(pni),
        None => user_id(service_id),
    }
}

/// The user login id of a linked account: its ACI.
pub fn login_id(aci: &str) -> String {
    aci.to_owned()
}

/// Splits a message id into its sender ACI and timestamp.
pub fn parse_message_id(id: &str) -> Result<(String, u64)> {
    let (sender, timestamp) = id
        .split_once('|')
        .ok_or_else(|| Error::Protocol("a message id has no separator".to_owned()))?;
    if uuid_to_bytes(sender).is_none() {
        return Err(Error::Protocol("a message id has no sender".to_owned()));
    }
    let timestamp = timestamp
        .parse::<u64>()
        .map_err(|_| Error::Protocol("a message id has no timestamp".to_owned()))?;
    Ok((sender.to_owned(), timestamp))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACI: &str = "a1b2c3d4-e5f6-4789-8abc-def012345678";

    #[test]
    fn uuids_round_trip_in_the_reference_format() {
        let bytes = uuid_to_bytes(ACI).expect("valid uuid");
        assert_eq!(uuid_from_bytes(&bytes).expect("16 bytes"), ACI);
        assert!(uuid_from_bytes(&bytes[..15]).is_err());
        assert!(uuid_to_bytes("not-a-uuid").is_none());
    }

    #[test]
    fn portal_and_user_ids_match_the_go_bridge() {
        assert_eq!(dm_portal_id(ACI), ACI);
        assert_eq!(pni_portal_id(ACI), format!("PNI:{ACI}"));
        assert_eq!(user_id_from_service_id(&format!("PNI:{ACI}")), format!("pni_{ACI}"));
        assert_eq!(user_id_from_service_id(ACI), ACI);
        assert_eq!(login_id(ACI), ACI);
    }

    #[test]
    fn message_and_part_ids_match_the_go_bridge() {
        assert_eq!(message_id(ACI, 1_700_000_000_123), format!("{ACI}|1700000000123"));
        assert_eq!(part_id(0), "");
        assert_eq!(part_id(2), "2");
        let (sender, ts) = parse_message_id(&message_id(ACI, 42)).expect("parses");
        assert_eq!((sender.as_str(), ts), (ACI, 42));
        assert!(parse_message_id("nope").is_err());
        assert!(parse_message_id(&format!("{ACI}|x")).is_err());
    }
}
