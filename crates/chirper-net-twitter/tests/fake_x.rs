//! End to end over real HTTP: a local fake of the X endpoints the driver uses. The test logs in
//! with cookies, connects, receives a message with a reaction and a photo, sends a reply, uploads
//! and sends a photo, backfills two pages, and checks the bridge events and the bytes the server
//! received.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use chirper_bridge::model::{Content, LoginStep, PortalRef, SessionState};
use chirper_bridge::{Command, Event, LoginCtx, LoginInput, Network, Secrets};
use chirper_net_twitter::{Bases, Twitter};
use zeroize::Zeroizing;

const ME: &str = "111";
const PEER: &str = "222";
const CONVERSATION: &str = "111:222";
const REST_CONVERSATION: &str = "111-222";
const PHOTO: &[u8] = b"\xff\xd8\xff\xe0fake-jpeg-bytes";

/// One request the fake server received.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    target: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

/// The shared state of the fake: what it received, how many polls it has answered, and whether
/// it refuses the session.
#[derive(Default)]
struct Shared {
    seen: Mutex<Vec<Seen>>,
    polls: Mutex<u32>,
    refuse: Mutex<bool>,
}

/// Secrets held in memory, as the runtime's store would hold them.
#[derive(Default)]
struct Memory(Mutex<HashMap<String, Vec<u8>>>);

impl Secrets for Memory {
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
    fn move_prefix(&self, from: &str, to: &str) -> chirper_bridge::Result<()> {
        let mut map = self.0.lock().unwrap();
        let keys: Vec<String> = map
            .keys()
            .filter(|k| k.starts_with(from))
            .cloned()
            .collect();
        for key in keys {
            if let Some(value) = map.remove(&key) {
                map.insert(format!("{to}{}", &key[from.len()..]), value);
            }
        }
        Ok(())
    }
}

fn user(id: &str, name: &str, screen: &str) -> Value {
    json!({"id_str": id, "name": name, "screen_name": screen,
        "profile_image_url_https": format!("https://pbs.twimg.com/{id}.jpg")})
}

fn text_message(id: &str, sender: &str, text: &str, time: &str, extra: Value) -> Value {
    let mut data = json!({"id": format!("{id}m"), "sender_id": sender, "text": text,
        "time": time, "reply_data": {}});
    if let Value::Object(fields) = extra {
        for (key, value) in fields {
            data[key] = value;
        }
    }
    json!({"message": {"id": id, "time": time, "conversation_id": CONVERSATION,
        "message_data": data}})
}

fn photo_message(id: &str, base: &str) -> Value {
    text_message(
        id,
        PEER,
        "a photo",
        "1700000002000",
        json!({"attachment": {"photo": {"type": "photo",
            "media_url_https": format!("{base}/media/cat.jpg"),
            "original_info": {"width": 4, "height": 3}}}}),
    )
}

/// The verification token of the messages page: 40 bytes, as the reference's pages carry.
fn verification_token() -> String {
    let bytes: Vec<u8> = (0..40).map(|i| ((i * 37 + 11) % 256) as u8).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The messages page: the verification meta tag, four loading icons with sixteen frames each, and
/// a reference to the `ondemand.s` script on the scripts origin.
fn messages_page() -> String {
    let frames = (0..16)
        .map(|j| {
            (0..11)
                .map(|k| ((j * 31 + k * 53) % 256).to_string())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("C");
    let icons: String = (0..4)
        .map(|i| {
            format!(
                r#"<svg id="loading-x-anim-{i}"><path d="a"/><path d="M00000000 {frames}"/></svg>"#
            )
        })
        .collect();
    format!(
        r#"<html><head><meta name="twitter-site-verification" content="{}"></head><body>{icons}<script>{{123:"ondemand.s",456:"zzz"}}{{123:"abc123"}}</script></body></html>"#,
        verification_token()
    )
}

/// The `ondemand.s` script: the byte indexes the animation is read from.
const ONDEMAND_S: &str = "x=[a(b[3],16)+c(d[7],16)+e(f[12],16)+g(h[20],16)]";

/// The response of the inbox at connect.
fn inbox_initial_state() -> Value {
    json!({"inbox_initial_state": {
        "cursor": "c0",
        "users": {PEER: user(PEER, "Ada", "ada"), ME: user(ME, "Me", "me")},
        "conversations": {CONVERSATION: {
            "conversation_id": CONVERSATION, "type": "ONE_TO_ONE",
            "sort_timestamp": "1700000000000",
            "participants": [{"user_id": ME}, {"user_id": PEER}]}},
        "entries": []}})
}

/// The updates of the first poll: a message, a photo, and a reaction on the message.
fn first_updates(base: &str) -> Value {
    json!({"user_events": {
    "cursor": "c1",
    "entries": [
        text_message("1001", PEER, "hello", "1700000001000", json!({})),
        photo_message("1002", base),
        {"reaction_create": {"id": "3001", "time": "1700000003000",
            "conversation_id": CONVERSATION, "message_id": "1001",
            "emoji_reaction": "🔥", "sender_id": PEER}},
    ]}})
}

/// Start the fake on a free local port. Returns its base URL.
async fn start(shared: Arc<Shared>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    let served_base = base.clone();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(handle(socket, served_base.clone(), shared.clone()));
        }
    });
    base
}

