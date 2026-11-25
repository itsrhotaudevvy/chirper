//! Delivery receipts and edits, driven through the core by a fake driver. The driver reports
//! receipts with the network's message id, as WhatsApp does, and edits as messages with an empty
//! id that name their target.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chirper_bridge::model::{
    BackfillPage, Capabilities, ChatMeta, Content, DeliveryStatus, Event, LoginFlow, LoginStep,
    PortalRef, Reaction, RemoteEventRef, RemoteMessage, RemoteUser,
};
use chirper_bridge::{
    Ack, Command, Error, LoginCtx, LoginInput, LoginProcess, Network, Result, Session,
};
use chirper_core::model::{ChatFilter, Message, MessageStatus};
use chirper_core::{Core, EventStream, HostNetworks, Options};
use tokio::sync::mpsc;

const NETWORK: &str = "fakenet";
const FLOW: &str = "fake";

/// A driver whose events come from the test through a channel.
struct FakeNetwork {
    feed: Mutex<Option<mpsc::UnboundedReceiver<Event>>>,
}

#[async_trait]
impl Network for FakeNetwork {
    fn id(&self) -> &'static str {
        NETWORK
    }

    fn name(&self) -> &'static str {
        "Fake network"
    }

    fn color(&self) -> &'static str {
        "#25D366"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            edits: true,
            read_receipts: true,
            replies: true,
            ..Default::default()
        }
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        vec![LoginFlow {
            id: FLOW.to_owned(),
            name: "Fake".to_owned(),
            description: "Test driver".to_owned(),
        }]
    }

    async fn start_login(&self, flow_id: &str, _ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        if flow_id != FLOW {
            return Err(Error::Unsupported("this login flow"));
        }
        Ok(Box::new(FakeLogin))
    }

    async fn connect(&self, _ctx: LoginCtx) -> Result<Box<dyn Session>> {
        let feed = self
            .feed
            .lock()
            .expect("feed")
            .take()
            .ok_or(Error::NotConnected)?;
        Ok(Box::new(FakeSession { feed, sent: 0 }))
    }
}

struct FakeLogin;

#[async_trait]
impl LoginProcess for FakeLogin {
    async fn next(&mut self, _input: LoginInput) -> Result<LoginStep> {
        Ok(LoginStep::Complete {
            login_id: "fake-login".to_owned(),
        })
    }
}

struct FakeSession {
    feed: mpsc::UnboundedReceiver<Event>,
    /// How many texts were sent; each one is acknowledged as `wa-<n>`.
    sent: u32,
}

#[async_trait]
impl Session for FakeSession {
    async fn next(&mut self) -> Option<Event> {
        self.feed.recv().await
    }

    async fn command(&mut self, cmd: Command) -> Result<Ack> {
        match cmd {
            Command::SendText { .. } => {
                self.sent += 1;
                Ok(Ack {
                    remote_event_id: Some(format!("wa-{}", self.sent)),
                })
            }
            _ => Ok(Ack::default()),
        }
    }

    async fn backfill(
        &mut self,
        _portal: &PortalRef,
        _cursor: Option<String>,
        _limit: usize,
    ) -> Result<BackfillPage> {
        Ok(BackfillPage {
            messages: Vec::new(),
            cursor: None,
            has_more: false,
        })
    }

    async fn logout_remote(&mut self) -> Result<()> {
        Ok(())
    }
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "chirper-receipts-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn portal() -> PortalRef {
    PortalRef {
        id: "chat-1".to_owned(),
        receiver: String::new(),
    }
}

fn ada() -> RemoteUser {
    RemoteUser {
        id: "ada".to_owned(),
        display_name: "Ada".to_owned(),
        avatar_url: String::new(),
        username: String::new(),
        phone: String::new(),
    }
}

fn chat_named(name: &str) -> Event {
    Event::Chat(ChatMeta {
        portal: portal(),
        name: name.to_owned(),
        is_dm: true,
        ..Default::default()
    })
}

fn delivery(txn_id: &str, status: DeliveryStatus, error: &str) -> Event {
    Event::Delivery {
        portal: portal(),
        txn_id: txn_id.to_owned(),
        status,
        error: error.to_owned(),
    }
}

/// A message from `ada`; `edit_of` names the remote message it replaces.
fn incoming(id: &str, edit_of: Option<&str>, body: &str) -> Event {
    Event::Message(RemoteMessage {
        portal: portal(),
        id: id.to_owned(),
        part: String::new(),
        sender: ada(),
        timestamp_ms: 1_700_000_000_000,
        content: Content::Text {
            body: body.to_owned(),
            formatted: None,
        },
        reply_to: None,
        edit_of: edit_of.map(str::to_owned),
        live: true,
        from_me: false,
    })
}

