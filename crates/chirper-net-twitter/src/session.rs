//! The live connection of one X login: the inbox at connect, the poll for updates, the commands
//! and the backfill. Reference: `connector/client.go` (`connect`), `twittermeow/polling.go`,
//! `connector/handletwit.go` (`HandlePollingEvent`), `connector/backfill.go`.
//!
//! `next` polls on the reference's ten-second interval. A poll reads the network and changes
//! nothing until it has every event of its page, so a dropped `next` future never loses or
//! duplicates an event: the cursor only moves when the batch is committed.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use async_trait::async_trait;
use log::debug;
use tokio::time::Instant;

use chirper_bridge::model::{
    Ack, BackfillPage, Content, DeliveryStatus, Event, Member, PortalRef, RemoteEventRef,
    RemoteMessage, RemoteUser, SessionState,
};
use chirper_bridge::{Command, Error, Result, Session};

use crate::api::cookies::Cookies;
use crate::api::media;
use crate::api::model::{Conversation, Entry, Inbox, Message, MessageData, User};
use crate::api::{Api, Bases, dm, random_uuid, transaction};
use crate::convert::{self, MAX_MEDIA_BYTES};
use crate::ids;

/// The secret that holds the cookie jar of a login.
pub const SECRET_COOKIES: &str = "cookies";
/// The secret that holds the browser fingerprint a login was made in.
pub const SECRET_SESSION: &str = "session";
/// How often the inbox is polled, as the reference does.
pub const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// The longest wait between polls that keep failing.
const MAX_BACKOFF: Duration = Duration::from_secs(180);
/// The most conversations the connect-time sync announces.
const SYNC_LIMIT: usize = 500;
/// The most messages one backfill request asks for.
const MAX_PAGE: usize = 100;

/// The events of one poll, and the state they will change when committed.
struct Batch {
    cursor: String,
    users: HashMap<String, User>,
    chats: HashSet<String>,
    reactors: HashMap<(String, String), Vec<(String, String)>>,
    last_message: HashMap<String, String>,
    events: Vec<Event>,
}

/// One live X connection.
pub struct DmSession {
    api: Api,
    me: RemoteUser,
    me_id: String,
    cursor: String,
    users: HashMap<String, User>,
    chats: HashSet<String>,
    reactors: HashMap<(String, String), Vec<(String, String)>>,
    last_message: HashMap<String, String>,
    queue: VecDeque<Event>,
    next_poll: Instant,
    interval: Duration,
    backoff: Duration,
    ended: bool,
}

/// Connect: check the cookies by loading the inbox, then queue the chat list and the state.
pub async fn connect(
    http: reqwest::Client,
    cookies: Cookies,
    bases: Bases,
    user_agent: &str,
    interval: Duration,
) -> Result<DmSession> {
    let me_id = cookies
        .user_id()
        .ok_or_else(|| Error::Auth("the cookies do not name an X account".to_owned()))?;
    let api = Api::new(http, cookies, bases).with_user_agent(user_agent);
    let signer = transaction::load(&api).await;
    let api = api.with_signer(signer);
    let inbox = dm::inbox_initial_state(&api, false).await?;
    let users = inbox.users.clone();
    let me = convert::remote_user(&me_id, &users);

    let mut conversations: Vec<&Conversation> = inbox.conversations.values().collect();
    conversations.sort_by(|a, b| compare_ids(&b.sort_timestamp, &a.sort_timestamp));
    let mut queue = VecDeque::new();
    queue.push_back(Event::State {
        state: SessionState::Connecting,
        error: String::new(),
        message: String::new(),
        profile: None,
    });
    let mut chats = HashSet::new();
    for conversation in conversations.into_iter().take(SYNC_LIMIT) {
        let meta = convert::chat_meta(conversation, &users, &me_id);
        chats.insert(meta.portal.id.clone());
        queue.push_back(Event::Chat(meta));
    }
    queue.push_back(Event::State {
        state: SessionState::Running,
        error: String::new(),
        message: String::new(),
        profile: Some(me.clone()),
    });
    Ok(DmSession {
        api,
        me,
        me_id,
        cursor: inbox.cursor,
        users,
        chats,
        reactors: HashMap::new(),
        last_message: HashMap::new(),
        queue,
        next_poll: Instant::now() + interval,
        interval,
        backoff: interval,
        ended: false,
    })
}

