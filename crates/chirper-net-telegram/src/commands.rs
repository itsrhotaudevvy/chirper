//! Outgoing commands: send, edit, redact, react, read receipts and typing.
//!
//! Each command makes its request and returns an [`Ack`] as soon as the server accepted it. The
//! verdict travels later as [`Event::Delivery`]. The updates the server returns are applied through
//! the same pipeline as pushes, so the sent message and the pts/seq state follow the server.

use std::sync::Arc;

use chirper_bridge::{Ack, Command, Error, PortalRef, Result};

use crate::client::Api;
use crate::ids::{self, PeerKind, Portal};
use crate::media;
use crate::mtproto::crypto;
use crate::peers;
use crate::session::{Session, sent};
use crate::tl::{enums, functions, types};

/// The reaction value of a Chirper reaction key: an emoticon, or `custom:<document id>`.
pub fn reaction_of_key(key: &str) -> Result<enums::Reaction> {
    if let Some(document) = key.strip_prefix("custom:") {
        let document_id = document
            .parse::<i64>()
            .map_err(|_| Error::Protocol("custom emoji key is not a number".into()))?;
        return Ok(enums::Reaction::ReactionCustomEmoji(
            types::ReactionCustomEmoji { document_id },
        ));
    }
    if key.is_empty() {
        return Err(Error::Protocol("empty reaction".into()));
    }
    Ok(enums::Reaction::ReactionEmoji(types::ReactionEmoji {
        emoticon: key.to_owned(),
    }))
}

/// A fresh random id for `messages.sendMessage` and friends.
fn random_id() -> Result<i64> {
    let bytes = crypto::random_array::<8>().map_err(|e| Error::Internal(e.to_string()))?;
    Ok(i64::from_le_bytes(bytes))
}

/// The id of the message our send created, from the updates the server returned.
fn sent_message_id(updates: &enums::Updates, random: i64) -> Option<i32> {
    let list: Vec<&enums::Update> = match updates {
        enums::Updates::Updates(u) => u.updates.iter().collect(),
        enums::Updates::UpdatesCombined(u) => u.updates.iter().collect(),
        enums::Updates::UpdateShortSentMessage(m) => return Some(m.id),
        _ => Vec::new(),
    };
    for update in list {
        match update {
            enums::Update::UpdateMessageID(m) if m.random_id == random => return Some(m.id),
            enums::Update::UpdateNewMessage(m) => {
                if let enums::Message::Message(message) = m.message.as_ref() {
                    return Some(message.id);
                }
            }
            enums::Update::UpdateNewChannelMessage(m) => {
                if let enums::Message::Message(message) = m.message.as_ref() {
                    return Some(message.id);
                }
            }
            _ => {}
        }
    }
    None
}

impl<A: Api> Session<A> {
    /// Execute one command. Only the commands in `Capabilities::ALL` are sent; sticker sends are
    /// refused.
    pub async fn command(&mut self, command: Command) -> Result<Ack> {
        match command {
            Command::SendText {
                portal,
                text,
                reply_to,
                txn_id,
            } => self.send_text(portal, text, reply_to, txn_id).await,
            Command::SendMedia {
                portal,
                data,
                mime,
                file_name,
                caption,
                kind,
                reply_to,
                txn_id,
            } => {
                self.send_media(
                    portal, data, mime, file_name, caption, kind, reply_to, txn_id,
                )
                .await
            }
            Command::Edit {
                portal,
                target,
                text,
            } => self.edit(portal, target, text).await,
            Command::Redact { portal, target } => self.redact(portal, target).await,
            Command::React {
                portal,
                target,
                key,
            } => self.react(portal, target, key).await,
            Command::Unreact {
                portal,
                target,
                key,
            } => self.unreact(portal, target, key).await,
            Command::Read { portal, event } => self.read(portal, event).await,
            Command::Typing { portal, typing } => self.typing(portal, typing).await,
        }
    }

