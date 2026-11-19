//! One live SMS login: the provider is watched for changes, new messages are read since a
//! persisted watermark, and sends are tracked to their verdict.
//!
//! Every `Session` method is cancel-safe: the runtime drops `next()` whenever a command arrives.
//! Work that awaits is done before any state changes, so a dropped call simply runs again.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use log::{debug, warn};
use tokio::sync::mpsc;
use tokio::time::Instant;

use chirper_bridge::model::{
    Ack, BackfillPage, ChatMeta, Content, Event, MediaInfo, PortalRef, RemoteMessage, RemoteUser,
    SessionState,
};
use chirper_bridge::{Command, Error, LoginCtx, Result, Session};

use crate::map;
use crate::platform::{MmsPart, SmsMessage, SmsPlatform, SmsPlatformEvent, SmsThread};

/// The secret that stores the watermark, under the login's namespace.
pub const WATERMARK_KEY: &str = "watermark";
/// How long the provider must stay quiet before it is queried.
const DEBOUNCE: Duration = Duration::from_millis(300);
/// The least time between two watermark writes.
const PERSIST_EVERY: Duration = Duration::from_secs(5);
/// The wait before a failed query is retried.
const RETRY_AFTER: Duration = Duration::from_secs(5);
/// Messages per catch-up query.
const CATCH_UP_PAGE: usize = 200;
/// Conversations read at connect and when a new one appears.
pub const THREAD_PAGE: usize = 500;
/// The largest MMS part that is read into memory.
pub const MAX_MMS_BYTES: u64 = 50 << 20;
/// How far back a first connect looks.
const LOOKBACK_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// The most messages one backfill page returns.
const BACKFILL_MAX: usize = 100;
/// The most send ids kept while their verdict is pending.
const MAX_PENDING_SENDS: usize = 512;

/// Runs a blocking platform call on the blocking pool.
pub async fn blocking<T, F>(work: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| Error::Internal(format!("the SMS task failed: {e}")))?
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The watermark stored for this login, or a week back when none is stored yet.
pub fn load_watermark(ctx: &LoginCtx) -> Result<i64> {
    let default = now_ms().saturating_sub(LOOKBACK_MS);
    Ok(match ctx.get_secret(WATERMARK_KEY)? {
        Some(bytes) => std::str::from_utf8(&bytes)
            .ok()
            .and_then(|text| text.trim().parse::<i64>().ok())
            .unwrap_or(default),
        None => default,
    })
}

/// The live SMS session.
pub struct SmsSession {
    platform: Arc<dyn SmsPlatform>,
    ctx: LoginCtx,
    events: mpsc::UnboundedReceiver<SmsPlatformEvent>,
    /// Events ready to hand to the runtime.
    outbox: VecDeque<Event>,
    /// Thread id to portal, for every conversation seen.
    threads: HashMap<i64, PortalRef>,
    /// Portal id to thread id, for backfill.
    portal_threads: HashMap<String, i64>,
    /// Portal ids already announced with `Event::Chat`.
    known: HashSet<String>,
    /// Messages at or after the watermark that were already emitted, by id, with their date.
    recent: HashMap<String, i64>,
    watermark: i64,
    /// When the watermark was last written; `None` before the first write.
    persisted_at: Option<Instant>,
    /// The watermark moved since it was last written.
    unsaved: bool,
    /// Whether the first catch-up (the history since the watermark) has completed.
    caught_up: bool,
    /// A catch-up is needed.
    pending: bool,
    /// The earliest time the pending catch-up may run.
    due: Option<Instant>,
    /// Sends waiting for their verdict: send id to portal and transaction id.
    sends: HashMap<String, (PortalRef, String)>,
}

