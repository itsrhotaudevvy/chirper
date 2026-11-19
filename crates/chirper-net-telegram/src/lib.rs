//! The Telegram driver for Chirper's unified bridge runtime. The protocol client (MTProto 2.0) is
//! built here from scratch; login and messaging land in the next phase, so the network is still
//! listed as unavailable.

#![forbid(unsafe_code)]

pub mod mtproto;
pub mod tl;

use async_trait::async_trait;

use chirper_bridge::model::{Capabilities, LoginFlow};
use chirper_bridge::{Error, LoginCtx, LoginProcess, Network, Result, Session};

/// The network id used in settings, account ids and every stored row. Never changes.
pub const NETWORK_ID: &str = "telegram";

/// The Telegram API id and hash this build was compiled with, if both were set.
pub fn api_credentials() -> Option<(i32, &'static str)> {
    let id = option_env!("CHIRPER_TELEGRAM_API_ID")?.parse().ok()?;
    let hash = option_env!("CHIRPER_TELEGRAM_API_HASH")?;
    Some((id, hash))
}

/// The Telegram driver.
#[derive(Debug, Default, Clone)]
pub struct Telegram {}

impl Telegram {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl Network for Telegram {
    fn id(&self) -> &'static str {
        NETWORK_ID
    }

    fn name(&self) -> &'static str {
        "Telegram"
    }

    fn color(&self) -> &'static str {
        "#2AABEE"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        Vec::new()
    }

    fn unavailable(&self) -> Option<String> {
        if api_credentials().is_none() {
            return Some(
                "This build has no Telegram API id (set CHIRPER_TELEGRAM_API_ID and CHIRPER_TELEGRAM_API_HASH)"
                    .to_owned(),
            );
        }
        Some("Telegram login lands in the next phase".to_owned())
    }

    async fn start_login(&self, _flow_id: &str, _ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        Err(Error::Unsupported("logging in to Telegram"))
    }

    async fn connect(&self, _ctx: LoginCtx) -> Result<Box<dyn Session>> {
        Err(Error::Unsupported("connecting to Telegram"))
    }
}