/// Read one HTTP request: its head, then exactly its declared body.
async fn read_request(socket: &mut TcpStream) -> Option<Seen> {
    let mut buffer = Vec::new();
    let head_end = loop {
        if let Some(position) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break position;
        }
        let mut chunk = [0u8; 4096];
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
        let mut chunk = [0u8; 4096];
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    Some(Seen {
        method,
        target,
        headers,
        body,
    })
}

async fn respond(socket: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(body).await;
}

async fn handle(mut socket: TcpStream, base: String, shared: Arc<Shared>) {
    let Some(seen) = read_request(&mut socket).await else {
        return;
    };
    let path = seen.target.split('?').next().unwrap_or_default().to_owned();
    let (status, content_type, body) = route(&base, &shared, &seen, &path);
    shared.seen.lock().unwrap().push(seen);
    respond(&mut socket, status, content_type, &body).await;
}

/// The canned answer of each endpoint.
fn route(
    base: &str,
    shared: &Shared,
    seen: &Seen,
    path: &str,
) -> (&'static str, &'static str, Vec<u8>) {
    const JSON: &str = "application/json";
    let ok = |value: Value| ("200 OK", JSON, value.to_string().into_bytes());
    let photo = ("200 OK", "image/jpeg", PHOTO.to_vec());
    if *shared.refuse.lock().unwrap() && path == "/i/api/1.1/dm/inbox_initial_state.json" {
        return (
            "401 Unauthorized",
            JSON,
            b"{\"errors\":[{\"code\":89}]}".to_vec(),
        );
    }
    let target = seen.target.as_str();
    match (seen.method.as_str(), path) {
        ("GET", "/i/api/1.1/dm/inbox_initial_state.json") => ok(inbox_initial_state()),
        ("GET", "/i/api/1.1/dm/user_updates.json") => {
            let mut polls = shared.polls.lock().unwrap();
            *polls += 1;
            if *polls == 1 {
                ok(first_updates(base))
            } else {
                ok(json!({"user_events": {"cursor": "c1", "entries": []}}))
            }
        }
        ("GET", "/media/cat.jpg") => photo,
        ("GET", "/messages") => ("200 OK", "text/html", messages_page().into_bytes()),
        ("GET", "/responsive-web/client-web/ondemand.s.abc123a.js") => (
            "200 OK",
            "application/javascript",
            ONDEMAND_S.as_bytes().to_vec(),
        ),
        ("POST", "/i/api/1.1/dm/new2.json") => {
            // X answers a send with the REST spelling of the conversation id.
            let mut created = text_message("2001", ME, "hi", "1700000004000", json!({}));
            created["message"]["conversation_id"] = json!(REST_CONVERSATION);
            ok(json!({"entries": [created]}))
        }
        ("GET", p) if p == format!("/i/api/1.1/dm/conversation/{REST_CONVERSATION}.json") => {
            let older = target.contains("max_id=900");
            let (min, status, entries) = if older {
                (
                    "800",
                    "AT_END",
                    json!([text_message(
                        "700",
                        PEER,
                        "older",
                        "1699999999000",
                        json!({})
                    )]),
                )
            } else {
                (
                    "900",
                    "HAS_MORE",
                    json!([
                        text_message("800", PEER, "old", "1699999999500", json!({})),
                        text_message("850", ME, "older", "1699999999800", json!({})),
                    ]),
                )
            };
            ok(json!({"conversation_timeline": {"status": status,
                "min_entry_id": min, "entries": entries}}))
        }
        ("POST", "/i/media/upload.json") => {
            if target.contains("command=INIT") {
                ok(json!({"media_id_string": "77"}))
            } else if target.contains("command=APPEND") {
                ("204 No Content", JSON, Vec::new())
            } else {
                ok(json!({"media_id_string": "77", "size": PHOTO.len()}))
            }
        }
        ("POST", p) if p.ends_with("/mark_read.json") => ok(json!({})),
        ("POST", p)
            if p.contains("/i/api/graphql/")
                && p.ends_with("VyDyV9pC2oZEj6g52hgnhA/useDMReactionMutationAddMutation") =>
        {
            ok(json!({"data": {"create_dm_reaction": {"__typename": "CreateDMReactionSuccess"}}}))
        }
        ("POST", p) if p.contains("/i/api/graphql/") => ok(json!({"data": {}})),
        _ => ("404 Not Found", JSON, b"{}".to_vec()),
    }
}

