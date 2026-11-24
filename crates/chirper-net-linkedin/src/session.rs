//! One live LinkedIn login: connect (credentials, chat list), the event pump, commands and
//! backfill. Reference: `connector/client.go`, `chatsync.go`, `backfill.go`, `handlematrix.go`.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::sync::mpsc;

use chirper_bridge::model::{
    Ack, BackfillPage, PortalRef, RemoteMessage, RemoteUser, SessionState,
};

use crate::api::model::Conversation;
use chirper_bridge::{Command, Error, Event, LoginCtx, Result, Session};

use crate::api::outbound::{self, QuotedMessage};
use crate::api::urn::Urn;
use crate::api::{Api, Cookies, uuid_v4};
use crate::convert;
use crate::ids;
use crate::realtime;

/// The `secrets` names of a login.
pub const SECRET_COOKIES: &str = "cookies";
pub const SECRET_SESSION: &str = "session";

/// Chats synced at connect, newest first.
const SYNC_LIMIT: usize = 500;
/// Conversations per sync page.
const SYNC_PAGE: u32 = 20;
/// Messages remembered for replies.
const REPLY_CACHE: usize = 2000;

/// What a login knows about the chats it has seen: whether each conversation is a group. The
/// message payloads do not always say, and a portal's receiver depends on it.
#[derive(Debug, Default)]
pub struct Shared {
    groups: Mutex<HashMap<String, bool>>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, bool>> {
        self.groups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Remember whether `conversation` is a group chat.
    pub fn record(&self, conversation: &str, group: bool) {
        self.lock().insert(conversation.to_owned(), group);
    }

    /// Whether `conversation` is a group chat, when this login has seen it.
    pub fn lookup(&self, conversation: &str) -> Option<bool> {
        self.lock().get(conversation).copied()
    }
}

/// Whether a conversation is a group: what the payload says, else what was seen before, else no.
pub fn group_of(shared: &Shared, conversation: &str, explicit: Option<bool>) -> bool {
    explicit
        .or_else(|| shared.lookup(conversation))
        .unwrap_or(false)
}

/// The portal of a conversation for this login.
pub fn portal_of(
    shared: &Shared,
    me_id: &str,
    conversation: &str,
    explicit: Option<bool>,
) -> PortalRef {
    let group = group_of(shared, conversation, explicit);
    PortalRef {
        id: ids::portal_id(conversation),
        receiver: ids::portal_receiver(group, me_id),
    }
}

/// The current time in Unix milliseconds.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

/// The cookies stored for a login.
pub fn load_cookies(ctx: &LoginCtx) -> Result<Cookies> {
    let secret = ctx
        .get_secret(SECRET_COOKIES)?
        .ok_or_else(|| Error::Auth("no LinkedIn cookies are stored".to_owned()))?;
    let map: BTreeMap<String, String> = serde_json::from_slice(&secret)
        .map_err(|_| Error::Auth("the stored LinkedIn cookies are unreadable".to_owned()))?;
    let cookies = Cookies::new(map);
    if !cookies.is_complete() {
        return Err(Error::Auth(
            "the stored LinkedIn cookies are incomplete".to_owned(),
        ));
    }
    Ok(cookies)
}

/// The user agent the login was made with, or the reference's default.
pub fn load_user_agent(ctx: &LoginCtx) -> Result<String> {
    let stored = ctx.get_secret(SECRET_SESSION)?;
    let agent = stored
        .and_then(|secret| serde_json::from_slice::<serde_json::Value>(&secret).ok())
        .and_then(|value| {
            value
                .get("user_agent")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        });
    Ok(agent
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| crate::api::USER_AGENT.to_owned()))
}

/// Connect a login: check the cookies, sync the chat list, then start the realtime stream.
pub async fn connect(ctx: LoginCtx, base: &str) -> Result<Box<dyn Session>> {
    let cookies = load_cookies(&ctx)?;
    let user_agent = load_user_agent(&ctx)?;
    let api = Api::new(ctx.http.clone(), base, cookies, &user_agent);
    let profile = api.profile().await?;
    let me_id = ids::user_id(&profile.mini.entity_urn);
    if me_id.is_empty() {
        return Err(Error::Auth(
            "LinkedIn returned no profile for these cookies".to_owned(),
        ));
    }
    let me = RemoteUser {
        id: me_id.clone(),
        display_name: format!("{} {}", profile.mini.first_name, profile.mini.last_name)
            .trim()
            .to_owned(),
        avatar_url: profile
            .mini
            .picture
            .and_then(|picture| picture.vector)
            .map(|vector| vector.largest_url())
            .unwrap_or_default(),
        ..RemoteUser::default()
    };
    let shared = Arc::new(Shared::default());
    let (tx, events) = mpsc::unbounded_channel();
    send(
        &tx,
        state(SessionState::Connecting, "", "Connecting to LinkedIn", None),
    );
    sync_chats(&api, &me_id, &shared, &tx).await?;
    send(&tx, state(SessionState::Running, "", "", Some(me.clone())));
    tokio::spawn(realtime::run(realtime::Link {
        api: api.clone(),
        me_id: me_id.clone(),
        shared: shared.clone(),
        tx,
    }));
    Ok(Box::new(LinkedInSession {
        api,
        me_id,
        me,
        events,
        replies: ReplyCache::default(),
    }))
}

