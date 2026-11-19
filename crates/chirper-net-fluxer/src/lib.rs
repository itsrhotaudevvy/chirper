//! The Fluxer driver for Chirper's unified bridge runtime.
//!
//! Fluxer is a Discord-like chat platform with self-hostable instances. This crate is a port of the
//! Go connector's behaviour: a session token, a bot token or a password (with an authenticator
//! code), the REST API, and the Gateway. It reports chats, messages, reactions and receipts as
//! domain events and accepts commands; there is no Matrix in between.
//!
//! Layout: `discovery` (instance URLs and endpoints), `api` (REST and the 429 policy), `captcha`
//! (the ALTCHA proof-of-work), `map` (payloads and their mapping), `gateway` (the WebSocket).

mod api;
mod captcha;
mod discovery;
mod gateway;
mod map;

use async_trait::async_trait;
use tokio::sync::mpsc;

use chirper_bridge::model::{
    Ack, BackfillPage, Capabilities, Event, InputField, LoginFlow, LoginStep, PortalRef, RemoteUser,
};
use chirper_bridge::network::LoginInput;
use chirper_bridge::{Command, Error, LoginCtx, LoginProcess, Network, Result, Session};

use api::{
    CallErr, Client, Credential, LoginResult, MessageBody, MessageQuery, MessageRef, Upload,
};
use discovery::{DEFAULT_INSTANCE, Endpoints, Policy, discover, normalize_origin};
use gateway::{GatewayCtx, LoginStore};
use map::User;

/// The network id used in settings, account ids and every stored row. Never changes.
pub const NETWORK_ID: &str = "fluxer";

/// The largest file this driver uploads. Instances publish their own limit; this is the default.
const MAX_UPLOAD_BYTES: usize = 25 << 20;
/// The largest page the message list accepts.
const MAX_BACKFILL_PAGE: usize = 100;
/// The longest nonce the create endpoint accepts.
const MAX_NONCE_CHARS: usize = 32;

const FLOW_TOKEN: &str = "token";
const FLOW_BOT: &str = "bot";
const FLOW_PASSWORD: &str = "password";

/// The Fluxer driver. Clones share the per-login gateway state.
#[derive(Debug, Clone, Default)]
pub struct FluxerNet {
    store: LoginStore,
}

impl FluxerNet {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Network for FluxerNet {
    fn id(&self) -> &'static str {
        NETWORK_ID
    }

    fn name(&self) -> &'static str {
        "Fluxer"
    }

    fn color(&self) -> &'static str {
        "#FF77A9"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            edits: true,
            reactions: true,
            replies: true,
            redactions: true,
            typing: true,
            read_receipts: true,
            stickers: false,
            voice: false,
        }
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        vec![
            LoginFlow {
                id: FLOW_TOKEN.to_owned(),
                name: "Session token".to_owned(),
                description: "Sign in with a session token from the Fluxer app".to_owned(),
            },
            LoginFlow {
                id: FLOW_BOT.to_owned(),
                name: "Bot token".to_owned(),
                description: "Sign in with the token of a Fluxer bot account".to_owned(),
            },
            LoginFlow {
                id: FLOW_PASSWORD.to_owned(),
                name: "Email or username & password".to_owned(),
                description: "Sign in with a password; authenticator codes are supported"
                    .to_owned(),
            },
        ]
    }

    async fn start_login(&self, flow_id: &str, ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        let flow = match flow_id {
            FLOW_TOKEN => Flow::Token,
            FLOW_BOT => Flow::Bot,
            FLOW_PASSWORD => Flow::Password,
            _ => return Err(Error::Unsupported("this login flow")),
        };
        let remote = HttpRemote {
            http: ctx.http.clone(),
        };
        Ok(Box::new(FluxerLogin::new(flow, ctx, remote)))
    }

    async fn connect(&self, ctx: LoginCtx) -> Result<Box<dyn Session>> {
        connect_session(self.store.clone(), ctx).await
    }
}

/// The three sign-in flows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Token,
    Bot,
    Password,
}

/// A discovered instance: its normalized origin and its endpoints.
struct Instance {
    origin: String,
    endpoints: Endpoints,
}

/// The network calls a sign-in makes. The real one talks to the instance; tests use a mock.
#[async_trait]
trait Remote: Send + Sync {
    /// Normalize the instance input (empty means fluxer.app) and discover its endpoints.
    async fn instance(&self, input: &str) -> Result<Instance>;
    /// `GET /users/@me` with the credential: validates a token and returns the account.
    async fn verify(&self, endpoints: &Endpoints, cred: Credential) -> Result<User>;
    async fn password_login(
        &self,
        endpoints: &Endpoints,
        login: &str,
        password: &str,
    ) -> std::result::Result<LoginResult, CallErr>;
    async fn totp_login(
        &self,
        endpoints: &Endpoints,
        ticket: &str,
        code: &str,
    ) -> std::result::Result<LoginResult, CallErr>;
}

