//! The password login over real HTTP, against a local fake of the login endpoints. Each test
//! scripts the answers; the driver's requests are recorded so the tests can check what it sent.
//!
//! Ported from the reference's login tests (`connector/login_password_test.go` and the
//! `linkedingo` login tests): the success, wrong password, code challenge and browser fallback
//! paths, plus the transport and rate-limit mappings and the field encryption.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use rsa::Oaep;
use rsa::pkcs8::EncodePublicKey;
use serde_json::{Value, json};
use sha1::Sha1;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use zeroize::Zeroizing;

use chirper_bridge::model::{LoginStep, SessionState};
use chirper_bridge::{Error, Event, LoginCtx, LoginInput, Network, Secrets, Session};
use chirper_net_linkedin::LinkedIn;

const ME: &str = "ACoAAME";
const PASSWORD_STEP: &str = "fi.mau.linkedin.login.credentials";
const EMAIL_CODE_STEP: &str = "fi.mau.linkedin.login.email_code";
const COOKIES_STEP: &str = "fi.mau.linkedin.login.enter_cookies";
const LOGIN_URL: &str = "https://www.linkedin.com/login";
const RSC_PATH: &str = "/flagship-web/rsc-action/actions/server-request";

/// One request the fake received.
#[derive(Clone, Debug)]
struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Request {
    fn cookie(&self) -> &str {
        self.headers.get("cookie").map_or("", String::as_str)
    }

    fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// One answer the fake sends.
struct Answer {
    status: &'static str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl Answer {
    fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: "200 OK",
            headers: vec![("Content-Type", "text/html".to_owned())],
            body: body.into(),
        }
    }

    fn json(value: Value) -> Self {
        Self {
            status: "200 OK",
            headers: vec![("Content-Type", "application/json".to_owned())],
            body: value.to_string().into_bytes(),
        }
    }

    fn status(status: &'static str, body: &str) -> Self {
        Self {
            status,
            headers: vec![("Content-Type", "text/plain".to_owned())],
            body: body.as_bytes().to_vec(),
        }
    }

    fn with(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    fn at(mut self, status: &'static str) -> Self {
        self.status = status;
        self
    }
}

/// The route handler of a test.
type Route = Arc<dyn Fn(&Request) -> Answer + Send + Sync>;

/// A fake LinkedIn: its origin and what it received.
struct Fake {
    base: String,
    seen: Arc<Mutex<Vec<Request>>>,
}

impl Fake {
    /// The requests to `path` with the given method, in arrival order.
    fn requests(&self, method: &str, path: &str) -> Vec<Request> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == method && r.path == path)
            .cloned()
            .collect()
    }
}

async fn start_fake(route: impl Fn(&Request) -> Answer + Send + Sync + 'static) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let base = format!("http://{}", listener.local_addr().expect("local address"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let route: Route = Arc::new(route);
    tokio::spawn(serve(listener, route, seen.clone()));
    Fake { base, seen }
}

async fn serve(listener: TcpListener, route: Route, seen: Arc<Mutex<Vec<Request>>>) {
    loop {
        let Ok((socket, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(handle(socket, route.clone(), seen.clone()));
    }
}

async fn handle(mut socket: TcpStream, route: Route, seen: Arc<Mutex<Vec<Request>>>) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };
    seen.lock().unwrap().push(request.clone());
    let answer = route(&request);
    let mut head = format!(
        "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        answer.status,
        answer.body.len()
    );
    for (name, value) in &answer.headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    head.push_str("\r\n");
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(&answer.body).await;
    let _ = socket.shutdown().await;
}

async fn read_request(socket: &mut TcpStream) -> Option<Request> {
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
    let path = target.split('?').next().unwrap_or_default().to_owned();
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
    Some(Request {
        method,
        path,
        headers,
        body,
    })
}