/// Send an event to the session's queue. The receiver is gone only once the login ended.
pub fn send(tx: &mpsc::UnboundedSender<Event>, event: Event) -> bool {
    tx.send(event).is_ok()
}

/// A state change event.
pub fn state(
    state: SessionState,
    error: &str,
    message: &str,
    profile: Option<RemoteUser>,
) -> Event {
    Event::State {
        state,
        error: error.to_owned(),
        message: message.to_owned(),
        profile,
    }
}

/// Sync the primary inbox, newest first, up to [`SYNC_LIMIT`] chats.
pub async fn sync_chats(
    api: &Api,
    me_id: &str,
    shared: &Shared,
    tx: &mpsc::UnboundedSender<Event>,
) -> Result<()> {
    let mailbox = ids::profile_urn(me_id);
    let mut before = now_ms();
    let mut synced = 0usize;
    loop {
        let page = api
            .conversations_before(&mailbox, before, SYNC_PAGE)
            .await?;
        if page.elements.is_empty() {
            return Ok(());
        }
        let mut oldest = before;
        for conversation in &page.elements {
            if let Some(event) = conversation_event(conversation, me_id, shared) {
                send(tx, event);
            }
            synced += 1;
            if conversation.last_activity_at > 0 && conversation.last_activity_at < oldest {
                oldest = conversation.last_activity_at;
            }
            if synced >= SYNC_LIMIT {
                return Ok(());
            }
        }
        if oldest >= before {
            return Ok(());
        }
        before = oldest;
    }
}

/// The event of one conversation: a chat, a removal, or nothing for other inbox categories.
pub fn conversation_event(
    conversation: &Conversation,
    me_id: &str,
    shared: &Shared,
) -> Option<Event> {
    if let Some(group) = conversation.group_chat {
        shared.record(&conversation.entity_urn, group);
    }
    let spam = conversation.categories.iter().any(|c| c == "SPAM");
    let member = conversation
        .participants
        .iter()
        .any(|participant| ids::user_id(&participant.entity_urn) == me_id);
    if spam || !member {
        let portal = portal_of(
            shared,
            me_id,
            &conversation.entity_urn,
            conversation.group_chat,
        );
        return Some(Event::ChatRemoved(portal));
    }
    if !conversation.categories.iter().any(|c| c == "INBOX") {
        return None;
    }
    Some(Event::Chat(convert::chat_meta(conversation, me_id)))
}

/// The messages a reply can quote, by remote id, oldest evicted first.
#[derive(Debug, Default)]
struct ReplyCache {
    by_id: HashMap<String, QuotedMessage>,
    order: VecDeque<String>,
}

impl ReplyCache {
    fn remember(&mut self, message: &RemoteMessage) {
        if message.id.is_empty() || !message.part.is_empty() {
            return;
        }
        let quoted = QuotedMessage {
            sender_id: message.sender.id.clone(),
            sent_at_ms: message.timestamp_ms,
            message_id: message.id.clone(),
            text: message.content.body().to_owned(),
        };
        if self.by_id.insert(message.id.clone(), quoted).is_none() {
            self.order.push_back(message.id.clone());
        }
        while self.order.len() > REPLY_CACHE {
            if let Some(oldest) = self.order.pop_front() {
                self.by_id.remove(&oldest);
            }
        }
    }

    fn get(&self, id: &str) -> Option<&QuotedMessage> {
        self.by_id.get(id)
    }
}

/// One connected LinkedIn login.
pub struct LinkedInSession {
    api: Api,
    me_id: String,
    me: RemoteUser,
    events: mpsc::UnboundedReceiver<Event>,
    replies: ReplyCache,
}

impl LinkedInSession {
    fn mailbox(&self) -> Urn {
        Urn::parse(&ids::profile_urn(&self.me_id))
    }

    fn quoted(&self, reply_to: Option<&str>) -> Result<Option<QuotedMessage>> {
        match reply_to {
            None => Ok(None),
            Some(id) => self
                .replies
                .get(id)
                .cloned()
                .map(Some)
                .ok_or(Error::Unsupported(
                    "replying to a message from before this session",
                )),
        }
    }

    fn origin(txn_id: &str) -> String {
        if txn_id.is_empty() {
            uuid_v4()
        } else {
            txn_id.to_owned()
        }
    }

