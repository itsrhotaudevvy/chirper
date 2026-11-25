//! The live session of one Google Voice login: the first sync of the thread list, the realtime
//! loop that re-lists threads when they change, outgoing commands, and history pages.
//!
//! Ported from `pkg/connector/{client,handlegvoice,handlematrix}.go` of mautrix-gvoice (AGPL-3.0).
//! The reference's bridge database (message dedupe, portal creation, thread-sync progress) is the
//! runtime's job here: this module keeps only what it needs to report each message once, namely
//! the newest timestamp it has reported per thread.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;

use chirper_bridge::model::{
    Ack, BackfillPage, DeliveryStatus, Event, PortalRef, RemoteMessage, RemoteUser, SessionState,
};
use chirper_bridge::{Command, Error, Result, Session};

use crate::api::Api;
use crate::convert::{self, Account};
use crate::realtime::{self, Signal};
use crate::wire::Msg;

/// Threads listed on the first sync, at most.
const MAX_INITIAL_THREADS: usize = 500;
/// The thread list is re-read this often even without a realtime signal.
const REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);
/// A realtime signal waits this long before the list is read, so a burst costs one read.
const REFRESH_DELAY: Duration = Duration::from_millis(500);
/// Messages asked for per history page; Google Voice serves at most this many per thread page.
const HISTORY_PAGE: i64 = 100;
/// History pages read for one backfill request, at most.
const MAX_HISTORY_PAGES: usize = 20;
/// `ReqSendSMS.Media.Type` values for the image types the network takes.
const IMAGE_TYPES: [(&str, i64); 5] = [
    ("image/jpeg", 1),
    ("image/png", 2),
    ("image/tiff", 3),
    ("image/gif", 4),
    ("image/bmp", 5),
];

/// The live session of one login. Events come from the background task started by [`start`].
pub struct GVoiceSession {
    api: Api,
    account: Account,
    me: RemoteUser,
    events: mpsc::UnboundedReceiver<Event>,
    /// Events the session produced itself (delivery verdicts), handed out before the channel.
    pending: VecDeque<Event>,
}

/// Start the background task of a connected login and return the session that reads its events.
pub fn start(api: Api, account: Account) -> GVoiceSession {
    let (tx, events) = mpsc::unbounded_channel();
    let me = account.me();
    let sync = Sync {
        api: api.clone(),
        account: account.clone(),
        tx,
        seen: HashMap::new(),
        known: HashSet::new(),
        live: false,
    };
    tokio::spawn(run(sync, me.clone()));
    GVoiceSession {
        api,
        account,
        me,
        events,
        pending: VecDeque::new(),
    }
}

/// The state the background task keeps: what it has reported, and where to report it.
struct Sync {
    api: Api,
    account: Account,
    tx: mpsc::UnboundedSender<Event>,
    /// Newest message timestamp reported per thread id.
    seen: HashMap<String, i64>,
    /// Thread ids whose chat has been reported.
    known: HashSet<String>,
    /// False while the first sync runs: its messages are history, not live.
    live: bool,
}

impl Sync {
    fn emit(&self, event: Event) {
        // A closed channel means the session was dropped; there is nobody left to tell.
        let _ = self.tx.send(event);
    }

    fn state(&self, state: SessionState, error: String, profile: Option<RemoteUser>) {
        self.emit(Event::State {
            state,
            error,
            message: String::new(),
            profile,
        });
    }

    /// Read every thread page, up to the initial limit, and report it.
    async fn first_sync(&mut self) -> Result<()> {
        let mut token = String::new();
        let mut threads = 0usize;
        loop {
            let page = self.api.list_threads(&token, "").await?;
            for thread in page.messages(1) {
                if threads >= MAX_INITIAL_THREADS {
                    return Ok(());
                }
                self.thread(thread).await;
                threads += 1;
            }
            token = page.string(2);
            if token.is_empty() {
                return Ok(());
            }
        }
    }

    /// Read the first thread page and report what is new in it.
    async fn refresh(&mut self) -> Result<()> {
        let page = self.api.list_threads("", "").await?;
        for thread in page.messages(1) {
            self.thread(thread).await;
        }
        Ok(())
    }

