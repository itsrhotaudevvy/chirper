//! Remote id formats. Each one is the string the Go bridge stored, because databases written by
//! that bridge are keyed by them. Reference: `connector/ids.go` (`ConvertConversationIDToREST`,
//! `ParsePortalID`, `MakePortalKeyForConversation`) and `twittermeow/methods/methods.go`.

/// Milliseconds between the Unix epoch and X's snowflake epoch.
const TWITTER_EPOCH_MS: i64 = 1_288_834_974_657;

/// The portal id of a conversation: the REST spelling, which is the conversation id with `:`
/// replaced by `-`. Group ids that start with `g` are kept as they are.
pub fn portal_id(conversation_id: &str) -> String {
    if conversation_id.starts_with('g') {
        conversation_id.to_owned()
    } else {
        conversation_id.replace(':', "-")
    }
}

/// The portal receiver. One-to-one conversations (their ids use `-` in the portal form) exist once
/// per account, so their receiver is the local user; group conversations are shared.
pub fn portal_receiver(portal_id: &str, me_id: &str) -> String {
    if portal_id.contains('-') {
        me_id.to_owned()
    } else {
        String::new()
    }
}

/// The message id: the message's own id as X spells it.
pub fn message_id(event_id: &str) -> String {
    event_id.to_owned()
}

/// The login id of an X account: the prefix plus the user id.
pub fn login_id(user_id: &str) -> String {
    format!("twitter_{user_id}")
}

/// The time of a snowflake id, in Unix milliseconds, or 0 when the id is not a snowflake.
pub fn snowflake_ms(id: &str) -> i64 {
    match id.parse::<u64>() {
        Ok(value) => i64::try_from(value >> 22).map_or(0, |ms| ms + TWITTER_EPOCH_MS),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_to_one_conversation_uses_dashes_in_its_portal_id() {
        assert_eq!(portal_id("111:222"), "111-222");
    }

    #[test]
    fn a_group_keeps_its_g_prefix_and_its_colon_free_form() {
        assert_eq!(portal_id("g1234567"), "g1234567");
    }

    #[test]
    fn only_one_to_one_portals_have_the_local_user_as_receiver() {
        assert_eq!(portal_receiver("111-222", "111"), "111");
        assert_eq!(portal_receiver("g1234567", "111"), "");
    }

    #[test]
    fn a_login_id_is_prefixed_with_the_user_id() {
        assert_eq!(login_id("1234"), "twitter_1234");
    }

    #[test]
    fn a_snowflake_gives_its_creation_time() {
        // 1 << 22 is one millisecond after the epoch.
        assert_eq!(
            snowflake_ms(&(1u64 << 22).to_string()),
            TWITTER_EPOCH_MS + 1
        );
        assert_eq!(snowflake_ms("not a number"), 0);
        assert_eq!(snowflake_ms(""), 0);
    }
}
