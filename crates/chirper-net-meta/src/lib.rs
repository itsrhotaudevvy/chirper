//! Messenger and Instagram drivers for Chirper's unified bridge runtime. Work in progress: the network is listed as
//! unavailable until its protocol client is complete.

#![forbid(unsafe_code)]

use async_trait::async_trait;

use chirper_bridge::model::{Capabilities, LoginFlow};
use chirper_bridge::{Error, LoginCtx, LoginProcess, Network, Result, Session};

/// The network id of Messenger, used in settings, account ids and every stored row. Never changes.
pub const MESSENGER_ID: &str = "messenger";

/// The Messenger driver.
#[derive(Debug, Default, Clone)]
pub struct Messenger {}

impl Messenger {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl Network for Messenger {
    fn id(&self) -> &'static str {
        MESSENGER_ID
    }

    fn name(&self) -> &'static str {
        "Messenger"
    }

    fn color(&self) -> &'static str {
        "#FF2542"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        Vec::new()
    }

    fn unavailable(&self) -> Option<String> {
        Some("Messenger is being ported to the unified Rust runtime.".to_owned())
    }

    async fn start_login(&self, _flow_id: &str, _ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        Err(Error::Unsupported("logging in to Messenger"))
    }

    async fn connect(&self, _ctx: LoginCtx) -> Result<Box<dyn Session>> {
        Err(Error::Unsupported("connecting to Messenger"))
    }
}

/// The network id of Instagram, used in settings, account ids and every stored row. Never changes.
pub const INSTAGRAM_ID: &str = "instagram";

/// The Instagram driver.
#[derive(Debug, Default, Clone)]
pub struct Instagram {}

impl Instagram {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl Network for Instagram {
    fn id(&self) -> &'static str {
        INSTAGRAM_ID
    }

    fn name(&self) -> &'static str {
        "Instagram"
    }

    fn color(&self) -> &'static str {
        "#E1306C"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        Vec::new()
    }

    fn unavailable(&self) -> Option<String> {
        Some("Instagram is being ported to the unified Rust runtime.".to_owned())
    }

    async fn start_login(&self, _flow_id: &str, _ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        Err(Error::Unsupported("logging in to Instagram"))
    }

    async fn connect(&self, _ctx: LoginCtx) -> Result<Box<dyn Session>> {
        Err(Error::Unsupported("connecting to Instagram"))
    }
}