impl SmsSession {
    /// Checks the permissions, reads the conversations, starts watching the provider and queues
    /// the first catch-up.
    pub async fn open(platform: Arc<dyn SmsPlatform>, ctx: LoginCtx) -> Result<Self> {
        let permissions = {
            let platform = platform.clone();
            blocking(move || platform.permissions()).await?
        };
        if !permissions.read {
            return Err(Error::Auth("SMS permission was revoked".to_owned()));
        }
        let watermark = load_watermark(&ctx)?;
        let (tx, rx) = mpsc::unbounded_channel();
        let sink: Arc<dyn Fn(SmsPlatformEvent) + Send + Sync> = Arc::new(move |event| {
            let _ = tx.send(event);
        });
        let threads = {
            let platform = platform.clone();
            blocking(move || platform.threads(THREAD_PAGE)).await?
        };
        let mut session = Self {
            platform: platform.clone(),
            ctx,
            events: rx,
            outbox: VecDeque::new(),
            threads: HashMap::new(),
            portal_threads: HashMap::new(),
            known: HashSet::new(),
            recent: HashMap::new(),
            watermark,
            persisted_at: None,
            unsaved: false,
            caught_up: false,
            pending: true,
            due: None,
            sends: HashMap::new(),
        };
        session.outbox.push_back(Event::State {
            state: SessionState::Running,
            error: String::new(),
            message: String::new(),
            profile: None,
        });
        for thread in &threads {
            if let Some(meta) = session.learn(thread) {
                session.outbox.push_back(Event::Chat(meta));
            }
        }
        {
            let platform = platform.clone();
            blocking(move || platform.watch(sink)).await?;
        }
        Ok(session)
    }

    /// Records a conversation. Returns its chat metadata when the portal was not known before.
    fn learn(&mut self, thread: &SmsThread) -> Option<ChatMeta> {
        let id = map::portal_id(&thread.addresses);
        if id.is_empty() {
            return None;
        }
        let portal = map::portal_ref(id.clone());
        let meta = map::chat_meta(portal.clone(), thread);
        self.threads.insert(thread.thread_id, portal);
        self.portal_threads.insert(id.clone(), thread.thread_id);
        self.known.insert(id).then_some(meta)
    }

    fn on_platform(&mut self, event: SmsPlatformEvent) {
        match event {
            SmsPlatformEvent::Changed => {
                self.pending = true;
                if self.due.is_none() {
                    self.due = Some(Instant::now() + DEBOUNCE);
                }
            }
            SmsPlatformEvent::SendStatus {
                send_id,
                status,
                error,
            } => {
                let Some(verdict) = map::delivery_status(&status) else {
                    debug!("sms: an unknown send status arrived");
                    return;
                };
                let terminal = matches!(
                    verdict,
                    chirper_bridge::DeliveryStatus::Delivered
                        | chirper_bridge::DeliveryStatus::Failed
                );
                let entry = if terminal {
                    self.sends.remove(&send_id)
                } else {
                    self.sends.get(&send_id).cloned()
                };
                if let Some((portal, txn_id)) = entry {
                    self.outbox.push_back(Event::Delivery {
                        portal,
                        txn_id,
                        status: verdict,
                        error,
                    });
                }
            }
        }
    }

    /// Reads the messages since the watermark and emits the new ones. Nothing is changed until
    /// every read has succeeded, so a failure leaves the state as it was.
    async fn catch_up(&mut self) -> Result<()> {
        let live = self.caught_up;
        let after = self.watermark;
        let platform = self.platform.clone();
        let mut page = blocking(move || platform.messages_since(after, CATCH_UP_PAGE)).await?;
        page.sort_by_key(|message| message.date_ms);
        let full = page.len() >= CATCH_UP_PAGE;

        // Conversations this page refers to that are not known yet.
        let unknown = page
            .iter()
            .any(|message| !self.threads.contains_key(&message.thread_id));
        let fresh: Vec<SmsThread> = if unknown {
            let platform = self.platform.clone();
            blocking(move || platform.threads(THREAD_PAGE)).await?
        } else {
            Vec::new()
        };

        // The watermark may not pass a message that is still sending: it is emitted once it
        // settles, and it must still be inside the query window then.
        let mut settled_max: Option<i64> = None;
        let mut sending_min: Option<i64> = None;
        let mut entries: Vec<(String, i64, PortalRef, Vec<RemoteMessage>)> = Vec::new();
        for message in &page {
            if message.status == "sending" {
                sending_min = Some(sending_min.map_or(message.date_ms, |m| m.min(message.date_ms)));
                continue;
            }
            settled_max = Some(settled_max.map_or(message.date_ms, |m| m.max(message.date_ms)));
            if self.recent.contains_key(&message.id) {
                continue;
            }
            let Some(portal) = self.portal_for(message.thread_id, &fresh) else {
                warn!("sms: a message belongs to an unknown conversation");
                continue;
            };
            let remote = self.expand(message, &portal, live).await?;
            entries.push((message.id.clone(), message.date_ms, portal, remote));
        }

        // Phase two: no awaits, so the state changes in one piece.
        for thread in &fresh {
            if let Some(meta) = self.learn(thread) {
                self.outbox.push_back(Event::Chat(meta));
            }
        }
        for (id, date, _portal, remote) in entries {
            for message in remote {
                self.outbox.push_back(Event::Message(message));
            }
            self.recent.insert(id, date);
        }
        let mut next = self.watermark;
        if let Some(max) = settled_max {
            next = next.max(max);
        }
        if let Some(min) = sending_min {
            next = next.min(min.saturating_sub(1)).max(self.watermark);
        }
        if next != self.watermark {
            self.watermark = next;
            self.recent.retain(|_, date| *date >= next);
            self.persist_watermark();
        }
        self.caught_up = true;
        self.pending = full;
        self.due = None;
        Ok(())
    }

