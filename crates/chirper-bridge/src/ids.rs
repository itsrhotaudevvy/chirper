//! Deterministic local identifiers.
//!
//! A remote chat, message or person needs a stable local id so that a reconnect, a replay or a
//! backfill page cannot create a duplicate row. The derivation is the one the Go core used, so
//! databases written by it keep resolving; the `chirper.local` suffix is part of that on-disk
//! format only. Nothing here knows about Matrix.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Server name used in locally generated ids. Kept for on-disk compatibility.
pub const SERVER_NAME: &str = "chirper.local";

/// The local user that owns every Chirper account.
pub const SELF_USER_ID: &str = "@me:chirper.local";

/// The first 16 bytes of the SHA-256 of the parts, hex-encoded. Parts are separated by NUL bytes so
/// that `("ab", "c")` and `("a", "bc")` never collide.
pub fn hash_hex(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            hasher.update([0u8]);
        }
        hasher.update(part.as_bytes());
    }
    hex::encode(&hasher.finalize()[..16])
}

/// `n` random bytes, hex-encoded.
pub fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).expect("the OS random source does not fail on supported platforms");
    hex::encode(buf)
}

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = std::convert::Infallible;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(s.to_owned()))
            }
        }
    };
}

id_type!(
    /// A chat (formerly "room") on this device.
    ChatId
);
id_type!(
    /// A timeline event (message, edit marker, reaction, redaction) on this device.
    EventId
);
id_type!(
    /// A remote person or bot, projected into a local sender id.
    SenderId
);

/// The local chat id of a portal. `receiver` is the login part of a portal key and is usually empty.
pub fn chat_id(network: &str, portal_id: &str, receiver: &str) -> ChatId {
    ChatId(format!(
        "!{network}.{}:{SERVER_NAME}",
        hash_hex(&[portal_id, receiver])
    ))
}

/// A random chat id for a portal the network did not name.
pub fn random_chat_id(network: &str) -> ChatId {
    ChatId(format!("!{network}.{}:{SERVER_NAME}", random_hex(16)))
}

/// The local event id of a remote message. `part` is empty for a whole message.
pub fn event_id_for(network: &str, chat: &str, message_id: &str, part: &str) -> EventId {
    EventId(format!(
        "${network}.{}",
        hash_hex(&[chat, message_id, part])
    ))
}

/// The local event id of a reaction by `sender` with `key` on `target`.
pub fn reaction_event_id(
    network: &str,
    chat: &str,
    target: &str,
    sender: &str,
    key: &str,
) -> EventId {
    EventId(format!(
        "${network}.{}",
        hash_hex(&[chat, target, sender, key])
    ))
}

/// An event id for a change that is not a message of its own: an edit, a redaction, a local send
/// the network has not confirmed yet.
pub fn local_event_id() -> EventId {
    EventId(format!("$local.{}", random_hex(16)))
}

/// A local sender id for a remote user. Matches the escaping of the Go core so that member rows
/// written by it keep resolving to the same person.
pub fn sender_id(network: &str, external_id: &str) -> SenderId {
    SenderId(format!(
        "@{network}_{}:{SERVER_NAME}",
        encode_localpart(external_id)
    ))
}

/// The escaping used in sender ids: the characters a Matrix localpart may not hold become `=XX`.
fn encode_localpart(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '@' => out.push_str("=40"),
            ':' => out.push_str("=3A"),
            '/' => out.push_str("=2F"),
            ' ' => out.push_str("=20"),
            '\x00' => out.push_str("=00"),
            '\n' => out.push_str("=0A"),
            '\r' => out.push_str("=0D"),
            '\t' => out.push_str("=09"),
            other => out.push(other),
        }
    }
    out
}

/// The external id encoded in a sender id of `network`, or `None` for another network's sender.
pub fn external_sender_id<'a>(network: &str, sender: &'a str) -> Option<&'a str> {
    let prefix = format!("@{network}_");
    let value = sender.strip_prefix(prefix.as_str())?;
    let local = value.strip_suffix(&format!(":{SERVER_NAME}"))?;
    Some(local)
}

/// `mxc://chirper.local/<id>` of a locally stored media file.
pub fn media_uri(media_id: &str) -> String {
    format!("mxc://{SERVER_NAME}/{media_id}")
}

/// The media id of a local `mxc://` URI, or `None` for another server or a malformed URI.
pub fn media_id_of(uri: &str) -> Option<&str> {
    let rest = uri.strip_prefix("mxc://")?;
    let (server, id) = rest.split_once('/')?;
    (server == SERVER_NAME && !id.is_empty() && !id.contains('/')).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_the_go_core_vectors() {
        assert_eq!(hash_hex(&["discord_123", ""]), "fbacdbf3e6fedc9259e8e085049c8eca");
        assert_eq!(
            event_id_for("discord", "!discord.abc:chirper.local", "m-1", "").as_str(),
            "$discord.b8fe0d74568f60c943054ee95fd1cb03"
        );
        assert_eq!(
            reaction_event_id(
                "discord",
                "!discord.abc:chirper.local",
                "$e1",
                "discord_9",
                "👍"
            )
            .as_str(),
            "$discord.d8aaf882bd73c3d9c12c3b140c010d7c"
        );
    }

    #[test]
    fn parts_never_collide_across_split_points() {
        assert_ne!(hash_hex(&["ab", "c"]), hash_hex(&["a", "bc"]));
        assert_eq!(hash_hex(&["ab", "c"]), hash_hex(&["ab", "c"]));
    }

    #[test]
    fn chat_ids_are_stable_and_well_formed() {
        let id = chat_id("signal", "group:abc", "");
        assert!(id.as_str().starts_with("!signal."));
        assert!(id.as_str().ends_with(":chirper.local"));
        assert_eq!(id, chat_id("signal", "group:abc", ""));
        assert_ne!(id, chat_id("signal", "group:abc", "other"));
    }

    #[test]
    fn sender_ids_round_trip_the_external_id() {
        let sender = sender_id("whatsapp", "15551234@s.whatsapp.net");
        assert_eq!(sender.as_str(), "@whatsapp_15551234=40s.whatsapp.net:chirper.local");
        assert_eq!(
            external_sender_id("whatsapp", sender.as_str()),
            Some("15551234=40s.whatsapp.net")
        );
        assert_eq!(external_sender_id("signal", sender.as_str()), None);
    }

    #[test]
    fn media_uris_are_local_only() {
        assert_eq!(media_uri("abc"), "mxc://chirper.local/abc");
        assert_eq!(media_id_of("mxc://chirper.local/abc"), Some("abc"));
        assert_eq!(media_id_of("mxc://matrix.org/abc"), None);
        assert_eq!(media_id_of("mxc://chirper.local/"), None);
        assert_eq!(media_id_of("https://example.com/abc"), None);
    }

    #[test]
    fn local_event_ids_are_unique() {
        let a = local_event_id();
        let b = local_event_id();
        assert!(a.as_str().starts_with("$local."));
        assert_ne!(a, b);
    }
}