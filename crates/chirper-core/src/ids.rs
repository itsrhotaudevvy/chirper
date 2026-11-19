//! Local identifiers, re-exported from the bridge runtime so the engine imports one place.

pub use chirper_bridge::ids::{
    ChatId, EventId, SELF_USER_ID, SERVER_NAME, SenderId, chat_id, event_id_for,
    external_sender_id, hash_hex, local_event_id, media_id_of, media_uri, random_chat_id,
    random_hex, reaction_event_id, sender_id,
};
