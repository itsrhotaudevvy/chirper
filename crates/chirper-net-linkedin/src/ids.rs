//! Remote id formats. Each one is the string the Go bridge stored, because databases written by
//! that bridge are keyed by them. Reference: `connector/ids.go` and `connector/chatsync.go`.

/// The portal (chat) id: the conversation's entity URN, exactly as LinkedIn spells it.
pub fn portal_id(conversation_urn: &str) -> String {
    conversation_urn.to_owned()
}

/// The portal receiver. One-to-one chats exist per account, so their receiver is the local user;
/// group chats are shared and have none. This is the bridge's default (`SplitPortals` off).
pub fn portal_receiver(is_group: bool, me_id: &str) -> String {
    if is_group {
        String::new()
    } else {
        me_id.to_owned()
    }
}

/// The message id: the message's entity URN, `urn:li:msg_message:(urn:li:fsd_profile:S,M)`.
pub fn message_id(entity_urn: &str) -> String {
    entity_urn.to_owned()
}

/// The user id: the profile id of a member, which is the URN's id part.
pub fn user_id(participant_urn: &str) -> String {
    crate::api::urn::Urn::parse(participant_urn).id().to_owned()
}

/// The profile URN of a member id.
pub fn profile_urn(user_id: &str) -> String {
    format!("urn:li:fsd_profile:{user_id}")
}

/// The participant URN of a member id, as a reply quotes it.
pub fn participant_urn(user_id: &str) -> String {
    format!("urn:li:msg_messagingParticipant:urn:li:fsd_profile:{user_id}")
}

/// The login id of a LinkedIn account: the prefix plus the member id.
pub fn login_id(user_id: &str) -> String {
    format!("linkedin_{user_id}")
}

/// The URL of a feed update, from the update URN a shared post carries.
/// `urn:li:fsd_update:(urn:li:activity:ID,MESSAGING_RESHARE,EMPTY,DEFAULT,false)` gives `ID`.
pub fn feed_update_url(host_urn: &str) -> Option<String> {
    let inner = host_urn.split_once('(')?.1;
    let activity = inner.split(',').next()?;
    if activity.is_empty() {
        return None;
    }
    Some(format!("https://www.linkedin.com/feed/update/{activity}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conversation_is_its_entity_urn() {
        let urn = "urn:li:msg_conversation:(urn:li:fsd_profile:ME,THREAD1)";
        assert_eq!(portal_id(urn), urn);
    }

    #[test]
    fn one_to_one_chats_belong_to_the_account_and_groups_to_nobody() {
        assert_eq!(portal_receiver(false, "ME"), "ME");
        assert_eq!(portal_receiver(true, "ME"), "");
    }

    #[test]
    fn a_message_id_is_its_entity_urn() {
        let urn = "urn:li:msg_message:(urn:li:fsd_profile:ACoAA1,2-abcd==)";
        assert_eq!(message_id(urn), urn);
    }

    #[test]
    fn a_user_id_is_the_profile_id_of_a_participant() {
        let urn = "urn:li:msg_messagingParticipant:urn:li:fsd_profile:ACoAA1";
        assert_eq!(user_id(urn), "ACoAA1");
        assert_eq!(profile_urn("ACoAA1"), "urn:li:fsd_profile:ACoAA1");
        assert_eq!(participant_urn("ACoAA1"), urn);
    }

    #[test]
    fn login_ids_are_prefixed() {
        assert_eq!(login_id("ACoAA1"), "linkedin_ACoAA1");
    }

    #[test]
    fn feed_updates_link_to_their_activity() {
        assert_eq!(
            feed_update_url(
                "urn:li:fsd_update:(urn:li:activity:7000,MESSAGING_RESHARE,EMPTY,DEFAULT,false)"
            )
            .as_deref(),
            Some("https://www.linkedin.com/feed/update/urn:li:activity:7000")
        );
        assert_eq!(feed_update_url("urn:li:fsd_update"), None);
    }
}
