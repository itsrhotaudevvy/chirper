//! Tests with a fake platform: no provider, no Android.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chirper_bridge::model::{Content, Event, LoginStep, PortalRef, SessionState};
use chirper_bridge::network::LoginInput;
use chirper_bridge::{Command, Error, LoginCtx, Network, Secrets, Session};
use zeroize::Zeroizing;

use crate::platform::{
    MmsPart, SmsAddress, SmsMessage, SmsPermissions, SmsPlatform, SmsPlatformEvent, SmsThread,
};
use crate::{Sms, UNAVAILABLE};

type Sink = Arc<dyn Fn(SmsPlatformEvent) + Send + Sync>;

/// A provider that lives in memory.
#[derive(Default)]
struct Fake {
    perms: Mutex<SmsPermissions>,
    granted: Mutex<SmsPermissions>,
    threads: Mutex<Vec<SmsThread>>,
    messages: Mutex<Vec<SmsMessage>>,
    sink: Mutex<Option<Sink>>,
    sent: Mutex<Vec<(Vec<String>, String)>>,
    history_calls: Mutex<Vec<(i64, Option<i64>, usize)>>,
    next_send_id: Mutex<String>,
}

impl Fake {
    fn all_granted() -> Self {
        let all = SmsPermissions {
            read: true,
            send: true,
            receive: true,
            contacts: true,
        };
        Self {
            perms: Mutex::new(all),
            granted: Mutex::new(all),
            ..Default::default()
        }
    }

    fn push(&self, message: SmsMessage) {
        self.messages.lock().unwrap().push(message);
    }

    fn fire(&self, event: SmsPlatformEvent) {
        let sink = self.sink.lock().unwrap().clone().expect("watching");
        sink(event);
    }
}

impl SmsPlatform for Fake {
    fn permissions(&self) -> chirper_bridge::Result<SmsPermissions> {
        Ok(*self.perms.lock().unwrap())
    }

    fn request_permissions(&self) -> chirper_bridge::Result<SmsPermissions> {
        Ok(*self.granted.lock().unwrap())
    }

    fn threads(&self, limit: usize) -> chirper_bridge::Result<Vec<SmsThread>> {
        Ok(self
            .threads
            .lock()
            .unwrap()
            .iter()
            .take(limit)
            .cloned()
            .collect())
    }

    fn messages(
        &self,
        thread_id: i64,
        before_ms: Option<i64>,
        limit: usize,
    ) -> chirper_bridge::Result<Vec<SmsMessage>> {
        self.history_calls
            .lock()
            .unwrap()
            .push((thread_id, before_ms, limit));
        let mut found: Vec<SmsMessage> = self
            .messages
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.thread_id == thread_id && before_ms.is_none_or(|b| m.date_ms < b))
            .cloned()
            .collect();
        found.sort_by_key(|m| std::cmp::Reverse(m.date_ms));
        found.truncate(limit);
        Ok(found)
    }

    fn messages_since(
        &self,
        after_ms: i64,
        limit: usize,
    ) -> chirper_bridge::Result<Vec<SmsMessage>> {
        let mut found: Vec<SmsMessage> = self
            .messages
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.date_ms > after_ms)
            .cloned()
            .collect();
        found.sort_by_key(|m| m.date_ms);
        found.truncate(limit);
        Ok(found)
    }

    fn send_text(
        &self,
        addresses: &[String],
        text: &str,
        _subscription_id: Option<i32>,
    ) -> chirper_bridge::Result<String> {
        self.sent
            .lock()
            .unwrap()
            .push((addresses.to_vec(), text.to_owned()));
        Ok(self.next_send_id.lock().unwrap().clone())
    }

    fn mms_part(&self, _part_id: i64, _max_bytes: u64) -> chirper_bridge::Result<Vec<u8>> {
        Ok(vec![1, 2, 3])
    }

    fn watch(&self, sink: Sink) -> chirper_bridge::Result<()> {
        *self.sink.lock().unwrap() = Some(sink);
        Ok(())
    }
}

#[derive(Default)]
struct MemSecrets(Mutex<HashMap<String, Vec<u8>>>);

