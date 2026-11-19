//! The engine: the facade the shell calls, and the one place where driver events become rows.
//!
//! One runtime, one database, one event stream. A driver reports what its network said; the engine
//! decides the local ids, stores the result and pushes the wire events the UI already understands.

mod chats;
mod convert;
mod dispatch;
mod logins;
pub mod networks;
mod settings;
pub mod validate;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::value::to_raw_value;
use tokio::sync::mpsc;
use zeroize::Zeroizing;

use chirper_bridge::Runtime;

use crate::model::{
    Account, AccountState, Chat, ChatFilter, CoreEvent, CoreSettings, Message, Network,
};
use crate::secure::FileSecrets;
use crate::store::{Login, Store};

pub use chats::MediaUpload;

/// Where the core keeps its data and how it opens it.
#[derive(Debug, Clone)]
pub struct Options {
    /// The application data directory. `chirper.db`, `media/`, `credentials/` and `settings.json`
    /// live here.
    pub data_dir: PathBuf,
    /// The raw SQLCipher key. `None` opens a plaintext database, which is only for development.
    pub db_key: Option<[u8; 32]>,
    /// Show the offline demo network.
    pub demo_network: bool,
    /// Drivers the host provides because they need the platform (the Android SMS provider).
    pub host_networks: networks::HostNetworks,
}

impl Options {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            db_key: None,
            demo_network: false,
            host_networks: networks::HostNetworks::default(),
        }
    }
}

/// The event channel the shell drains. Batches are assembled by the shell.
pub struct EventStream {
    rx: mpsc::UnboundedReceiver<CoreEvent>,
}

impl EventStream {
    /// The next event, or `None` once the core is shut down.
    pub async fn recv(&mut self) -> Option<CoreEvent> {
        self.rx.recv().await
    }
}

