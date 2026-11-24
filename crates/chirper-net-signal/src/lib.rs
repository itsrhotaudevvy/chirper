//! Signal driver for Chirper's unified bridge runtime. Work in progress: the network is listed as
//! unavailable until its protocol client is complete.

#![forbid(unsafe_code)]

mod account;
mod ids;
mod keystore;
mod net;
mod proto;
mod provision;
mod tls;
mod ws;

use async_trait::async_trait;

use chirper_bridge::model::{Capabilities, LoginFlow};
use chirper_bridge::{Error, LoginCtx, LoginProcess, Network, Result, Session};

/// Where the driver talks to Signal. Production uses [`Endpoints::production`]; tests point it at
/// a local server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// Websocket base, such as `wss://chat.signal.org`.
    pub ws: String,
    /// HTTPS base for the REST API, such as `https://chat.signal.org`.
    pub http: String,
}

impl Endpoints {
    /// Signal's production servers.
    pub fn production() -> Self {
        Self {
            ws: "wss://chat.signal.org".to_owned(),
            http: "https://chat.signal.org".to_owned(),
        }
    }
}

/// The network id of Signal, used in settings, account ids and every stored row. Never changes.
pub const NETWORK_ID: &str = "signal";

/// The Signal driver.
#[derive(Debug, Default, Clone)]
pub struct Signal {}

impl Signal {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl Network for Signal {
    fn id(&self) -> &'static str {
        NETWORK_ID
    }

    fn name(&self) -> &'static str {
        "Signal"
    }

    fn color(&self) -> &'static str {
        "#3A76F0"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        Vec::new()
    }

    fn unavailable(&self) -> Option<String> {
        Some("Signal is being ported to the unified Rust runtime.".to_owned())
    }

    async fn start_login(&self, _flow_id: &str, _ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        Err(Error::Unsupported("logging in to Signal"))
    }

    async fn connect(&self, _ctx: LoginCtx) -> Result<Box<dyn Session>> {
        Err(Error::Unsupported("connecting to Signal"))
    }
}