    /// Make a reply header for a message of this chat (or of another chat, by its id).
    fn reply_header(&self, portal: &Portal, target: &str) -> Result<enums::InputReplyTo> {
        let (channel, message) = ids::parse_message_id(target)?;
        let peer = match (portal.kind, channel) {
            (PeerKind::Channel, _) | (_, None) => None,
            (_, Some(channel)) => Some(peers::input_peer(
                &self.store,
                &Portal {
                    kind: PeerKind::Channel,
                    id: channel,
                    topic: 0,
                },
            )?),
        };
        let top = (portal.topic != 0).then_some(portal.topic);
        Ok(enums::InputReplyTo::InputReplyToMessage(
            types::InputReplyToMessage {
                reply_to_msg_id: message,
                top_msg_id: top,
                reply_to_peer_id: peer.map(Box::new),
                quote_text: None,
                quote_entities: None,
                quote_offset: None,
                monoforum_peer_id: None,
                todo_item_id: None,
                poll_option: None,
                quote_rich: None,
            },
        ))
    }

    /// The channel or peer whose messages a remote message id refers to, for edits and deletes.
    fn message_target(&self, portal: &PortalRef, target: &str) -> Result<(Portal, i32)> {
        let parsed = ids::parse_portal(&portal.id)?;
        let (channel, message) = ids::parse_message_id(target)?;
        let parsed = match channel {
            Some(channel) => Portal {
                kind: PeerKind::Channel,
                id: channel,
                topic: 0,
            },
            None => parsed,
        };
        Ok((parsed, message))
    }