    async fn send_text(
        &self,
        portal: &PortalRef,
        text: &str,
        reply_to: Option<&str>,
        txn_id: &str,
    ) -> Result<Ack> {
        let quoted = self.quoted(reply_to)?;
        let payload = outbound::text_payload(
            &self.mailbox(),
            &portal.id,
            text,
            quoted.as_ref(),
            &Self::origin(txn_id),
        );
        let sent = self.api.create_message(&payload).await?;
        Ok(ack_of(&sent.entity_urn))
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_media(
        &self,
        portal: &PortalRef,
        data: &[u8],
        mime: &str,
        file_name: &str,
        caption: &str,
        kind: &str,
        reply_to: Option<&str>,
        txn_id: &str,
    ) -> Result<Ack> {
        let upload_type = outbound::upload_type(kind, mime, data.len())?;
        let quoted = self.quoted(reply_to)?;
        let asset = self
            .api
            .upload_asset(upload_type, file_name, mime, data)
            .await?;
        let render = outbound::attachment_render(kind, asset, file_name, mime, data.len());
        let payload = outbound::media_payload(
            &self.mailbox(),
            &portal.id,
            caption,
            render,
            quoted.as_ref(),
            &Self::origin(txn_id),
        );
        let sent = self.api.create_message(&payload).await?;
        Ok(ack_of(&sent.entity_urn))
    }
}

/// The acknowledgement of a created message: its remote id, when LinkedIn returned one.
fn ack_of(entity_urn: &str) -> Ack {
    Ack {
        remote_event_id: (!entity_urn.is_empty()).then(|| ids::message_id(entity_urn)),
    }
}

#[async_trait]
impl Session for LinkedInSession {
    async fn next(&mut self) -> Option<Event> {
        let event = self.events.recv().await?;
        if let Event::Message(message) = &event {
            self.replies.remember(message);
        }
        Some(event)
    }

    async fn command(&mut self, command: Command) -> Result<Ack> {
        match command {
            Command::SendText {
                portal,
                text,
                reply_to,
                txn_id,
            } => {
                self.send_text(&portal, &text, reply_to.as_deref(), &txn_id)
                    .await
            }
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
                    &portal,
                    &data,
                    &mime,
                    &file_name,
                    &caption,
                    &kind,
                    reply_to.as_deref(),
                    &txn_id,
                )
                .await
            }
            Command::Edit { target, text, .. } => {
                self.api.edit_message(&target, &text).await?;
                Ok(Ack::default())
            }
            Command::Redact { target, .. } => {
                self.api.recall_message(&target).await?;
                Ok(Ack::default())
            }
            Command::React { target, key, .. } => {
                self.api.react(&target, &key, true).await?;
                Ok(Ack::default())
            }
            Command::Unreact { target, key, .. } => {
                self.api.react(&target, &key, false).await?;
                Ok(Ack::default())
            }
            Command::Read { portal, .. } => {
                self.api.mark_read(&portal.id).await?;
                Ok(Ack::default())
            }
            Command::Typing { portal, typing } => {
                if typing {
                    self.api.start_typing(&portal.id).await?;
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
        let count = u32::try_from(limit.clamp(1, 100)).unwrap_or(50);
        let page = match cursor.as_deref().filter(|c| !c.is_empty()) {
            Some(prev) => self.api.messages_prev(&portal.id, prev, count).await?,
            None => {
                self.api
                    .messages_before(&portal.id, now_ms(), count)
                    .await?
            }
        };
        // A portal has no receiver exactly when it is a group chat.
        let group = portal.receiver.is_empty();
        let mut messages = page.elements;
        messages.sort_by_key(|message| message.delivered_at);
        let mut converted = Vec::new();
        for message in messages.iter().filter(|m| m.render_format != "RECALLED") {
            for part in convert::messages(&self.api, message, group, false, &self.me_id).await {
                self.replies.remember(&part);
                converted.push(part);
            }
        }
        let prev = page.metadata.prev_cursor;
        let has_more = !prev.is_empty() && !messages.is_empty();
        Ok(BackfillPage {
            messages: converted,
            cursor: has_more.then_some(prev),
            has_more,
        })
    }

    async fn logout_remote(&mut self) -> Result<()> {
        self.api.logout().await
    }

    fn user(&self) -> Option<RemoteUser> {
        Some(self.me.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_is_recognised_from_the_payload_or_what_was_seen() {
        let shared = Shared::default();
        assert!(!group_of(&shared, "c1", None));
        shared.record("c1", true);
        assert!(group_of(&shared, "c1", None));
        assert!(!group_of(&shared, "c1", Some(false)));
    }

    #[test]
    fn the_reply_cache_evicts_the_oldest_message() {
        let mut cache = ReplyCache::default();
        let message = |id: &str| RemoteMessage {
            portal: PortalRef::default(),
            id: id.to_owned(),
            part: String::new(),
            sender: RemoteUser::default(),
            timestamp_ms: 1,
            content: chirper_bridge::model::Content::Text {
                body: "hi".to_owned(),
                formatted: None,
            },
            reply_to: None,
            edit_of: None,
            live: true,
            from_me: false,
        };
        for n in 0..=REPLY_CACHE {
            cache.remember(&message(&format!("m{n}")));
        }
        assert!(cache.get("m0").is_none());
        assert!(cache.get(&format!("m{REPLY_CACHE}")).is_some());
    }
}