/// The bootstrap page: the password action, and optionally the encryption row.
fn bootstrap_page(encryption: Option<Value>) -> String {
    let mut payload = json!({
        "authenticationType": "AuthenticationType_PASSWORD",
        "isLoginWithProfile": false,
        "bcookie": "from-bootstrap",
        "bscookie": "server-action-cookie",
    });
    let mut keys = Vec::new();
    for field in [
        "rememberMeOptInCheckboxState",
        "apfc",
        "identifier",
        "password",
    ] {
        let encrypted = field == "password" && encryption.is_some();
        payload[field] = json!({ "key": format!("{field}-state"), "namespace": "MemoryNamespace" });
        keys.push(json!({
            "key": { "value": { "$case": "id", "id": format!("{field}-state") } },
            "isEncrypted": encrypted,
        }));
    }
    let action = json!({
        "requestId": "com.linkedin.sdui.requests.login.authenticate",
        "requestedArguments": {
            "$type": "proto.sdui.actions.requests.RequestedArguments",
            "payload": payload,
            "requestedStateKeys": keys,
            "requestMetadata": { "$type": "proto.sdui.common.RequestMetadata" },
        },
        "isApfcEnabled": false,
        "isStreaming": false,
    });
    let mut rows = vec![
        "0:I[123]\n".to_owned(),
        format!("1:{}\n", json!(["$", "div", null, { "onClick": action }])),
    ];
    if let Some(encryption) = encryption {
        rows.push(format!("2:{encryption}\n"));
    }
    let chunks = serde_json::to_string(&rows).unwrap_or_default();
    format!(
        r#"<html><script id="rehydrate-data">window.__como_rehydration__ = {chunks}</script></html>"#
    )
}

/// The answer to the GET of the login page: the bootstrap headers and a JSESSIONID.
fn bootstrap_answer(encryption: Option<Value>) -> Answer {
    Answer::ok(bootstrap_page(encryption))
        .with("X-Li-Application-Version", "test-version")
        .with("X-Li-Application-Instance", "test-instance")
        .with("X-Li-Page-Instance-Tracking-Id", "test-tracking")
        .with("Set-Cookie", "JSESSIONID=\"ajax:1\"; Path=/; Secure")
        .with("Set-Cookie", "bcookie=current; Path=/; Secure")
}

/// A submission answer that redirects the browser to `destination`.
fn redirect_to(destination: &str) -> String {
    let data = json!({
        "states": [],
        "response": {
            "errors": [],
            "isRetryable": false,
            "completionAction": { "actions": [
                { "value": { "content": { "url": { "urlValue": { "url": destination } } } } }
            ] }
        }
    });
    format!("1:[]\n0:{data}\n")
}

/// A submission answer that shows the inline credential error.
fn wrong_password_answer() -> String {
    let data = json!({
        "response": {
            "errors": [],
            "completionAction": { "actions": [{
                "$type": "proto.sdui.actions.core.ReplaceComponent",
                "value": { "content": { "newComponent": ["$", "div", null, {
                    "viewTrackingSpecs": { "viewName": "identifier-password-inline-feedback" },
                    "textProps": { "children": ["Wrong email or password."] }
                }] } }
            }] }
        }
    });
    format!("0:{data}\n")
}

/// An e-mail code page: the form the reference recognises, with a six-digit field.
fn email_checkpoint_page(csrf: &str) -> String {
    format!(
        r#"<html><form id="email-pin-challenge" method="post" action="/checkpoint/challenge/verify">
<input type="hidden" name="csrfToken" value="{csrf}">
<input type="hidden" name="challengeId" value="challenge-sentinel">
<input type="hidden" name="challengeData" value="opaque&amp;value=preserved">
<input name="pin" type="number" maxlength="6" autocomplete="one-time-code">
</form></html>"#
    )
}

/// The answer that sends the browser to the checkpoint page.
fn checkpoint_redirect() -> Answer {
    Answer::ok(redirect_to("/checkpoint/challenge/token-x?ut=abc")).with(
        "Set-Cookie",
        "checkpoint=cookie-value; Path=/; Secure; HttpOnly",
    )
}

