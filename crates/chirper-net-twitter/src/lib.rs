//! X (Twitter) driver for Chirper's unified bridge runtime: direct messages over X's own web
//! client. Cookie login, the inbox sync, the update poll, sends, reactions, read marks, typing,
//! media, and backfill. The password flow and encrypted (XChat) conversations are not ported:
//! the password flow is listed and reports itself unavailable, and XChat messages are shown as
//! unsupported.
//!
//! There is no bridge library and no Matrix type in this crate; ids match the Go bridge's.

#![forbid(unsafe_code)]

mod api;
mod convert;
mod ids;
mod login;
mod session;

use std::time::Duration;

use async_trait::async_trait;

use chirper_bridge::model::{Capabilities, LoginFlow};
use chirper_bridge::{Error, LoginCtx, LoginProcess, Network, Result, Session};

pub use api::Bases;

/// The network id of X, used in settings, account ids and every stored row. Never changes.
pub const NETWORK_ID: &str = "twitter";

/// The X driver.
#[derive(Debug, Clone)]
pub struct Twitter {
    bases: Bases,
    interval: Duration,
}

impl Default for Twitter {
    fn default() -> Self {
        Self::new()
    }
}

impl Twitter {
    /// The driver against x.com.
    pub fn new() -> Self {
        Self::with_bases(Bases::default())
    }

    /// A driver pointed at other origins that speak the same API (tests, a local mirror).
    pub fn with_bases(bases: Bases) -> Self {
        Self {
            bases,
            interval: session::POLL_INTERVAL,
        }
    }

    /// The same driver with another update poll interval. The default is the reference's.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }
}

#[async_trait]
impl Network for Twitter {
    fn id(&self) -> &'static str {
        NETWORK_ID
    }

    fn name(&self) -> &'static str {
        "X"
    }

    fn color(&self) -> &'static str {
        "#7C5CFF"
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
                id: login::FLOW_COOKIES.to_owned(),
                name: "Cookies".to_owned(),
                description: "Sign in to x.com and let Chirper read the session cookies."
                    .to_owned(),
            },
            LoginFlow {
                id: login::FLOW_PASSWORD.to_owned(),
                name: "Password".to_owned(),
                description: "Not available yet: X's sign-in needs an anti-bot token step that \
                    this build does not include. Use the cookies sign-in."
                    .to_owned(),
            },
        ]
    }

    async fn start_login(&self, flow_id: &str, ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        match flow_id {
            login::FLOW_COOKIES => Ok(Box::new(login::CookieLogin::new(ctx, self.bases.clone()))),
            _ => Err(Error::Unsupported("this login flow")),
        }
    }

    async fn connect(&self, ctx: LoginCtx) -> Result<Box<dyn Session>> {
        let stored = ctx
            .get_secret(session::SECRET_COOKIES)?
            .ok_or_else(|| Error::Auth("no X cookies are stored".to_owned()))?;
        let map: std::collections::BTreeMap<String, String> = serde_json::from_slice(&stored)
            .map_err(|_| Error::Auth("the stored X cookies are unreadable".to_owned()))?;
        let user_agent = match ctx.get_secret(session::SECRET_SESSION)? {
            Some(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|v| {
                    v.get("user_agent")
                        .and_then(|u| u.as_str())
                        .map(str::to_owned)
                })
                .unwrap_or_default(),
            None => String::new(),
        };
        let live = session::connect(
            ctx.http.clone(),
            api::cookies::Cookies::from_map(map),
            self.bases.clone(),
            &user_agent,
            self.interval,
        )
        .await?;
        Ok(Box::new(live))
    }

    fn supports_remote_logout(&self) -> bool {
        false
    }
}