impl Secrets for MemSecrets {
    fn get(&self, key: &str) -> chirper_bridge::Result<Option<Zeroizing<Vec<u8>>>> {
        Ok(self.0.lock().unwrap().get(key).cloned().map(Zeroizing::new))
    }

    fn set(&self, key: &str, value: &[u8]) -> chirper_bridge::Result<()> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> chirper_bridge::Result<()> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

struct Harness {
    secrets: Arc<MemSecrets>,
}

impl Harness {
    fn new() -> Self {
        Self {
            secrets: Arc::new(MemSecrets::default()),
        }
    }

    fn ctx(&self) -> LoginCtx {
        LoginCtx {
            network: "sms".to_owned(),
            login_id: "device".to_owned(),
            data_dir: std::path::PathBuf::new(),
            secrets: self.secrets.clone(),
            http: chirper_bridge::http_client(),
        }
    }

    fn watermark(&self) -> Option<String> {
        self.secrets
            .0
            .lock()
            .unwrap()
            .get("sms/device/watermark")
            .map(|v| String::from_utf8(v.clone()).unwrap())
    }

    fn seed_watermark(&self, value: i64) {
        self.secrets.0.lock().unwrap().insert(
            "sms/device/watermark".to_owned(),
            value.to_string().into_bytes(),
        );
    }
}

fn address(raw: &str, e164: &str) -> SmsAddress {
    SmsAddress {
        raw: raw.to_owned(),
        e164: e164.to_owned(),
        name: String::new(),
    }
}

fn thread(thread_id: i64, addresses: Vec<SmsAddress>) -> SmsThread {
    SmsThread {
        thread_id,
        addresses,
        snippet_date_ms: 0,
    }
}

fn msg(id: &str, thread_id: i64, date_ms: i64, body: &str, status: &str) -> SmsMessage {
    SmsMessage {
        id: id.to_owned(),
        thread_id,
        sender: address("+15550000001", "+15550000001"),
        from_me: false,
        date_ms,
        body: body.to_owned(),
        parts: Vec::new(),
        status: status.to_owned(),
    }
}

async fn connect(sms: &Sms, harness: &Harness) -> Box<dyn Session> {
    match sms.connect(harness.ctx()).await {
        Ok(session) => session,
        Err(err) => panic!("connect failed: {err}"),
    }
}

/// Events until the session stays quiet for `quiet`.
async fn collect(session: &mut Box<dyn Session>, quiet: Duration) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(Some(event)) = tokio::time::timeout(quiet, session.next()).await {
        out.push(event);
    }
    out
}

/// (id, part, live, text-or-kind) of every message event.
fn messages(events: &[Event]) -> Vec<(String, String, bool, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Message(m) => Some((
                m.id.clone(),
                m.part.clone(),
                m.live,
                match &m.content {
                    Content::Text { body, .. } => body.clone(),
                    other => other.kind().to_owned(),
                },
            )),
            _ => None,
        })
        .collect()
}

const QUIET: Duration = Duration::from_millis(900);

#[test]
fn unavailable_without_a_platform() {
    let sms = Sms::new();
    assert_eq!(sms.unavailable().as_deref(), Some(UNAVAILABLE));
    assert_eq!(sms.id(), "sms");
    assert_eq!(sms.name(), "SMS");
    assert_eq!(sms.color(), "#8E8E93");
    assert_eq!(sms.capabilities(), chirper_bridge::Capabilities::default());
    assert!(!sms.supports_remote_logout());
}

#[test]
fn messages_do_not_print_bodies_or_numbers() {
    let message = msg("sms:1", 1, 5, "a secret body", "received");
    let text = format!("{message:?}");
    assert!(!text.contains("a secret body"));
    assert!(!text.contains("+15550000001"));
    let address = format!("{:?}", message.sender);
    assert!(!address.contains("+15550000001"));
}