/// The real [`Remote`]: the instance's HTTP API.
struct HttpRemote {
    http: reqwest::Client,
}

#[async_trait]
impl Remote for HttpRemote {
    async fn instance(&self, input: &str) -> Result<Instance> {
        let origin = normalize_origin(input, Policy::strict())?;
        let endpoints = discover(&self.http, &origin, Policy::strict()).await?;
        Ok(Instance { origin, endpoints })
    }

    async fn verify(&self, endpoints: &Endpoints, cred: Credential) -> Result<User> {
        Client::new(self.http.clone(), endpoints.clone(), cred)
            .me()
            .await
    }

    async fn password_login(
        &self,
        endpoints: &Endpoints,
        login: &str,
        password: &str,
    ) -> std::result::Result<LoginResult, CallErr> {
        Client::new(
            self.http.clone(),
            endpoints.clone(),
            Credential::anonymous(),
        )
        .login(login, password)
        .await
    }

    async fn totp_login(
        &self,
        endpoints: &Endpoints,
        ticket: &str,
        code: &str,
    ) -> std::result::Result<LoginResult, CallErr> {
        Client::new(
            self.http.clone(),
            endpoints.clone(),
            Credential::anonymous(),
        )
        .login_totp(ticket, code)
        .await
    }
}

/// Where a sign-in is.
#[derive(Clone)]
enum Stage {
    /// Nothing asked yet.
    Start,
    /// The form is shown and waits for input.
    Form,
    /// A password sign-in waits for the authenticator code.
    Code {
        origin: String,
        endpoints: Endpoints,
        ticket: String,
    },
    /// The account is saved.
    Done,
}

/// One sign-in, driven by the user through its steps.
struct FluxerLogin<R> {
    ctx: LoginCtx,
    remote: R,
    flow: Flow,
    stage: Stage,
}

impl<R: Remote> FluxerLogin<R> {
    fn new(flow: Flow, ctx: LoginCtx, remote: R) -> Self {
        Self {
            ctx,
            remote,
            flow,
            stage: Stage::Start,
        }
    }

    fn instance_field() -> InputField {
        InputField {
            id: "instance".to_owned(),
            name: "Instance".to_owned(),
            kind: "text".to_owned(),
            description: format!("Fluxer instance URL. Leave empty for {DEFAULT_INSTANCE}"),
            options: Vec::new(),
            pattern: String::new(),
        }
    }

    fn form(&self) -> LoginStep {
        match self.flow {
            Flow::Token | Flow::Bot => LoginStep::UserInput {
                step_id: "token".to_owned(),
                instructions: "Enter the instance and token".to_owned(),
                fields: vec![
                    Self::instance_field(),
                    InputField {
                        id: "token".to_owned(),
                        name: "Token".to_owned(),
                        kind: "password".to_owned(),
                        description: "A session token, or the token of a bot account".to_owned(),
                        options: Vec::new(),
                        pattern: String::new(),
                    },
                ],
            },
            Flow::Password => LoginStep::UserInput {
                step_id: "password".to_owned(),
                instructions: "Enter your instance, email or username, and password".to_owned(),
                fields: vec![
                    Self::instance_field(),
                    InputField {
                        id: "login".to_owned(),
                        name: "Email or username".to_owned(),
                        kind: "text".to_owned(),
                        description: "The email address or username of the account".to_owned(),
                        options: Vec::new(),
                        pattern: String::new(),
                    },
                    InputField {
                        id: "password".to_owned(),
                        name: "Password".to_owned(),
                        kind: "password".to_owned(),
                        description: "The account password".to_owned(),
                        options: Vec::new(),
                        pattern: String::new(),
                    },
                ],
            },
        }
    }

    fn code_step() -> LoginStep {
        LoginStep::UserInput {
            step_id: "password_mfa".to_owned(),
            instructions: "Enter the 6-digit code from your authenticator app, or a backup code"
                .to_owned(),
            fields: vec![InputField {
                id: "code".to_owned(),
                name: "Code".to_owned(),
                kind: "text".to_owned(),
                description: "Authenticator or backup code".to_owned(),
                options: Vec::new(),
                pattern: String::new(),
            }],
        }
    }