fn login_ctx(secrets: &Arc<Memory>, login_id: &str) -> LoginCtx {
    LoginCtx {
        network: "twitter".to_owned(),
        login_id: login_id.to_owned(),
        data_dir: std::path::PathBuf::new(),
        secrets: secrets.clone(),
        http: chirper_bridge::http_client(),
    }
}

fn find<'a>(seen: &'a [Seen], method: &str, fragment: &str) -> &'a Seen {
    seen.iter()
        .find(|s| s.method == method && s.target.contains(fragment))
        .unwrap_or_else(|| panic!("no {method} request containing {fragment}"))
}

/// The transaction id of the send request.
fn sent_signature(seen: &[Seen]) -> String {
    find(seen, "POST", "/i/api/1.1/dm/new2.json").headers["x-client-transaction-id"].clone()
}

fn json_body(seen: &Seen) -> Value {
    serde_json::from_slice(&seen.body).expect("a JSON body")
}

/// The next event of a given kind, skipping the rest.
async fn next_matching<T>(
    session: &mut dyn chirper_bridge::Session,
    mut pick: impl FnMut(Event) -> Option<T>,
) -> T {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(20), session.next())
            .await
            .expect("an event within 20 seconds")
            .expect("the session is open");
        if let Some(found) = pick(event) {
            return found;
        }
    }
}

