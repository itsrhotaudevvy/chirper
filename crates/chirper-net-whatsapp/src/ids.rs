//! Remote id formats, identical to the Go bridge's `pkg/waid/id.go` and `connector/id.go` (AGPL-3.0,
//! mautrix-whatsapp). Databases written by the Go build are keyed by these strings: changing a
//! format orphans the user's history, so every format is pinned by a test below.
//!
//! * portal id: the chat JID without its device, `123@s.whatsapp.net`, `1-2@g.us`
//! * portal receiver: the login id for one-to-one chats, empty for groups
//! * user id: the user part, `bot-<user>` for bots, `lid-<user>` for LIDs
//! * user login id: the user part of a phone-number account
//! * message id: `<chat>:<sender>:<id>`, both JIDs without devices

use crate::binary::{Jid, Server};

/// The parts of a message id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedMessageId {
    pub chat: Jid,
    pub sender: Jid,
    pub id: String,
}

/// The portal id of a chat: its JID without a device.
pub fn portal_id(chat: &Jid) -> String {
    Jid {
        device: 0,
        agent: 0,
        ..chat.clone()
    }
    .to_string()
}

/// The portal receiver: one-to-one chats exist once per account, groups once for everyone.
pub fn portal_receiver(chat: &Jid, login_id: &str) -> String {
    match chat.server {
        Server::Pn | Server::Bot | Server::Lid | Server::Broadcast => login_id.to_owned(),
        _ => String::new(),
    }
}

/// The user id of a sender.
pub fn user_id(user: &Jid) -> String {
    match user.server {
        Server::Pn => user.user.clone(),
        Server::Bot => format!("bot-{}", user.user),
        Server::Lid => format!("lid-{}", user.user),
        _ => String::new(),
    }
}

/// The login id of an account: the user part of its phone-number JID.
pub fn user_login_id(user: &Jid) -> String {
    match user.server {
        Server::Pn => user.user.clone(),
        _ => String::new(),
    }
}

/// The remote message id of a message in `chat` sent by `sender`.
pub fn message_id(chat: &Jid, sender: &Jid, id: &str) -> String {
    format!("{}:{}:{id}", portal_id(chat), portal_id(sender))
}

/// Parses a message id made by [`message_id`]. Fake ids and anything malformed are `None`.
pub fn parse_message_id(text: &str) -> Option<ParsedMessageId> {
    let mut parts = text.splitn(3, ':');
    let chat = parts.next()?.parse::<Jid>().ok()?;
    let sender = parts.next()?.parse::<Jid>().ok()?;
    let id = parts.next()?;
    if id.is_empty() {
        return None;
    }
    Some(ParsedMessageId {
        chat,
        sender,
        id: id.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jid(text: &str) -> Jid {
        text.parse().expect("test JID")
    }

    #[test]
    fn portal_ids_strip_the_device() {
        assert_eq!(
            portal_id(&jid("123:7@s.whatsapp.net")),
            "123@s.whatsapp.net"
        );
        assert_eq!(portal_id(&jid("120363-1@g.us")), "120363-1@g.us");
        assert_eq!(portal_id(&jid("status@broadcast")), "status@broadcast");
    }

    #[test]
    fn receivers_are_the_login_for_one_to_one_chats_only() {
        assert_eq!(portal_receiver(&jid("1@s.whatsapp.net"), "me"), "me");
        assert_eq!(portal_receiver(&jid("1-2@g.us"), "me"), "");
        assert_eq!(portal_receiver(&jid("status@broadcast"), "me"), "me");
    }

    #[test]
    fn user_ids_match_the_bridge_prefixes() {
        assert_eq!(user_id(&jid("555:3@s.whatsapp.net")), "555");
        assert_eq!(user_id(&jid("123@lid")), "lid-123");
        assert_eq!(user_id(&jid("13135550002@bot")), "bot-13135550002");
        assert_eq!(user_id(&jid("1-2@g.us")), "");
    }

    #[test]
    fn login_ids_are_phone_numbers_only() {
        assert_eq!(user_login_id(&jid("555:3@s.whatsapp.net")), "555");
        assert_eq!(user_login_id(&jid("123@lid")), "");
    }

    #[test]
    fn message_ids_are_chat_sender_and_id() {
        let chat = jid("1-2@g.us");
        let sender = jid("555:9@s.whatsapp.net");
        let id = message_id(&chat, &sender, "3EB0ABC");
        assert_eq!(id, "1-2@g.us:555@s.whatsapp.net:3EB0ABC");
        let parsed = parse_message_id(&id).expect("round trip");
        assert_eq!(parsed.chat, chat);
        assert_eq!(parsed.sender, jid("555@s.whatsapp.net"));
        assert_eq!(parsed.id, "3EB0ABC");
    }

    #[test]
    fn malformed_message_ids_do_not_parse() {
        assert!(parse_message_id("nope").is_none());
        assert!(parse_message_id("1@s.whatsapp.net:2@s.whatsapp.net:").is_none());
        assert!(parse_message_id("fake:1@s.whatsapp.net:x").is_none());
    }
}