/// The profile of the account, for the cookies that must authorise it.
fn profile_answer(request: &Request) -> Answer {
    if request.cookie().contains("li_at=token-1") && request.cookie().contains("JSESSIONID=ajax:1")
    {
        Answer::json(
            json!({"miniProfile": {"firstName": "Me", "lastName": "Test", "entityUrn": format!("urn:li:fsd_profile:{ME}")}}),
        )
    } else {
        Answer::status("401 Unauthorized", "{}")
    }
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

fn values(pairs: &[(&str, &str)]) -> LoginInput {
    LoginInput::values(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
    )
}

fn credentials(identifier: &str, password: &str) -> LoginInput {
    values(&[("identifier", identifier), ("password", password)])
}

/// The user input step of a login, or a panic with the step that came back instead.
fn user_input(step: LoginStep) -> (String, String, Vec<chirper_bridge::model::InputField>) {
    match step {
        LoginStep::UserInput {
            step_id,
            instructions,
            fields,
        } => (step_id, instructions, fields),
        other => panic!("expected a user input step, got {other:?}"),
    }
}

/// The body of a submission, as the driver sent it.
fn submitted_states(request: &Request) -> Vec<Value> {
    let body: Value = serde_json::from_slice(&request.body).expect("a JSON submission");
    body["states"].as_array().cloned().unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_password_login_stores_the_cookies_and_connects() {
    let fake = start_fake(|request| match request.path.as_str() {
        "/login" => bootstrap_answer(None),
        RSC_PATH => Answer::ok(redirect_to("/feed/"))
            .with("Set-Cookie", "li_at=token-1; Path=/; Secure; HttpOnly"),
        "/voyager/api/me" => profile_answer(request),
        // An account with no conversations yet: the chat list is empty.
        "/voyager/api/voyagerMessagingGraphQL/graphql" => Answer::json(json!({"data": {
            "messengerConversationsByCategoryQuery": {"metadata": {}, "elements": []}
        }})),
        _ => Answer::status("404 Not Found", "not found"),
    })
    .await;
    let driver = LinkedIn::with_base(fake.base.clone());
    assert!(
        driver
            .login_flows()
            .iter()
            .any(|flow| flow.id == "password"),
        "the password flow is listed"
    );
    let secrets = Arc::new(MemorySecrets::default());

    let mut login = driver
        .start_login("password", ctx(secrets.clone()))
        .await
        .expect("login starts");
    let (step_id, _, fields) =
        user_input(login.next(LoginInput::start()).await.expect("first step"));
    assert_eq!(step_id, PASSWORD_STEP);
    let identifier = fields
        .iter()
        .find(|f| f.id == "identifier")
        .expect("identifier field");
    assert_eq!(identifier.kind, "email");
    let password = fields
        .iter()
        .find(|f| f.id == "password")
        .expect("password field");
    assert_eq!(password.kind, "password");

    let complete = login
        .next(credentials("alice@example.invalid", "password-1"))
        .await
        .expect("the password is accepted");
    let LoginStep::Complete { login_id } = complete else {
        panic!("a successful password login completes");
    };
    assert_eq!(login_id, format!("linkedin_{ME}"));

    // The submission carried the plain password (the page asked for no encryption) and the CSRF.
    let posts = fake.requests("POST", RSC_PATH);
    assert_eq!(posts.len(), 1, "the password is submitted once");
    let states = submitted_states(&posts[0]);
    assert_eq!(states.len(), 4);
    assert_eq!(states[2]["value"], "alice@example.invalid");
    assert_eq!(states[3]["value"], "password-1");
    assert!(posts[0].cookie().contains("JSESSIONID=\"ajax:1\""));

    // The cookies are stored as the cookie flow stores them, and connect works unchanged.
    assert!(
        secrets
            .get("linkedin/pending/cookies")
            .ok()
            .flatten()
            .is_some()
    );
    let mut session: Box<dyn Session> =
        driver.connect(ctx(secrets.clone())).await.expect("connect");
    assert_eq!(session.user().map(|u| u.id), Some(ME.to_owned()));
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(15), session.next())
            .await
            .expect("an event arrives")
            .expect("the session stays open"),
        Event::State {
            state: SessionState::Connecting,
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_password_asks_for_the_credentials_again() {
    let fake = start_fake(|request| match request.path.as_str() {
        "/login" => bootstrap_answer(None),
        RSC_PATH => Answer::ok(wrong_password_answer()),
        _ => Answer::status("404 Not Found", "not found"),
    })
    .await;
    let driver = LinkedIn::with_base(fake.base.clone());
    let secrets = Arc::new(MemorySecrets::default());
    let mut login = driver
        .start_login("password", ctx(secrets.clone()))
        .await
        .expect("login starts");
    let _ = login.next(LoginInput::start()).await.expect("first step");
    let (step_id, instructions, fields) = user_input(
        login
            .next(credentials("alice@example.invalid", "wrong-password"))
            .await
            .expect("a wrong password is a step, not an error"),
    );
    assert_eq!(step_id, PASSWORD_STEP);
    assert!(
        instructions.starts_with("LinkedIn rejected the sign-in."),
        "{instructions}"
    );
    assert_eq!(fields.len(), 2);
    assert!(
        secrets
            .get("linkedin/pending/cookies")
            .ok()
            .flatten()
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_code_challenge_is_answered_and_a_rejected_code_is_asked_again() {
    let verifies = Arc::new(AtomicUsize::new(0));
    let counter = verifies.clone();
    let fake = start_fake(move |request| match request.path.as_str() {
        "/login" => bootstrap_answer(None),
        RSC_PATH => checkpoint_redirect(),
        "/checkpoint/challenge/token-x" => Answer::ok(email_checkpoint_page("csrf-sentinel")),
        "/checkpoint/challenge/verify" => {
            // The first code is refused with the same form; the second one is accepted.
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                Answer::ok(email_checkpoint_page("rotated-csrf")).at("400 Bad Request")
            } else {
                Answer::status("303 See Other", "")
                    .with("Location", "/feed/")
                    .with("Set-Cookie", "li_at=token-1; Path=/; Secure; HttpOnly")
            }
        }
        "/feed/" => Answer::ok("<html>feed</html>"),
        "/voyager/api/me" => profile_answer(request),
        _ => Answer::status("404 Not Found", "not found"),
    })
    .await;
    let driver = LinkedIn::with_base(fake.base.clone());
    let secrets = Arc::new(MemorySecrets::default());
    let mut login = driver
        .start_login("password", ctx(secrets.clone()))
        .await
        .expect("login starts");
    let _ = login.next(LoginInput::start()).await.expect("first step");

    let (step_id, _, fields) = user_input(
        login
            .next(credentials("alice@example.invalid", "password-1"))
            .await
            .expect("the code is asked for"),
    );
    assert_eq!(step_id, EMAIL_CODE_STEP);
    let code = fields.first().expect("the code field");
    assert_eq!(code.id, "code");
    assert_eq!(code.pattern, "^[0-9]{6}$");

    let (step_id, instructions, _) = user_input(
        login
            .next(values(&[("code", "111111")]))
            .await
            .expect("a rejected code is a step"),
    );
    assert_eq!(step_id, EMAIL_CODE_STEP);
    assert!(
        instructions.contains("did not accept that code"),
        "{instructions}"
    );

    let complete = login
        .next(values(&[("code", " 012345 ")]))
        .await
        .expect("the code is accepted");
    assert!(matches!(complete, LoginStep::Complete { .. }));

    // The codes went to the verify form, the password was only posted once.
    let verify = fake.requests("POST", "/checkpoint/challenge/verify");
    assert_eq!(verify.len(), 2);
    let form = verify[1].body_text();
    assert!(form.contains("pin=012345"), "{form}");
    assert!(form.contains("challengeId=challenge-sentinel"), "{form}");
    assert!(
        form.contains("challengeData=opaque%26value%3Dpreserved"),
        "{form}"
    );
    assert_eq!(fake.requests("POST", RSC_PATH).len(), 1);
    assert!(
        secrets
            .get("linkedin/pending/cookies")
            .ok()
            .flatten()
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_checkpoint_that_cannot_be_answered_hands_over_to_the_browser() {
    let fake = start_fake(|request| match request.path.as_str() {
        "/login" => bootstrap_answer(None),
        RSC_PATH => checkpoint_redirect(),
        "/checkpoint/challenge/token-x" => Answer::ok(email_checkpoint_page("csrf-sentinel")),
        // The verify form answers with a page that is not the form the driver knows.
        "/checkpoint/challenge/verify" => Answer::status("400 Bad Request", "<html>private</html>"),
        "/voyager/api/me" => profile_answer(request),
        _ => Answer::status("404 Not Found", "not found"),
    })
    .await;
    let driver = LinkedIn::with_base(fake.base.clone());
    let secrets = Arc::new(MemorySecrets::default());
    let mut login = driver
        .start_login("password", ctx(secrets.clone()))
        .await
        .expect("login starts");
    let _ = login.next(LoginInput::start()).await.expect("first step");
    let _ = login
        .next(credentials("alice@example.invalid", "password-1"))
        .await
        .expect("the code is asked for");

    let step = login
        .next(values(&[("code", "012345")]))
        .await
        .expect("the fallback is a step");
    let LoginStep::Cookies {
        step_id,
        url,
        instructions,
        fields,
        ..
    } = step
    else {
        panic!("an unanswerable checkpoint becomes a cookies step, got {step:?}");
    };
    assert_eq!(step_id, COOKIES_STEP);
    assert_eq!(url, LOGIN_URL);
    assert!(
        instructions.contains("through the browser"),
        "{instructions}"
    );
    assert!(fields.iter().any(|f| f.id == "li_at" && f.required));

    // The user finishes in the web view; the cookies complete the login like the cookie flow.
    let cookies = [("li_at", "token-1"), ("JSESSIONID", "\"ajax:1\"")]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    let complete = login
        .next(LoginInput::cookies(cookies))
        .await
        .expect("the browser cookies are accepted");
    assert!(matches!(complete, LoginStep::Complete { .. }));
    assert!(
        secrets
            .get("linkedin/pending/cookies")
            .ok()
            .flatten()
            .is_some()
    );
    assert_eq!(
        fake.requests("POST", RSC_PATH).len(),
        1,
        "the password is not replayed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unrecognised_redirect_hands_over_to_the_browser() {
    // The submission is redirected to a page that is neither a checkpoint nor the feed.
    let fake = start_fake(|request| match request.path.as_str() {
        "/login" => bootstrap_answer(None),
        RSC_PATH => unknown_redirect(),
        _ => Answer::status("404 Not Found", "not found"),
    })
    .await;
    let driver = LinkedIn::with_base(fake.base.clone());
    let secrets = Arc::new(MemorySecrets::default());
    let mut login = driver
        .start_login("password", ctx(secrets))
        .await
        .expect("login starts");
    let _ = login.next(LoginInput::start()).await.expect("first step");
    let step = login
        .next(credentials("alice@example.invalid", "password-1"))
        .await
        .expect("the browser is offered");
    let LoginStep::Cookies { step_id, url, .. } = step else {
        panic!("an unknown challenge becomes a cookies step, got {step:?}");
    };
    assert_eq!(step_id, COOKIES_STEP);
    assert_eq!(url, LOGIN_URL);
}

/// The answer that redirects the submission to a page the driver does not know.
fn unknown_redirect() -> Answer {
    Answer::status("307 Temporary Redirect", "").with("Location", "/welcome")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bootstrap_outage_is_a_retryable_network_error() {
    let fake = start_fake(|_| Answer::status("503 Service Unavailable", "down")).await;
    let driver = LinkedIn::with_base(fake.base.clone());
    let secrets = Arc::new(MemorySecrets::default());
    let mut login = driver
        .start_login("password", ctx(secrets))
        .await
        .expect("login starts");
    let _ = login.next(LoginInput::start()).await.expect("first step");
    let error = login
        .next(credentials("alice@example.invalid", "password-1"))
        .await
        .expect_err("the login cannot reach LinkedIn");
    assert!(matches!(error, Error::Network(_)), "got {error:?}");
    assert!(error.is_retryable());
    assert!(
        fake.requests("POST", RSC_PATH).is_empty(),
        "no credentials are posted"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rate_limit_is_reported_with_its_wait() {
    let fake = start_fake(|request| match request.path.as_str() {
        "/login" => bootstrap_answer(None),
        RSC_PATH => Answer::status("429 Too Many Requests", "slow down").with("Retry-After", "7"),
        _ => Answer::status("404 Not Found", "not found"),
    })
    .await;
    let driver = LinkedIn::with_base(fake.base.clone());
    let mut login = driver
        .start_login("password", ctx(Arc::new(MemorySecrets::default())))
        .await
        .expect("login starts");
    let _ = login.next(LoginInput::start()).await.expect("first step");
    let error = login
        .next(credentials("alice@example.invalid", "password-1"))
        .await
        .expect_err("the login is rate limited");
    assert!(
        matches!(
            error,
            Error::RateLimited {
                retry_after_ms: 7000
            }
        ),
        "got {error:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_password_is_encrypted_when_the_page_asks_for_it() {
    let mut rng = rand::thread_rng();
    let private = rsa::RsaPrivateKey::new(&mut rng, 2048).expect("a 2048-bit key");
    let der = private
        .to_public_key()
        .to_public_key_der()
        .expect("the public key encodes");
    let salt = [0u8, 1, 2, 3, 4, 5, 6, 7];
    let encryption = json!({
        "rsaPublicKey": STANDARD.encode(der.as_bytes()),
        "encryptionSalt": STANDARD.encode(salt),
        "tokenTtlMs": 0,
    });
    let fake = start_fake(move |request| match request.path.as_str() {
        "/login" => bootstrap_answer(Some(encryption.clone())),
        RSC_PATH => {
            Answer::ok(redirect_to("/feed/")).with("Set-Cookie", "li_at=token-1; Path=/; Secure")
        }
        "/voyager/api/me" => profile_answer(request),
        _ => Answer::status("404 Not Found", "not found"),
    })
    .await;
    let driver = LinkedIn::with_base(fake.base.clone());
    let mut login = driver
        .start_login("password", ctx(Arc::new(MemorySecrets::default())))
        .await
        .expect("login starts");
    let _ = login.next(LoginInput::start()).await.expect("first step");
    let _ = login
        .next(credentials("alice@example.invalid", " spaces \" and ü "))
        .await
        .expect("login completes");

    let posts = fake.requests("POST", RSC_PATH);
    let states = submitted_states(&posts[0]);
    assert_eq!(
        states[2]["value"], "alice@example.invalid",
        "the identifier is not encrypted"
    );
    let token = URL_SAFE_NO_PAD
        .decode(states[3]["value"].as_str().unwrap_or_default())
        .expect("the password is a token");
    assert_eq!(&token[..2], &[1, 1]);
    let plaintext = private
        .decrypt(Oaep::new::<Sha1>(), &token[2..])
        .expect("the server key opens it");
    assert_eq!(&plaintext[..8], &salt);
    assert_eq!(&plaintext[8..], " spaces \" and ü ".as_bytes());
}
