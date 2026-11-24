//! The XRPC response shapes the driver reads. Optional arrays default to empty and fields the bridge
//! does not use are left out, so a newer server does not break parsing.

use std::fmt;

use serde::Deserialize;
use serde_json::Value;

/// The answer of `createSession` and `refreshSession`. Holds bearer tokens, so `Debug` redacts it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSession {
    pub did: String,
    pub handle: String,
    pub access_jwt: String,
    pub refresh_jwt: String,
    #[serde(default)]
    pub did_doc: Option<Value>,
}

impl fmt::Debug for AuthSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// A person as the network describes them in a profile or a member list.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub did: String,
    #[serde(default)]
    pub handle: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
}

/// The author of a message or a reaction.
#[derive(Clone, Debug, Deserialize)]
pub struct Sender {
    pub did: String,
}

/// One reaction on a message.
#[derive(Clone, Debug, Deserialize)]
pub struct ReactionView {
    pub value: String,
    pub sender: Sender,
}

/// A message that still exists.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    pub id: String,
    #[serde(default)]
    pub text: String,
    /// Rich text facets, kept as JSON: each is parsed leniently by the conversion.
    #[serde(default)]
    pub facets: Vec<Value>,
    #[serde(default)]
    pub reactions: Vec<ReactionView>,
    pub sender: Sender,
    pub sent_at: String,
}

/// A message that was deleted.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedMessageView {
    pub id: String,
    pub sender: Sender,
    pub sent_at: String,
}

/// One conversation of `listConvos`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvoView {
    pub id: String,
    #[serde(default)]
    pub members: Vec<Profile>,
}

/// A page of conversations, newest first.
#[derive(Debug, Deserialize)]
pub struct ConvoPage {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub convos: Vec<ConvoView>,
}

/// A page of the event log. Entries stay JSON: each one is dispatched on its `$type`.
#[derive(Debug, Deserialize)]
pub struct LogPage {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub logs: Vec<Value>,
}

/// A page of history, newest first. Entries stay JSON: each one is dispatched on its `$type`.
#[derive(Debug, Deserialize)]
pub struct MessagePage {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub messages: Vec<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn debug_of_an_auth_session_hides_the_tokens() {
        let session: AuthSession = serde_json::from_value(json!({
            "did": "did:plc:abc", "handle": "ada.test",
            "accessJwt": "secret-access", "refreshJwt": "secret-refresh"
        }))
        .expect("session parses");
        let text = format!("{session:?}");
        assert_eq!(text, "[redacted]");
        assert!(!text.contains("secret"));
    }

    #[test]
    fn a_profile_needs_only_its_did() {
        let profile: Profile = serde_json::from_value(json!({"did": "did:plc:abc"})).unwrap();
        assert!(profile.handle.is_empty());
        assert!(profile.display_name.is_none());
    }

    #[test]
    fn a_message_view_defaults_its_optional_arrays() {
        let view: MessageView = serde_json::from_value(json!({
            "$type": "chat.bsky.convo.defs#messageView",
            "id": "m1", "rev": "r", "text": "hi",
            "sender": {"did": "did:plc:abc"}, "sentAt": "2026-01-01T00:00:00.000Z"
        }))
        .expect("message parses");
        assert!(view.facets.is_empty());
        assert!(view.reactions.is_empty());
    }
}
