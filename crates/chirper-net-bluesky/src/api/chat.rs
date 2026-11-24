//! Direct messages (`chat.bsky.convo.*`), reached through the PDS proxy to the chat service. Also
//! the parsing of the union types they return, dispatched on their `$type`.

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use chirper_bridge::{Error, Result};

use super::types::{ConvoPage, DeletedMessageView, LogPage, MessagePage, MessageView};
use super::{ApiError, Auth, Pds};

const MESSAGE_VIEW: &str = "chat.bsky.convo.defs#messageView";
const DELETED_VIEW: &str = "chat.bsky.convo.defs#deletedMessageView";
const LOG_CREATE: &str = "chat.bsky.convo.defs#logCreateMessage";
const LOG_DELETE: &str = "chat.bsky.convo.defs#logDeleteMessage";
const LOG_ADD_REACTION: &str = "chat.bsky.convo.defs#logAddReaction";
const LOG_REMOVE_REACTION: &str = "chat.bsky.convo.defs#logRemoveReaction";

/// Conversations of the account, newest first.
pub async fn list_convos(
    pds: &Pds,
    access_jwt: &str,
    cursor: Option<&str>,
) -> std::result::Result<ConvoPage, ApiError> {
    let mut params = vec![("limit", "100".to_owned())];
    push_cursor(&mut params, cursor);
    pds.query(
        "chat.bsky.convo.listConvos",
        &params,
        Auth::chat(access_jwt),
    )
    .await
}

/// The event log after `cursor`; without one, from the start of the log.
pub async fn get_log(
    pds: &Pds,
    access_jwt: &str,
    cursor: Option<&str>,
) -> std::result::Result<LogPage, ApiError> {
    let mut params = Vec::new();
    push_cursor(&mut params, cursor);
    pds.query("chat.bsky.convo.getLog", &params, Auth::chat(access_jwt))
        .await
}

/// A page of a conversation's history, newest first.
pub async fn get_messages(
    pds: &Pds,
    access_jwt: &str,
    convo: &str,
    limit: usize,
    cursor: Option<&str>,
) -> std::result::Result<MessagePage, ApiError> {
    let mut params = vec![("convoId", convo.to_owned()), ("limit", limit.to_string())];
    push_cursor(&mut params, cursor);
    pds.query(
        "chat.bsky.convo.getMessages",
        &params,
        Auth::chat(access_jwt),
    )
    .await
}

/// Send a text message with its rich text facets.
pub async fn send_message(
    pds: &Pds,
    access_jwt: &str,
    convo: &str,
    text: &str,
    facets: &[Value],
) -> std::result::Result<MessageView, ApiError> {
    let mut message = json!({"text": text});
    if !facets.is_empty() {
        message["facets"] = Value::Array(facets.to_vec());
    }
    let body = json!({"convoId": convo, "message": message});
    pds.procedure(
        "chat.bsky.convo.sendMessage",
        Some(&body),
        Auth::chat(access_jwt),
    )
    .await
}

/// Delete a message for this account only; the other side keeps it.
pub async fn delete_for_self(
    pds: &Pds,
    access_jwt: &str,
    convo: &str,
    message: &str,
) -> std::result::Result<(), ApiError> {
    let body = json!({"convoId": convo, "messageId": message});
    procedure_ignored(
        pds,
        "chat.bsky.convo.deleteMessageForSelf",
        &body,
        access_jwt,
    )
    .await
}

/// Add a reaction to a message.
pub async fn add_reaction(
    pds: &Pds,
    access_jwt: &str,
    convo: &str,
    message: &str,
    value: &str,
) -> std::result::Result<(), ApiError> {
    reaction(
        pds,
        access_jwt,
        "chat.bsky.convo.addReaction",
        convo,
        message,
        value,
    )
    .await
}

/// Remove this account's reaction from a message.
pub async fn remove_reaction(
    pds: &Pds,
    access_jwt: &str,
    convo: &str,
    message: &str,
    value: &str,
) -> std::result::Result<(), ApiError> {
    reaction(
        pds,
        access_jwt,
        "chat.bsky.convo.removeReaction",
        convo,
        message,
        value,
    )
    .await
}

/// Mark the conversation read up to `message`, or entirely when there is none.
pub async fn update_read(
    pds: &Pds,
    access_jwt: &str,
    convo: &str,
    message: Option<&str>,
) -> std::result::Result<(), ApiError> {
    let mut body = json!({"convoId": convo});
    if let Some(message) = message {
        body["messageId"] = Value::String(message.to_owned());
    }
    procedure_ignored(pds, "chat.bsky.convo.updateRead", &body, access_jwt).await
}

fn push_cursor(params: &mut Vec<(&'static str, String)>, cursor: Option<&str>) {
    if let Some(cursor) = cursor {
        params.push(("cursor", cursor.to_owned()));
    }
}

async fn reaction(
    pds: &Pds,
    access_jwt: &str,
    nsid: &str,
    convo: &str,
    message: &str,
    value: &str,
) -> std::result::Result<(), ApiError> {
    let body = json!({"convoId": convo, "messageId": message, "value": value});
    procedure_ignored(pds, nsid, &body, access_jwt).await
}

/// A procedure whose answer the driver does not read: only its status matters.
async fn procedure_ignored(
    pds: &Pds,
    nsid: &str,
    body: &Value,
    access_jwt: &str,
) -> std::result::Result<(), ApiError> {
    let _: Value = pds
        .procedure(nsid, Some(body), Auth::chat(access_jwt))
        .await?;
    Ok(())
}

/// A message as the event log or history reports it.
#[derive(Clone, Debug)]
pub enum Message {
    View(MessageView),
    Deleted(DeletedMessageView),
    /// A union member this driver does not know.
    Other,
}

impl Message {
    /// The message id, when this is a known message.
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::View(view) => Some(&view.id),
            Self::Deleted(view) => Some(&view.id),
            Self::Other => None,
        }
    }

    /// Every DID this message names: its sender and the senders of its reactions.
    pub fn dids(&self) -> Vec<String> {
        match self {
            Self::View(view) => view_dids(view),
            Self::Deleted(view) => vec![view.sender.did.clone()],
            Self::Other => Vec::new(),
        }
    }
}

