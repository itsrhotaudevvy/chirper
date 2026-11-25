//! History pages: `messages.getHistory` walked backwards with the oldest id as the cursor.

use chirper_bridge::{BackfillPage, Error, PortalRef, Result};

use crate::client::Api;
use crate::ids;
use crate::peers;
use crate::session::Session;
use crate::tl::{enums, functions};

/// The most messages one `messages.getHistory` call returns.
pub const MAX_PAGE: usize = 100;

/// Parse a backfill cursor: the decimal id of the oldest message already fetched.
pub fn parse_cursor(cursor: Option<&str>) -> Result<i32> {
    match cursor {
        None => Ok(0),
        Some(text) => text
            .parse::<i32>()
            .map_err(|_| Error::Protocol("backfill cursor is not a message id".into())),
    }
}

/// The messages of a history answer, newest first as Telegram sends them.
pub fn history_messages(answer: enums::messages::Messages) -> Vec<enums::Message> {
    match answer {
        enums::messages::Messages::Messages(m) => m.messages,
        enums::messages::Messages::MessagesSlice(m) => m.messages,
        enums::messages::Messages::ChannelMessages(m) => m.messages,
        enums::messages::Messages::MessagesNotModified(_) => Vec::new(),
    }
}

impl<A: Api> Session<A> {
    /// One page of history of a chat, oldest message first.
    pub async fn backfill_page(
        &mut self,
        portal: &PortalRef,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<BackfillPage> {
        let parsed = ids::parse_portal(&portal.id)?;
        if parsed.topic != 0 {
            return Err(Error::Unsupported("history of a forum topic"));
        }
        let offset_id = parse_cursor(cursor.as_deref())?;
        let page = limit.clamp(1, MAX_PAGE);
        let peer = peers::input_peer(&self.store, &parsed)?;
        let request = functions::messages::GetHistory {
            peer: Box::new(peer),
            offset_id,
            offset_date: 0,
            add_offset: 0,
            limit: i32::try_from(page).unwrap_or(MAX_PAGE as i32),
            max_id: 0,
            min_id: 0,
            hash: 0,
        };
        let answer = self.api.call(&request).await.map_err(Error::from)?;
        let raw = history_messages(answer);
        let full = raw.len() >= page;
        let oldest = raw.iter().filter_map(message_id_of).min();
        let mut messages = Vec::new();
        for message in raw.iter().rev() {
            if let Some(remote) = self.remote_message(message, false, false).await {
                messages.push(remote);
            }
        }
        Ok(BackfillPage {
            messages,
            cursor: oldest.map(|id| id.to_string()),
            has_more: full,
        })
    }
}

fn message_id_of(message: &enums::Message) -> Option<i32> {
    match message {
        enums::Message::Message(m) => Some(m.id),
        enums::Message::MessageService(s) => Some(s.id),
        enums::Message::MessageEmpty(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_parse_or_fail_cleanly() {
        assert_eq!(parse_cursor(None).unwrap(), 0);
        assert_eq!(parse_cursor(Some("42")).unwrap(), 42);
        assert!(parse_cursor(Some("4x")).is_err());
    }
}