/// The order of two snowflake-like ids: shorter is older, equal lengths compare as text.
pub fn compare_ids(a: &str, b: &str) -> std::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// A request id for an outgoing message: the engine's transaction id when it is a UUID, else a
/// fresh one.
fn request_id(txn_id: &str) -> String {
    if txn_id.len() == 36 && txn_id.matches('-').count() == 4 {
        txn_id.to_owned()
    } else {
        random_uuid()
    }
}

impl DmSession {
    /// The state the poll would commit on, copied for a batch.
    fn batch(&self) -> Batch {
        Batch {
            cursor: self.cursor.clone(),
            users: self.users.clone(),
            chats: self.chats.clone(),
            reactors: HashMap::new(),
            last_message: HashMap::new(),
            events: Vec::new(),
        }
    }

    /// Make the batch's copy of a message's reactors, the first time it is touched.
    fn reactors_of<'a>(
        &self,
        batch: &'a mut Batch,
        key: (String, String),
    ) -> &'a mut Vec<(String, String)> {
        let base = self.reactors.get(&key).cloned().unwrap_or_default();
        batch.reactors.entry(key).or_insert(base)
    }

    /// One poll: read the updates after the cursor and turn them into a batch.
    async fn poll(&self) -> Result<Batch> {
        let inbox = dm::user_updates(&self.api, &self.cursor).await?;
        let mut batch = self.batch();
        batch.users.extend(inbox.users.clone());
        for entry in inbox.parsed_entries() {
            self.apply(&mut batch, &inbox, entry).await;
        }
        if !inbox.cursor.is_empty() {
            batch.cursor = inbox.cursor;
        }
        Ok(batch)
    }

    /// Turn one entry of an updates page into events on the batch.
    async fn apply(&self, batch: &mut Batch, inbox: &Inbox, entry: Entry) {
        match entry {
            Entry::Message(message) => self.apply_message(batch, inbox, *message).await,
            Entry::ReactionCreate(reaction) => self.apply_reaction(batch, &reaction, true),
            Entry::ReactionDelete(reaction) => self.apply_reaction(batch, &reaction, false),
            Entry::ConversationRead(read) => {
                if read.last_read_event_id.is_empty() {
                    return;
                }
                let sender = if read.sender_id.is_empty() {
                    self.me_id.as_str()
                } else {
                    read.sender_id.as_str()
                };
                batch.events.push(Event::ReadReceipt {
                    portal: convert::portal(&read.conversation_id, &self.me_id),
                    user: convert::remote_user(sender, &batch.users),
                    event: RemoteEventRef::new(read.last_read_event_id),
                });
            }
            Entry::ConversationRemoved(removal) => {
                let portal = convert::portal(&removal.conversation_id, &self.me_id);
                batch.chats.remove(&portal.id);
                batch.events.push(Event::ChatRemoved(portal));
            }
            Entry::ParticipantsJoin(change) => self.apply_members(batch, &change, "join"),
            Entry::ParticipantsLeave(change) => self.apply_members(batch, &change, "leave"),
            Entry::Other(name) => debug!("twitter: skipping an update of type {name}"),
        }
    }

    /// A new message: announce its chat when it is new to us, then the message itself.
    async fn apply_message(&self, batch: &mut Batch, inbox: &Inbox, message: Message) {
        let portal = convert::portal(&message.conversation_id, &self.me_id);
        let announce = !batch.chats.contains(&portal.id);
        if let Some(conversation) = inbox
            .conversations
            .get(&message.conversation_id)
            .filter(|_| announce)
        {
            let meta = convert::chat_meta(conversation, &batch.users, &self.me_id);
            batch.events.push(Event::Chat(meta));
            batch.chats.insert(portal.id.clone());
        }
        let sender_id = message.message_data.sender_id.clone();
        let content = self.content_of(&message.message_data).await;
        let content = if message.message_data.conversation_key_version.is_empty() {
            content
        } else {
            encrypted()
        };
        let reply_to = Some(message.message_data.reply_data.id.clone()).filter(|id| !id.is_empty());
        batch
            .last_message
            .insert(portal.id.clone(), message.id.clone());
        batch.events.push(Event::Message(RemoteMessage {
            portal,
            id: ids::message_id(&message.id),
            part: String::new(),
            sender: convert::remote_user(&sender_id, &batch.users),
            timestamp_ms: convert::timestamp_ms(&message.time, &message.id),
            content,
            reply_to,
            edit_of: None,
            live: true,
            from_me: sender_id == self.me_id,
        }));
    }

    /// Add or remove one reactor, then emit the message's full reaction set.
    fn apply_reaction(&self, batch: &mut Batch, reaction: &crate::api::model::Reaction, add: bool) {
        if reaction.message_id.is_empty() || reaction.emoji_reaction.is_empty() {
            return;
        }
        let portal = convert::portal(&reaction.conversation_id, &self.me_id);
        let sender = if reaction.sender_id.is_empty() {
            self.me_id.clone()
        } else {
            reaction.sender_id.clone()
        };
        let key = (portal.id.clone(), reaction.message_id.clone());
        let pair = (reaction.emoji_reaction.clone(), sender);
        {
            let list = self.reactors_of(batch, key.clone());
            if add {
                if !list.contains(&pair) {
                    list.push(pair);
                }
            } else {
                list.retain(|existing| *existing != pair);
            }
        }
        let current = batch.reactors.get(&key).cloned().unwrap_or_default();
        let summary = convert::reaction_summary(&current, &batch.users, &self.me_id);
        batch.events.push(Event::Reactions {
            portal,
            target: RemoteEventRef::new(ids::message_id(&reaction.message_id)),
            reactions: summary,
        });
    }

    /// Membership changes, one event per person.
    fn apply_members(
        &self,
        batch: &mut Batch,
        change: &crate::api::model::Membership,
        membership: &str,
    ) {
        let portal = convert::portal(&change.conversation_id, &self.me_id);
        for participant in &change.participants {
            let user = convert::remote_user(&participant.user_id, &batch.users);
            batch.events.push(Event::Member {
                portal: portal.clone(),
                member: Member {
                    user,
                    membership: membership.to_owned(),
                },
            });
        }
    }

    /// The content of a message. Media is downloaded here; a file that cannot be had is described.
    async fn content_of(&self, data: &MessageData) -> Content {
        let Some(source) = convert::media_of(data) else {
            let (body, formatted) = convert::text_of(data);
            if body.is_empty() && data.attachment.is_some() {
                return Content::Unsupported {
                    body: "A shared post or link preview. Open X to view it.".to_owned(),
                };
            }
            return Content::Text { body, formatted };
        };
        let (caption, _) = convert::text_of(data);
        match self.api.download(&source.url, MAX_MEDIA_BYTES).await {
            Ok(Some(bytes)) => convert::media_content(&source, caption, bytes),
            Ok(None) => Content::Unsupported {
                body: format!("A {} larger than 100 MiB. Open X to view it.", source.kind),
            },
            Err(err) => {
                debug!("twitter: an attachment could not be downloaded: {err}");
                Content::Unsupported {
                    body: format!("A {} that could not be downloaded.", source.kind),
                }
            }
        }
    }

    /// Commit a batch: the cursor, the caches and the events all move together.
    fn commit(&mut self, batch: Batch) {
        self.cursor = batch.cursor;
        self.users = batch.users;
        self.chats = batch.chats;
        self.reactors.extend(batch.reactors);
        self.last_message.extend(batch.last_message);
        self.queue.extend(batch.events);
    }

    /// Run one poll and act on its outcome. A failure that the runtime should retry ends the
    /// connection; a bad page is reported and polled again after a longer wait.
    async fn poll_once(&mut self) {
        match self.poll().await {
            Ok(batch) => {
                self.commit(batch);
                self.backoff = self.interval;
                self.next_poll = Instant::now() + self.interval;
            }
            Err(Error::Auth(message)) => {
                self.queue
                    .push_back(state(SessionState::NeedsReauth, &message));
                self.ended = true;
            }
            Err(err @ (Error::Network(_) | Error::RateLimited { .. })) => {
                self.queue
                    .push_back(state(SessionState::TransientDisconnect, &err.to_string()));
                self.ended = true;
            }
            Err(err) => {
                self.queue.push_back(Event::Error(err.to_string()));
                self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
                self.next_poll = Instant::now() + self.backoff;
            }
        }
    }

    /// Send one direct message and report its verdict on the event stream.
    async fn send_built(
        &mut self,
        portal: PortalRef,
        txn_id: String,
        outgoing: Outgoing,
    ) -> Result<Ack> {
        let request = request_id(&txn_id);
        let payload = dm::SendPayload {
            conversation_id: &portal.id,
            media_id: &outgoing.media_id,
            reply_to_dm_id: &outgoing.reply_to,
            request_id: &request,
            text: &outgoing.text,
            ..Default::default()
        };
        let inbox = dm::send(&self.api, &payload, &dm::messages_referer(&portal.id)).await?;
        let remote_id = dm::created_message(&inbox, &portal.id).map(|m| m.id);
        self.queue.push_back(Event::Delivery {
            portal,
            txn_id,
            status: DeliveryStatus::Delivered,
            error: String::new(),
        });
        Ok(Ack {
            remote_event_id: remote_id,
        })
    }
}

