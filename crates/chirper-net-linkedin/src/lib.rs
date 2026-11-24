//! LinkedIn driver for Chirper's unified bridge runtime.
//!
//! Messaging over LinkedIn's own web API, as the reference's `linkedingo` client does it: a
//! cookie login or a password login, the chat list, the realtime event stream (server-sent events with a heartbeat),
//! sends, edits, recalls, reactions, read state, typing and backfill. There is no bridge library
//! and no Matrix type in this crate; ids match the Go bridge's.

#![forbid(unsafe_code)]

mod api;
mod convert;
mod fmt;
mod ids;
mod login;
mod realtime;
mod session;

use async_trait::async_trait;

use chirper_bridge::model::{Capabilities, LoginFlow};
use chirper_bridge::{Error, LoginCtx, LoginProcess, Network, Result, Session};

/// The network id of LinkedIn, used in settings, account ids and every stored row. Never changes.
pub const NETWORK_ID: &str = "linkedin";

/// The LinkedIn driver.
#[derive(Debug, Clone)]
pub struct LinkedIn {
    base: String,
}

impl Default for LinkedIn {
    fn default() -> Self {
        Self::new()
    }
}

impl LinkedIn {
    /// The driver against linkedin.com.
    pub fn new() -> Self {
        Self::with_base(api::WWW)
    }

    /// A driver pointed at another origin that speaks the same API (tests, a local mirror).
    pub fn with_base(base: impl Into<String>) -> Self {
        Self { base: base.into() }
    }
}

#[async_trait]
impl Network for LinkedIn {
    fn id(&self) -> &'static str {
        NETWORK_ID
    }

    fn name(&self) -> &'static str {
        "LinkedIn"
    }

    fn color(&self) -> &'static str {
        "#FF2D7A"
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
            voice: true,
        }
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        vec![
            LoginFlow {
                id: login::FLOW_COOKIES.to_owned(),
                name: "Cookies".to_owned(),
                description: "Sign in to linkedin.com and let Chirper read the session cookies."
                    .to_owned(),
            },
            LoginFlow {
                id: login::FLOW_PASSWORD.to_owned(),
                name: "Email and password".to_owned(),
                description: "Log in using your email/phone and password".to_owned(),
            },
        ]
    }

    async fn start_login(&self, flow_id: &str, ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        match flow_id {
            login::FLOW_COOKIES => Ok(Box::new(login::CookieLogin::new(ctx, &self.base))),
            login::FLOW_PASSWORD => Ok(Box::new(login::PasswordLogin::new(ctx, &self.base))),
            _ => Err(Error::Unsupported("this login flow")),
        }
    }

    async fn connect(&self, ctx: LoginCtx) -> Result<Box<dyn Session>> {
        session::connect(ctx, &self.base).await
    }
}
