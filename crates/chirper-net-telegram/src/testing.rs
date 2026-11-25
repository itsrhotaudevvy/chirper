//! Test doubles for the login and session code.
//!
//! [`FakeApi`] answers TL calls from canned objects, keyed by the constructor id of the request, and
//! records every request it received. [`ctx`] gives a login context with in-memory secrets and a
//! throwaway data directory, so a test never touches the network or the user's files.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chirper_bridge::{Error, LoginCtx, Result, Secrets};
use zeroize::Zeroizing;

use crate::client::{Api, DEFAULT_DC};
use crate::error::CallError;
use crate::tl::{Cursor, Deserializable, RemoteCall, Serializable, enums, types};

/// Secrets kept in memory, for one test login.
#[derive(Default)]
pub struct MemorySecrets {
    values: Mutex<HashMap<String, Vec<u8>>>,
}

impl Secrets for MemorySecrets {
    fn get(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        Ok(self
            .values
            .lock()
            .map_err(|_| Error::Internal("the test secrets are poisoned".into()))?
            .get(key)
            .cloned()
            .map(Zeroizing::new))
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<()> {
        self.values
            .lock()
            .map_err(|_| Error::Internal("the test secrets are poisoned".into()))?
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.values
            .lock()
            .map_err(|_| Error::Internal("the test secrets are poisoned".into()))?
            .remove(key);
        Ok(())
    }

    fn move_prefix(&self, from: &str, to: &str) -> Result<()> {
        let mut values = self
            .values
            .lock()
            .map_err(|_| Error::Internal("the test secrets are poisoned".into()))?;
        let moved: Vec<(String, Vec<u8>)> = values
            .iter()
            .filter_map(|(key, value)| {
                let suffix = key.strip_prefix(from)?;
                Some((format!("{to}{suffix}"), value.clone()))
            })
            .collect();
        for (key, value) in moved {
            values.insert(key, value);
        }
        values.retain(|key, _| !key.starts_with(from));
        Ok(())
    }
}

/// A login context for a test: in-memory secrets and a fresh directory under the system temp directory.
pub fn ctx(login_id: &str) -> LoginCtx {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let data_dir = std::env::temp_dir()
        .join("chirper-telegram-tests")
        .join(format!("{}-{login_id}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::create_dir_all(&data_dir);
    LoginCtx {
        network: crate::NETWORK_ID.to_owned(),
        login_id: login_id.to_owned(),
        data_dir,
        secrets: Arc::new(MemorySecrets::default()),
        http: chirper_bridge::http_client(),
    }
}

/// One request the fake received: its constructor id and its serialised body.
#[derive(Clone, Debug)]
pub struct Seen {
    /// The constructor id the request carried.
    pub constructor: u32,
    /// The whole request, constructor id included, as it would go on the wire.
    pub bytes: Vec<u8>,
}

impl Seen {
    /// True when the request carried `text` as a TL string, for asserting on what a caller sent.
    ///
    /// TL strings are raw UTF-8 with a length prefix, so this finds them without a schema reader.
    pub fn carries(&self, text: &str) -> bool {
        String::from_utf8_lossy(&self.bytes).contains(text)
    }
}

/// An [`Api`] that answers from queued objects and records what it was asked.
#[derive(Default)]
pub struct FakeApi {
    answers: Mutex<HashMap<u32, VecDeque<Queued>>>,
    seen: Mutex<Vec<Seen>>,
    home_dc: Mutex<Option<i32>>,
}

/// One queued answer: the bytes to hand back or the failure to report, and whether it repeats.
struct Queued {
    answer: Stored,
    repeat: bool,
}

/// The payload of a queued answer.
#[derive(Clone)]
enum Stored {
    Object(Vec<u8>),
    Fail(CallError),
}

impl FakeApi {
    /// A fake with nothing queued.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Queue `answer` as the reply to every request of `request`'s constructor id.
    pub fn answer_always<R: RemoteCall + Sync, T: Serializable>(&self, request: &R, answer: &T) {
        self.queue(request, Stored::Object(serialize(answer)), true);
    }

    /// Queue `value` as the reply to the next answer to the request with constructor id `id`. For
    /// answers whose type the request cannot be written for in a test.
    pub fn answer_to<T: Serializable>(&self, id: u32, value: &T) {
        self.push(id, Stored::Object(serialize(value)), false);
    }

    /// Queue already-framed bytes as the reply to the next request of this constructor id.
    pub fn answer_raw(&self, id: u32, bytes: Vec<u8>) {
        self.push(id, Stored::Object(bytes), false);
    }

    /// Make the next request of this constructor id fail with `error`.
    pub fn fail<R: RemoteCall + Sync>(&self, request: &R, error: CallError) {
        self.queue(request, Stored::Fail(error), false);
    }

    /// Every request the fake received, in order.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("the fake is not poisoned").clone()
    }

    /// The requests the fake received with this constructor id.
    pub fn seen_with(&self, constructor: u32) -> Vec<Seen> {
        self.seen()
            .into_iter()
            .filter(|seen| seen.constructor == constructor)
            .collect()
    }

    fn queue<R: RemoteCall + Sync>(&self, request: &R, answer: Stored, repeat: bool) {
        self.push(constructor_id_of(&serialize(request)), answer, repeat);
    }

    fn push(&self, id: u32, answer: Stored, repeat: bool) {
        self.answers
            .lock()
            .expect("the fake is not poisoned")
            .entry(id)
            .or_default()
            .push_back(Queued { answer, repeat });
    }
}

#[async_trait]
impl Api for FakeApi {
    async fn call<R>(&self, request: &R) -> std::result::Result<R::Return, CallError>
    where
        R: RemoteCall + Sync,
        R::Return: Send,
    {
        // A real call suspends on the network; yielding here gives `next()` a point to be dropped at.
        tokio::task::yield_now().await;
        let bytes = serialize(request);
        let id = constructor_id_of(&bytes);
        self.seen
            .lock()
            .expect("the fake is not poisoned")
            .push(Seen {
                constructor: id,
                bytes: bytes.to_vec(),
            });
        let mut answers = self.answers.lock().expect("the fake is not poisoned");
        let Some(queue) = answers.get_mut(&id) else {
            return Err(CallError::Protocol(format!(
                "the test queued no answer for constructor {id:#010x}"
            )));
        };
        // A repeating answer stays queued; a one-shot answer is taken.
        let answer = if queue.front().is_some_and(|entry| entry.repeat) {
            queue.front().map(|entry| entry.answer.clone())
        } else {
            queue.pop_front().map(|entry| entry.answer)
        };
        let Some(answer) = answer else {
            return Err(CallError::Protocol(format!(
                "the test queued no answer for constructor {id:#010x}"
            )));
        };
        match &answer {
            Stored::Fail(error) => Err(error.clone()),
            Stored::Object(bytes) => {
                R::Return::deserialize(&mut Cursor::new(bytes)).map_err(|error| {
                    CallError::Protocol(format!(
                        "the test's answer to {id:#010x} does not parse: {error:?}"
                    ))
                })
            }
        }
    }

    async fn call_on<R>(&self, _dc: i32, request: &R) -> std::result::Result<R::Return, CallError>
    where
        R: RemoteCall + Sync,
        R::Return: Send,
    {
        self.call(request).await
    }

    async fn authorize_dc(&self, _dc: i32) -> std::result::Result<(), CallError> {
        Ok(())
    }

    fn home_dc(&self) -> i32 {
        self.home_dc
            .lock()
            .expect("the fake is not poisoned")
            .unwrap_or(DEFAULT_DC)
    }

    fn set_home_dc(&self, dc: i32) {
        *self.home_dc.lock().expect("the fake is not poisoned") = Some(dc);
    }
}

/// The TL bytes of anything serialisable.
pub fn serialize<T: Serializable>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    value.serialize(&mut bytes);
    bytes
}

/// The constructor id at the front of a serialised object.
pub fn constructor_id_of(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// An account of the fake: `self_` set, with a first name and a phone number.
pub fn user(id: i64) -> types::User {
    types::User {
        self_: true,
        contact: false,
        mutual_contact: false,
        deleted: false,
        bot: false,
        bot_chat_history: false,
        bot_nochats: false,
        verified: false,
        restricted: false,
        min: false,
        bot_inline_geo: false,
        support: false,
        scam: false,
        apply_min_photo: false,
        fake: false,
        bot_attach_menu: false,
        premium: false,
        attach_menu_enabled: false,
        bot_can_edit: false,
        close_friend: false,
        stories_hidden: false,
        stories_unavailable: false,
        contact_require_premium: false,
        bot_business: false,
        bot_has_main_app: false,
        bot_forum_view: false,
        bot_forum_can_manage_topics: false,
        bot_can_manage_bots: false,
        bot_guestchat: false,
        bot_guard: false,
        bot_forum_sidebar: false,
        id,
        access_hash: Some(42),
        first_name: Some("Ada".to_owned()),
        last_name: None,
        username: Some("ada".to_owned()),
        phone: Some("15550001".to_owned()),
        photo: None,
        status: None,
        bot_info_version: None,
        restriction_reason: None,
        bot_inline_placeholder: None,
        lang_code: None,
        emoji_status: None,
        usernames: None,
        stories_max_id: None,
        color: None,
        profile_color: None,
        bot_active_users: None,
        bot_verification_icon: None,
        send_paid_messages_stars: None,
        linked_community_id: None,
    }
}

/// A message in the private chat with `peer`: `from` wrote it at `date`, and `out` marks the local
/// user as its author. In a private chat Telegram sets the peer to the other person either way.
pub fn text_message(
    id: i32,
    from: i64,
    peer: i64,
    body: &str,
    date: i32,
    out: bool,
) -> types::Message {
    types::Message {
        out,
        mentioned: false,
        media_unread: false,
        silent: false,
        post: false,
        from_scheduled: false,
        legacy: false,
        edit_hide: false,
        pinned: false,
        noforwards: false,
        invert_media: false,
        offline: false,
        video_processing_pending: false,
        paid_suggested_post_stars: false,
        paid_suggested_post_ton: false,
        id,
        from_id: Some(Box::new(enums::Peer::PeerUser(types::PeerUser {
            user_id: from,
        }))),
        from_boosts_applied: None,
        from_rank: None,
        peer_id: Box::new(enums::Peer::PeerUser(types::PeerUser { user_id: peer })),
        saved_peer_id: None,
        fwd_from: None,
        via_bot_id: None,
        via_business_bot_id: None,
        guestchat_via_from: None,
        reply_to: None,
        date,
        message: body.to_owned(),
        media: None,
        reply_markup: None,
        entities: None,
        views: None,
        forwards: None,
        replies: None,
        edit_date: None,
        post_author: None,
        grouped_id: None,
        reactions: None,
        restriction_reason: None,
        ttl_period: None,
        quick_reply_shortcut_id: None,
        effect: None,
        factcheck: None,
        report_delivery_until_date: None,
        paid_message_stars: None,
        suggested_post: None,
        schedule_repeat_period: None,
        summary_from_language: None,
        rich_message: None,
    }
}

/// One private chat with `peer`, with `top_message` as its last message.
pub fn dialog(peer: i64, top_message: i32) -> types::Dialog {
    types::Dialog {
        pinned: false,
        unread_mark: false,
        view_forum_as_messages: false,
        peer: Box::new(enums::Peer::PeerUser(types::PeerUser { user_id: peer })),
        top_message,
        read_inbox_max_id: 0,
        read_outbox_max_id: 0,
        unread_count: 0,
        unread_mentions_count: 0,
        unread_reactions_count: 0,
        unread_poll_votes_count: 0,
        notify_settings: Box::new(enums::PeerNotifySettings::PeerNotifySettings(
            types::PeerNotifySettings {
                show_previews: None,
                silent: None,
                mute_until: None,
                ios_sound: None,
                android_sound: None,
                other_sound: None,
                stories_muted: None,
                stories_hide_sender: None,
                stories_ios_sound: None,
                stories_android_sound: None,
                stories_other_sound: None,
            },
        )),
        pts: Some(1),
        draft: None,
        folder_id: None,
        ttl_period: None,
    }
}

/// The update state a fresh connection starts from.
pub fn state(pts: i32) -> types::updates::State {
    types::updates::State {
        pts,
        qts: 0,
        date: 1_700_000_000,
        seq: 1,
        unread_count: 0,
    }
}

/// Pushes carrying `updates`, as the server sends them.
pub fn updates(updates: Vec<enums::Update>, users: Vec<enums::User>) -> types::Updates {
    types::Updates {
        updates,
        users,
        chats: Vec::new(),
        date: 1_700_000_100,
        seq: 2,
    }
}

/// A login code sent by SMS.
pub fn sent_code_sms(hash: &str) -> enums::auth::SentCode {
    enums::auth::SentCode::SentCode(types::auth::SentCode {
        r#type: Box::new(enums::auth::SentCodeType::SentCodeTypeSms(
            types::auth::SentCodeTypeSms { length: 5 },
        )),
        phone_code_hash: hash.to_owned(),
        next_type: None,
        timeout: None,
    })
}

/// The authorization of `id`, as `auth.signIn` answers it.
pub fn authorization(id: i64) -> enums::auth::Authorization {
    enums::auth::Authorization::Authorization(types::auth::Authorization {
        setup_password_required: false,
        otherwise_relogin_days: None,
        tmp_sessions: None,
        future_auth_token: None,
        user: Box::new(enums::User::User(user(id))),
    })
}

/// A user of the fake's address book: not `self_`, with an access hash the store can use.
pub fn peer_user(id: i64) -> types::User {
    let mut other = user(id);
    other.self_ = false;
    other.username = None;
    other.first_name = Some(format!("Peer {id}"));
    other
}

/// A `Vector<T>` answer, framed the way the schema writes a boxed vector.
pub fn vector<T: Serializable>(items: &[T]) -> Vec<u8> {
    let mut bytes = Vec::new();
    crate::tl::write_vector_header(&mut bytes, items.len());
    for item in items {
        item.serialize(&mut bytes);
    }
    bytes
}
