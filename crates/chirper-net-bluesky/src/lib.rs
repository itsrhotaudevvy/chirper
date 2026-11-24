//! The Bluesky driver: direct messages over the AT Protocol, with a password login.
//!
//! It is a bridge runtime driver, not a bridge: it reports conversations, messages and reactions as
//! domain events and accepts commands, with no Matrix in between. The Go bridge's portals, puppets
//! and databases are the runtime's and the engine's job; the remote id formats are kept identical
//! so that stored history keeps resolving.

#![forbid(unsafe_code)]

mod account;
mod api;
mod convert;
mod ids;
mod login;
mod session;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use chirper_bridge::model::{Capabilities, LoginFlow};
use chirper_bridge::{Error, Event, LoginCtx, LoginProcess, Network, Result, Session};

/// The network id of Bluesky, used in settings, account ids and every stored row. Never changes.
pub const NETWORK_ID: &str = "bluesky";

/// Where the driver reaches the network's directories.
#[derive(Clone, Debug)]
pub struct Hosts {
    /// The PLC directory that serves `did:plc` documents.
    pub plc: String,
    /// The scheme of every PDS, DID document and `did:web` host: `https` in production.
    pub scheme: String,
}

impl Hosts {
    /// The public network.
    pub fn live() -> Self {
        Self {
            plc: "https://plc.directory".to_owned(),
            scheme: "https".to_owned(),
        }
    }
}

/// The Bluesky driver.
#[derive(Clone, Debug)]
pub struct Bluesky {
    hosts: Hosts,
}

impl Bluesky {
    /// A driver for the public network.
    pub fn new() -> Self {
        Self::with_hosts(Hosts::live())
    }

    /// A driver for other directories (tests, a self-hosted directory).
    pub fn with_hosts(hosts: Hosts) -> Self {
        Self { hosts }
    }
}

impl Default for Bluesky {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Network for Bluesky {
    fn id(&self) -> &'static str {
        NETWORK_ID
    }

    fn name(&self) -> &'static str {
        "Bluesky"
    }

    fn color(&self) -> &'static str {
        "#1185FE"
    }

    /// Reactions and read receipts work both ways. Redactions delete a message for this account only:
    /// the other side keeps its copy. Edits, replies, typing and media are not in the chat API.
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            edits: false,
            reactions: true,
            replies: false,
            redactions: true,
            typing: false,
            read_receipts: true,
            stickers: false,
            voice: false,
        }
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        vec![LoginFlow {
            id: "password".to_owned(),
            name: "Username & password".to_owned(),
            description: "Log in by entering your Bluesky username and password".to_owned(),
        }]
    }

    async fn start_login(&self, flow_id: &str, ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        match flow_id {
            "password" => Ok(Box::new(login::PasswordLogin::new(ctx, self.hosts.clone()))),
            _ => Err(Error::Unsupported("this login flow")),
        }
    }

    async fn connect(&self, ctx: LoginCtx) -> Result<Box<dyn Session>> {
        let stored = account::load(&ctx)?;
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = tx.send(Event::State {
            state: chirper_bridge::model::SessionState::Connecting,
            error: String::new(),
            message: String::new(),
            profile: None,
        });
        let account = Arc::new(account::Account::new(ctx, self.hosts.clone(), stored));
        let me = account.me().await?;
        tokio::spawn(session::run(Arc::clone(&account), tx, me.clone()));
        Ok(Box::new(session::BlueskySession::new(account, me, rx)))
    }

    fn supports_remote_logout(&self) -> bool {
        true
    }
}