    async fn submit(&mut self, input: &LoginInput) -> Result<LoginStep> {
        match self.flow {
            Flow::Token | Flow::Bot => self.submit_token(input).await,
            Flow::Password => self.submit_password(input).await,
        }
    }

    async fn submit_token(&mut self, input: &LoginInput) -> Result<LoginStep> {
        let token = required(input, "token")?;
        let instance = self.remote.instance(optional(input, "instance")).await?;
        let bot = self.flow == Flow::Bot;
        let user = self
            .remote
            .verify(&instance.endpoints, Credential::new(token.clone(), bot))
            .await?;
        self.finish(&instance.origin, &token, bot, &user)
    }

    async fn submit_password(&mut self, input: &LoginInput) -> Result<LoginStep> {
        let login = required(input, "login")?;
        let password = required(input, "password")?;
        let instance = self.remote.instance(optional(input, "instance")).await?;
        let answer = self
            .remote
            .password_login(&instance.endpoints, &login, &password)
            .await
            .map_err(login_error)?;
        if !answer.mfa {
            let user = account_of(&answer)?;
            return self.finish(&instance.origin, &answer.token, false, &user);
        }
        if !(answer.totp || answer.backup_codes) {
            return Err(Error::Unsupported(
                "signing in with a passkey as the second factor",
            ));
        }
        if answer.ticket.is_empty() {
            return Err(Error::Protocol("Fluxer sent no MFA ticket".to_owned()));
        }
        self.stage = Stage::Code {
            origin: instance.origin,
            endpoints: instance.endpoints,
            ticket: answer.ticket,
        };
        Ok(Self::code_step())
    }

    async fn submit_code(&mut self, input: &LoginInput) -> Result<LoginStep> {
        let Stage::Code {
            origin,
            endpoints,
            ticket,
        } = self.stage.clone()
        else {
            return Err(Error::Internal("no MFA ticket is pending".to_owned()));
        };
        let code = required(input, "code")?;
        let answer = self
            .remote
            .totp_login(&endpoints, &ticket, &code)
            .await
            .map_err(login_error)?;
        let user = account_of(&answer)?;
        self.finish(&origin, &answer.token, false, &user)
    }

    /// Save the credential under the provisional login, then name the account as the login id.
    /// The core moves the secrets when it sees the real id.
    fn finish(&mut self, origin: &str, token: &str, bot: bool, user: &User) -> Result<LoginStep> {
        if token.is_empty() {
            return Err(Error::Protocol("Fluxer sent no session token".to_owned()));
        }
        if user.id.is_empty() {
            return Err(Error::Protocol("Fluxer sent no account id".to_owned()));
        }
        self.ctx.set_secret("token", token.as_bytes())?;
        self.ctx.set_secret("instance", origin.as_bytes())?;
        self.ctx.set_secret("bot", if bot { b"1" } else { b"0" })?;
        self.stage = Stage::Done;
        Ok(LoginStep::Complete {
            login_id: user.id.clone(),
        })
    }
}

#[async_trait]
impl<R: Remote + 'static> LoginProcess for FluxerLogin<R> {
    async fn next(&mut self, input: LoginInput) -> Result<LoginStep> {
        match self.stage {
            Stage::Done => Err(Error::Protocol("the login already finished".to_owned())),
            Stage::Start => {
                self.stage = Stage::Form;
                Ok(self.form())
            }
            Stage::Form => self.submit(&input).await,
            Stage::Code { .. } => self.submit_code(&input).await,
        }
    }

    async fn cancel(&mut self) {
        if !matches!(self.stage, Stage::Done) {
            for name in ["token", "instance", "bot"] {
                let _ = self.ctx.delete_secret(name);
            }
        }
    }
}

/// A submitted value, required to be non-empty after trimming.
fn required(input: &LoginInput, field: &str) -> Result<String> {
    input
        .values
        .get(field)
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Error::Protocol(format!("no {field} was entered")))
}

/// A submitted value that may be empty. An empty instance means the default.
fn optional<'a>(input: &'a LoginInput, field: &str) -> &'a str {
    input.values.get(field).map_or("", |v| v.trim())
}

/// The account of a sign-in answer, from the embedded user or the bare user id.
fn account_of(answer: &LoginResult) -> Result<User> {
    if !answer.user.id.is_empty() {
        return Ok(answer.user.clone());
    }
    if answer.user_id.is_empty() {
        return Err(Error::Protocol("Fluxer sent no account".to_owned()));
    }
    Ok(User {
        id: answer.user_id.clone(),
        ..Default::default()
    })
}