#[tokio::test]
async fn login_is_refused_without_permission() {
    let fake = Arc::new(Fake::default());
    let sms = Sms::with_platform(fake.clone());
    let harness = Harness::new();
    let mut process = sms
        .start_login("device", harness.ctx())
        .await
        .unwrap_or_else(|err| panic!("start failed: {err}"));
    match process.next(LoginInput::start()).await {
        Ok(LoginStep::UserInput { fields, .. }) => {
            assert_eq!(fields.len(), 1);
            assert_eq!(fields[0].id, "allow");
            assert_eq!(fields[0].kind, "button");
        }
        other => panic!("expected the permission step, got {other:?}"),
    }
    match process.next(LoginInput::start()).await {
        Err(Error::Auth(message)) => assert_eq!(message, "SMS permission was not granted"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn login_completes_with_permission() {
    let fake = Arc::new(Fake::all_granted());
    let sms = Sms::with_platform(fake.clone());
    let harness = Harness::new();
    let mut process = sms.start_login("device", harness.ctx()).await.ok().unwrap();
    let _ = process.next(LoginInput::start()).await;
    match process.next(LoginInput::start()).await {
        Ok(LoginStep::Complete { login_id }) => assert_eq!(login_id, "device"),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[tokio::test]
async fn connect_refuses_a_revoked_read_permission() {
    let fake = Arc::new(Fake::default());
    let sms = Sms::with_platform(fake);
    let harness = Harness::new();
    match sms.connect(harness.ctx()).await {
        Err(Error::Auth(message)) => assert_eq!(message, "SMS permission was revoked"),
        Err(other) => panic!("unexpected error {other}"),
        Ok(_) => panic!("connected without permission"),
    }
}

#[tokio::test]
async fn changes_emit_only_new_messages_once() {
    let fake = Arc::new(Fake::all_granted());
    *fake.threads.lock().unwrap() =
        vec![thread(1, vec![address("(555) 000-0001", "+15550000001")])];
    fake.push(msg("sms:1", 1, 1000, "old", "received"));
    fake.push(msg("sms:2", 1, 2000, "two", "received"));
    fake.push(msg("sms:3", 1, 2500, "sending", "sending"));
    let sms = Sms::with_platform(fake.clone());
    let harness = Harness::new();
    harness.seed_watermark(1500);
    let mut session = connect(&sms, &harness).await;

    let first = collect(&mut session, QUIET).await;
    assert!(matches!(
        first.first(),
        Some(Event::State {
            state: SessionState::Running,
            ..
        })
    ));
    let chats: Vec<&PortalRef> = first
        .iter()
        .filter_map(|e| match e {
            Event::Chat(meta) => Some(&meta.portal),
            _ => None,
        })
        .collect();
    assert_eq!(chats.len(), 1);
    assert_eq!(chats[0].id, "+15550000001");
    assert_eq!(
        messages(&first),
        vec![("sms:2".into(), String::new(), false, "two".into())]
    );
    assert_eq!(harness.watermark().as_deref(), Some("2000"));

    // The settled message appears, the new one too, and nothing is emitted twice.
    fake.messages.lock().unwrap()[2].status = "sent".to_owned();
    fake.push(msg("sms:4", 1, 3000, "four", "received"));
    fake.fire(SmsPlatformEvent::Changed);
    fake.fire(SmsPlatformEvent::Changed);
    let second = collect(&mut session, QUIET).await;
    assert_eq!(
        messages(&second),
        vec![
            ("sms:3".into(), String::new(), true, "sending".into()),
            ("sms:4".into(), String::new(), true, "four".into()),
        ]
    );
    // The write is rate limited; dropping the session writes the rest.
    assert_eq!(harness.watermark().as_deref(), Some("2000"));
    drop(session);
    assert_eq!(harness.watermark().as_deref(), Some("3000"));
}

#[tokio::test]
async fn send_status_maps_to_delivery() {
    let fake = Arc::new(Fake::all_granted());
    *fake.next_send_id.lock().unwrap() = "send-1".to_owned();
    let sms = Sms::with_platform(fake.clone());
    let harness = Harness::new();
    harness.seed_watermark(0);
    let mut session = connect(&sms, &harness).await;
    collect(&mut session, QUIET).await;

    let portal = PortalRef {
        id: "+15550000001".to_owned(),
        receiver: String::new(),
    };
    let ack = session
        .command(Command::SendText {
            portal: portal.clone(),
            text: "hi".to_owned(),
            reply_to: None,
            txn_id: "txn-1".to_owned(),
        })
        .await;
    assert!(ack.is_ok());
    assert_eq!(
        fake.sent.lock().unwrap().clone(),
        vec![(vec!["+15550000001".to_owned()], "hi".to_owned())]
    );

    fake.fire(SmsPlatformEvent::SendStatus {
        send_id: "send-1".to_owned(),
        status: "sent".to_owned(),
        error: String::new(),
    });
    fake.fire(SmsPlatformEvent::SendStatus {
        send_id: "send-1".to_owned(),
        status: "delivered".to_owned(),
        error: String::new(),
    });
    fake.fire(SmsPlatformEvent::SendStatus {
        send_id: "send-1".to_owned(),
        status: "delivered".to_owned(),
        error: String::new(),
    });
    let events = collect(&mut session, QUIET).await;
    let verdicts: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::Delivery { txn_id, status, .. } => Some((txn_id.clone(), *status)),
            _ => None,
        })
        .collect();
    assert_eq!(
        verdicts,
        vec![
            ("txn-1".to_owned(), chirper_bridge::DeliveryStatus::Sent),
            (
                "txn-1".to_owned(),
                chirper_bridge::DeliveryStatus::Delivered
            ),
        ]
    );
}

#[tokio::test]
async fn mms_is_refused_for_sending() {
    let fake = Arc::new(Fake::all_granted());
    let sms = Sms::with_platform(fake);
    let harness = Harness::new();
    harness.seed_watermark(0);
    let mut session = connect(&sms, &harness).await;
    let result = session
        .command(Command::SendMedia {
            portal: PortalRef::default(),
            data: Arc::new(vec![1]),
            mime: "image/png".to_owned(),
            file_name: "a.png".to_owned(),
            caption: String::new(),
            kind: "image".to_owned(),
            reply_to: None,
            txn_id: "t".to_owned(),
        })
        .await;
    assert!(matches!(result, Err(Error::Unsupported(_))));
}

#[tokio::test]
async fn backfill_pages_by_cursor_and_fetches_media() {
    let fake = Arc::new(Fake::all_granted());
    *fake.threads.lock().unwrap() = vec![thread(7, vec![address("+15550000001", "+15550000001")])];
    for (i, date) in [1000_i64, 1100, 1200, 1250].into_iter().enumerate() {
        fake.push(msg(&format!("sms:{i}"), 7, date, "text", "received"));
    }
    fake.push(SmsMessage {
        parts: vec![MmsPart {
            part_id: 9,
            mime: "image/jpeg".to_owned(),
            name: "a.jpg".to_owned(),
            size: 3,
        }],
        ..msg("mms:5", 7, 1300, "", "received")
    });
    let sms = Sms::with_platform(fake.clone());
    let harness = Harness::new();
    harness.seed_watermark(1_000_000);
    let mut session = connect(&sms, &harness).await;
    collect(&mut session, QUIET).await;
    let portal = PortalRef {
        id: "+15550000001".to_owned(),
        receiver: String::new(),
    };

    let first = session
        .backfill(&portal, None, 2)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(first.has_more);
    let cursor = first.cursor.clone().expect("a cursor");
    assert_eq!(cursor, "1250");
    assert!(first.messages.iter().all(|m| !m.live));

    let second = session
        .backfill(&portal, Some(cursor), 500)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(second.messages.len(), 3);
    let calls = fake.history_calls.lock().unwrap().clone();
    assert_eq!(calls, vec![(7, None, 2), (7, Some(1250), 100)]);

    let media = first
        .messages
        .iter()
        .find(|m| m.part == "9")
        .expect("the media part");
    match &media.content {
        Content::Media { kind, data, .. } => {
            assert_eq!(kind, "image");
            assert_eq!(data.as_slice(), &[1, 2, 3]);
        }
        other => panic!("expected media, got {other:?}"),
    }

    let bad = session.backfill(&portal, Some("soon".to_owned()), 2).await;
    assert!(matches!(bad, Err(Error::Protocol(_))));
}
