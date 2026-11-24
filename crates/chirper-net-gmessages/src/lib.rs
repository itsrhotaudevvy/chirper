//! Google Messages driver for Chirper's unified bridge runtime. Work in progress: the network is listed as
//! unavailable until its protocol client is complete.

#![forbid(unsafe_code)]

mod api;
mod auth;
mod convert;
mod crypto;
mod emoji;
mod gaia;
mod ids;
mod link;
mod media;
mod pair;
mod pblite;
mod proto;
mod wire;

use async_trait::async_trait;

use chirper_bridge::model::{Capabilities, LoginFlow};
use chirper_bridge::{Error, LoginCtx, LoginProcess, Network, Result, Session};

/// The network id of Google Messages, used in settings, account ids and every stored row. Never changes.
pub const NETWORK_ID: &str = "gmessages";

/// The Google Messages driver.
#[derive(Debug, Default, Clone)]
pub struct GMessages {}

impl GMessages {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl Network for GMessages {
    fn id(&self) -> &'static str {
        NETWORK_ID
    }

    fn name(&self) -> &'static str {
        "Google Messages"
    }

    fn color(&self) -> &'static str {
        "#1A73E8"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        Vec::new()
    }

    fn unavailable(&self) -> Option<String> {
        Some("Google Messages is being ported to the unified Rust runtime.".to_owned())
    }

    async fn start_login(&self, _flow_id: &str, _ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        Err(Error::Unsupported("logging in to Google Messages"))
    }

    async fn connect(&self, _ctx: LoginCtx) -> Result<Box<dyn Session>> {
        Err(Error::Unsupported("connecting to Google Messages"))
    }
}
