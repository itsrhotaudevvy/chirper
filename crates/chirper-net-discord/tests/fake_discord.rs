//! End to end over real sockets: a local fake of the Discord REST endpoints and of the gateway.
//! The browser and bot logins are checked against the fake, and so is `connect`: the identify
//! payload must carry the bot authorization and the bot intents when the login was a bot.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use chirper_bridge::model::{LoginStep, SessionState};
use chirper_bridge::{Event, LoginCtx, LoginInput, Network, Result, Secrets, Session};
use chirper_net_discord::Discord;
use zeroize::Zeroizing;

const USER_TOKEN: &str = "user-token-value-123456";
const BOT_TOKEN: &str = "good-bot-token";
/// The intents of a user identify (the driver's user set).
const USER_INTENTS: i64 = 65025;
/// The intents of a bot identify: the user set plus GUILD_MEMBERS.
const BOT_INTENTS: i64 = 65027;

/// An in-memory credential store.
#[derive(Default)]
struct Memory(Mutex<HashMap<String, Vec<u8>>>);

impl Secrets for Memory {
    fn get(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        Ok(self.0.lock().unwrap().get(key).cloned().map(Zeroizing::new))
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

/// What the fake received: every HTTP request as (target, authorization), and every identify.
#[derive(Clone, Default)]
struct Log {
    requests: Arc<Mutex<Vec<(String, String)>>>,
    identify: Arc<Mutex<Vec<Value>>>,
}

/// A running fake: the REST base URL and the log of what it saw.
struct Fake {
    base: String,
    log: Log,
}

fn ctx(secrets: &Arc<Memory>) -> LoginCtx {
    LoginCtx {
        network: "discord".to_owned(),
        login_id: "discord-test".to_owned(),
        data_dir: std::env::temp_dir(),
        secrets: secrets.clone(),
        http: reqwest::Client::new(),
    }
}

fn values(pairs: &[(&str, &str)]) -> LoginInput {
    LoginInput::values(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn cookies(pairs: &[(&str, &str)]) -> LoginInput {
    LoginInput::cookies(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn user(id: &str, name: &str, bot: bool) -> Value {
    json!({ "id": id, "username": name, "discriminator": "0", "avatar": null, "bot": bot })
}

/// The account an authorization header belongs to, as Discord would answer for it.
fn account_for(authorization: &str) -> Option<Value> {
    match authorization {
        USER_TOKEN => Some(user("111", "ada", false)),
        "Bot good-bot-token" => Some(user("222", "chirper-bot", true)),
        // A user account does not answer to the bot prefix; the bot flow must refuse it.
        "Bot user-token-value-123456" => Some(user("111", "ada", false)),
        _ => None,
    }
}

async fn start_fake() -> Fake {
    let http = TcpListener::bind("127.0.0.1:0").await.expect("bind http");
    let gateway = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind gateway");
    let base = format!("http://{}", http.local_addr().expect("http address"));
    let gateway_url = format!("ws://{}", gateway.local_addr().expect("gateway address"));
    let log = Log::default();
    tokio::spawn(serve_http(http, gateway_url, log.clone()));
    tokio::spawn(serve_gateway(gateway, log.clone()));
    Fake { base, log }
}

async fn serve_http(listener: TcpListener, gateway_url: String, log: Log) {
    while let Ok((socket, _)) = listener.accept().await {
        tokio::spawn(handle_http(socket, gateway_url.clone(), log.clone()));
    }
}

/// The request line's target and the authorization header, read up to the end of the head.
async fn read_head(socket: &mut TcpStream) -> Option<(String, String)> {
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
    let target = lines.next()?.split(' ').nth(1)?.to_owned();
    let authorization = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.trim().to_owned())
        .unwrap_or_default();
    Some((target, authorization))
}

async fn respond(socket: &mut TcpStream, status: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(body.as_bytes()).await;
}

async fn handle_http(mut socket: TcpStream, gateway_url: String, log: Log) {
    let Some((target, authorization)) = read_head(&mut socket).await else {
        return;
    };
    log.requests
        .lock()
        .unwrap()
        .push((target.clone(), authorization.clone()));
    let (status, body) = match (target.as_str(), account_for(&authorization)) {
        ("/gateway", _) => ("200 OK", json!({ "url": gateway_url }).to_string()),
        ("/users/@me", Some(account)) => ("200 OK", account.to_string()),
        ("/users/@me", None) => (
            "401 Unauthorized",
            json!({ "message": "401: Unauthorized", "code": 0 }).to_string(),
        ),
        _ => ("404 Not Found", "{}".to_owned()),
    };
    respond(&mut socket, status, &body).await;
}

async fn serve_gateway(listener: TcpListener, log: Log) {
    while let Ok((socket, _)) = listener.accept().await {
        tokio::spawn(handle_gateway(socket, log.clone()));
    }
}

/// Greet the driver, record its identify, and answer with a READY for the account.
async fn handle_gateway(socket: TcpStream, log: Log) {
    let Ok(mut ws) = tokio_tungstenite::accept_async(socket).await else {
        return;
    };
    let hello = json!({ "op": 10, "d": { "heartbeat_interval": 60_000 } });
    if ws
        .send(WsMessage::Text(hello.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    while let Some(Ok(WsMessage::Text(text))) = ws.next().await {
        let Ok(payload) = serde_json::from_str::<Value>(text.as_str()) else {
            continue;
        };
        if payload["op"] != 2 {
            continue;
        }
        let bot = payload["d"]["token"] == "Bot good-bot-token";
        log.identify.lock().unwrap().push(payload);
        let ready = json!({
            "op": 0, "t": "READY", "s": 1,
            "d": {
                "session_id": "fake-session",
                "resume_gateway_url": "ws://127.0.0.1:9",
                "user": user(if bot { "222" } else { "111" }, "account", bot),
                "guilds": [], "private_channels": [], "users": []
            }
        });
        if ws
            .send(WsMessage::Text(ready.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
    }
}

fn requests_to(log: &Log, target: &str) -> Vec<String> {
    log.requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(seen, _)| seen == target)
        .map(|(_, authorization)| authorization.clone())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_flow_validates_the_stored_token_and_completes() {
    let fake = start_fake().await;
    let secrets = Arc::new(Memory::default());
    let driver = Discord::with_base(fake.base.clone());
    let mut process = driver.start_login("browser", ctx(&secrets)).await.unwrap();

    match process.next(LoginInput::start()).await.unwrap() {
        LoginStep::Cookies {
            step_id,
            url,
            fields,
            extract_js,
            wait_for_url_pattern,
            ..
        } => {
            assert_eq!(step_id, "fi.chirper.discord.login.browser");
            assert_eq!(url, "https://discord.com/login");
            assert_eq!(wait_for_url_pattern, r"discord\.com/channels");
            assert!(extract_js.contains("localStorage.getItem('token')"));
            assert_eq!(fields.len(), 1);
            assert_eq!(fields[0].id, "token");
            assert!(fields[0].required);
        }
        other => panic!("expected the browser cookies step, got {other:?}"),
    }

    // The web app stores the token as a JSON string, so the quotes come with it.
    let step = process
        .next(cookies(&[("token", &format!("\"{USER_TOKEN}\""))]))
        .await
        .unwrap();
    assert!(matches!(step, LoginStep::Complete { ref login_id } if login_id == "discord_111"));
    assert_eq!(
        requests_to(&fake.log, "/users/@me"),
        vec![USER_TOKEN.to_owned()]
    );
    let stored = secrets
        .get(&ctx(&secrets).secret_key("token"))
        .unwrap()
        .unwrap();
    assert_eq!(stored.as_slice(), USER_TOKEN.as_bytes());
    assert!(
        secrets
            .get(&ctx(&secrets).secret_key("bot"))
            .unwrap()
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_flow_refuses_a_token_discord_does_not_accept() {
    let fake = start_fake().await;
    let secrets = Arc::new(Memory::default());
    let driver = Discord::with_base(fake.base.clone());
    let mut process = driver.start_login("browser", ctx(&secrets)).await.unwrap();
    process.next(LoginInput::start()).await.unwrap();
    let error = process
        .next(cookies(&[("token", "not-a-token-at-all-1234")]))
        .await
        .unwrap_err();
    assert!(matches!(error, chirper_bridge::Error::Auth(_)), "{error:?}");
    assert!(
        secrets
            .get(&ctx(&secrets).secret_key("token"))
            .unwrap()
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bot_flow_authenticates_with_the_bot_prefix_and_completes() {
    let fake = start_fake().await;
    let secrets = Arc::new(Memory::default());
    let driver = Discord::with_base(fake.base.clone());
    let mut process = driver.start_login("bot", ctx(&secrets)).await.unwrap();

    match process.next(LoginInput::start()).await.unwrap() {
        LoginStep::UserInput {
            step_id, fields, ..
        } => {
            assert_eq!(step_id, "fi.chirper.discord.login.enter_bot_token");
            assert_eq!(fields[0].id, "token");
        }
        other => panic!("expected the bot token step, got {other:?}"),
    }

    // The Developer Portal shows the token with its `Bot ` prefix; the driver adds it back itself.
    let step = process
        .next(values(&[("token", &format!("Bot {BOT_TOKEN}"))]))
        .await
        .unwrap();
    assert!(matches!(step, LoginStep::Complete { ref login_id } if login_id == "discord_222"));
    assert_eq!(
        requests_to(&fake.log, "/users/@me"),
        vec![format!("Bot {BOT_TOKEN}")]
    );
    let ctx = ctx(&secrets);
    let stored = secrets.get(&ctx.secret_key("token")).unwrap().unwrap();
    assert_eq!(stored.as_slice(), BOT_TOKEN.as_bytes());
    let flag = secrets.get(&ctx.secret_key("bot")).unwrap().unwrap();
    assert_eq!(flag.as_slice(), b"1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bot_flow_refuses_a_user_account() {
    let fake = start_fake().await;
    let secrets = Arc::new(Memory::default());
    let driver = Discord::with_base(fake.base.clone());
    let mut process = driver.start_login("bot", ctx(&secrets)).await.unwrap();
    process.next(LoginInput::start()).await.unwrap();
    let error = process
        .next(values(&[("token", USER_TOKEN)]))
        .await
        .unwrap_err();
    // The pasted user token is sent with the bot prefix, which the fake answers with a user.
    assert!(
        matches!(error, chirper_bridge::Error::Auth(message) if message.contains("user account"))
    );
    assert!(
        secrets
            .get(&ctx(&secrets).secret_key("token"))
            .unwrap()
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_flow_still_accepts_a_user_token_and_a_pasted_bot_token() {
    let fake = start_fake().await;
    let secrets = Arc::new(Memory::default());
    let driver = Discord::with_base(fake.base.clone());

    let mut user_login = driver.start_login("token", ctx(&secrets)).await.unwrap();
    match user_login.next(LoginInput::start()).await.unwrap() {
        LoginStep::UserInput { step_id, .. } => {
            assert_eq!(step_id, "fi.chirper.discord.login.enter_token");
        }
        other => panic!("expected the token step, got {other:?}"),
    }
    let step = user_login
        .next(values(&[("token", USER_TOKEN)]))
        .await
        .unwrap();
    assert!(matches!(step, LoginStep::Complete { ref login_id } if login_id == "discord_111"));
    assert!(
        secrets
            .get(&ctx(&secrets).secret_key("bot"))
            .unwrap()
            .is_none()
    );

    let secrets = Arc::new(Memory::default());
    let mut bot_login = driver.start_login("token", ctx(&secrets)).await.unwrap();
    bot_login.next(LoginInput::start()).await.unwrap();
    let step = bot_login
        .next(values(&[("token", &format!("Bot {BOT_TOKEN}"))]))
        .await
        .unwrap();
    assert!(matches!(step, LoginStep::Complete { ref login_id } if login_id == "discord_222"));
    let flag = secrets
        .get(&ctx(&secrets).secret_key("bot"))
        .unwrap()
        .unwrap();
    assert_eq!(flag.as_slice(), b"1");
}

/// Wait for the first event of a session, failing the test if none comes in time.
async fn first_event(session: &mut Box<dyn Session>) -> Event {
    tokio::time::timeout(Duration::from_secs(5), session.next())
        .await
        .expect("the gateway answers in time")
        .expect("the session yields an event")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_with_the_bot_flag_identifies_with_the_bot_payload() {
    let fake = start_fake().await;
    let secrets = Arc::new(Memory::default());
    let driver = Discord::with_base(fake.base.clone());
    let mut process = driver.start_login("bot", ctx(&secrets)).await.unwrap();
    process.next(LoginInput::start()).await.unwrap();
    process.next(values(&[("token", BOT_TOKEN)])).await.unwrap();

    let mut session = driver.connect(ctx(&secrets)).await.unwrap();
    match first_event(&mut session).await {
        Event::State { state, .. } => assert_eq!(state, SessionState::Running),
        other => panic!("expected the running state, got {other:?}"),
    }

    let identify = fake.log.identify.lock().unwrap().clone();
    assert_eq!(identify.len(), 1, "one identify per connection");
    assert_eq!(identify[0]["d"]["token"], "Bot good-bot-token");
    assert_eq!(identify[0]["d"]["intents"], BOT_INTENTS);
    assert_eq!(
        requests_to(&fake.log, "/users/@me")
            .last()
            .map(String::as_str),
        Some("Bot good-bot-token")
    );
    assert_eq!(requests_to(&fake.log, "/gateway").len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_with_a_user_token_identifies_as_a_user() {
    let fake = start_fake().await;
    let secrets = Arc::new(Memory::default());
    let driver = Discord::with_base(fake.base.clone());
    let mut process = driver.start_login("token", ctx(&secrets)).await.unwrap();
    process.next(LoginInput::start()).await.unwrap();
    process
        .next(values(&[("token", USER_TOKEN)]))
        .await
        .unwrap();

    let mut session = driver.connect(ctx(&secrets)).await.unwrap();
    assert!(matches!(
        first_event(&mut session).await,
        Event::State {
            state: SessionState::Running,
            ..
        }
    ));

    let identify = fake.log.identify.lock().unwrap().clone();
    assert_eq!(identify.len(), 1);
    assert_eq!(identify[0]["d"]["token"], USER_TOKEN);
    assert_eq!(identify[0]["d"]["intents"], USER_INTENTS);
}