/// What an encrypted XChat message shows until XChat is ported.
fn encrypted() -> Content {
    Content::Unsupported {
        body: "Encrypted X chat. Open X to read it.".to_owned(),
    }
}

/// What one outgoing direct message carries.
struct Outgoing {
    text: String,
    reply_to: String,
    media_id: String,
}

/// A state event with no profile.
fn state(state: SessionState, error: &str) -> Event {
    Event::State {
        state,
        error: error.to_owned(),
        message: String::new(),
        profile: None,
    }
}

#[async_trait]
impl Session for DmSession {
    async fn next(&mut self) -> Option<Event> {
        loop {
            if let Some(event) = self.queue.pop_front() {
                return Some(event);
            }
            if self.ended {
                return None;
            }
            tokio::time::sleep_until(self.next_poll).await;
            self.poll_once().await;
        }
    }

    async fn command(&mut self, command: Command) -> Result<Ack> {
        match command {
            Command::SendText {
                portal,
                text,
                reply_to,
                txn_id,
            } => {
                let outgoing = Outgoing {
                    text,
                    reply_to: reply_to.unwrap_or_default(),
                    media_id: String::new(),
                };
                self.send_built(portal, txn_id, outgoing).await
            }
            Command::SendMedia {
                portal,
                data,
                mime,
                file_name: _,
                caption,
                kind,
                reply_to,
                txn_id,
            } => {
                let (media_type, category) = match kind.as_str() {
                    "image" if mime == "image/gif" => (mime.as_str(), media::CATEGORY_GIF),
                    "image" => (mime.as_str(), media::CATEGORY_IMAGE),
                    "video" => (mime.as_str(), media::CATEGORY_VIDEO),
                    _ => {
                        return Err(Error::Unsupported(
                            "this attachment type in X direct messages",
                        ));
                    }
                };
                let uploaded = media::upload(&self.api, media_type, category, &data).await?;
                let outgoing = Outgoing {
                    text: caption,
                    reply_to: reply_to.unwrap_or_default(),
                    media_id: uploaded.media_id,
                };
                self.send_built(portal, txn_id, outgoing).await
            }
            Command::Edit { .. } => Err(Error::Unsupported(
                "editing messages outside encrypted X chats",
            )),
            Command::Redact { target, .. } => {
                dm::delete_for_me(&self.api, &target, &random_uuid()).await?;
                Ok(Ack::default())
            }
            Command::React {
                portal,
                target,
                key,
            } => {
                dm::react(&self.api, &portal.id, &target, &key, false).await?;
                Ok(Ack::default())
            }
            Command::Unreact {
                portal,
                target,
                key,
            } => {
                dm::react(&self.api, &portal.id, &target, &key, true).await?;
                Ok(Ack::default())
            }
            Command::Read { portal, event } => {
                let target = event.or_else(|| self.last_message.get(&portal.id).cloned());
                if let Some(target) = target {
                    dm::mark_read(&self.api, &portal.id, &target).await?;
                }
                Ok(Ack::default())
            }
            Command::Typing { portal, typing } => {
                if typing {
                    dm::typing(&self.api, &portal.id).await?;
                }
                Ok(Ack::default())
            }
        }
    }

