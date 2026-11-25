//! The Slack driver: three login flows (email, browser token, app token), the Web API, the
//! real-time websocket and the message conversions, reported to the bridge runtime as domain events.
//!
//! Remote ids are the Go bridge's (see [`ids`]); the logic follows the mautrix Slack bridge
//! (AGPL-3.0, `third_party/slack/pkg`), reimplemented without `slack-go`.

#![forbid(unsafe_code)]

mod api;
mod blocks;
mod convert;
mod directory;
mod emoji;
mod html;
pub mod ids;
mod login;
mod login_email;
mod mrkdwn;
mod rtm;
mod session;
mod signin;
mod wire;

use async_trait::async_trait;

use chirper_bridge::model::{Capabilities, LoginFlow};
use chirper_bridge::{Error, LoginCtx, LoginProcess, Network, Result, Session};

use crate::api::SLACK_API;
use crate::login::{APP_FLOW, AppLogin, TOKEN_FLOW, TokenLogin};
use crate::login_email::EmailLogin;

/// The network id of Slack, used in settings, account ids and every stored row. Never changes.
pub const NETWORK_ID: &str = "slack";

/// Where the driver talks to Slack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Endpoints {
    /// The Web API root, ending in `/api`.
    pub api: String,
    /// The sign-in host (`https://slack.com`).
    pub root: String,
    /// The app host (`https://app.slack.com`), where magic logins and client credentials live.
    pub app: String,
}

impl Endpoints {
    /// The public Slack cloud.
    fn production() -> Self {
        Self {
            api: SLACK_API.to_owned(),
            root: "https://slack.com".to_owned(),
            app: "https://app.slack.com".to_owned(),
        }
    }

    /// Every endpoint under one base, for tests against a local server. `base` is the Web API root
    /// (`…/api`); the sign-in and app paths hang off the same host.
    fn under(base: &str) -> Self {
        let api = base.trim_end_matches('/').to_owned();
        let root = api.strip_suffix("/api").unwrap_or(&api).to_owned();
        Self {
            app: root.clone(),
            root,
            api,
        }
    }
}

/// The Slack driver.
#[derive(Debug, Clone)]
pub struct Slack {
    endpoints: Endpoints,
}

impl Default for Slack {
    fn default() -> Self {
        Self::new()
    }
}

impl Slack {
    /// The driver for the public Slack cloud.
    pub fn new() -> Self {
        Self {
            endpoints: Endpoints::production(),
        }
    }

    /// A driver whose Web API root is `base` (ending in `/api`), for tests against a local server.
    pub fn with_base(base: impl Into<String>) -> Self {
        Self {
            endpoints: Endpoints::under(&base.into()),
        }
    }
}

#[async_trait]
impl Network for Slack {
    fn id(&self) -> &'static str {
        NETWORK_ID
    }

    fn name(&self) -> &'static str {
        "Slack"
    }

    fn color(&self) -> &'static str {
        "#4A154B"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            edits: true,
            reactions: true,
            replies: true,
            redactions: true,
            typing: true,
            read_receipts: true,
            // Slack has no stickers; they arrive as images.
            stickers: false,
            voice: false,
        }
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        vec![
            LoginFlow {
                id: login_email::FLOW.to_owned(),
                name: "Email".to_owned(),
                description: "Sign in with your Slack email address".to_owned(),
            },
            LoginFlow {
                id: TOKEN_FLOW.to_owned(),
                name: "Auth token & cookie".to_owned(),
                description:
                    "Log in with an auth token (and a cookie, if the token is from a browser)"
                        .to_owned(),
            },
            LoginFlow {
                id: APP_FLOW.to_owned(),
                name: "Slack app".to_owned(),
                description: "Log in with a Slack app".to_owned(),
            },
        ]
    }

    async fn start_login(&self, flow_id: &str, ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        match flow_id {
            login_email::FLOW => {
                let http = ctx.http.clone();
                Ok(Box::new(EmailLogin::new(ctx, self.endpoints.clone(), http)))
            }
            TOKEN_FLOW => Ok(Box::new(TokenLogin {
                api_base: self.endpoints.api.clone(),
                ctx,
                done: false,
                asked: false,
            })),
            APP_FLOW => Ok(Box::new(AppLogin {
                api_base: self.endpoints.api.clone(),
                ctx,
                done: false,
            })),
            _ => Err(Error::Unsupported("this login flow")),
        }
    }

    async fn connect(&self, ctx: LoginCtx) -> Result<Box<dyn Session>> {
        let session = session::connect(&self.endpoints, ctx).await?;
        Ok(Box::new(session))
    }
}
