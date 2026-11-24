//! End to end over real HTTP: a local fake of the LinkedIn endpoints the driver uses. The test
//! logs in with cookies, connects, receives a realtime message, sends a reply, uploads and sends
//! a file, backfills two pages, and checks the bridge events and the bytes the server received.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use chirper_bridge::model::{Content, LoginStep, SessionState};
use chirper_bridge::{Command, Event, LoginCtx, LoginInput, Network, Secrets, Session};
use chirper_net_linkedin::LinkedIn;
use zeroize::Zeroizing;

const ME: &str = "ACoAAME";
const PEER: &str = "ACoAAPEER";
const CONVERSATION: &str = "urn:li:msg_conversation:(urn:li:fsd_profile:ACoAAME,THREAD1)";
const LIVE_ID: &str = "urn:li:msg_message:(urn:li:fsd_profile:ACoAAPEER,2-LIVE==)";
const SENT_ID: &str = "urn:li:msg_message:(urn:li:fsd_profile:ACoAAME,2-SENT==)";
const OLD_ID: &str = "urn:li:msg_message:(urn:li:fsd_profile:ACoAAPEER,2-OLD==)";
const TEXT_ID: &str = "urn:li:msg_message:(urn:li:fsd_profile:ACoAAPEER,2-TEXT==)";
const FILE_ID: &str = "urn:li:msg_message:(urn:li:fsd_profile:ACoAAPEER,2-FILE==)";

/// One request the fake server received.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    target: String,
    cookie: String,
    body: Vec<u8>,
}