/// Turn a failed sign-in call into a message that is safe to show the user.
fn login_error(err: CallErr) -> Error {
    match err {
        CallErr::Api(api) if api.has_code("IP_AUTHORIZATION_REQUIRED") => {
            Error::Unsupported("signing in from a new IP address that needs email authorization")
        }
        CallErr::Api(api) if api.status == 401 => {
            Error::Protocol(format!("Fluxer rejected the login: {}", api.describe()))
        }
        other => other.into(),
    }
}

/// Build a live session from the stored credential: discovery, the account check, the Gateway task
/// and the command channel.
async fn connect_session(store: LoginStore, ctx: LoginCtx) -> Result<Box<dyn Session>> {
    let token = ctx
        .get_secret("token")?
        .ok_or_else(|| Error::Auth("no Fluxer credential is stored".to_owned()))?;
    let origin = ctx
        .get_secret("instance")?
        .ok_or_else(|| Error::Auth("no Fluxer instance is stored".to_owned()))?;
    let bot = ctx.get_secret("bot")?.is_some_and(|v| v.as_slice() == b"1");
    let origin = String::from_utf8(origin.to_vec())
        .map_err(|_| Error::Auth("the stored Fluxer instance is not text".to_owned()))?;
    let token = String::from_utf8(token.to_vec())
        .map_err(|_| Error::Auth("the stored Fluxer token is not text".to_owned()))?;

    let endpoints = discover(&ctx.http, &origin, Policy::strict()).await?;
    let client = Client::new(ctx.http.clone(), endpoints, Credential::new(token, bot));
    let me = client.me().await?;
    let gateway_url = if bot {
        let url = client.bot_gateway_url().await?;
        Policy::strict().check_url(&url, "wss", "ws")?;
        url
    } else {
        client.endpoints().gateway.clone()
    };

    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(gateway::run(GatewayCtx {
        url: gateway_url,
        client: client.clone(),
        login_id: ctx.login_id.clone(),
        me: me.clone(),
        tx,
        store,
    }));
    Ok(Box::new(FluxerSession {
        client,
        me,
        events: rx,
    }))
}

/// One live Fluxer connection: the Gateway task feeds `events`, commands go over REST.
struct FluxerSession {
    client: Client,
    me: User,
    events: mpsc::UnboundedReceiver<Event>,
}

/// The nonce a create call sends, so that a retry does not duplicate the message.
fn nonce_for(txn_id: &str) -> Option<String> {
    (!txn_id.is_empty() && txn_id.chars().count() <= MAX_NONCE_CHARS).then(|| txn_id.to_owned())
}

fn reply_ref(portal: &PortalRef, reply_to: Option<String>) -> Option<MessageRef> {
    reply_to.map(|id| MessageRef {
        message_id: Some(id),
        channel_id: Some(portal.id.clone()),
    })
}

