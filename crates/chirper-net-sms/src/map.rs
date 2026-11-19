//! Identity and mapping: how provider rows become remote ids and domain types.
//!
//! The portal id is the set of participants, so it survives reinstalls and does not depend on the
//! provider's thread ids. Thread ids are only used in-session to reach the provider.

use chirper_bridge::model::{ChatMeta, DeliveryStatus, PortalRef, RemoteUser};

use crate::platform::{SmsAddress, SmsMessage, SmsThread};

/// The user id of the local phone's own messages.
pub const SELF_ID: &str = "self";

/// The address with spaces, dashes, dots and parentheses removed. Used when no E.164 form exists.
pub fn normalize_address(raw: &str) -> String {
    raw.chars()
        .filter(|c| !matches!(c, ' ' | '-' | '.' | '(' | ')'))
        .collect()
}

/// The identity of one participant: the E.164 form when known, else the normalized raw address.
pub fn address_key(address: &SmsAddress) -> String {
    let e164 = address.e164.trim();
    if e164.is_empty() {
        normalize_address(&address.raw)
    } else {
        e164.to_owned()
    }
}

/// The distinct participants of a conversation, sorted by identity, without empty identities.
pub fn participants(addresses: &[SmsAddress]) -> Vec<RemoteUser> {
    let mut seen: Vec<(String, &SmsAddress)> = addresses
        .iter()
        .map(|address| (address_key(address), address))
        .filter(|(key, _)| !key.is_empty())
        .collect();
    seen.sort_by(|a, b| a.0.cmp(&b.0));
    seen.dedup_by(|a, b| a.0 == b.0);
    seen.into_iter()
        .map(|(key, address)| {
            let display_name = if address.name.is_empty() {
                key.clone()
            } else {
                address.name.clone()
            };
            RemoteUser {
                id: key.clone(),
                display_name,
                phone: key,
                ..Default::default()
            }
        })
        .collect()
}

/// The portal id of a conversation: its sorted, distinct participant identities joined by `,`.
pub fn portal_id(addresses: &[SmsAddress]) -> String {
    participants(addresses)
        .iter()
        .map(|user| user.id.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// The participant identities of a portal id, as the platform wants them to send to.
pub fn portal_addresses(portal_id: &str) -> Vec<String> {
    portal_id
        .split(',')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A portal reference for a portal id. SMS has no receiver part.
pub fn portal_ref(id: String) -> PortalRef {
    PortalRef {
        id,
        receiver: String::new(),
    }
}

/// The chat metadata of a conversation.
pub fn chat_meta(portal: PortalRef, thread: &SmsThread) -> ChatMeta {
    let users = participants(&thread.addresses);
    let name = users
        .iter()
        .map(|user| user.display_name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let is_dm = users.len() == 1;
    let dm_user = if is_dm { users.first().cloned() } else { None };
    ChatMeta {
        portal,
        name,
        is_dm,
        dm_user,
        ..Default::default()
    }
}

/// The user of a participant, as it appears on a message.
pub fn remote_user(address: &SmsAddress) -> RemoteUser {
    let key = address_key(address);
    let display_name = if address.name.is_empty() {
        key.clone()
    } else {
        address.name.clone()
    };
    RemoteUser {
        id: key.clone(),
        display_name,
        phone: key,
        ..Default::default()
    }
}

/// The user a message is from: the local phone for its own messages.
pub fn sender_of(message: &SmsMessage) -> RemoteUser {
    if message.from_me {
        RemoteUser {
            id: SELF_ID.to_owned(),
            display_name: String::new(),
            ..Default::default()
        }
    } else {
        remote_user(&message.sender)
    }
}

/// The media kind of a MIME type, in the vocabulary of `Content::Media`.
pub fn media_kind(mime: &str) -> &'static str {
    let mime = mime.to_ascii_lowercase();
    if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("video/") {
        "video"
    } else if mime.starts_with("audio/") {
        "audio"
    } else {
        "file"
    }
}

/// The delivery verdict of a platform send status. `None` for anything unknown.
pub fn delivery_status(status: &str) -> Option<DeliveryStatus> {
    match status {
        "sent" => Some(DeliveryStatus::Sent),
        "delivered" => Some(DeliveryStatus::Delivered),
        "failed" => Some(DeliveryStatus::Failed),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(raw: &str, e164: &str) -> SmsAddress {
        SmsAddress {
            raw: raw.to_owned(),
            e164: e164.to_owned(),
            name: String::new(),
        }
    }

    #[test]
    fn formatted_and_e164_addresses_share_a_portal_id() {
        let formatted = portal_id(&[address("(555) 123-4567", "+15551234567")]);
        let plain = portal_id(&[address("+15551234567", "+15551234567")]);
        assert_eq!(formatted, "+15551234567");
        assert_eq!(formatted, plain);
    }

    #[test]
    fn raw_only_addresses_are_normalized() {
        assert_eq!(
            portal_id(&[address("(555) 123-4567", "")]),
            "5551234567".to_owned()
        );
        assert_eq!(normalize_address("+1.555-123 (4567)"), "+15551234567");
    }

    #[test]
    fn group_portal_ids_are_sorted_and_distinct() {
        let a = address("+15550000001", "+15550000001");
        let b = address("+15550000002", "+15550000002");
        let first = portal_id(&[b.clone(), a.clone()]);
        let second = portal_id(&[a.clone(), b.clone(), a.clone()]);
        assert_eq!(first, "+15550000001,+15550000002");
        assert_eq!(first, second);
    }

    #[test]
    fn empty_identities_are_dropped() {
        assert_eq!(portal_id(&[address("", ""), address("  ", "")]), "");
        assert!(portal_addresses("").is_empty());
    }

    #[test]
    fn chat_meta_names_dms_and_groups() {
        let mut named = address("+15550000001", "+15550000001");
        named.name = "Ada".to_owned();
        let dm = SmsThread {
            thread_id: 1,
            addresses: vec![named.clone()],
            snippet_date_ms: 0,
        };
        let meta = chat_meta(portal_ref(portal_id(&dm.addresses)), &dm);
        assert!(meta.is_dm);
        assert_eq!(meta.name, "Ada");
        assert_eq!(meta.dm_user.map(|u| u.display_name), Some("Ada".to_owned()));

        let group = SmsThread {
            thread_id: 2,
            addresses: vec![named, address("+15550000002", "+15550000002")],
            snippet_date_ms: 0,
        };
        let meta = chat_meta(portal_ref(portal_id(&group.addresses)), &group);
        assert!(!meta.is_dm);
        assert!(meta.dm_user.is_none());
        assert_eq!(meta.name, "Ada, +15550000002");
    }

    #[test]
    fn media_kinds_follow_mime_types() {
        assert_eq!(media_kind("Image/JPEG"), "image");
        assert_eq!(media_kind("video/mp4"), "video");
        assert_eq!(media_kind("audio/amr"), "audio");
        assert_eq!(media_kind("text/vcard"), "file");
    }

    #[test]
    fn send_statuses_map_to_delivery() {
        assert_eq!(delivery_status("sent"), Some(DeliveryStatus::Sent));
        assert_eq!(
            delivery_status("delivered"),
            Some(DeliveryStatus::Delivered)
        );
        assert_eq!(delivery_status("failed"), Some(DeliveryStatus::Failed));
        assert_eq!(delivery_status("pending"), None);
    }
}