/// One entry of the event log.
#[derive(Clone, Debug)]
pub enum Log {
    /// A message was sent (or is a deleted message, when the log reports it that way).
    Created { convo: String, message: Message },
    /// A message was deleted.
    Deleted { convo: String, message: Message },
    /// The reactions of a message changed; the message carries the full set.
    Reactions { convo: String, message: MessageView },
    /// An entry type this driver does not handle.
    Other,
}

impl Log {
    /// Every DID the entry names, so their profiles can be fetched before it is converted.
    pub fn dids(&self) -> Vec<String> {
        match self {
            Self::Created { message, .. } | Self::Deleted { message, .. } => message.dids(),
            Self::Reactions { message, .. } => view_dids(message),
            Self::Other => Vec::new(),
        }
    }
}

fn view_dids(view: &MessageView) -> Vec<String> {
    std::iter::once(view.sender.did.clone())
        .chain(view.reactions.iter().map(|r| r.sender.did.clone()))
        .collect()
}

fn type_of(value: &Value) -> &str {
    value
        .get("$type")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|err| Error::Protocol(format!("Bluesky message: {err}")))
}

/// Parse one message union member.
pub fn parse_message(value: Value) -> Result<Message> {
    match type_of(&value) {
        MESSAGE_VIEW => Ok(Message::View(decode(value)?)),
        DELETED_VIEW => Ok(Message::Deleted(decode(value)?)),
        _ => Ok(Message::Other),
    }
}

/// Parse one event-log entry.
pub fn parse_log(mut value: Value) -> Result<Log> {
    let kind = type_of(&value).to_owned();
    if !matches!(
        kind.as_str(),
        LOG_CREATE | LOG_DELETE | LOG_ADD_REACTION | LOG_REMOVE_REACTION
    ) {
        return Ok(Log::Other);
    }
    let convo = value
        .get("convoId")
        .and_then(Value::as_str)
        .filter(|convo| !convo.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| Error::Protocol("a Bluesky log entry names no conversation".to_owned()))?;
    let message = value
        .get_mut("message")
        .map(Value::take)
        .unwrap_or_default();
    match kind.as_str() {
        LOG_CREATE => Ok(Log::Created {
            convo,
            message: parse_message(message)?,
        }),
        LOG_DELETE => Ok(Log::Deleted {
            convo,
            message: parse_message(message)?,
        }),
        _ => Ok(Log::Reactions {
            convo,
            message: decode(message)?,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(id: &str) -> Value {
        json!({
            "$type": MESSAGE_VIEW, "id": id, "rev": "1", "text": "hello",
            "sender": {"did": "did:plc:bob"}, "sentAt": "2026-01-01T00:00:00.000Z",
            "reactions": [{"value": "👍", "sender": {"did": "did:plc:ada"}}]
        })
    }

    #[test]
    fn a_created_message_entry_parses_to_its_message() {
        let entry = json!({
            "$type": LOG_CREATE, "rev": "2", "convoId": "c1", "message": view("m1")
        });
        match parse_log(entry).unwrap() {
            Log::Created { convo, message } => {
                assert_eq!(convo, "c1");
                assert_eq!(message.id(), Some("m1"));
                assert_eq!(message.dids(), vec!["did:plc:bob", "did:plc:ada"]);
            }
            other => panic!("expected a created message, got {other:?}"),
        }
    }

    #[test]
    fn a_deleted_message_is_a_known_message_without_text() {
        let entry = json!({
            "$type": LOG_DELETE, "convoId": "c1",
            "message": {
                "$type": DELETED_VIEW, "id": "m2",
                "sender": {"did": "did:plc:bob"}, "sentAt": "2026-01-01T00:00:00.000Z"
            }
        });
        assert!(matches!(
            parse_log(entry).unwrap(),
            Log::Deleted { message: Message::Deleted(ref v), .. } if v.id == "m2"
        ));
    }

    #[test]
    fn reaction_entries_carry_the_whole_message() {
        let entry = json!({
            "$type": LOG_ADD_REACTION, "convoId": "c1",
            "message": view("m3"), "reaction": {"value": "👍", "sender": {"did": "did:plc:ada"}}
        });
        match parse_log(entry).unwrap() {
            Log::Reactions { message, .. } => assert_eq!(message.reactions.len(), 1),
            other => panic!("expected reactions, got {other:?}"),
        }
    }

    #[test]
    fn unknown_entries_are_skipped_and_malformed_ones_are_protocol_errors() {
        assert!(matches!(
            parse_log(json!({"$type": "chat.bsky.convo.defs#logBeginConvo", "convoId": "c"})),
            Ok(Log::Other)
        ));
        assert!(matches!(
            parse_log(json!({"$type": LOG_CREATE, "message": view("m")})),
            Err(Error::Protocol(_))
        ));
        assert!(matches!(
            parse_log(
                json!({"$type": LOG_CREATE, "convoId": "c", "message": {"$type": MESSAGE_VIEW}})
            ),
            Err(Error::Protocol(_))
        ));
        assert!(matches!(
            parse_message(json!({"$type": "unknown#thing"})),
            Ok(Message::Other)
        ));
    }
}