#[async_trait]
impl Session for FluxerSession {
    async fn next(&mut self) -> Option<Event> {
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
                let body = MessageBody {
                    content: Some(text),
                    nonce: nonce_for(&txn_id),
                    message_reference: reply_ref(&portal, reply_to),
                    attachments: Vec::new(),
                };
                let sent = self.client.create_message(&portal.id, &body, &[]).await?;
                Ok(Ack {
                    remote_event_id: Some(sent.id),
                })
            }
            Command::SendMedia {
                portal,
                data,
                mime,
                file_name,
                caption,
                reply_to,
                txn_id,
                ..
            } => {
                if data.len() > MAX_UPLOAD_BYTES {
                    return Err(Error::Protocol("the file is larger than 25 MiB".to_owned()));
                }
                let upload = Upload {
                    id: 0,
                    filename: if file_name.is_empty() {
                        "file".to_owned()
                    } else {
                        file_name
                    },
                    content_type: mime,
                    data: data.to_vec(),
                };
                let body = MessageBody {
                    content: (!caption.is_empty()).then_some(caption),
                    nonce: nonce_for(&txn_id),
                    message_reference: reply_ref(&portal, reply_to),
                    attachments: Vec::new(),
                };
                let sent = self
                    .client
                    .create_message(&portal.id, &body, std::slice::from_ref(&upload))
                    .await?;
                Ok(Ack {
                    remote_event_id: Some(sent.id),
                })
            }
            Command::Edit {
                portal,
                target,
                text,
            } => {
                self.client.edit_message(&portal.id, &target, &text).await?;
                Ok(Ack::default())
            }
            Command::Redact { portal, target } => {
                self.client.delete_message(&portal.id, &target).await?;
                Ok(Ack::default())
            }
            Command::React {
                portal,
                target,
                key,
            } => {
                self.client
                    .set_reaction(&portal.id, &target, &key, true)
                    .await?;
                Ok(Ack::default())
            }
            Command::Unreact {
                portal,
                target,
                key,
            } => {
                self.client
                    .set_reaction(&portal.id, &target, &key, false)
                    .await?;
                Ok(Ack::default())
            }
            Command::Read { portal, event } => {
                // Bots cannot acknowledge messages, so their receipts are dropped.
                if self.client.credential().is_bot() {
                    return Ok(Ack::default());
                }
                if let Some(message) = event {
                    self.client.ack(&portal.id, &message).await?;
                }
                Ok(Ack::default())
            }
            Command::Typing { portal, typing } => {
                if typing {
                    self.client.typing(&portal.id).await?;
                }
                Ok(Ack::default())
            }
        }
    }

    async fn backfill(
        &mut self,
        portal: &PortalRef,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<BackfillPage> {
        let count = limit.clamp(1, MAX_BACKFILL_PAGE);
        let page = self
            .client
            .messages(
                &portal.id,
                MessageQuery {
                    limit: count,
                    before: cursor.as_deref(),
                },
            )
            .await?;
        // The API lists newest first; the oldest message of the page is the next cursor.
        let next = page.last().map(|m| m.id.clone());
        let has_more = page.len() == count;
        let mut messages = Vec::new();
        for message in page.iter().rev() {
            messages.extend(
                map::messages_from(&self.client, message, portal, &self.me.id, false).await,
            );
        }
        Ok(BackfillPage {
            messages,
            cursor: next,
            has_more,
        })
    }

    async fn logout_remote(&mut self) -> Result<()> {
        // Bot tokens are kept, because the bot's owner manages them.
        if self.client.credential().is_bot() {
            return Ok(());
        }
        self.client.logout().await
    }

    fn user(&self) -> Option<RemoteUser> {
        Some(map::remote_user(&self.me, self.client.media_base()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use super::*;
    use chirper_bridge::network::{LoginInput, Secrets};
    use zeroize::Zeroizing;

    /// An in-memory secret store, as the core provides it.
    #[derive(Default)]
    struct MemSecrets(Mutex<HashMap<String, Vec<u8>>>);

    impl Secrets for MemSecrets {
        fn get(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
            Ok(self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(key)
                .cloned()
                .map(Zeroizing::new))
        }

        fn set(&self, key: &str, value: &[u8]) -> Result<()> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(key.to_owned(), value.to_vec());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<()> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(key);
            Ok(())
        }
    }

    fn ctx(secrets: Arc<MemSecrets>) -> LoginCtx {
        LoginCtx {
            network: NETWORK_ID.to_owned(),
            login_id: "provisional".to_owned(),
            data_dir: PathBuf::new(),
            secrets,
            http: reqwest::Client::new(),
        }
    }

    fn values(pairs: &[(&str, &str)]) -> LoginInput {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        LoginInput::values(map)
    }

    fn user(id: &str) -> User {
        User {
            id: id.to_owned(),
            username: "ada".to_owned(),
            ..Default::default()
        }
    }

    fn answer(json: &str) -> LoginResult {
        serde_json::from_str(json).expect("test answer is valid JSON")
    }

    /// A [`Remote`] with canned answers that records what it was asked.
    #[derive(Default)]
    struct MockRemote {
        account: Option<User>,
        reject_token: bool,
        password: Option<LoginResult>,
        password_api_code: Option<&'static str>,
        totp: Option<LoginResult>,
        seen: Mutex<Vec<String>>,
    }

    impl MockRemote {
        fn note(&self, what: String) {
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(what);
        }

        fn calls(&self) -> Vec<String> {
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    fn endpoints() -> Endpoints {
        Endpoints {
            api: "https://api.example.test".to_owned(),
            gateway: "wss://gw.example.test".to_owned(),
            media: String::new(),
        }
    }

    #[async_trait]
    impl Remote for MockRemote {
        async fn instance(&self, input: &str) -> Result<Instance> {
            self.note(format!("instance:{input}"));
            let origin = if input.is_empty() {
                DEFAULT_INSTANCE.to_owned()
            } else {
                input.to_owned()
            };
            Ok(Instance {
                origin,
                endpoints: endpoints(),
            })
        }

        async fn verify(&self, _endpoints: &Endpoints, cred: Credential) -> Result<User> {
            self.note(format!("verify:bot={}", cred.is_bot()));
            if self.reject_token {
                return Err(Error::Auth("Fluxer refused the token".to_owned()));
            }
            Ok(self.account.clone().unwrap_or_else(|| user("42")))
        }

        async fn password_login(
            &self,
            _endpoints: &Endpoints,
            login: &str,
            password: &str,
        ) -> std::result::Result<LoginResult, CallErr> {
            self.note(format!("login:{login}:{}", password.len()));
            if let Some(code) = self.password_api_code {
                return Err(CallErr::Api(api::ApiError::from_response(
                    400,
                    None,
                    format!(r#"{{"code":"{code}"}}"#).as_bytes(),
                )));
            }
            Ok(self.password.clone().unwrap_or_default())
        }

        async fn totp_login(
            &self,
            _endpoints: &Endpoints,
            ticket: &str,
            code: &str,
        ) -> std::result::Result<LoginResult, CallErr> {
            self.note(format!("totp:{ticket}:{code}"));
            Ok(self.totp.clone().unwrap_or_default())
        }
    }

    #[test]
    fn the_driver_describes_itself() {
        let net = FluxerNet::new();
        assert_eq!(net.id(), NETWORK_ID);
        assert_eq!(net.name(), "Fluxer");
        let caps = net.capabilities();
        assert!(caps.edits && caps.reactions && caps.replies && caps.redactions);
        assert!(caps.typing && caps.read_receipts);
        assert!(!caps.stickers && !caps.voice);
        assert!(net.unavailable().is_none());
        let ids: Vec<String> = net.login_flows().into_iter().map(|f| f.id).collect();
        assert_eq!(ids, ["token", "bot", "password"]);
        assert!(net.supports_remote_logout());
    }

    #[tokio::test]
    async fn unknown_flow_is_unsupported() {
        let net = FluxerNet::new();
        let secrets = Arc::new(MemSecrets::default());
        let result = net.start_login("qr", ctx(secrets)).await;
        assert!(matches!(result, Err(Error::Unsupported(_))));
    }

    #[tokio::test]
    async fn token_flow_asks_for_instance_and_token_first() {
        let secrets = Arc::new(MemSecrets::default());
        let mut process = FluxerLogin::new(Flow::Token, ctx(secrets), MockRemote::default());
        let step = process.next(LoginInput::start()).await.expect("first step");
        match step {
            LoginStep::UserInput {
                step_id, fields, ..
            } => {
                assert_eq!(step_id, "token");
                let ids: Vec<&str> = fields.iter().map(|f| f.id.as_str()).collect();
                assert_eq!(ids, ["instance", "token"]);
                assert_eq!(fields[1].kind, "password");
            }
            other => panic!("expected a user-input step, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn token_flow_stores_the_credential_and_completes_with_the_user_id() {
        let secrets = Arc::new(MemSecrets::default());
        let context = ctx(secrets);
        let mut process = FluxerLogin::new(Flow::Token, context.clone(), MockRemote::default());
        process.next(LoginInput::start()).await.expect("first step");
        let step = process
            .next(values(&[("token", "  user-token  "), ("instance", "")]))
            .await
            .expect("token accepted");
        match step {
            LoginStep::Complete { login_id } => assert_eq!(login_id, "42"),
            other => panic!("expected completion, got {other:?}"),
        }
        let stored = |name: &str| {
            context
                .get_secret(name)
                .expect("readable")
                .map(|v| v.to_vec())
        };
        assert_eq!(stored("token").as_deref(), Some(b"user-token".as_slice()));
        assert_eq!(
            stored("instance").as_deref(),
            Some(b"https://fluxer.app".as_slice())
        );
        assert_eq!(stored("bot").as_deref(), Some(b"0".as_slice()));
    }

    #[tokio::test]
    async fn bot_flow_is_stored_as_a_bot() {
        let secrets = Arc::new(MemSecrets::default());
        let context = ctx(secrets);
        let mut process = FluxerLogin::new(Flow::Bot, context.clone(), MockRemote::default());
        process.next(LoginInput::start()).await.expect("first step");
        process
            .next(values(&[("token", "123.secret")]))
            .await
            .expect("accepted");
        assert_eq!(
            context
                .get_secret("bot")
                .expect("readable")
                .map(|v| v.to_vec())
                .as_deref(),
            Some(b"1".as_slice())
        );
    }

    #[tokio::test]
    async fn a_rejected_token_is_an_auth_error_and_the_form_stays_open() {
        let secrets = Arc::new(MemSecrets::default());
        let remote = MockRemote {
            reject_token: true,
            ..Default::default()
        };
        let mut process = FluxerLogin::new(Flow::Token, ctx(secrets), remote);
        process.next(LoginInput::start()).await.expect("first step");
        let err = process
            .next(values(&[("token", "bad")]))
            .await
            .expect_err("rejected");
        assert!(matches!(err, Error::Auth(_)));
        // The process did not finish, so the user may try again.
        assert!(!matches!(process.stage, Stage::Done));
    }

    #[tokio::test]
    async fn an_empty_token_is_refused_before_any_call() {
        let secrets = Arc::new(MemSecrets::default());
        let remote = MockRemote::default();
        let mut process = FluxerLogin::new(Flow::Token, ctx(secrets), remote);
        process.next(LoginInput::start()).await.expect("first step");
        let err = process
            .next(values(&[("token", "   ")]))
            .await
            .expect_err("empty");
        assert!(matches!(err, Error::Protocol(_)));
        assert!(
            process.remote.calls().is_empty(),
            "no request for an empty token"
        );
    }

    #[tokio::test]
    async fn password_flow_without_mfa_completes_directly() {
        let secrets = Arc::new(MemSecrets::default());
        let context = ctx(secrets);
        let remote = MockRemote {
            password: Some(answer(
                r#"{"token":"session-tok","user_id":"7","user":{"id":"7","username":"me"}}"#,
            )),
            ..Default::default()
        };
        let mut process = FluxerLogin::new(Flow::Password, context.clone(), remote);
        process.next(LoginInput::start()).await.expect("first step");
        let step = process
            .next(values(&[
                ("login", "me@example.com"),
                ("password", "hunter2hunter2"),
            ]))
            .await
            .expect("password accepted");
        assert!(matches!(step, LoginStep::Complete { ref login_id } if login_id == "7"));
        assert_eq!(
            context
                .get_secret("token")
                .expect("readable")
                .map(|v| v.to_vec())
                .as_deref(),
            Some(b"session-tok".as_slice())
        );
    }

    #[tokio::test]
    async fn password_flow_with_an_authenticator_asks_for_the_code_then_completes() {
        let secrets = Arc::new(MemSecrets::default());
        let remote = MockRemote {
            password: Some(answer(
                r#"{"mfa":true,"ticket":"tk","totp":true,"allowed_methods":["totp"]}"#,
            )),
            totp: Some(answer(
                r#"{"token":"after-mfa","user_id":"7","user":{"id":"7","username":"me"}}"#,
            )),
            ..Default::default()
        };
        let mut process = FluxerLogin::new(Flow::Password, ctx(secrets), remote);
        process.next(LoginInput::start()).await.expect("first step");
        let step = process
            .next(values(&[("login", "me"), ("password", "pw")]))
            .await
            .expect("password accepted");
        match step {
            LoginStep::UserInput {
                step_id, fields, ..
            } => {
                assert_eq!(step_id, "password_mfa");
                assert_eq!(fields[0].id, "code");
            }
            other => panic!("expected the MFA step, got {other:?}"),
        }
        let step = process
            .next(values(&[("code", "123456")]))
            .await
            .expect("code");
        assert!(matches!(step, LoginStep::Complete { ref login_id } if login_id == "7"));
        assert!(
            process
                .remote
                .calls()
                .contains(&"totp:tk:123456".to_owned())
        );
    }

    #[tokio::test]
    async fn a_passkey_only_second_factor_is_unsupported() {
        let secrets = Arc::new(MemSecrets::default());
        let remote = MockRemote {
            password: Some(answer(r#"{"mfa":true,"ticket":"tk","webauthn":true}"#)),
            ..Default::default()
        };
        let mut process = FluxerLogin::new(Flow::Password, ctx(secrets), remote);
        process.next(LoginInput::start()).await.expect("first step");
        let err = process
            .next(values(&[("login", "me"), ("password", "pw")]))
            .await
            .expect_err("passkey only");
        assert!(matches!(err, Error::Unsupported(_)));
    }

    #[tokio::test]
    async fn a_new_ip_needs_email_authorization_and_says_so() {
        let secrets = Arc::new(MemSecrets::default());
        let remote = MockRemote {
            password_api_code: Some("IP_AUTHORIZATION_REQUIRED"),
            ..Default::default()
        };
        let mut process = FluxerLogin::new(Flow::Password, ctx(secrets), remote);
        process.next(LoginInput::start()).await.expect("first step");
        let err = process
            .next(values(&[("login", "me"), ("password", "pw")]))
            .await
            .expect_err("needs authorization");
        assert!(matches!(err, Error::Unsupported(text) if text.contains("IP")));
    }

    #[tokio::test]
    async fn a_wrong_password_is_a_protocol_error_not_a_logout() {
        let secrets = Arc::new(MemSecrets::default());
        let remote = MockRemote {
            password_api_code: Some("INVALID_LOGIN"),
            ..Default::default()
        };
        let mut process = FluxerLogin::new(Flow::Password, ctx(secrets), remote);
        process.next(LoginInput::start()).await.expect("first step");
        let err = process
            .next(values(&[("login", "me"), ("password", "pw")]))
            .await
            .expect_err("wrong password");
        assert!(matches!(err, Error::Protocol(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn a_finished_login_takes_no_more_input() {
        let secrets = Arc::new(MemSecrets::default());
        let mut process = FluxerLogin::new(Flow::Token, ctx(secrets), MockRemote::default());
        process.next(LoginInput::start()).await.expect("first step");
        process
            .next(values(&[("token", "t")]))
            .await
            .expect("accepted");
        let err = process
            .next(values(&[("token", "again")]))
            .await
            .expect_err("done");
        assert!(matches!(err, Error::Protocol(_)));
    }

    #[tokio::test]
    async fn cancel_removes_what_an_unfinished_login_stored() {
        let secrets = Arc::new(MemSecrets::default());
        let context = ctx(secrets);
        context.set_secret("token", b"leftover").expect("stored");
        context
            .set_secret("instance", b"https://fluxer.app")
            .expect("stored");
        let mut process = FluxerLogin::new(Flow::Token, context.clone(), MockRemote::default());
        process.cancel().await;
        assert!(context.get_secret("token").expect("readable").is_none());
        assert!(context.get_secret("instance").expect("readable").is_none());
    }

    #[tokio::test]
    async fn instance_input_is_passed_to_discovery() {
        let secrets = Arc::new(MemSecrets::default());
        let mut process = FluxerLogin::new(Flow::Token, ctx(secrets), MockRemote::default());
        process.next(LoginInput::start()).await.expect("first step");
        process
            .next(values(&[
                ("token", "t"),
                ("instance", "https://chat.example"),
            ]))
            .await
            .expect("accepted");
        assert!(
            process
                .remote
                .calls()
                .contains(&"instance:https://chat.example".to_owned())
        );
    }

    #[tokio::test]
    async fn connect_without_a_stored_credential_is_an_auth_error() {
        let net = FluxerNet::new();
        let secrets = Arc::new(MemSecrets::default());
        match net.connect(ctx(secrets)).await {
            Err(Error::Auth(_)) => {}
            Err(other) => panic!("expected an auth error, got {other:?}"),
            Ok(_) => panic!("connect must not succeed without credentials"),
        }
    }

    #[test]
    fn nonces_follow_the_create_endpoint_limit() {
        assert_eq!(nonce_for("abc").as_deref(), Some("abc"));
        assert_eq!(nonce_for(""), None);
        assert_eq!(nonce_for(&"x".repeat(33)), None);
        assert!(nonce_for(&"x".repeat(32)).is_some());
    }

    #[test]
    fn replies_name_the_target_and_the_channel() {
        let portal = PortalRef {
            id: "10".to_owned(),
            receiver: String::new(),
        };
        let reference = reply_ref(&portal, Some("99".to_owned())).expect("a reply");
        assert_eq!(reference.message_id.as_deref(), Some("99"));
        assert_eq!(reference.channel_id.as_deref(), Some("10"));
        assert!(reply_ref(&portal, None).is_none());
    }

    #[test]
    fn login_error_for_a_bad_status_is_not_auth() {
        let err = login_error(CallErr::Api(api::ApiError::from_response(
            401,
            None,
            br#"{"code":"INVALID_LOGIN"}"#,
        )));
        assert!(matches!(err, Error::Protocol(_)));
    }

    #[test]
    fn account_comes_from_the_user_object_or_the_bare_id() {
        let embedded = answer(r#"{"user_id":"1","user":{"id":"2"}}"#);
        assert_eq!(account_of(&embedded).expect("account").id, "2");
        let bare = answer(r#"{"user_id":"3"}"#);
        assert_eq!(account_of(&bare).expect("account").id, "3");
        assert!(account_of(&answer("{}")).is_err());
    }

    #[test]
    fn required_fields_are_trimmed_and_non_empty() {
        let input = values(&[("token", "  t  "), ("blank", "   ")]);
        assert_eq!(required(&input, "token").expect("present"), "t");
        assert!(required(&input, "blank").is_err());
        assert!(required(&input, "missing").is_err());
        assert_eq!(optional(&input, "missing"), "");
    }
}