/// A fake LinkedIn: its base URL and what it received.
struct Fake {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

fn participant(id: &str, first: &str) -> Value {
    json!({
        "entityUrn": format!("urn:li:msg_messagingParticipant:urn:li:fsd_profile:{id}"),
        "participantType": { "member": { "firstName": { "text": first }, "lastName": { "text": "Test" } } }
    })
}

fn message(id: &str, text: &str, delivered: i64, extra: Value) -> Value {
    let mut value = json!({
        "entityUrn": id,
        "deliveredAt": delivered,
        "sender": participant(PEER, "Ada"),
        "messageBodyRenderFormat": "DEFAULT",
        "conversationUrn": CONVERSATION,
        "body": { "text": text, "attributes": [] },
        "renderContent": []
    });
    if let Value::Object(fields) = extra {
        for (key, field) in fields {
            value[key] = field;
        }
    }
    value
}

async fn serve(listener: TcpListener, base: String, seen: Arc<Mutex<Vec<Seen>>>) {
    loop {
        let Ok((socket, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(handle(socket, base.clone(), seen.clone()));
    }
}

async fn read_request(
    socket: &mut TcpStream,
) -> Option<(String, String, HashMap<String, String>, Vec<u8>)> {
    let mut buffer = Vec::new();
    let head_end = loop {
        if let Some(position) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break position;
        }
        let mut chunk = [0u8; 1024];
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut start = lines.next()?.split(' ');
    let method = start.next()?.to_owned();
    let target = start.next()?.to_owned();
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buffer[head_end + 4..].to_vec();
    while body.len() < length {
        let mut chunk = [0u8; 1024];
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    Some((method, target, headers, body))
}

fn query_value(target: &str, key: &str) -> Option<String> {
    let needle = format!("{key}:");
    let start = target.find(&needle)? + needle.len();
    Some(
        target[start..]
            .chars()
            .take_while(|c| *c != ',' && *c != ')')
            .collect(),
    )
}

async fn respond(socket: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(body).await;
}

async fn handle(mut socket: TcpStream, base: String, seen: Arc<Mutex<Vec<Seen>>>) {
    let Some((method, target, headers, body)) = read_request(&mut socket).await else {
        return;
    };
    let cookie = headers.get("cookie").cloned().unwrap_or_default();
    seen.lock().unwrap().push(Seen {
        method: method.clone(),
        target: target.clone(),
        cookie: cookie.clone(),
        body: body.clone(),
    });
    let path = target.split('?').next().unwrap_or("").to_owned();
    let authed = cookie.contains("li_at=token-1") && cookie.contains("JSESSIONID=ajax:1");
    if path == "/voyager/api/me" {
        if !authed {
            return respond(&mut socket, "401 Unauthorized", "application/json", b"{}").await;
        }
        let profile = json!({"miniProfile": {"firstName": "Me", "lastName": "Test", "entityUrn": format!("urn:li:fsd_profile:{ME}")}});
        return respond(
            &mut socket,
            "200 OK",
            "application/json",
            profile.to_string().as_bytes(),
        )
        .await;
    }
    if !authed {
        return respond(&mut socket, "401 Unauthorized", "application/json", b"{}").await;
    }
    if path == "/realtime/connect" {
        return stream_events(&mut socket).await;
    }
    let answer = route(&method, &target, &path, &base, &body);
    match answer {
        Some((status, content_type, bytes)) => {
            respond(&mut socket, status, content_type, &bytes).await
        }
        None => respond(&mut socket, "404 Not Found", "text/plain", b"not found").await,
    }
}

/// The realtime stream: one live message, then the connection is held open.
async fn stream_events(socket: &mut TcpStream) {
    let live = message(LIVE_ID, "live hello", 1_700_000_300_000, json!({}));
    let event = json!({
        "com.linkedin.realtimefrontend.DecoratedEvent": {
            "topic": "urn:li-realtime:messagesTopic:urn:li:fsd_profile:ACoAAME",
            "payload": { "data": {
                "_type": "x",
                "doDecorateMessageMessengerRealtimeDecoration": { "result": live }
            } }
        }
    });
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket
        .write_all(format!("data: {event}\n\n").as_bytes())
        .await;
    tokio::time::sleep(Duration::from_secs(30)).await;
}

/// The canned answer of every other endpoint.
fn route(
    method: &str,
    target: &str,
    path: &str,
    base: &str,
    body: &[u8],
) -> Option<(&'static str, &'static str, Vec<u8>)> {
    let json_ok =
        |value: Value| Some(("200 OK", "application/json", value.to_string().into_bytes()));
    match (method, path) {
        ("GET", "/voyager/api/voyagerMessagingGraphQL/graphql")
            if target.contains("messengerConversations.") =>
        {
            let before: i64 = query_value(target, "lastUpdatedBefore")?.parse().ok()?;
            let elements = if before > 1_700_000_000_000 {
                vec![json!({
                    "entityUrn": CONVERSATION,
                    "lastActivityAt": 1_700_000_000_000i64,
                    "groupChat": false,
                    "categories": ["INBOX"],
                    "conversationParticipants": [participant(ME, "Me"), participant(PEER, "Ada")]
                })]
            } else {
                Vec::new()
            };
            json_ok(
                json!({"data": {"messengerConversationsByCategoryQuery": {"metadata": {}, "elements": elements}}}),
            )
        }
        ("GET", "/voyager/api/voyagerMessagingGraphQL/graphql")
            if target.contains("messengerMessages.d8ea") =>
        {
            let file = json!({"renderContent": [{"file": {
                "name": "notes.pdf", "mediaType": "application/pdf", "byteSize": 9,
                "url": format!("{base}/media/notes.pdf")
            }}]});
            let elements = vec![
                message(FILE_ID, "see attached", 1_700_000_200_000, file),
                message(TEXT_ID, "older text", 1_700_000_100_000, json!({})),
            ];
            json_ok(
                json!({"data": {"messengerMessagesByAnchorTimestamp": {"metadata": {"prevCursor": "CUR1"}, "elements": elements}}}),
            )
        }
        ("GET", "/voyager/api/voyagerMessagingGraphQL/graphql")
            if target.contains("messengerMessages.34c9") =>
        {
            let elements = vec![message(OLD_ID, "oldest", 1_700_000_000_500i64, json!({}))];
            json_ok(
                json!({"data": {"messengerMessagesByConversation": {"metadata": {"prevCursor": ""}, "elements": elements}}}),
            )
        }
        ("POST", "/voyager/api/voyagerMessagingDashMessengerMessages")
            if target.contains("action=createMessage") =>
        {
            let sent = message(SENT_ID, "sent", 1_700_000_400_000, json!({}));
            json_ok(json!({ "value": sent }))
        }
        ("POST", "/voyager/api/voyagerVideoDashMediaUploadMetadata") => json_ok(json!({
            "data": {"value": {"urn": "urn:li:digitalmediaAsset:1", "singleUploadUrl": format!("{base}/upload/1")}}
        })),
        ("PUT", "/upload/1") => Some(("201 Created", "text/plain", Vec::new())),
        ("GET", "/media/notes.pdf") => Some(("200 OK", "application/pdf", b"%PDF-test".to_vec())),
        ("POST", "/voyager/api/voyagerMessagingDashMessengerConversations")
            if target.contains("ids=List") =>
        {
            json_ok(json!({"errors": {}}))
        }
        ("POST", _)
            if path.starts_with("/voyager/api/voyagerMessagingDashMessengerMessages")
                || path.starts_with("/voyager/api/voyagerMessagingDashMessengerConversations")
                || path.starts_with(
                    "/voyager/api/voyagerMessagingDashMessengerMessageDeliveryAcknowledgements",
                )
                || path.starts_with("/realtime/") =>
        {
            let _ = body;
            Some(("200 OK", "application/json", b"{}".to_vec()))
        }
        _ => None,
    }
}

async fn start_fake() -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let base = format!("http://{}", listener.local_addr().expect("local address"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    tokio::spawn(serve(listener, base.clone(), seen.clone()));
    Fake { base, seen }
}

/// An in-memory secret store, namespaced by the runtime's key format.
#[derive(Default)]
struct MemorySecrets {
    values: Mutex<HashMap<String, Vec<u8>>>,
}

impl Secrets for MemorySecrets {
    fn get(&self, key: &str) -> chirper_bridge::Result<Option<Zeroizing<Vec<u8>>>> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .map(Zeroizing::new))
    }
    fn set(&self, key: &str, value: &[u8]) -> chirper_bridge::Result<()> {
        self.values
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> chirper_bridge::Result<()> {
        self.values.lock().unwrap().remove(key);
        Ok(())
    }
}

fn ctx(secrets: Arc<MemorySecrets>) -> LoginCtx {
    LoginCtx {
        network: "linkedin".to_owned(),
        login_id: "pending".to_owned(),
        data_dir: std::env::temp_dir(),
        secrets,
        http: chirper_bridge::http_client(),
    }
}

async fn next(session: &mut Box<dyn Session>) -> Event {
    tokio::time::timeout(Duration::from_secs(15), session.next())
        .await
        .expect("an event arrives in time")
        .expect("the session stays open")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cookie_login_connects_receives_sends_and_backfills() {
    let fake = start_fake().await;
    let driver = LinkedIn::with_base(fake.base.clone());
    assert!(driver.login_flows().iter().any(|flow| flow.id == "cookies"));
    let secrets = Arc::new(MemorySecrets::default());

    // The cookies step, then the cookies themselves.
    let mut login = driver
        .start_login("cookies", ctx(secrets.clone()))
        .await
        .expect("login starts");
    let step = login.next(LoginInput::start()).await.expect("first step");
    let LoginStep::Cookies { fields, url, .. } = step else {
        panic!("the cookie login starts with a cookies step");
    };
    assert_eq!(url, "https://www.linkedin.com/login");
    assert!(
        fields
            .iter()
            .any(|field| field.id == "li_at" && field.required)
    );
    let cookies = [
        ("li_at", "token-1"),
        ("JSESSIONID", "\"ajax:1\""),
        ("user_agent", "Test Agent/1.0"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let complete = login
        .next(LoginInput::cookies(cookies))
        .await
        .expect("cookies are accepted");
    let LoginStep::Complete { login_id } = complete else {
        panic!("a valid login completes");
    };
    assert_eq!(login_id, format!("linkedin_{ME}"));

    // Connect with the stored credentials, under the same login id.
    let mut session = driver.connect(ctx(secrets.clone())).await.expect("connect");
    assert_eq!(session.user().map(|u| u.id), Some(ME.to_owned()));
    assert!(matches!(
        next(&mut session).await,
        Event::State {
            state: SessionState::Connecting,
            ..
        }
    ));
    let Event::Chat(chat) = next(&mut session).await else {
        panic!("the chat list is synced before Running");
    };
    assert_eq!(chat.portal.id, CONVERSATION);
    assert_eq!(
        chat.portal.receiver, ME,
        "a one-to-one chat belongs to the account"
    );
    assert_eq!(chat.name, "Ada Test");
    assert!(chat.is_dm);
    assert!(matches!(
        next(&mut session).await,
        Event::State {
            state: SessionState::Running,
            ..
        }
    ));

    // The realtime stream delivers a message.
    let live = loop {
        if let Event::Message(message) = next(&mut session).await {
            break message;
        }
    };
    assert_eq!(live.id, LIVE_ID);
    assert!(live.live && !live.from_me);
    assert_eq!(live.content.body(), "live hello");
    let portal = live.portal.clone();

    // Reply to it: the reply quotes the message and the text is sent.
    let ack = session
        .command(Command::SendText {
            portal: portal.clone(),
            text: "reply".to_owned(),
            reply_to: Some(LIVE_ID.to_owned()),
            txn_id: "txn-1".to_owned(),
        })
        .await
        .expect("send");
    assert_eq!(ack.remote_event_id.as_deref(), Some(SENT_ID));
    let created = fake
        .seen
        .lock()
        .unwrap()
        .iter()
        .find(|s| {
            s.target.contains("createMessage")
                && String::from_utf8_lossy(&s.body).contains("\"reply\"")
        })
        .cloned()
        .expect("the server received the send");
    let payload: Value = serde_json::from_slice(&created.body).expect("send body is JSON");
    assert_eq!(payload["message"]["originToken"], "txn-1");
    assert_eq!(payload["message"]["repliedMessageContent"], Value::Null);
    let quoted = &payload["message"]["renderContentUnions"][0]["repliedMessageContent"];
    assert_eq!(quoted["originalMessageUrn"], LIVE_ID);
    assert_eq!(quoted["messageBody"]["text"], "live hello");

    // A file attachment: uploaded, then sent with its caption.
    session
        .command(Command::SendMedia {
            portal: portal.clone(),
            data: Arc::new(b"%PDF-out".to_vec()),
            mime: "application/pdf".to_owned(),
            file_name: "out.pdf".to_owned(),
            caption: "the file".to_owned(),
            kind: "file".to_owned(),
            reply_to: None,
            txn_id: String::new(),
        })
        .await
        .expect("media send");
    let uploaded = fake
        .seen
        .lock()
        .unwrap()
        .iter()
        .any(|s| s.method == "PUT" && s.target == "/upload/1" && s.body == b"%PDF-out");
    assert!(uploaded, "the bytes were uploaded to the signed URL");

    // Edit, react and mark read go through without error.
    session
        .command(Command::Edit {
            portal: portal.clone(),
            target: SENT_ID.to_owned(),
            text: "edited".to_owned(),
        })
        .await
        .expect("edit");
    session
        .command(Command::React {
            portal: portal.clone(),
            target: LIVE_ID.to_owned(),
            key: "👍".to_owned(),
        })
        .await
        .expect("react");
    session
        .command(Command::Read {
            portal: portal.clone(),
            event: None,
        })
        .await
        .expect("read");

    // Backfill: the first page is oldest first and has an older page; the second page ends it.
    let first = session
        .backfill(&portal, None, 50)
        .await
        .expect("first page");
    let ids: Vec<&str> = first.messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec![TEXT_ID, FILE_ID]);
    assert!(first.messages.iter().all(|m| !m.live));
    assert!(first.has_more);
    assert_eq!(first.cursor.as_deref(), Some("CUR1"));
    match &first.messages[1].content {
        Content::Media {
            kind, info, data, ..
        } => {
            assert_eq!(kind, "file");
            assert_eq!(info.file_name, "notes.pdf");
            assert_eq!(data.as_slice(), b"%PDF-test");
        }
        other => panic!("the file attachment is downloaded, got {other:?}"),
    }
    let second = session
        .backfill(&portal, first.cursor.clone(), 50)
        .await
        .expect("second page");
    assert_eq!(second.messages.len(), 1);
    assert!(!second.has_more);
    assert!(second.cursor.is_none());

    // The cookies are stored as one secret, and the send carried the session cookie.
    assert!(
        secrets
            .get("linkedin/pending/cookies")
            .ok()
            .flatten()
            .is_some()
    );
    assert!(created.cookie.contains("li_at=token-1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_cookies_are_an_auth_failure() {
    let fake = start_fake().await;
    let driver = LinkedIn::with_base(fake.base.clone());
    let secrets = Arc::new(MemorySecrets::default());
    let mut login = driver
        .start_login("cookies", ctx(secrets))
        .await
        .expect("login starts");
    let _ = login.next(LoginInput::start()).await.expect("first step");
    let cookies = [("li_at", "wrong"), ("JSESSIONID", "ajax:9")]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    let error = login
        .next(LoginInput::cookies(cookies))
        .await
        .expect_err("the cookies are refused");
    assert!(
        matches!(error, chirper_bridge::Error::Auth(_)),
        "got {error:?}"
    );
}