#[tokio::test]
async fn cookies_login_connect_receive_send_backfill_and_media() {
    let shared = Arc::new(Shared::default());
    let base = start(shared.clone()).await;
    let bases = Bases {
        web: base.clone(),
        api: base.clone(),
        upload: base.clone(),
        scripts: base.clone(),
    };
    let driver = Twitter::with_bases(bases).with_poll_interval(Duration::from_millis(50));
    let secrets = Arc::new(Memory::default());

    // Login: the first call asks for the cookies, the second checks and stores them.
    let mut login = driver
        .start_login("cookies", login_ctx(&secrets, "pending"))
        .await
        .expect("the cookie flow starts");
    let step = login.next(LoginInput::start()).await.expect("first step");
    assert!(
        matches!(step, LoginStep::Cookies { ref step_id, .. } if step_id == "fi.mau.twitter.login.enter_cookies")
    );
    let cookies = [
        ("ct0", "csrf-value"),
        ("auth_token", "session-value"),
        ("twid", "u%3D111"),
    ]
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .into_iter()
    .collect();
    let done = login
        .next(LoginInput::cookies(cookies))
        .await
        .expect("the cookies are accepted");
    let LoginStep::Complete { login_id } = done else {
        panic!("the login completes");
    };
    assert_eq!(login_id, "twitter_111");
    secrets
        .move_prefix("twitter/pending/", "twitter/twitter_111/")
        .expect("move");

    // Connect: the chat list, then the running state with the profile.
    let ctx = login_ctx(&secrets, &login_id);
    let mut session = driver.connect(ctx).await.expect("connect");
    let first = session.next().await.expect("an event");
    assert!(matches!(
        first,
        Event::State {
            state: SessionState::Connecting,
            ..
        }
    ));
    let chat = session.next().await.expect("the chat");
    let Event::Chat(meta) = chat else {
        panic!("a chat comes first: {chat:?}");
    };
    assert_eq!(meta.portal.id, REST_CONVERSATION);
    assert_eq!(meta.portal.receiver, ME);
    assert_eq!(meta.name, "Ada");
    assert!(meta.is_dm);
    let running = session.next().await.expect("running");
    let Event::State { state, profile, .. } = running else {
        panic!("the state follows the chats");
    };
    assert_eq!(state, SessionState::Running);
    assert_eq!(profile.expect("a profile").id, ME);

    // Receive: the message, its photo, and the reaction on it.
    let message = next_matching(session.as_mut(), |event| match event {
        Event::Message(message) => Some(message),
        _ => None,
    })
    .await;
    assert_eq!(message.id, "1001");
    assert_eq!(message.portal.id, REST_CONVERSATION);
    assert!(message.live);
    assert!(!message.from_me);
    assert!(matches!(&message.content, Content::Text { body, .. } if body == "hello"));
    let photo = next_matching(session.as_mut(), |event| match event {
        Event::Message(message) if message.id == "1002" => Some(message),
        _ => None,
    })
    .await;
    match &photo.content {
        Content::Media {
            kind, info, data, ..
        } => {
            assert_eq!(kind, "image");
            assert_eq!(info.mime, "image/jpeg");
            assert_eq!(data.as_slice(), PHOTO);
        }
        other => panic!("expected media, got {other:?}"),
    }
    let reactions = next_matching(session.as_mut(), |event| match event {
        Event::Reactions { reactions, .. } => Some(reactions),
        _ => None,
    })
    .await;
    assert_eq!(reactions.len(), 1);
    assert_eq!(reactions[0].key, "🔥");
    assert_eq!(reactions[0].count, 1);
    assert!(!reactions[0].from_me);

    let portal = PortalRef {
        id: REST_CONVERSATION.to_owned(),
        receiver: ME.to_owned(),
    };

    // Send a reply: the server gets the text and the reply target, and the verdict follows.
    let txn = "123e4567-e89b-42d3-a456-426614174000".to_owned();
    let ack = session
        .command(Command::SendText {
            portal: portal.clone(),
            text: "hi".to_owned(),
            reply_to: Some("1001".to_owned()),
            txn_id: txn.clone(),
        })
        .await
        .expect("the send is accepted");
    assert_eq!(ack.remote_event_id.as_deref(), Some("2001"));
    let delivered = next_matching(session.as_mut(), |event| match event {
        Event::Delivery { txn_id, status, .. } => Some((txn_id, status)),
        _ => None,
    })
    .await;
    assert_eq!(delivered.0, txn);

    // Upload and send a photo.
    let upload = session
        .command(Command::SendMedia {
            portal: portal.clone(),
            data: Arc::new(PHOTO.to_vec()),
            mime: "image/jpeg".to_owned(),
            file_name: "cat.jpg".to_owned(),
            caption: "look".to_owned(),
            kind: "image".to_owned(),
            reply_to: None,
            txn_id: "txn-media".to_owned(),
        })
        .await
        .expect("the photo is sent");
    assert_eq!(upload.remote_event_id.as_deref(), Some("2001"));

    // Reactions, read marks and typing go to their endpoints.
    session
        .command(Command::React {
            portal: portal.clone(),
            target: "1001".to_owned(),
            key: "👍".to_owned(),
        })
        .await
        .expect("react");
    session
        .command(Command::Read {
            portal: portal.clone(),
            event: Some("1001".to_owned()),
        })
        .await
        .expect("read");
    session
        .command(Command::Typing {
            portal: portal.clone(),
            typing: true,
        })
        .await
        .expect("typing");

    // Backfill: two pages, oldest first, with the cursor the server sent.
    let first_page = session
        .backfill(&portal, None, 50)
        .await
        .expect("first page");
    let ids: Vec<&str> = first_page.messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["800m", "850m"]);
    assert!(first_page.has_more);
    assert_eq!(first_page.cursor.as_deref(), Some("900"));
    assert!(!first_page.messages[0].live);
    assert!(!first_page.messages[0].from_me);
    assert!(first_page.messages[1].from_me);

    let older = session
        .backfill(&portal, first_page.cursor.clone(), 50)
        .await
        .expect("older page");
    let older_ids: Vec<&str> = older.messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(older_ids, ["700m"]);
    assert!(!older.has_more);
    assert_eq!(older.cursor, None);

    // What the server received.
    let seen = shared.seen.lock().unwrap().clone();

    let login_check = find(&seen, "GET", "/i/api/1.1/dm/inbox_initial_state.json");
    assert!(login_check.headers["cookie"].contains("auth_token=session-value"));
    assert_eq!(login_check.headers["x-csrf-token"], "csrf-value");
    assert!(login_check.headers["authorization"].starts_with("Bearer AAAA"));

    let poll = find(&seen, "GET", "/i/api/1.1/dm/user_updates.json");
    assert!(poll.headers.contains_key("x-client-uuid"));
    assert!(!poll.target.contains("filter_low_quality=true"));

    let sent = json_body(find(&seen, "POST", "/i/api/1.1/dm/new2.json"));
    assert_eq!(sent["text"], "hi");
    assert_eq!(sent["reply_to_dm_id"], "1001");
    assert_eq!(sent["conversation_id"], REST_CONVERSATION);
    assert_eq!(sent["request_id"], txn);

    let upload_init = find(&seen, "POST", "command=INIT");
    assert!(upload_init.target.contains("media_category=dm_image"));
    let appended = seen
        .iter()
        .find(|s| s.target.contains("command=APPEND"))
        .expect("an APPEND");
    assert!(appended.body.windows(PHOTO.len()).any(|w| w == PHOTO));
    let finalize = find(&seen, "POST", "command=FINALIZE");
    assert!(finalize.target.contains("original_md5="));
    let photo_send = seen
        .iter()
        .rev()
        .find(|s| s.target.contains("new2.json"))
        .map(json_body)
        .expect("the photo send");
    assert_eq!(photo_send["media_id"], "77");

    let react = find(&seen, "POST", "useDMReactionMutationAddMutation");
    let react_body = json_body(react);
    assert_eq!(react_body["variables"]["messageId"], "1001");
    assert_eq!(react_body["variables"]["emojiReactions"][0], "👍");

    let read = find(&seen, "POST", "mark_read.json");
    let read_body = String::from_utf8_lossy(&read.body).to_string();
    assert!(read_body.contains("last_read_event_id=1001"));
    assert!(read_body.contains("conversationId=111-222"));

    // Every API request carries a signature made from the messages page, not the fallback.
    let signed = sent_signature(&seen);
    assert_ne!(signed, "ZTo");
    assert!(signed.len() > 60);

    let typing = find(&seen, "POST", "useTypingNotifierMutation");
    assert_eq!(
        json_body(typing)["variables"]["conversationId"],
        REST_CONVERSATION
    );
}

#[tokio::test]
async fn cookies_the_network_refuses_are_an_auth_error() {
    let shared = Arc::new(Shared::default());
    *shared.refuse.lock().unwrap() = true;
    let base = start(shared.clone()).await;
    let bases = Bases {
        web: base.clone(),
        api: base.clone(),
        upload: base.clone(),
        scripts: base,
    };
    let driver = Twitter::with_bases(bases);
    let secrets = Arc::new(Memory::default());
    let mut login = driver
        .start_login("cookies", login_ctx(&secrets, "pending"))
        .await
        .expect("the cookie flow starts");
    let cookies = [
        ("ct0", "csrf"),
        ("auth_token", "session"),
        ("twid", "u%3D111"),
    ]
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .into_iter()
    .collect();
    let refused = login.next(LoginInput::cookies(cookies)).await;
    assert!(
        matches!(refused, Err(chirper_bridge::Error::Auth(_))),
        "{refused:?}"
    );
    assert!(
        driver
            .start_login("password", login_ctx(&secrets, "x"))
            .await
            .is_err()
    );
}
