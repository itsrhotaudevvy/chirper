//! The two traits a network implements, and the context it gets.
//!
//! A driver describes itself ([`Network`]), runs one live login ([`Session`]) and, while logging in,
//! walks the user through steps ([`LoginProcess`]). It never touches the database, the id model or
//! the UI, and the runtime owns reconnection, rate limiting, capability gating and event routing.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::model::{Ack, BackfillPage, Capabilities, Event, LoginFlow, LoginStep, PortalRef};
use crate::model::{Content, RemoteUser};

/// A failure of a driver or of the runtime.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The network cannot do this (capability off, or not implemented for this chat).
    #[error("{0} is not supported on this network")]
    Unsupported(&'static str),
    /// The saved credentials no longer work.
    #[error("credentials are invalid: {0}")]
    Auth(String),
    /// A transport failure worth retrying.
    #[error("network error: {0}")]
    Network(String),
    /// The network asked us to slow down.
    #[error("rate limited, retry in {retry_after_ms} ms")]
    RateLimited { retry_after_ms: u64 },
    /// The network answered something this driver cannot use.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// A programming error or a bug.
    #[error("internal error: {0}")]
    Internal(String),
    /// The login is not running, so there is nothing to send to.
    #[error("the account is not connected")]
    NotConnected,
}

impl Error {
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Network(_) => true,
            Self::RateLimited { .. } => true,
            Self::Auth(_) => false,
            Self::Unsupported(_) | Self::Protocol(_) | Self::Internal(_) | Self::NotConnected => {
                false
            }
        }
    }

    /// The session state this failure leaves the login in.
    pub fn session_state(&self) -> crate::model::SessionState {
        use crate::model::SessionState;
        match self {
            Self::Auth(_) => SessionState::NeedsReauth,
            Self::Network(_) | Self::RateLimited { .. } => SessionState::TransientDisconnect,
            Self::Unsupported(_) | Self::NotConnected => SessionState::UnknownError,
            Self::Protocol(_) | Self::Internal(_) => SessionState::UnknownError,
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A value the user submitted to a login step.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginInput {
    /// Field id to value, for a `user_input` step.
    pub values: std::collections::BTreeMap<String, String>,
    /// Field id to value, for a `cookies` step.
    pub cookies: std::collections::BTreeMap<String, String>,
}

impl LoginInput {
    pub fn start() -> Self {
        Self::default()
    }

    pub fn values(map: std::collections::BTreeMap<String, String>) -> Self {
        Self {
            values: map,
            cookies: Default::default(),
        }
    }

    pub fn cookies(map: std::collections::BTreeMap<String, String>) -> Self {
        Self {
            values: Default::default(),
            cookies: map,
        }
    }
}

/// Per-login secret storage. The core implements it; drivers only see typed values.
pub trait Secrets: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>>;
    fn set(&self, key: &str, value: &[u8]) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;

    /// Move every secret whose key starts with `from` to the same suffix under `to`. A login flow
    /// starts under a provisional login id; when the network reports the real one, the credentials
    /// it already stored must follow, and the provisional copies must not stay behind.
    fn move_prefix(&self, from: &str, to: &str) -> Result<()> {
        let _ = (from, to);
        Err(Error::Unsupported("moving stored credentials"))
    }
}

/// Everything a driver may use during one login.
#[derive(Clone)]
pub struct LoginCtx {
    pub network: String,
    pub login_id: String,
    /// A private directory for this login (created by the runtime, mode 0700).
    pub data_dir: PathBuf,
    pub secrets: Arc<dyn Secrets>,
    /// One hardened HTTP client for every network: rustls, no proxy surprises, bounded bodies.
    pub http: reqwest::Client,
}

impl fmt::Debug for LoginCtx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginCtx")
            .field("network", &self.network)
            .field("login_id", &self.login_id)
            .field("data_dir", &self.data_dir)
            .finish_non_exhaustive()
    }
}

impl LoginCtx {
    /// Namespaced secret key, so two networks can use `"token"` without colliding.
    pub fn secret_key(&self, name: &str) -> String {
        format!("{}/{}/{name}", self.network, self.login_id)
    }

