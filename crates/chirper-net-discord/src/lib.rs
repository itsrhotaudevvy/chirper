//! The Discord driver: a user token or bot token, the v10 REST API and the gateway.
//!
//! It is a bridge runtime driver, not a bridge: it reports chats, messages and reactions as domain
//! events and accepts commands, with no Matrix in between. Everything the old Go bridge did for
//! Discord — portals, puppet ids, per-bridge databases — is now the runtime's and the engine's job.

mod api;
mod gateway;
mod map;

use async_trait::async_trait;
use log::debug;
use tokio::sync::mpsc;

use chirper_bridge::model::{
    Ack, BackfillPage, Capabilities, Content, LoginFlow, LoginStep, PortalRef, RemoteUser,
};
use chirper_bridge::network::LoginInput;
use chirper_bridge::{
    Command, Error, Event, LoginCtx, LoginProcess, Network, Result, Session,
};

pub use api::API;

/// The network id used in settings, account ids and every stored row.
pub const NETWORK_ID: &str = "discord";

/// The Discord driver.
#[derive(Debug, Default, Clone)]
pub struct Discord {
    base: String,
}

impl Discord {
    pub fn new() -> Self {
        Self {
            base: API.to_owned(),
        }
    }

    /// A driver pointed at another Discord-compatible API base (tests, self-hosted).
    pub fn with_base(base: impl Into<String>) -> Self {
        Self { base: base.into() }
    }
}

#[async_trait]
impl Network for Discord {
    fn id(&self) -> &'static str {
        NETWORK_ID
    }

    fn name(&self) -> &'static str {
        "Discord"
    }

    fn color(&self) -> &'static str {
        "#5865F2"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            edits: true,
            reactions: true,
            replies: true,
            redactions: true,
            typing: true,
            // Discord has no read receipts.
            read_receipts: false,
            stickers: true,
            voice: false,
        }
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        vec![LoginFlow {
            id: "token".to_owned(),
            name: "Account token".to_owned(),
            description: "Paste a Discord user token, or a bot token starting with `Bot `."
                .to_owned(),
        }]
    }

    async fn start_login(&self, flow_id: &str, ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        if flow_id != "token" {
            return Err(Error::Unsupported("this login flow"));
        }
        Ok(Box::new(TokenLogin {
            ctx,
            base: self.base.clone(),
            done: false,
        }))
    }

    async fn connect(&self, ctx: LoginCtx) -> Result<Box<dyn Session>> {
        let secret = ctx
            .get_secret("token")?
            .ok_or_else(|| Error::Auth("no Discord token is stored".to_owned()))?;
        let token = String::from_utf8(secret.to_vec())
            .map_err(|_| Error::Auth("the stored Discord token is not text".to_owned()))?;
        let api = api::Api::new(ctx.http.clone(), &token, &self.base);
        let me = api.me().await?;
        let gateway_url = api.gateway_url().await?;
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(gateway::run(api.clone(), gateway_url, token, tx));
        Ok(Box::new(DiscordSession {
            api,
            me: map::to_remote_user(&me),
            events: rx,
        }))
    }

    fn supports_remote_logout(&self) -> bool {
        false
    }
}

/// The token login: one input step, then a live account.
struct TokenLogin {
    ctx: LoginCtx,
    base: String,
    done: bool,
}

#[async_trait]
impl LoginProcess for TokenLogin {
    async fn next(&mut self, input: LoginInput) -> Result<LoginStep> {
        if self.done {
            return Err(Error::Protocol("the login already finished".to_owned()));
        }
        if input.values.is_empty() {
            return Ok(LoginStep::UserInput {
                step_id: "token".to_owned(),
                instructions: "Paste your Discord token. It is stored in the system credential store."
                    .to_owned(),
                fields: vec![chirper_bridge::model::InputField {
                    id: "token".to_owned(),
                    name: "Token".to_owned(),
                    kind: "password".to_owned(),
                    description: "A user token, or a bot token starting with `Bot `.".to_owned(),
                    options: Vec::new(),
                    pattern: String::new(),
                }],
            });
        }
        let token = input
            .values
            .get("token")
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .ok_or_else(|| Error::Auth("no token was entered".to_owned()))?;
        let api = api::Api::new(self.ctx.http.clone(), &token, &self.base);
        let me = api.me().await.map_err(|err| match err {
            Error::Network(message) => Error::Network(message),
            _ => Error::Auth("Discord refused this token".to_owned()),
        })?;
        self.ctx.set_secret("token", token.as_bytes())?;
        let bot = api::is_bot_token(&token);
        debug!("discord: logged in as {} (bot: {bot})", me.id);
        self.done = true;
        Ok(LoginStep::Complete {
            login_id: format!("discord_{}", me.id),
        })
    }

    async fn cancel(&mut self) {
        let _ = self.ctx.delete_secret("token");
    }
}

/// One live Discord connection: the gateway feeds `events`, commands go over REST.
struct DiscordSession {
    api: api::Api,
    me: RemoteUser,
    events: mpsc::UnboundedReceiver<Event>,
}

#[async_trait]
impl Session for DiscordSession {
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
                let message = self
                    .api
                    .send_text(&portal.id, &text, reply_to.as_deref())
                    .await?;
                let _ = txn_id;
                Ok(Ack {
                    remote_event_id: Some(message.id),
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
                let message = self
                    .api
                    .send_media(
                        &portal.id,
                        data.to_vec(),
                        &file_name,
                        &mime,
                        &caption,
                        reply_to.as_deref(),
                    )
                    .await?;
                let _ = txn_id;
                Ok(Ack {
                    remote_event_id: Some(message.id),
                })
            }
            Command::Edit {
                portal,
                target,
                text,
            } => {
                self.api.edit(&portal.id, &target, &text).await?;
                Ok(Ack::default())
            }
            Command::Redact { portal, target } => {
                self.api.delete(&portal.id, &target).await?;
                Ok(Ack::default())
            }
            Command::React {
                portal,
                target,
                key,
            } => {
                self.api.react(&portal.id, &target, &key, true).await?;
                Ok(Ack::default())
            }
            Command::Unreact {
                portal,
                target,
                key,
            } => {
                self.api.react(&portal.id, &target, &key, false).await?;
                Ok(Ack::default())
            }
            Command::Read { .. } => Err(Error::Unsupported("read receipts")),
            Command::Typing { portal, typing } => {
                if typing {
                    self.api.typing(&portal.id).await?;
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
        let page = self.api.history(&portal.id, cursor.as_deref()).await?;
        let mut messages = Vec::with_capacity(page.len());
        for message in page {
            if map::is_system(&message) {
                continue;
            }
            messages.push(map::to_remote_message(&self.api, &message, false).await);
        }
        let has_more = messages.len() >= limit.min(50) && messages.len() >= 2;
        let next = messages.last().map(|m| m.id.clone());
        let _ = limit;
        Ok(BackfillPage {
            messages,
            cursor: if has_more { next } else { None },
            has_more,
        })
    }

    async fn logout_remote(&mut self) -> Result<()> {
        Err(Error::Unsupported("logging out a Discord token remotely"))
    }

    fn user(&self) -> Option<RemoteUser> {
        Some(self.me.clone())
    }
}

/// Apply the caption-only rule of media messages: a caption is the body.
pub fn caption_of(content: &Content) -> String {
    content.body().to_owned()
}