    async fn send_text(
        &mut self,
        portal: PortalRef,
        text: String,
        reply_to: Option<String>,
        txn_id: String,
    ) -> Result<Ack> {
        let parsed = ids::parse_portal(&portal.id)?;
        let peer = peers::input_peer(&self.store, &parsed)?;
        let reply = match reply_to {
            Some(target) => Some(Box::new(self.reply_header(&parsed, &target)?)),
            None => None,
        };
        let random = random_id()?;
        let request = functions::messages::SendMessage {
            no_webpage: false,
            silent: false,
            background: false,
            clear_draft: false,
            noforwards: false,
            update_stickersets_order: false,
            invert_media: false,
            allow_paid_floodskip: false,
            peer: Box::new(peer),
            reply_to: reply,
            message: text,
            random_id: random,
            reply_markup: None,
            entities: None,
            schedule_date: None,
            schedule_repeat_period: None,
            send_as: None,
            quick_reply_shortcut: None,
            effect: None,
            allow_paid_stars: None,
            suggested_post: None,
            rich_message: None,
        };
        let updates = self.api.call(&request).await.map_err(Error::from)?;
        let id = sent_message_id(&updates, random);
        self.handle_answer(updates).await?;
        self.pending.push_back(sent(portal, txn_id));
        Ok(Ack {
            remote_event_id: id.map(|id| ids::message_id(channel_of(&parsed), id)),
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_media(
        &mut self,
        portal: PortalRef,
        data: Arc<Vec<u8>>,
        mime: String,
        file_name: String,
        caption: String,
        kind: String,
        reply_to: Option<String>,
        txn_id: String,
    ) -> Result<Ack> {
        if kind == "sticker" {
            return Err(Error::Unsupported("sending stickers"));
        }
        let parsed = ids::parse_portal(&portal.id)?;
        let peer = peers::input_peer(&self.store, &parsed)?;
        let reply = match reply_to {
            Some(target) => Some(Box::new(self.reply_header(&parsed, &target)?)),
            None => None,
        };
        let file = media::upload(&self.api, &data, &file_name).await?;
        let media = if kind == "image" {
            enums::InputMedia::InputMediaUploadedPhoto(types::InputMediaUploadedPhoto {
                spoiler: false,
                live_photo: false,
                file: Box::new(file),
                stickers: None,
                ttl_seconds: None,
                video: None,
            })
        } else {
            let mut attributes = vec![enums::DocumentAttribute::DocumentAttributeFilename(
                types::DocumentAttributeFilename {
                    file_name: file_name.clone(),
                },
            )];
            match kind.as_str() {
                "voice" => attributes.push(enums::DocumentAttribute::DocumentAttributeAudio(
                    types::DocumentAttributeAudio {
                        voice: true,
                        duration: 0,
                        title: None,
                        performer: None,
                        waveform: None,
                    },
                )),
                "audio" => attributes.push(enums::DocumentAttribute::DocumentAttributeAudio(
                    types::DocumentAttributeAudio {
                        voice: false,
                        duration: 0,
                        title: None,
                        performer: None,
                        waveform: None,
                    },
                )),
                "video" => attributes.push(enums::DocumentAttribute::DocumentAttributeVideo(
                    types::DocumentAttributeVideo {
                        round_message: false,
                        supports_streaming: true,
                        nosound: false,
                        duration: 0.0,
                        w: 0,
                        h: 0,
                        preload_prefix_size: None,
                        video_start_ts: None,
                        video_codec: None,
                    },
                )),
                _ => {}
            }
            enums::InputMedia::InputMediaUploadedDocument(types::InputMediaUploadedDocument {
                nosound_video: false,
                force_file: kind == "file",
                spoiler: false,
                file: Box::new(file),
                thumb: None,
                mime_type: mime,
                attributes,
                stickers: None,
                video_cover: None,
                video_timestamp: None,
                ttl_seconds: None,
            })
        };
        let random = random_id()?;
        let request = functions::messages::SendMedia {
            silent: false,
            background: false,
            clear_draft: false,
            noforwards: false,
            update_stickersets_order: false,
            invert_media: false,
            allow_paid_floodskip: false,
            peer: Box::new(peer),
            reply_to: reply,
            media: Box::new(media),
            message: caption,
            random_id: random,
            reply_markup: None,
            entities: None,
            schedule_date: None,
            schedule_repeat_period: None,
            send_as: None,
            quick_reply_shortcut: None,
            effect: None,
            allow_paid_stars: None,
            suggested_post: None,
        };
        let updates = self.api.call(&request).await.map_err(Error::from)?;
        let id = sent_message_id(&updates, random);
        self.handle_answer(updates).await?;
        self.pending.push_back(sent(portal, txn_id));
        Ok(Ack {
            remote_event_id: id.map(|id| ids::message_id(channel_of(&parsed), id)),
        })
    }

    async fn edit(&mut self, portal: PortalRef, target: String, text: String) -> Result<Ack> {
        let (parsed, message) = self.message_target(&portal, &target)?;
        let peer = peers::input_peer(&self.store, &parsed)?;
        let request = functions::messages::EditMessage {
            no_webpage: false,
            invert_media: false,
            peer: Box::new(peer),
            id: message,
            message: Some(text),
            media: None,
            reply_markup: None,
            rich_message: None,
            entities: None,
            schedule_date: None,
            schedule_repeat_period: None,
            quick_reply_shortcut_id: None,
        };
        let updates = self.api.call(&request).await.map_err(Error::from)?;
        self.handle_answer(updates).await?;
        Ok(Ack::default())
    }

    async fn redact(&mut self, portal: PortalRef, target: String) -> Result<Ack> {
        let (parsed, message) = self.message_target(&portal, &target)?;
        match parsed.kind {
            PeerKind::Channel => {
                let channel = peers::input_channel(&self.store, parsed.id)?;
                let request = functions::channels::DeleteMessages {
                    channel: Box::new(channel),
                    id: vec![message],
                };
                self.api.call(&request).await.map_err(Error::from)?;
            }
            _ => {
                let request = functions::messages::DeleteMessages {
                    revoke: true,
                    id: vec![message],
                };
                self.api.call(&request).await.map_err(Error::from)?;
            }
        }
        Ok(Ack::default())
    }

    async fn react(&mut self, portal: PortalRef, target: String, key: String) -> Result<Ack> {
        let reaction = reaction_of_key(&key)?;
        self.send_reaction(&portal, &target, vec![reaction]).await
    }

    /// Remove one of our reactions: send the others back. The server keeps only what we send.
    async fn unreact(&mut self, portal: PortalRef, target: String, key: String) -> Result<Ack> {
        let (parsed, message) = self.message_target(&portal, &target)?;
        let remaining = self.own_reactions(&parsed, message).await?;
        let keep: Vec<enums::Reaction> = remaining
            .into_iter()
            .filter(|existing| key_of(existing) != key)
            .collect();
        self.send_reaction(&portal, &target, keep).await
    }

    /// The reactions the account itself has on a message.
    async fn own_reactions(
        &mut self,
        parsed: &Portal,
        message: i32,
    ) -> Result<Vec<enums::Reaction>> {
        let raw = match parsed.kind {
            PeerKind::Channel => {
                let channel = peers::input_channel(&self.store, parsed.id)?;
                let request = functions::channels::GetMessages {
                    channel: Box::new(channel),
                    id: vec![enums::InputMessage::InputMessageID(types::InputMessageID {
                        id: message,
                    })],
                };
                self.api.call(&request).await.map_err(Error::from)?
            }
            _ => {
                let request = functions::messages::GetMessages {
                    id: vec![enums::InputMessage::InputMessageID(types::InputMessageID {
                        id: message,
                    })],
                };
                self.api.call(&request).await.map_err(Error::from)?
            }
        };
        let found = crate::backfill::history_messages(raw);
        let Some(enums::Message::Message(m)) = found.into_iter().next() else {
            return Err(Error::Protocol("the reacted message is gone".into()));
        };
        let Some(enums::MessageReactions::MessageReactions(summary)) = m.reactions.map(|r| *r)
        else {
            return Ok(Vec::new());
        };
        Ok(summary
            .results
            .into_iter()
            .filter_map(|count| {
                let enums::ReactionCount::ReactionCount(count) = count;
                count.chosen_order.map(|_| *count.reaction)
            })
            .collect())
    }

    async fn send_reaction(
        &mut self,
        portal: &PortalRef,
        target: &str,
        reactions: Vec<enums::Reaction>,
    ) -> Result<Ack> {
        let (parsed, message) = self.message_target(portal, target)?;
        let peer = peers::input_peer(&self.store, &parsed)?;
        let request = functions::messages::SendReaction {
            big: false,
            add_to_recent: true,
            peer: Box::new(peer),
            msg_id: message,
            reaction: Some(reactions),
        };
        let updates = self.api.call(&request).await.map_err(Error::from)?;
        self.handle_answer(updates).await?;
        Ok(Ack::default())
    }

    async fn read(&mut self, portal: PortalRef, event: Option<String>) -> Result<Ack> {
        let parsed = ids::parse_portal(&portal.id)?;
        let max_id = match event {
            Some(target) => self.message_target(&portal, &target)?.1,
            None => i32::MAX,
        };
        match parsed.kind {
            PeerKind::Channel => {
                let channel = peers::input_channel(&self.store, parsed.id)?;
                let request = functions::channels::ReadHistory {
                    channel: Box::new(channel),
                    max_id,
                };
                self.api.call(&request).await.map_err(Error::from)?;
            }
            _ => {
                let peer = peers::input_peer(&self.store, &parsed)?;
                let request = functions::messages::ReadHistory {
                    peer: Box::new(peer),
                    max_id,
                };
                self.api.call(&request).await.map_err(Error::from)?;
            }
        }
        Ok(Ack::default())
    }

    async fn typing(&mut self, portal: PortalRef, typing: bool) -> Result<Ack> {
        let parsed = ids::parse_portal(&portal.id)?;
        let peer = peers::input_peer(&self.store, &parsed)?;
        let action = if typing {
            enums::SendMessageAction::SendMessageTypingAction(types::SendMessageTypingAction {})
        } else {
            enums::SendMessageAction::SendMessageCancelAction(types::SendMessageCancelAction {})
        };
        let request = functions::messages::SetTyping {
            peer: Box::new(peer),
            top_msg_id: None,
            action: Box::new(action),
        };
        self.api.call(&request).await.map_err(Error::from)?;
        Ok(Ack::default())
    }

    /// `auth.logOut` on the account's connection.
    pub async fn logout_remote(&mut self) -> Result<()> {
        self.api
            .call(&functions::auth::LogOut {})
            .await
            .map(|_| ())
            .map_err(Error::from)
    }
}

/// The channel of a parsed portal, when it is one.
fn channel_of(portal: &Portal) -> Option<i64> {
    (portal.kind == PeerKind::Channel).then_some(portal.id)
}

/// The reaction key of a TL reaction, as the bridge names it.
fn key_of(reaction: &enums::Reaction) -> String {
    match reaction {
        enums::Reaction::ReactionEmoji(e) => e.emoticon.clone(),
        enums::Reaction::ReactionCustomEmoji(e) => ids::custom_emoji_key(e.document_id),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reaction_keys_round_trip() {
        assert!(matches!(
            reaction_of_key("👍"),
            Ok(enums::Reaction::ReactionEmoji(_))
        ));
        match reaction_of_key("custom:9") {
            Ok(reaction) => assert_eq!(key_of(&reaction), "custom:9"),
            Err(error) => panic!("unexpected {error}"),
        }
        assert!(reaction_of_key("custom:x").is_err());
        assert!(reaction_of_key("").is_err());
    }
}