    async fn backfill(
        &mut self,
        portal: &PortalRef,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<BackfillPage> {
        let count = limit.clamp(1, MAX_PAGE);
        let max_id = cursor.unwrap_or_default();
        let inbox = dm::conversation_page(&self.api, &portal.id, &max_id, count).await?;

        let mut raw: Vec<Message> = inbox
            .parsed_entries()
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Message(message)
                    if ids::portal_id(&message.conversation_id) == portal.id =>
                {
                    Some(*message)
                }
                _ => None,
            })
            .collect();
        raw.sort_by(|a, b| compare_ids(&a.id, &b.id));
        let raw_count = raw.len();
        let mut messages = Vec::with_capacity(raw_count);
        for message in raw {
            let sender_id = message.message_data.sender_id.clone();
            // The reference keys history by the message's own id, and falls back to the entry id.
            let id = if message.message_data.id.is_empty() {
                message.id.clone()
            } else {
                message.message_data.id.clone()
            };
            let content = if message.message_data.conversation_key_version.is_empty() {
                self.content_of(&message.message_data).await
            } else {
                encrypted()
            };
            messages.push(RemoteMessage {
                portal: portal.clone(),
                id: ids::message_id(&id),
                part: String::new(),
                sender: convert::remote_user(&sender_id, &self.users),
                timestamp_ms: convert::timestamp_ms(&message.time, &message.id),
                content,
                reply_to: Some(message.message_data.reply_data.id.clone())
                    .filter(|id| !id.is_empty()),
                edit_of: None,
                live: false,
                from_me: sender_id == self.me_id,
            });
        }

        let has_more = if inbox.status.is_empty() {
            raw_count >= count
        } else {
            inbox.status == "HAS_MORE"
        };
        let cursor = (has_more && !inbox.min_entry_id.is_empty()).then_some(inbox.min_entry_id);
        Ok(BackfillPage {
            messages,
            has_more: cursor.is_some(),
            cursor,
        })
    }

    async fn logout_remote(&mut self) -> Result<()> {
        Err(Error::Unsupported("logging out of X remotely"))
    }

    fn user(&self) -> Option<RemoteUser> {
        Some(self.me.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_compare_by_length_then_text() {
        assert_eq!(compare_ids("9", "10"), std::cmp::Ordering::Less);
        assert_eq!(compare_ids("100", "99"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn a_uuid_transaction_id_is_kept_and_anything_else_replaced() {
        let uuid = "123e4567-e89b-42d3-a456-426614174000";
        assert_eq!(request_id(uuid), uuid);
        assert_ne!(request_id("not-a-uuid"), "not-a-uuid");
    }

    #[test]
    fn a_state_event_carries_no_profile() {
        assert!(matches!(
            state(SessionState::NeedsReauth, "x"),
            Event::State { profile: None, .. }
        ));
    }
}
