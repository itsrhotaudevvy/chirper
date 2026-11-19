//! The Telegram driver for Chirper's unified bridge runtime. Work in progress: the network is listed
//! as unavailable until its protocol client is complete.

#![forbid(unsafe_code)]

use async_trait::async_trait;

use chirper_bridge::model::{Capabilities, LoginFlow};
use chirper_bridge::{Error, LoginCtx, LoginProcess, Network, Result, Session};

/// The network id used in settings, account ids and every stored row. Never changes.
pub const NETWORK_ID: &str = "telegram";

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
        Some("Telegram is being ported to the unified Rust runtime.".to_owned())
    }

    async fn start_login(&self, _flow_id: &str, _ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        Err(Error::Unsupported("logging in to Telegram"))
    }

    async fn connect(&self, _ctx: LoginCtx) -> Result<Box<dyn Session>> {
        Err(Error::Unsupported("connecting to Telegram"))
    }
}
