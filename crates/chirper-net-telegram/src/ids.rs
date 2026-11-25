//! Remote id formats, identical to the Go bridge's `pkg/connector/ids/ids.go`.
//!
//! Databases written by the Go build are keyed by these strings, so every format here is pinned
//! by a test. Portals: `user:<id>`, `chat:<id>`, `channel:<id>`, forum topics
//! `channel:<id>:<topic>`. Messages: `<msg_id>`, or `<channel_id>.<msg_id>` in channels. Users:
//! `<id>`, or `channel-<id>` for a channel posting as itself. The login id is the user id.
//! `PortalRef.receiver` is the login id for user and chat portals and empty for channels.

use chirper_bridge::{Error, PortalRef};

/// The kind of a Telegram peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerKind {
    User,
    Chat,
    Channel,
}

impl PeerKind {
    /// The `PeerType` string of the Go bridge.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Chat => "chat",
            Self::Channel => "channel",
        }
    }
}

/// A chat, parsed back from its portal id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Portal {
    pub kind: PeerKind,
    pub id: i64,
    /// Forum topic; `0` for the chat itself.
    pub topic: i32,
}

/// The portal of a peer, as the bridge model sees it.
pub fn portal(kind: PeerKind, id: i64, topic: i32, login_id: &str) -> PortalRef {
    let receiver = match kind {
        PeerKind::User | PeerKind::Chat => login_id.to_owned(),
        PeerKind::Channel => String::new(),
    };
    let id = if kind == PeerKind::Channel && topic != 0 {
        format!("channel:{id}:{topic}")
    } else {
        format!("{}:{id}", kind.as_str())
    };
    PortalRef { id, receiver }
}

/// Parse a portal id produced by [`portal`].
pub fn parse_portal(id: &str) -> Result<Portal, Error> {
    let mut parts = id.split(':');
    let kind = match parts.next() {
        Some("user") => PeerKind::User,
        Some("chat") => PeerKind::Chat,
        Some("channel") => PeerKind::Channel,
        _ => return Err(Error::Protocol("unknown portal kind".into())),
    };
    let number = parts
        .next()
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| Error::Protocol("portal id has no number".into()))?;
    let topic = match parts.next() {
        Some(value) if kind == PeerKind::Channel => value
            .parse::<i32>()
            .map_err(|_| Error::Protocol("portal topic is not a number".into()))?,
        Some(_) => return Err(Error::Protocol("portal id has too many parts".into())),
        None => 0,
    };
    if parts.next().is_some() {
        return Err(Error::Protocol("portal id has too many parts".into()));
    }
    Ok(Portal {
        kind,
        id: number,
        topic,
    })
}

/// A message id: `<msg_id>` in a user or basic group, `<channel_id>.<msg_id>` in a channel.
pub fn message_id(channel: Option<i64>, message: i32) -> String {
    match channel {
        Some(channel) => format!("{channel}.{message}"),
        None => message.to_string(),
    }
}

/// Parse a message id produced by [`message_id`]: the channel (if any) and the message id.
pub fn parse_message_id(id: &str) -> Result<(Option<i64>, i32), Error> {
    if id.starts_with("migrated:") || id.starts_with("call:") {
        return Err(Error::Unsupported("this kind of message"));
    }
    match id.split_once('.') {
        None => {
            let message = id
                .parse::<i32>()
                .map_err(|_| Error::Protocol("message id is not a number".into()))?;
            Ok((None, message))
        }
        Some((channel, message)) => {
            let channel = channel
                .parse::<i64>()
                .map_err(|_| Error::Protocol("channel id is not a number".into()))?;
            let message = message
                .parse::<i32>()
                .map_err(|_| Error::Protocol("message id is not a number".into()))?;
            Ok((Some(channel), message))
        }
    }
}

/// The remote user id of a person.
pub fn user_id(id: i64) -> String {
    id.to_string()
}

/// The remote user id of a channel posting as itself.
pub fn channel_user_id(id: i64) -> String {
    format!("channel-{id}")
}

/// The login id of an account: its user id.
pub fn login_id(user: i64) -> String {
    user.to_string()
}

/// The reaction key of a custom emoji (`custom:<document id>`), or the emoticon itself.
pub fn custom_emoji_key(document_id: i64) -> String {
    format!("custom:{document_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_ids_match_the_bridge() {
        assert_eq!(portal(PeerKind::User, 42, 0, "7").id, "user:42");
        assert_eq!(portal(PeerKind::User, 42, 0, "7").receiver, "7");
        assert_eq!(portal(PeerKind::Chat, 5, 0, "7").id, "chat:5");
        assert_eq!(portal(PeerKind::Chat, 5, 0, "7").receiver, "7");
        let channel = portal(PeerKind::Channel, 9, 0, "7");
        assert_eq!(channel.id, "channel:9");
        assert_eq!(channel.receiver, "");
        assert_eq!(portal(PeerKind::Channel, 9, 33, "7").id, "channel:9:33");
    }

    #[test]
    fn portal_ids_parse_back() {
        assert_eq!(
            parse_portal("channel:9:33").unwrap(),
            Portal {
                kind: PeerKind::Channel,
                id: 9,
                topic: 33
            }
        );
        assert_eq!(parse_portal("user:42").unwrap().id, 42);
        assert!(parse_portal("user:x").is_err());
        assert!(parse_portal("user:1:2").is_err());
        assert!(parse_portal("bogus:1").is_err());
    }

    #[test]
    fn message_ids_match_the_bridge() {
        assert_eq!(message_id(None, 17), "17");
        assert_eq!(message_id(Some(9), 17), "9.17");
        assert_eq!(parse_message_id("17").unwrap(), (None, 17));
        assert_eq!(parse_message_id("9.17").unwrap(), (Some(9), 17));
        assert!(parse_message_id("x.1").is_err());
        assert!(parse_message_id("call:1").is_err());
    }

    #[test]
    fn user_and_login_ids_match_the_bridge() {
        assert_eq!(user_id(-3), "-3");
        assert_eq!(channel_user_id(12), "channel-12");
        assert_eq!(login_id(99), "99");
        assert_eq!(custom_emoji_key(5), "custom:5");
    }
}