    /// Report a thread's chat if it is new, and every message newer than the last one reported.
    async fn thread(&mut self, raw: &Msg) {
        let messages: Vec<&Msg> = raw.messages(3).collect();
        if messages.is_empty() {
            return;
        }
        let thread_id = raw.string(1);
        let portal = self.account.portal(raw);
        if self.known.insert(thread_id.clone()) {
            self.emit(Event::Chat(self.account.chat(portal.clone(), raw)));
        }
        let previous = self.seen.get(&thread_id).copied();
        let mut newest = previous;
        // The list is newest first; report oldest first so the store keeps the order.
        for message in messages.iter().rev() {
            let timestamp = message.int(2);
            if previous.is_some_and(|seen| timestamp <= seen) {
                continue;
            }
            for part in
                convert::messages(&self.api, &self.account, &portal, message, self.live).await
            {
                self.emit(Event::Message(part));
            }
            newest = Some(newest.map_or(timestamp, |seen| seen.max(timestamp)));
        }
        if let Some(newest) = newest {
            self.seen.insert(thread_id, newest);
        }
    }

    /// Report the end of the session: the state the failure leaves the login in.
    fn end(&self, err: &Error) {
        self.state(err.session_state(), err.to_string(), None);
    }
}

/// The background task: connect, sync, then follow the realtime channel until it fails.
async fn run(mut sync: Sync, me: RemoteUser) {
    sync.state(SessionState::Connecting, String::new(), None);
    if let Err(err) = sync.first_sync().await {
        sync.end(&err);
        return;
    }
    sync.live = true;
    sync.state(SessionState::Running, String::new(), Some(me.clone()));

    let (signals_tx, mut signals) = mpsc::unbounded_channel();
    let channel_api = sync.api.clone();
    let mut channel = tokio::spawn(async move {
        realtime::run(&channel_api, &mut |signal| {
            let _ = signals_tx.send(signal);
        })
        .await
    });
    let mut refresh = tokio::time::interval(REFRESH_INTERVAL);
    // The first tick of an interval is immediate, and the first sync has just run.
    refresh.tick().await;

    let err = loop {
        tokio::select! {
            Some(signal) = signals.recv() => match signal {
                Signal::Connected => {
                    sync.state(SessionState::Running, String::new(), Some(me.clone()));
                }
                Signal::NewMessages => {
                    tokio::time::sleep(REFRESH_DELAY).await;
                    if let Err(err) = sync.refresh().await {
                        if matches!(err, Error::Auth(_)) {
                            break err;
                        }
                        sync.emit(Event::Error(err.to_string()));
                    }
                }
            },
            _ = refresh.tick() => {
                if let Err(err) = sync.refresh().await {
                    if matches!(err, Error::Auth(_)) {
                        break err;
                    }
                    sync.emit(Event::Error(err.to_string()));
                }
            }
            joined = &mut channel => {
                break match joined {
                    Ok(Err(err)) => err,
                    Ok(Ok(())) => Error::Network("the realtime channel stopped".to_owned()),
                    Err(_) => Error::Internal("the realtime task stopped unexpectedly".to_owned()),
                };
            }
        }
    };
    channel.abort();
    sync.end(&err);
}

#[async_trait]
impl Session for GVoiceSession {
    async fn next(&mut self) -> Option<Event> {
        if let Some(event) = self.pending.pop_front() {
            return Some(event);
        }
        self.events.recv().await
    }

    async fn command(&mut self, command: Command) -> Result<Ack> {
        match command {
            Command::SendText {
                portal,
                text,
                reply_to,
                txn_id,
            } => {
                refuse_reply(reply_to.as_deref())?;
                let reply = self.api.send_text(&portal.id, &text).await?;
                Ok(self.accepted(&portal, txn_id, &reply))
            }
            Command::SendMedia {
                portal,
                data,
                mime,
                kind,
                caption,
                reply_to,
                txn_id,
                ..
            } => {
                refuse_reply(reply_to.as_deref())?;
                let media = image_type(&mime)
                    .filter(|_| kind == "image")
                    .ok_or(Error::Unsupported("sending this kind of file"))?;
                let reply = self
                    .api
                    .send_image(&portal.id, media, &data, &caption)
                    .await?;
                Ok(self.accepted(&portal, txn_id, &reply))
            }
            Command::Edit { .. } => Err(Error::Unsupported("editing messages")),
            Command::Redact { .. } => Err(Error::Unsupported("deleting messages")),
            Command::React { .. } | Command::Unreact { .. } => Err(Error::Unsupported("reactions")),
            Command::Read { portal, .. } => {
                self.api.mark_read(&portal.id).await?;
                Ok(Ack::default())
            }
            Command::Typing { .. } => Err(Error::Unsupported("typing notifications")),
        }
    }

