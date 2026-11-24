//! End to end against a fake PDS and PLC directory on a local TCP listener: a password login (with
//! a second-factor step and an expired access token that is refreshed on connect), a received
//! message, a sent message with facets, reactions, read receipts, a backfill page and logout.
//!
//! The fake speaks minimal HTTP/1.1 and serves canned JSON. It records every XRPC call so the test
//! can check what the driver sent. Nothing here touches the internet.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use percent_encoding::percent_decode_str;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use chirper_bridge::model::{Content, LoginFlow, LoginStep, PortalRef, SessionState};
use chirper_bridge::network::LoginInput;
use chirper_bridge::{Command, Error, Event, LoginCtx, Network, Secrets, Session};
use chirper_net_bluesky::{Bluesky, Hosts};

const ALICE: &str = "did:plc:alice";
const BOB: &str = "did:plc:bob";
const CAROL: &str = "did:plc:carol";
const APP_PASSWORD: &str = "app-pw";
const SECOND_FACTOR: &str = "123456";

/// One XRPC call the driver made.
#[derive(Clone, Debug)]
struct Call {
    nsid: String,
    bearer: Option<String>,
    proxy: Option<String>,
    body: Value,
}

#[derive(Default)]
struct World {
    base: String,
    calls: Vec<Call>,
    log_served: bool,
}

type Shared = Arc<Mutex<World>>;

/// The secret store of the test: a map, with the engine's prefix move.
#[derive(Default)]
struct MemSecrets(Mutex<HashMap<String, Vec<u8>>>);

impl Secrets for MemSecrets {
    fn get(&self, key: &str) -> chirper_bridge::Result<Option<zeroize::Zeroizing<Vec<u8>>>> {
        Ok(self
            .0
            .lock()
            .expect("secrets")
            .get(key)
            .cloned()
            .map(zeroize::Zeroizing::new))
    }

    fn set(&self, key: &str, value: &[u8]) -> chirper_bridge::Result<()> {
        self.0
            .lock()
            .expect("secrets")
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> chirper_bridge::Result<()> {
        self.0.lock().expect("secrets").remove(key);
        Ok(())
    }

    fn move_prefix(&self, from: &str, to: &str) -> chirper_bridge::Result<()> {
        let mut map = self.0.lock().expect("secrets");
        let moved: Vec<(String, Vec<u8>)> = map
            .iter()
            .filter(|(key, _)| key.starts_with(from))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        for (key, value) in moved {
            map.remove(&key);
            map.insert(format!("{to}{}", &key[from.len()..]), value);
        }
        Ok(())
    }
}

impl MemSecrets {
    fn session_json(&self, login: &str) -> Value {
        let map = self.0.lock().expect("secrets");
        let bytes = map
            .get(&format!("bluesky/{login}/session"))
            .expect("the session secret is stored under the login");
        serde_json::from_slice(bytes).expect("the session secret is JSON")
    }
}

fn ctx(secrets: &Arc<MemSecrets>, login_id: &str) -> LoginCtx {
    LoginCtx {
        network: "bluesky".to_owned(),
        login_id: login_id.to_owned(),
        data_dir: std::env::temp_dir(),
        secrets: secrets.clone(),
        http: reqwest::Client::new(),
    }
}

fn profile(did: &str) -> Value {
    match did {
        ALICE => json!({"did": ALICE, "handle": "alice.test", "displayName": "Alice"}),
        BOB => json!({"did": BOB, "handle": "bob.test", "displayName": "Bob"}),
        _ => json!({"did": CAROL, "handle": "carol.test"}),
    }
}

fn did_for_handle(handle: &str) -> Option<&'static str> {
    match handle {
        "alice.test" => Some(ALICE),
        "bob.test" => Some(BOB),
        "carol.test" => Some(CAROL),
        _ => None,
    }
}

fn message_view(id: &str, sender: &str, text: &str, facets: Value) -> Value {
    json!({
        "$type": "chat.bsky.convo.defs#messageView", "id": id, "rev": "1", "text": text,
        "facets": facets, "reactions": [], "sender": {"did": sender},
        "sentAt": "2026-01-01T00:00:00.000Z"
    })
}