    pub fn get_secret(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        self.secrets.get(&self.secret_key(name))
    }

    pub fn set_secret(&self, name: &str, value: &[u8]) -> Result<()> {
        self.secrets.set(&self.secret_key(name), value)
    }

    pub fn delete_secret(&self, name: &str) -> Result<()> {
        self.secrets.delete(&self.secret_key(name))
    }
}

/// The commands the app can send to a live login.
#[derive(Clone, Debug)]
pub enum Command {
    SendText {
        portal: PortalRef,
        text: String,
        /// Remote id of the message being replied to.
        reply_to: Option<String>,
        /// The engine's transaction id; the driver echoes it in [`Event::Delivery`].
        txn_id: String,
    },
    SendMedia {
        portal: PortalRef,
        data: Arc<Vec<u8>>,
        mime: String,
        file_name: String,
        caption: String,
        /// `image`, `video`, `audio`, `voice`, `file`, `sticker`.
        kind: String,
        reply_to: Option<String>,
        txn_id: String,
    },
    Edit {
        portal: PortalRef,
        /// Remote id of the message to edit.
        target: String,
        text: String,
    },
    Redact {
        portal: PortalRef,
        target: String,
    },
    React {
        portal: PortalRef,
        target: String,
        key: String,
    },
    Unreact {
        portal: PortalRef,
        target: String,
        key: String,
    },
    /// Report the user's read position. `None` means "everything in the chat".
    Read {
        portal: PortalRef,
        event: Option<String>,
    },
    Typing {
        portal: PortalRef,
        typing: bool,
    },
}

/// One live connection of a login. The runtime owns it and drives it to completion.
#[async_trait]
pub trait Session: Send {
    /// The next thing the transport reports. Resolves to `None` when the connection ended. Must be
    /// cancel-safe: the runtime drops this future whenever a command arrives.
    async fn next(&mut self) -> Option<Event>;

    /// Send a command. Drivers acknowledge as soon as the network accepted the request and report
    /// the final verdict later through [`Event::Delivery`].
    async fn command(&mut self, cmd: Command) -> Result<Ack>;

    /// Fetch a page of history. `cursor` is the driver's own opaque token.
    async fn backfill(
        &mut self,
        portal: &PortalRef,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<BackfillPage>;

    /// Tell the network the user logged out where the network supports it.
    async fn logout_remote(&mut self) -> Result<()>;

    /// The logged-in user, when the driver knows it.
    fn user(&self) -> Option<RemoteUser> {
        None
    }

    /// A hint for the chat list when a message arrives without metadata.
    fn content_hint(&self) -> Option<Content> {
        None
    }
}

/// One login process, driven by the user through the four wire steps.
#[async_trait]
pub trait LoginProcess: Send {
    /// Advance the process. `LoginInput::start()` returns the first step; a step that is waiting
    /// for a scan ignores further input and keeps waiting.
    async fn next(&mut self, input: LoginInput) -> Result<LoginStep>;
    /// Abandon the process. The driver may keep credentials it already stored.
    async fn cancel(&mut self) {}
}

/// A network. Static description plus the two entry points into the runtime.
#[async_trait]
pub trait Network: Send + Sync + 'static {
    /// Stable id, used in `Account.network` and stored in every row. Never changes.
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    /// Banner colour of the network, from the design system.
    fn color(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    fn login_flows(&self) -> Vec<LoginFlow>;
    /// Why the network cannot be used right now, if it cannot. Listed to the user as `reason`.
    fn unavailable(&self) -> Option<String> {
        None
    }
    /// Begin a login flow.
    async fn start_login(&self, flow_id: &str, ctx: LoginCtx) -> Result<Box<dyn LoginProcess>>;
    /// Connect a login that already has credentials.
    async fn connect(&self, ctx: LoginCtx) -> Result<Box<dyn Session>>;
    /// Whether [`Session::logout_remote`] does anything for this network.
    fn supports_remote_logout(&self) -> bool {
        true
    }
}