struct Harness {
    core: Core,
    /// Kept alive so the shell never looks closed while the test runs.
    _stream: EventStream,
    feed: mpsc::UnboundedSender<Event>,
    dir: std::path::PathBuf,
    room: String,
    syncs: usize,
}

impl Harness {
    async fn start(name: &str) -> Self {
        let dir = temp_dir(name);
        let (feed, rx) = mpsc::unbounded_channel();
        let mut options = Options::new(&dir);
        options.host_networks = HostNetworks(vec![Arc::new(FakeNetwork {
            feed: Mutex::new(Some(rx)),
        })]);
        let (core, stream) = Core::open(options).unwrap();
        core.start_login(NETWORK, FLOW).await.unwrap();
        feed.send(chat_named("Ada")).unwrap();
        let mut harness = Self {
            core,
            _stream: stream,
            feed,
            dir,
            room: String::new(),
            syncs: 0,
        };
        harness.room = harness.wait_for_chat("Ada").await;
        harness
    }

    async fn wait_for_chat(&self, name: &str) -> String {
        for _ in 0..500 {
            let chats = self.core.list_chats(&ChatFilter::default()).await.unwrap();
            if let Some(chat) = chats.iter().find(|chat| chat.name == name) {
                return chat.room_id.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("chat {name} never appeared");
    }

    /// Feed one driver event, then wait until the engine has applied it. Events are applied in
    /// order, so a chat renamed to a new unique name marks the end of everything before it.
    async fn feed_and_sync(&mut self, event: Event) {
        self.feed.send(event).unwrap();
        self.syncs += 1;
        let name = format!("sync {}", self.syncs);
        self.feed.send(chat_named(&name)).unwrap();
        self.wait_for_chat(&name).await;
    }

    async fn send(&self, text: &str) -> String {
        self.core
            .send_text(&self.room, text, None, "txn.test")
            .await
            .unwrap()
    }

    async fn message(&self, event_id: &str) -> Message {
        self.core
            .list_messages(&self.room, 0, 200)
            .await
            .unwrap()
            .into_iter()
            .find(|message| message.event_id == event_id)
            .unwrap_or_else(|| panic!("message {event_id} is not stored"))
    }

    async fn wait_for_status(&self, event_id: &str, want: MessageStatus) {
        let mut last = MessageStatus::Unknown;
        for _ in 0..500 {
            last = self.message(event_id).await.status;
            if last == want {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("status never reached {want:?}; last was {last:?}");
    }

    async fn message_count(&self) -> usize {
        self.core
            .list_messages(&self.room, 0, 200)
            .await
            .unwrap()
            .len()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.core.shutdown();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receipts_addressed_by_network_id_move_a_message_forward() {
    let mut h = Harness::start("forward").await;
    let sent = h.send("hello").await;
    h.wait_for_status(&sent, MessageStatus::Sent).await;

    // The driver names the message by the id the network gave it on acknowledgement.
    h.feed_and_sync(delivery("wa-1", DeliveryStatus::Delivered, ""))
        .await;
    assert_eq!(h.message(&sent).await.status, MessageStatus::Delivered);

    h.feed_and_sync(delivery("wa-1", DeliveryStatus::Read, ""))
        .await;
    assert_eq!(h.message(&sent).await.status, MessageStatus::Read);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_delivered_does_not_downgrade_read() {
    let mut h = Harness::start("late").await;
    let sent = h.send("hello").await;
    h.wait_for_status(&sent, MessageStatus::Sent).await;

    h.feed_and_sync(delivery("wa-1", DeliveryStatus::Read, ""))
        .await;
    assert_eq!(h.message(&sent).await.status, MessageStatus::Read);

    h.feed_and_sync(delivery("wa-1", DeliveryStatus::Delivered, ""))
        .await;
    h.feed_and_sync(delivery("wa-1", DeliveryStatus::Sent, ""))
        .await;
    assert_eq!(h.message(&sent).await.status, MessageStatus::Read);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_yields_only_to_a_later_success() {
    let mut h = Harness::start("failed").await;
    let sent = h.send("hello").await;
    h.wait_for_status(&sent, MessageStatus::Sent).await;

    h.feed_and_sync(delivery("wa-1", DeliveryStatus::Failed, "refused"))
        .await;
    let message = h.message(&sent).await;
    assert_eq!(message.status, MessageStatus::Failed);
    assert_eq!(message.error, "refused");

    // A later success replaces the failure and clears its text.
    h.feed_and_sync(delivery("wa-1", DeliveryStatus::Delivered, ""))
        .await;
    let message = h.message(&sent).await;
    assert_eq!(message.status, MessageStatus::Delivered);
    assert!(message.error.is_empty());

    // A failure that arrives after the network confirmed the message does not undo it.
    h.feed_and_sync(delivery("wa-1", DeliveryStatus::Failed, "late"))
        .await;
    assert_eq!(h.message(&sent).await.status, MessageStatus::Delivered);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_without_an_id_updates_its_target_and_adds_no_row() {
    let mut h = Harness::start("edit").await;
    h.feed_and_sync(incoming("in-1", None, "original")).await;
    let before = h.message_count().await;

    h.feed_and_sync(incoming("", Some("in-1"), "edited")).await;
    assert_eq!(h.message_count().await, before, "the edit created a row");
    let target = h
        .core
        .list_messages(&h.room, 0, 200)
        .await
        .unwrap()
        .into_iter()
        .find(|message| message.body == "edited")
        .expect("the target shows the edited text");
    assert!(target.edited);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_names_an_outgoing_message_by_its_network_id() {
    let mut h = Harness::start("edit-own").await;
    let sent = h.send("first draft").await;
    h.wait_for_status(&sent, MessageStatus::Sent).await;
    let before = h.message_count().await;

    // The outgoing message is stored under its local id, not the id the network derives.
    h.feed_and_sync(incoming("", Some("wa-1"), "final text"))
        .await;
    assert_eq!(h.message_count().await, before, "the edit created a row");
    let message = h.message(&sent).await;
    assert_eq!(message.body, "final text");
    assert!(message.edited);
}

/// The stored message with this body.
async fn by_body(h: &Harness, body: &str) -> Message {
    h.core
        .list_messages(&h.room, 0, 200)
        .await
        .unwrap()
        .into_iter()
        .find(|message| message.body == body)
        .unwrap_or_else(|| panic!("no message says {body:?}"))
}

fn thumbs_up() -> Event {
    Event::Reactions {
        portal: portal(),
        target: RemoteEventRef::new("wa-1"),
        reactions: vec![Reaction {
            key: "+1".to_owned(),
            count: 1,
            from_me: false,
            senders: vec![ada()],
        }],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reactions_and_redactions_find_our_own_sent_message() {
    let mut h = Harness::start("own-reaction").await;
    let sent = h.send("mine").await;
    h.wait_for_status(&sent, MessageStatus::Sent).await;

    h.feed_and_sync(thumbs_up()).await;
    let message = h.message(&sent).await;
    assert_eq!(
        message.reactions.len(),
        1,
        "the reaction missed our message"
    );
    assert_eq!(message.reactions[0].key, "+1");

    h.feed_and_sync(Event::Redact {
        portal: portal(),
        event: RemoteEventRef::new("wa-1"),
    })
    .await;
    assert!(
        h.message(&sent).await.redacted,
        "the redaction missed our message"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reactions_and_redactions_find_an_incoming_message() {
    let mut h = Harness::start("incoming-reaction").await;
    h.feed_and_sync(incoming("in-1", None, "hi")).await;
    let event_id = by_body(&h, "hi").await.event_id;

    h.feed_and_sync(Event::Reactions {
        portal: portal(),
        target: RemoteEventRef::new("in-1"),
        reactions: vec![Reaction {
            key: "+1".to_owned(),
            count: 1,
            from_me: false,
            senders: vec![ada()],
        }],
    })
    .await;
    assert_eq!(h.message(&event_id).await.reactions.len(), 1);

    h.feed_and_sync(Event::Redact {
        portal: portal(),
        event: RemoteEventRef::new("in-1"),
    })
    .await;
    assert!(h.message(&event_id).await.redacted);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reply_to_our_own_sent_message_points_at_it() {
    let mut h = Harness::start("own-reply").await;
    let sent = h.send("mine").await;
    h.wait_for_status(&sent, MessageStatus::Sent).await;

    h.feed_and_sync(Event::Message(RemoteMessage {
        portal: portal(),
        id: "in-2".to_owned(),
        part: String::new(),
        sender: ada(),
        timestamp_ms: 1_700_000_000_000,
        content: Content::Text {
            body: "answer".to_owned(),
            formatted: None,
        },
        reply_to: Some("wa-1".to_owned()),
        edit_of: None,
        live: true,
        from_me: false,
    }))
    .await;
    let answer = by_body(&h, "answer").await;
    assert_eq!(answer.reply_to.map(|reply| reply.event_id), Some(sent));
}