/// The answer of one request: a status and a JSON body (empty for none).
fn route(
    world: &mut World,
    method: &str,
    path: &str,
    query: &[(String, String)],
    headers: &HashMap<String, String>,
    body: Value,
) -> (u16, String) {
    let bearer = headers
        .get("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_owned);
    let proxy = headers.get("atproto-proxy").cloned();
    let param = |name: &str| {
        query
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };

    if let Some(did) = path.strip_prefix("/did:plc:") {
        let did = format!("did:plc:{did}");
        return (200, json!({
            "id": did,
            "service": [{"id": "#atproto_pds", "type": "AtprotoPersonalDataServer", "serviceEndpoint": world.base}]
        })
        .to_string());
    }
    let Some(nsid) = path.strip_prefix("/xrpc/") else {
        return (404, String::new());
    };
    let _ = method;
    match nsid {
        "com.atproto.identity.resolveHandle" => {
            return match param("handle").as_deref().and_then(did_for_handle) {
                Some(did) => (200, json!({"did": did}).to_string()),
                None => (400, json!({"error": "HandleNotFound"}).to_string()),
            };
        }
        "com.atproto.server.createSession" => {
            return create_session(&body);
        }
        "com.atproto.server.refreshSession" => {
            if bearer.as_deref() != Some("refresh-1") {
                return (401, json!({"error": "InvalidToken"}).to_string());
            }
            return (200, json!({"did": ALICE, "handle": "alice.test", "accessJwt": "access-2", "refreshJwt": "refresh-2"}).to_string());
        }
        _ => {}
    }
    world.calls.push(Call {
        nsid: nsid.to_owned(),
        bearer: bearer.clone(),
        proxy,
        body: body.clone(),
    });
    // The first token is expired by the time the driver uses it, so connect must refresh it.
    match bearer.as_deref() {
        Some("access-1") => return (400, json!({"error": "ExpiredToken"}).to_string()),
        Some("access-2") => {}
        _ => return (401, json!({"error": "AuthenticationRequired"}).to_string()),
    }
    if nsid != "com.atproto.server.deleteSession" && nsid.starts_with("chat.") {
        let proxied = world.calls.last().and_then(|call| call.proxy.as_deref());
        if proxied != Some("did:web:api.bsky.chat#bsky_chat") {
            return (400, json!({"error": "InvalidRequest"}).to_string());
        }
    }
    let answer = match nsid {
        "app.bsky.actor.getProfiles" => {
            let profiles: Vec<Value> = query
                .iter()
                .filter(|(k, _)| k == "actors")
                .map(|(_, did)| profile(did))
                .collect();
            json!({"profiles": profiles})
        }
        "chat.bsky.convo.listConvos" => json!({"convos": [{
            "id": "c1", "members": [profile(ALICE), profile(BOB)]
        }]}),
        "chat.bsky.convo.getLog" => {
            if world.log_served || param("cursor").is_some() {
                json!({"cursor": "cur-1", "logs": []})
            } else {
                world.log_served = true;
                json!({"cursor": "cur-1", "logs": [{
                    "$type": "chat.bsky.convo.defs#logCreateMessage", "rev": "2", "convoId": "c1",
                    "message": message_view("m1", BOB, "hello from bob https://example.com",
                        json!([{"index": {"byteStart": 15, "byteEnd": 34},
                                "features": [{"$type": "app.bsky.richtext.facet#link", "uri": "https://example.com"}]}]))
                }]})
            }
        }
        "chat.bsky.convo.sendMessage" => json!(message_view("m2", ALICE, "sent", json!([]))),
        "chat.bsky.convo.getMessages" => match param("cursor").as_deref() {
            None => json!({"cursor": "older", "messages": [
                message_view("m3", BOB, "third", json!([])),
                message_view("m2", ALICE, "second", json!([]))
            ]}),
            Some(_) => json!({"messages": [
                message_view("m1", BOB, "first", json!([])),
                {"$type": "chat.bsky.convo.defs#deletedMessageView", "id": "m0",
                 "sender": {"did": BOB}, "sentAt": "2025-12-31T23:00:00.000Z"}
            ]}),
        },
        "chat.bsky.convo.deleteMessageForSelf"
        | "chat.bsky.convo.addReaction"
        | "chat.bsky.convo.removeReaction"
        | "chat.bsky.convo.updateRead" => json!({}),
        "com.atproto.server.deleteSession" => return (200, String::new()),
        _ => return (404, json!({"error": "MethodNotImplemented"}).to_string()),
    };
    (200, answer.to_string())
}

fn create_session(body: &Value) -> (u16, String) {
    let identifier = body
        .get("identifier")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let password = body
        .get("password")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if password != APP_PASSWORD {
        return (401, json!({"error": "AuthenticationRequired"}).to_string());
    }
    let factor = body.get("authFactorToken").and_then(Value::as_str);
    if identifier == "carol.test" {
        match factor {
            None => return (401, json!({"error": "AuthFactorTokenRequired"}).to_string()),
            Some(code) if code != SECOND_FACTOR => {
                return (401, json!({"error": "AuthenticationRequired"}).to_string());
            }
            Some(_) => {
                return (200, json!({"did": CAROL, "handle": "carol.test", "accessJwt": "carol-access", "refreshJwt": "carol-refresh"}).to_string());
            }
        }
    }
    (200, json!({"did": ALICE, "handle": "alice.test", "accessJwt": "access-1", "refreshJwt": "refresh-1"}).to_string())
}

async fn read_request(
    stream: &mut TcpStream,
) -> Option<(String, String, HashMap<String, String>, Vec<u8>)> {
    let mut buf = Vec::new();
    let head_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split(' ');
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some((method, target, headers, body))
}

fn query_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let decode = |s: &str| {
                percent_decode_str(&s.replace('+', " "))
                    .decode_utf8_lossy()
                    .into_owned()
            };
            (decode(key), decode(value))
        })
        .collect()
}

