//! The Google Voice driver: cookie login, the Voice thread list and realtime channel, text and image
//! messages, read state, and history pages.
//!
//! It is a bridge runtime driver, not a bridge: it reports chats and messages as domain events and
//! accepts commands, with no Matrix in between. The protocol is ported from mautrix-gvoice
//! (AGPL-3.0) behaviour; see each module for its source file. The Electron helper the reference
//! uses to sign send requests is not ported: sends carry the default tracking data the reference
//! uses when the helper is absent.

#![forbid(unsafe_code)]

mod api;
mod chunks;
mod convert;
mod ids;
mod login;
mod realtime;
mod schema;
mod session;
mod wire;

use std::collections::BTreeMap;

use async_trait::async_trait;

use chirper_bridge::model::{Capabilities, LoginFlow};
use chirper_bridge::{Error, LoginCtx, LoginProcess, Network, Result, Session};

use crate::api::{Api, Hosts};
use crate::convert::Account;

/// The network id of Google Voice, used in settings, account ids and every stored row. Never changes.
pub const NETWORK_ID: &str = "gvoice";

/// The Google Voice driver.
#[derive(Debug, Clone)]
pub struct GVoice {
    hosts: Hosts,
}

impl GVoice {
    /// A driver for Google's own hosts.
    pub fn new() -> Self {
        Self {
            hosts: Hosts::production(),
        }
    }

    /// A driver whose every endpoint sits under `root` (tests, a local stand-in).
    pub fn with_base(root: impl Into<String>) -> Self {
        Self {
            hosts: Hosts::single(&root.into()),
        }
    }
}

impl Default for GVoice {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Network for GVoice {
    fn id(&self) -> &'static str {
        NETWORK_ID
    }

    fn name(&self) -> &'static str {
        "Google Voice"
    }

    fn color(&self) -> &'static str {
        "#34A853"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            edits: false,
            reactions: false,
            replies: false,
            redactions: false,
            typing: false,
            read_receipts: true,
            stickers: false,
            voice: false,
        }
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        login::flows()
    }

    async fn start_login(&self, flow_id: &str, ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        match flow_id {
            login::FLOW_COOKIES => Ok(Box::new(login::CookieLogin {
                ctx,
                hosts: self.hosts.clone(),
                done: false,
            })),
            _ => Err(Error::Unsupported("this login flow")),
        }
    }

    async fn connect(&self, ctx: LoginCtx) -> Result<Box<dyn Session>> {
        let cookies = stored_cookies(&ctx)?;
        let api = Api::new(ctx.http.clone(), self.hosts.clone(), &cookies)?;
        let account_record = api.get_account().await?;
        let account = Account {
            login_id: ctx.login_id.clone(),
            e164: login::primary_number(&account_record)?,
        };
        Ok(Box::new(session::start(api, account)))
    }

    fn supports_remote_logout(&self) -> bool {
        false
    }
}

/// The cookies the login stored, as name to value.
fn stored_cookies(ctx: &LoginCtx) -> Result<BTreeMap<String, String>> {
    let secret = ctx
        .get_secret(login::COOKIES_SECRET)?
        .ok_or_else(|| Error::Auth("no Google Voice cookies are stored".to_owned()))?;
    serde_json::from_slice(&secret)
        .map_err(|_| Error::Auth("the stored Google Voice cookies cannot be read".to_owned()))
}