    async fn backfill(
        &mut self,
        portal: &PortalRef,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<BackfillPage> {
        let mut token = cursor.unwrap_or_default();
        // Newest first, as the network serves them.
        let mut newest_first: Vec<Msg> = Vec::new();
        let mut pages = 0usize;
        loop {
            let thread = self
                .api
                .get_thread(&portal.id, HISTORY_PAGE, &token)
                .await?;
            newest_first.extend(thread.messages(3).cloned());
            token = thread.string(4);
            pages += 1;
            if token.is_empty() || newest_first.len() >= limit || pages >= MAX_HISTORY_PAGES {
                break;
            }
        }
        let mut groups = Vec::with_capacity(newest_first.len());
        for raw in &newest_first {
            groups.push(convert::messages(&self.api, &self.account, portal, raw, false).await);
        }
        let messages: Vec<RemoteMessage> = groups.into_iter().rev().flatten().collect();
        let has_more = !token.is_empty();
        Ok(BackfillPage {
            messages,
            cursor: has_more.then_some(token),
            has_more,
        })
    }

    async fn logout_remote(&mut self) -> Result<()> {
        Err(Error::Unsupported("logging out of Google Voice remotely"))
    }

    fn user(&self) -> Option<RemoteUser> {
        Some(self.me.clone())
    }
}

impl GVoiceSession {
    /// The acknowledgement of an accepted send: the delivery is reported as sent, and the id of
    /// the new message is returned to the engine.
    fn accepted(&mut self, portal: &PortalRef, txn_id: String, reply: &Msg) -> Ack {
        self.pending.push_back(Event::Delivery {
            portal: portal.clone(),
            txn_id,
            status: DeliveryStatus::Sent,
            error: String::new(),
        });
        let id = reply.string(3);
        Ack {
            remote_event_id: (!id.is_empty()).then_some(id),
        }
    }
}

fn refuse_reply(reply_to: Option<&str>) -> Result<()> {
    match reply_to {
        Some(_) => Err(Error::Unsupported("replies")),
        None => Ok(()),
    }
}

/// The `ReqSendSMS.Media.Type` of an image type the network takes.
fn image_type(mime: &str) -> Option<i64> {
    IMAGE_TYPES
        .iter()
        .find(|(known, _)| *known == mime)
        .map(|(_, value)| *value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_types_follow_the_reference_enum() {
        assert_eq!(image_type("image/jpeg"), Some(1));
        assert_eq!(image_type("image/png"), Some(2));
        assert_eq!(image_type("image/tiff"), Some(3));
        assert_eq!(image_type("image/gif"), Some(4));
        assert_eq!(image_type("image/bmp"), Some(5));
        assert_eq!(image_type("image/webp"), None);
    }

    #[test]
    fn a_reply_is_refused_because_the_network_has_none() {
        assert!(matches!(
            refuse_reply(Some("m")),
            Err(Error::Unsupported("replies"))
        ));
        assert!(refuse_reply(None).is_ok());
    }

    #[test]
    fn an_accepted_send_reports_delivery_and_returns_the_message_id() {
        let api = Api::new(
            reqwest::Client::new(),
            crate::api::Hosts::single("http://127.0.0.1:1"),
            &std::collections::BTreeMap::from([("SAPISID".to_owned(), "s".to_owned())]),
        )
        .expect("api");
        let account = Account {
            login_id: "gvoice_x".to_owned(),
            e164: "+15550000".to_owned(),
        };
        let (_tx, events) = mpsc::unbounded_channel();
        let mut session = GVoiceSession {
            api,
            me: account.me(),
            account,
            events,
            pending: VecDeque::new(),
        };
        let reply = Msg::new().add(3, crate::wire::Val::Str("item-9".to_owned()));
        let portal = PortalRef {
            id: "1".to_owned(),
            receiver: "gvoice_x".to_owned(),
        };
        let ack = session.accepted(&portal, "txn-1".to_owned(), &reply);
        assert_eq!(ack.remote_event_id.as_deref(), Some("item-9"));
        assert!(matches!(
            session.pending.front(),
            Some(Event::Delivery { txn_id, status: DeliveryStatus::Sent, .. }) if txn_id == "txn-1"
        ));
    }
}