async fn handle(mut stream: TcpStream, world: Shared) {
    let Some((method, target, headers, body)) = read_request(&mut stream).await else {
        return;
    };
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let body: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    let query = query_pairs(query);
    let (status, answer) = {
        let mut guard = world.lock().expect("world");
        route(&mut guard, &method, path, &query, &headers, body)
    };
    let reason = if status == 200 { "OK" } else { "Error" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
        answer.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Start the fake and return its base URL, `http://127.0.0.1:<port>`.
async fn start(world: Shared) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    world.lock().expect("world").base = base.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(handle(stream, world.clone()));
        }
    });
    base
}

async fn next_event(session: &mut Box<dyn Session>) -> Event {
    tokio::time::timeout(Duration::from_secs(5), session.next())
        .await
        .expect("an event arrives in time")
        .expect("the session stays open")
}

fn call<'a>(world: &'a World, nsid: &str) -> &'a Call {
    world
        .calls
        .iter()
        .find(|call| call.nsid == nsid)
        .unwrap_or_else(|| panic!("the driver called {nsid}"))
}

fn portal() -> PortalRef {
    PortalRef {
        id: "c1".to_owned(),
        receiver: ALICE.to_owned(),
    }
}

#[tokio::test]
async fn login_connect_receive_send_backfill_and_logout() {
    let world: Shared = Arc::default();
    let base = start(world.clone()).await;
    let driver = Bluesky::with_hosts(Hosts {
        plc: base.clone(),
        scheme: "http".to_owned(),
    });
    let secrets = Arc::new(MemSecrets::default());

    // The flow is listed, then the form, then the sign-in with a handle.
    let flows: Vec<LoginFlow> = driver.login_flows();
    assert_eq!(flows.len(), 1);
    assert_eq!(flows[0].id, "password");
    let mut login = driver
        .start_login("password", ctx(&secrets, "provisional"))
        .await
        .expect("the password flow starts");
    let form = login.next(LoginInput::start()).await.expect("the form");
    assert!(matches!(&form, LoginStep::UserInput { fields, .. } if fields.len() == 3));
    let mut values = std::collections::BTreeMap::new();
    values.insert(
        "domain".to_owned(),
        base.trim_start_matches("http://").to_owned(),
    );
    values.insert("username".to_owned(), "alice.test".to_owned());
    values.insert("password".to_owned(), APP_PASSWORD.to_owned());
    let done = login
        .next(LoginInput::values(values))
        .await
        .expect("sign in");
    match done {
        LoginStep::Complete { login_id } => assert_eq!(login_id, ALICE),
        other => panic!("expected completion, got {other:?}"),
    }
    // The engine moves the stored session from the provisional id to the DID.
    secrets
        .move_prefix("bluesky/provisional/", &format!("bluesky/{ALICE}/"))
        .expect("move");

    // Connect: the first access token is expired, so the driver refreshes it.
    let mut session = driver
        .connect(ctx(&secrets, ALICE))
        .await
        .expect("connect after refresh");
    assert_eq!(secrets.session_json(ALICE)["accessJwt"], "access-2");

    // Connecting, the chat list, then running with the profile.
    assert!(matches!(
        next_event(&mut session).await,
        Event::State {
            state: SessionState::Connecting,
            ..
        }
    ));
    match next_event(&mut session).await {
        Event::Chat(meta) => {
            assert_eq!(meta.portal, portal());
            assert!(meta.is_dm);
            assert_eq!(meta.name, "Bob");
        }
        other => panic!("expected the chat, got {other:?}"),
    }
    match next_event(&mut session).await {
        Event::State {
            state: SessionState::Running,
            profile: Some(me),
            ..
        } => {
            assert_eq!(me.id, "plc-alice");
            assert_eq!(me.display_name, "Alice");
        }
        other => panic!("expected running, got {other:?}"),
    }

    // A message from Bob arrives in real time, with its link marked up.
    match next_event(&mut session).await {
        Event::Message(message) => {
            assert_eq!(message.id, "c1:m1");
            assert_eq!(message.sender.id, "plc-bob");
            assert!(message.live);
            assert!(!message.from_me);
            match &message.content {
                Content::Text { body, formatted } => {
                    assert_eq!(body, "hello from bob https://example.com");
                    assert_eq!(
                        formatted.as_deref(),
                        Some(
                            "hello from bob <a href=\"https://example.com\">https://example.com</a>"
                        )
                    );
                }
                other => panic!("expected text, got {other:?}"),
            }
        }
        other => panic!("expected a message, got {other:?}"),
    }

    // Sending: a link and a mention become facets with byte offsets; the id is the bridge's.
    let text = "see https://example.com/x and @bob.test";
    let ack = session
        .command(Command::SendText {
            portal: portal(),
            text: text.to_owned(),
            reply_to: None,
            txn_id: "txn-1".to_owned(),
        })
        .await
        .expect("send");
    assert_eq!(ack.remote_event_id.as_deref(), Some("c1:m2"));
    {
        let world = world.lock().expect("world");
        let sent = call(&world, "chat.bsky.convo.sendMessage");
        assert_eq!(sent.body["convoId"], "c1");
        assert_eq!(sent.body["message"]["text"], text);
        let facets = sent.body["message"]["facets"].as_array().expect("facets");
        assert_eq!(facets.len(), 2);
        assert_eq!(facets[0]["index"]["byteStart"], 4);
        assert_eq!(facets[0]["index"]["byteEnd"], 25);
        assert_eq!(
            facets[0]["features"][0]["$type"],
            "app.bsky.richtext.facet#link"
        );
        assert_eq!(facets[1]["index"]["byteStart"], 30);
        assert_eq!(facets[1]["index"]["byteEnd"], 39);
        assert_eq!(facets[1]["features"][0]["did"], BOB);
        assert_eq!(sent.bearer.as_deref(), Some("access-2"));
        assert_eq!(
            sent.proxy.as_deref(),
            Some("did:web:api.bsky.chat#bsky_chat")
        );
    }

    // Reactions, read receipts and deletion for self.
    session
        .command(Command::React {
            portal: portal(),
            target: "c1:m1".to_owned(),
            key: "👍".to_owned(),
        })
        .await
        .expect("react");
    session
        .command(Command::Read {
            portal: portal(),
            event: Some("c1:m1".to_owned()),
        })
        .await
        .expect("read");
    session
        .command(Command::Redact {
            portal: portal(),
            target: "c1:m2".to_owned(),
        })
        .await
        .expect("redact");
    {
        let world = world.lock().expect("world");
        assert_eq!(
            call(&world, "chat.bsky.convo.addReaction").body["value"],
            "👍"
        );
        assert_eq!(
            call(&world, "chat.bsky.convo.updateRead").body["messageId"],
            "m1"
        );
        assert_eq!(
            call(&world, "chat.bsky.convo.deleteMessageForSelf").body["messageId"],
            "m2"
        );
    }
    // A message of another chat is refused before anything is sent.
    let wrong = session
        .command(Command::Redact {
            portal: portal(),
            target: "other:m2".to_owned(),
        })
        .await;
    assert!(matches!(wrong, Err(Error::Protocol(_))));
    // Edits are not in the chat API.
    let edit = session
        .command(Command::Edit {
            portal: portal(),
            target: "c1:m2".to_owned(),
            text: "x".to_owned(),
        })
        .await;
    assert!(matches!(edit, Err(Error::Unsupported(_))));

    // Backfill: a page is oldest first; a full page with a cursor has more.
    let first = session.backfill(&portal(), None, 2).await.expect("history");
    let ids: Vec<&str> = first.messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["c1:m2", "c1:m3"]);
    assert!(first.messages[0].from_me);
    assert!(!first.messages[1].from_me);
    assert!(!first.messages[0].live);
    assert!(first.has_more);
    let second = session
        .backfill(&portal(), first.cursor.clone(), 2)
        .await
        .expect("older history");
    assert_eq!(second.messages.len(), 2);
    assert!(
        matches!(&second.messages[0].content, Content::Notice { body } if body == "Deleted message")
    );
    assert_eq!(second.messages[1].id, "c1:m1");
    assert!(!second.has_more);

    // Logging out ends the server session.
    session.logout_remote().await.expect("logout");
    assert_eq!(
        call(
            &world.lock().expect("world"),
            "com.atproto.server.deleteSession"
        )
        .bearer
        .as_deref(),
        Some("access-2")
    );

    // The network is usable end to end, so it is no longer listed as unavailable.
    assert!(driver.unavailable().is_none());
}