    /// The portal of a thread, from the known threads or the ones just read.
    fn portal_for(&self, thread_id: i64, fresh: &[SmsThread]) -> Option<PortalRef> {
        if let Some(portal) = self.threads.get(&thread_id) {
            return Some(portal.clone());
        }
        fresh
            .iter()
            .find(|thread| thread.thread_id == thread_id)
            .map(|thread| map::portal_ref(map::portal_id(&thread.addresses)))
            .filter(|portal| !portal.id.is_empty())
    }

    /// The runtime's messages for one provider message: the text, then one per media part.
    async fn expand(
        &self,
        message: &SmsMessage,
        portal: &PortalRef,
        live: bool,
    ) -> Result<Vec<RemoteMessage>> {
        let mut out = Vec::new();
        if message.parts.is_empty() || !message.body.is_empty() {
            out.push(self.remote(
                message,
                portal,
                String::new(),
                live,
                Content::Text {
                    body: message.body.clone(),
                    formatted: None,
                },
            ));
        }
        for part in &message.parts {
            let content = self.media(part).await?;
            out.push(self.remote(message, portal, part.part_id.to_string(), live, content));
        }
        Ok(out)
    }

    fn remote(
        &self,
        message: &SmsMessage,
        portal: &PortalRef,
        part: String,
        live: bool,
        content: Content,
    ) -> RemoteMessage {
        RemoteMessage {
            portal: portal.clone(),
            id: message.id.clone(),
            part,
            sender: map::sender_of(message),
            timestamp_ms: message.date_ms,
            content,
            reply_to: None,
            edit_of: None,
            live,
            from_me: message.from_me,
        }
    }

    /// The bytes of one MMS part. A part that cannot be read becomes a placeholder, except when
    /// the permission is gone, which ends the login.
    async fn media(&self, part: &MmsPart) -> Result<Content> {
        let platform = self.platform.clone();
        let part_id = part.part_id;
        match blocking(move || platform.mms_part(part_id, MAX_MMS_BYTES)).await {
            Ok(data) => Ok(Content::Media {
                kind: map::media_kind(&part.mime).to_owned(),
                caption: String::new(),
                info: MediaInfo {
                    mime: part.mime.clone(),
                    file_name: part.name.clone(),
                    size: data.len(),
                    ..Default::default()
                },
                data: Arc::new(data),
                thumbnail: None,
            }),
            Err(err @ Error::Auth(_)) => Err(err),
            Err(err) => {
                debug!("sms: an attachment could not be read: {err}");
                Ok(Content::Unsupported {
                    body: "Attachment could not be loaded".to_owned(),
                })
            }
        }
    }

    /// Writes the watermark at most every [`PERSIST_EVERY`]; a later write or the drop of the
    /// session writes what is left.
    fn persist_watermark(&mut self) {
        self.unsaved = true;
        let due = self
            .persisted_at
            .is_none_or(|at| at.elapsed() >= PERSIST_EVERY);
        if due {
            self.write_watermark();
        }
    }

    fn write_watermark(&mut self) {
        match self
            .ctx
            .set_secret(WATERMARK_KEY, self.watermark.to_string().as_bytes())
        {
            Ok(()) => {
                self.persisted_at = Some(Instant::now());
                self.unsaved = false;
            }
            Err(err) => warn!("sms: cannot save the watermark: {err}"),
        }
    }