/// A failure of a facade call.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    InvalidInput(String),
    #[error("not found")]
    NotFound,
    #[error("not connected")]
    NotConnected,
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("{0}")]
    Failed(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Store(#[from] crate::store::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the core is closed")]
    Closed,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<chirper_bridge::Error> for Error {
    fn from(err: chirper_bridge::Error) -> Self {
        use chirper_bridge::Error as E;
        match err {
            E::Unsupported(what) => Self::Unsupported(what.to_owned()),
            E::NotConnected => Self::NotConnected,
            E::Auth(message)
            | E::Network(message)
            | E::Protocol(message)
            | E::Internal(message) => Self::Failed(message),
            E::RateLimited { retry_after_ms } => {
                Self::Failed(format!("rate limited, retry in {retry_after_ms} ms"))
            }
        }
    }
}

impl From<crate::secure::vault::Error> for Error {
    fn from(err: crate::secure::vault::Error) -> Self {
        Self::Io(std::io::Error::other(err.to_string()))
    }
}

/// A login process that is being walked through the wire steps.
pub(crate) struct ActiveLogin {
    pub network: String,
    pub login_id: String,
    pub ctx: chirper_bridge::LoginCtx,
    pub process: Arc<tokio::sync::Mutex<Box<dyn chirper_bridge::LoginProcess>>>,
    /// The last step handed to the user, so a repeated `waitLoginStep` is idempotent.
    pub last_step: Mutex<Option<chirper_bridge::model::LoginStep>>,
}

/// The engine and everything it owns. Cloning is cheap and shares one runtime.
#[derive(Clone)]
pub struct Core {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub data_dir: PathBuf,
    pub store: Arc<Store>,
    pub secrets: Arc<FileSecrets>,
    pub runtime: Arc<Runtime>,
    /// Every driver of this build, in registry order.
    pub drivers: Vec<Arc<dyn chirper_bridge::Network>>,
    pub settings: Mutex<CoreSettings>,
    /// Live account state, by network and login.
    pub accounts: Mutex<HashMap<(String, String), Account>>,
    pub logins: Mutex<HashMap<String, ActiveLogin>>,
    /// Display names of the people typing in a room, by room id.
    pub typing: Mutex<HashMap<String, Vec<String>>>,
    pub events: mpsc::UnboundedSender<CoreEvent>,
    pub closed: AtomicBool,
}

impl Inner {
    /// Push one wire event. A closed stream is not an error: the shell has gone away.
    pub fn emit<T: Serialize>(&self, kind: &str, payload: T) {
        let payload = match to_raw_value(&payload) {
            Ok(raw) => raw,
            Err(err) => {
                log::warn!("cannot serialise a {kind} event: {err}");
                return;
            }
        };
        let _ = self.events.send(CoreEvent {
            kind: kind.to_owned(),
            payload,
        });
    }

    pub fn error(&self, message: impl Into<String>) {
        self.emit(
            "core_error",
            serde_json::json!({ "message": message.into() }),
        );
    }

    pub fn account(&self, network: &str, login_id: &str) -> Account {
        let key = (network.to_owned(), login_id.to_owned());
        if let Some(account) = self.accounts.lock().expect("account map").get(&key) {
            return account.clone();
        }
        let stored = self
            .store
            .logins()
            .unwrap_or_default()
            .into_iter()
            .find(|l| l.network == network && l.login_id == login_id);
        Account {
            network: network.to_owned(),
            login_id: login_id.to_owned(),
            name: stored.as_ref().map(|l| l.name.clone()).unwrap_or_default(),
            avatar_url: stored
                .as_ref()
                .map(|l| l.avatar_url.clone())
                .unwrap_or_default(),
            state: if self.runtime.is_running(network, login_id) {
                AccountState::Connected
            } else {
                AccountState::Unknown
            },
            ..Default::default()
        }
    }

    /// Record an account state and tell the shell. Returns the account that was emitted.
    pub fn set_account(&self, mut account: Account) -> Account {
        if account.name.is_empty() || account.avatar_url.is_empty() {
            if let Ok(logins) = self.store.logins() {
                if let Some(stored) = logins
                    .iter()
                    .find(|l| l.network == account.network && l.login_id == account.login_id)
                {
                    if account.name.is_empty() {
                        account.name = stored.name.clone();
                    }
                    if account.avatar_url.is_empty() {
                        account.avatar_url = stored.avatar_url.clone();
                    }
                }
            }
        }
        self.accounts.lock().expect("account map").insert(
            (account.network.clone(), account.login_id.clone()),
            account.clone(),
        );
        self.emit("account", &account);
        account
    }

    pub fn require_open(&self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        Ok(())
    }
}

impl Core {
    /// Open the database, build the network registry and start the dispatcher.
    ///
    /// Must be called inside a Tokio runtime; the returned [`EventStream`] belongs to the caller.
    pub fn open(options: Options) -> Result<(Self, EventStream)> {
        let data_dir = options.data_dir.clone();
        std::fs::create_dir_all(&data_dir)?;
        crate::secure::tighten_dir(&data_dir)?;

        let store = Arc::new(Store::open(
            &data_dir.join("chirper.db"),
            options.db_key.as_ref(),
        )?);
        let secrets = Arc::new(FileSecrets::new(&data_dir, options.db_key.as_ref())?);
        let settings = settings::load(&data_dir);

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (routed_tx, routed_rx) = mpsc::unbounded_channel();
        let drivers = networks::available(options.demo_network, &options.host_networks);
        let runtime = Arc::new(Runtime::new(drivers.clone(), routed_tx));

        let inner = Arc::new(Inner {
            data_dir,
            store,
            secrets,
            runtime,
            drivers,
            settings: Mutex::new(settings),
            accounts: Mutex::new(HashMap::new()),
            logins: Mutex::new(HashMap::new()),
            typing: Mutex::new(HashMap::new()),
            events: event_tx,
            closed: AtomicBool::new(false),
        });

        tokio::spawn(dispatch::run(inner.clone(), routed_rx));
        Ok((Self { inner }, EventStream { rx: event_rx }))
    }

    pub(crate) fn inner(&self) -> &Inner {
        &self.inner
    }

    /// `start`: connect every login that is stored, without blocking on it.
    pub async fn start(&self) -> Result<()> {
        self.inner.require_open()?;
        let logins = self.inner.store.logins()?;
        for login in logins {
            self.inner.start_session(&login.network, &login.login_id);
        }
        Ok(())
    }

    /// `networks`: every network of this build, with the reason when it cannot be used.
    pub async fn networks(&self) -> Result<Vec<Network>> {
        self.inner.require_open()?;
        Ok(networks::all(&self.inner.drivers))
    }

    /// `accounts`: every stored login, with its live state.
    pub async fn accounts(&self) -> Result<Vec<Account>> {
        self.inner.require_open()?;
        let logins = self.inner.store.logins()?;
        Ok(logins
            .into_iter()
            .map(|login| self.inner.account(&login.network, &login.login_id))
            .collect())
    }

    /// `listChats`.
    pub async fn list_chats(&self, filter: &ChatFilter) -> Result<Vec<Chat>> {
        self.inner.require_open()?;
        Ok(self.inner.store.list_chats(filter)?)
    }

    /// `getChat`: `None` when the room is unknown or gone.
    pub async fn get_chat(&self, room_id: &str) -> Result<Option<Chat>> {
        self.inner.require_open()?;
        validate::room_id(room_id)?;
        match self.inner.store.chat(room_id) {
            Ok(chat) => Ok(Some(chat)),
            Err(crate::store::Error::NotFound) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// `listMessages`.
    pub async fn list_messages(
        &self,
        room_id: &str,
        before_order: i64,
        limit: u32,
    ) -> Result<Vec<Message>> {
        self.inner.require_open()?;
        validate::room_id(room_id)?;
        if before_order < 0 {
            return Err(Error::InvalidInput("before order".to_owned()));
        }
        validate::page_limit(limit)?;
        Ok(self
            .inner
            .store
            .list_messages(room_id, before_order, limit as usize)?)
    }

    /// `search`.
    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<Message>> {
        self.inner.require_open()?;
        validate::query(query)?;
        validate::page_limit(limit)?;
        Ok(self.inner.store.search(query, limit as usize)?)
    }

    /// `settings`.
    pub async fn settings(&self) -> Result<CoreSettings> {
        self.inner.require_open()?;
        Ok(self.inner.settings.lock().expect("settings").clone())
    }

    /// `setSettings`. The demo network follows `demoNetwork`, and a change is stored immediately.
    pub async fn set_settings(&self, settings: &CoreSettings) -> Result<()> {
        self.inner.require_open()?;
        *self.inner.settings.lock().expect("settings") = settings.clone();
        settings::save(&self.inner.data_dir, settings)?;
        Ok(())
    }

    /// `mediaFile`: the local path of a stored media file, if it still exists.
    pub async fn media_file(&self, url: &str) -> Result<PathBuf> {
        self.inner.require_open()?;
        let media_id = crate::ids::media_id_of(url)
            .ok_or_else(|| Error::InvalidInput("media url".to_owned()))?;
        let media = self.inner.store.media(media_id)?;
        Ok(PathBuf::from(media.path))
    }

    /// Shut down: stop every supervisor, forget login processes and close the database.
    pub fn shutdown(&self) {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.inner.runtime.stop_all();
        self.inner.logins.lock().expect("login map").clear();
        let _ = crate::secure::tighten_tree(&self.inner.data_dir);
        log::info!("chirper core stopped");
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.runtime.stop_all();
    }
}

impl Inner {
    /// Start the supervisor of a login with a fresh context.
    pub(crate) fn start_session(&self, network: &str, login_id: &str) {
        let Some(driver) = self.runtime.network(network) else {
            return;
        };
        if driver.unavailable().is_some() {
            return;
        }
        match self.login_ctx(network, login_id) {
            Ok(ctx) => {
                self.runtime.start_login(network, login_id, ctx);
                log::debug!("{network}/{login_id}: supervisor started");
            }
            Err(err) => self.error(format!("{network}/{login_id}: {err}")),
        }
    }

    /// The context of one login: its private directory and its secrets.
    pub(crate) fn login_ctx(
        &self,
        network: &str,
        login_id: &str,
    ) -> Result<chirper_bridge::LoginCtx> {
        let dir = self
            .data_dir
            .join("logins")
            .join(format!("{network}.{login_id}"));
        std::fs::create_dir_all(&dir)?;
        crate::secure::tighten_dir(&dir)?;
        Ok(chirper_bridge::LoginCtx {
            network: network.to_owned(),
            login_id: login_id.to_owned(),
            data_dir: dir,
            secrets: self.secrets.clone(),
            http: chirper_bridge::http_client(),
        })
    }

    /// A stored login row for an account that just completed a login flow.
    pub(crate) fn put_login(&self, network: &str, login_id: &str) {
        let _ = self.store.put_login(&Login {
            network: network.to_owned(),
            login_id: login_id.to_owned(),
            ..Default::default()
        });
        self.set_account(Account {
            network: network.to_owned(),
            login_id: login_id.to_owned(),
            state: AccountState::Connecting,
            ..Default::default()
        });
    }

    pub(crate) fn forget_login(&self, network: &str, login_id: &str) {
        let _ = self.store.delete_login(network, login_id);
        self.accounts
            .lock()
            .expect("account map")
            .remove(&(network.to_owned(), login_id.to_owned()));
    }
}

/// The zeroizing key helper shared by the shell: `zeroize` without leaking the type everywhere.
pub fn secret_key(bytes: &[u8]) -> Zeroizing<Vec<u8>> {
    Zeroizing::new(bytes.to_vec())
}

/// The settings file name, for the shell's documentation.
pub const SETTINGS_FILE: &str = settings::FILE;