#[tokio::test]
async fn a_second_factor_is_asked_for_and_a_wrong_password_is_refused() {
    let world: Shared = Arc::default();
    let base = start(world).await;
    let driver = Bluesky::with_hosts(Hosts {
        plc: base.clone(),
        scheme: "http".to_owned(),
    });
    let secrets = Arc::new(MemSecrets::default());
    let domain = base.trim_start_matches("http://").to_owned();

    let mut login = driver
        .start_login("password", ctx(&secrets, "provisional"))
        .await
        .expect("start");
    login.next(LoginInput::start()).await.expect("form");
    let mut values = std::collections::BTreeMap::new();
    values.insert("domain".to_owned(), domain.clone());
    values.insert("username".to_owned(), "carol.test".to_owned());
    values.insert("password".to_owned(), APP_PASSWORD.to_owned());
    let code_step = login
        .next(LoginInput::values(values))
        .await
        .expect("factor step");
    assert!(matches!(&code_step, LoginStep::UserInput { step_id, .. } if step_id == "code"));

    let mut wrong = std::collections::BTreeMap::new();
    wrong.insert("code".to_owned(), "000000".to_owned());
    assert!(matches!(
        login.next(LoginInput::values(wrong)).await,
        Err(Error::Auth(_))
    ));

    let mut login = driver
        .start_login("password", ctx(&secrets, "provisional"))
        .await
        .expect("start again");
    login.next(LoginInput::start()).await.expect("form");
    let mut bad = std::collections::BTreeMap::new();
    bad.insert("domain".to_owned(), domain);
    bad.insert("username".to_owned(), "alice.test".to_owned());
    bad.insert("password".to_owned(), "not-the-password".to_owned());
    match login.next(LoginInput::values(bad)).await {
        Err(Error::Auth(message)) => assert!(!message.contains("not-the-password")),
        other => panic!("a wrong password must be an auth error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_login_without_a_stored_session_cannot_connect() {
    let driver = Bluesky::new();
    let secrets = Arc::new(MemSecrets::default());
    assert!(matches!(
        driver.connect(ctx(&secrets, ALICE)).await,
        Err(Error::Auth(_))
    ));
}