    fn catch_up_failed(&mut self, err: Error) {
        match err {
            Error::Auth(message) => {
                self.pending = false;
                self.due = None;
                self.outbox.push_back(Event::State {
                    state: SessionState::NeedsReauth,
                    error: message,
                    message: String::new(),
                    profile: None,
                });
            }
            other => {
                warn!("sms: cannot read the provider: {other}");
                self.due = Some(Instant::now() + RETRY_AFTER);
            }
        }
    }

    /// The portal a backfill belongs to, read again from the provider when it is not known.
    async fn thread_of(&mut self, portal: &PortalRef) -> Result<i64> {
        if let Some(thread) = self.portal_threads.get(&portal.id) {
            return Ok(*thread);
        }
        let platform = self.platform.clone();
        let fresh = blocking(move || platform.threads(THREAD_PAGE)).await?;
        for thread in &fresh {
            if let Some(meta) = self.learn(thread) {
                self.outbox.push_back(Event::Chat(meta));
            }
        }
        self.portal_threads
            .get(&portal.id)
            .copied()
            .ok_or(Error::Protocol(
                "the SMS conversation is unknown".to_owned(),
            ))
    }
}

impl Drop for SmsSession {
    fn drop(&mut self) {
        if self.unsaved {
            self.write_watermark();
        }
    }
}

async fn wait_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending::<()>().await,
    }
}

#[async_trait]
impl Session for SmsSession {
    async fn next(&mut self) -> Option<Event> {
        loop {
            if let Some(event) = self.outbox.pop_front() {
                return Some(event);
            }
            if self.pending && self.due.is_none_or(|due| due <= Instant::now()) {
                if let Err(err) = self.catch_up().await {
                    self.catch_up_failed(err);
                }
                continue;
            }
            let wake = if self.pending { self.due } else { None };
            tokio::select! {
                event = self.events.recv() => {
                    let event = event?;
                    self.on_platform(event);
                }
                () = wait_until(wake) => {}
            }
        }
    }

    async fn command(&mut self, command: Command) -> Result<Ack> {
        match command {
            Command::SendText {
                portal,
                text,
                txn_id,
                ..
            } => {
                let addresses = map::portal_addresses(&portal.id);
                if addresses.is_empty() {
                    return Err(Error::Protocol(
                        "the SMS conversation has no address".to_owned(),
                    ));
                }
                let platform = self.platform.clone();
                let send_id = blocking(move || platform.send_text(&addresses, &text, None)).await?;
                if self.sends.len() >= MAX_PENDING_SENDS {
                    self.sends.clear();
                }
                self.sends.insert(send_id, (portal, txn_id));
                // The provider row appears later, with its own id, through a change.
                Ok(Ack::default())
            }
            Command::SendMedia { .. } => Err(Error::Unsupported("sending MMS")),
            _ => Err(Error::Unsupported("this action on SMS")),
        }
    }

    async fn backfill(
        &mut self,
        portal: &PortalRef,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<BackfillPage> {
        let thread_id = self.thread_of(portal).await?;
        let before = match cursor {
            Some(text) => Some(text.parse::<i64>().map_err(|_| {
                Error::Protocol("the SMS history cursor is not a number".to_owned())
            })?),
            None => None,
        };
        let size = limit.min(BACKFILL_MAX);
        let platform = self.platform.clone();
        let mut page = blocking(move || platform.messages(thread_id, before, size)).await?;
        page.sort_by_key(|message| message.date_ms);
        let oldest = page.iter().map(|message| message.date_ms).min();
        let has_more = size > 0 && page.len() == size && oldest.is_some();
        let mut messages = Vec::new();
        for message in &page {
            messages.extend(self.expand(message, portal, false).await?);
        }
        Ok(BackfillPage {
            messages,
            cursor: if has_more {
                oldest.map(|date| date.to_string())
            } else {
                None
            },
            has_more,
        })
    }

    async fn logout_remote(&mut self) -> Result<()> {
        Err(Error::Unsupported("logging out of SMS remotely"))
    }

    fn user(&self) -> Option<RemoteUser> {
        None
    }
}
