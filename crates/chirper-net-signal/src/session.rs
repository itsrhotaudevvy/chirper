//! One live Signal connection: the authenticated websocket, the key store and the event stream.
//!
//! `connect` reads the linked account, opens `/v1/websocket/` with its basic-auth credentials,
//! keeps the one-time prekeys topped up and starts the receive loop (in `receive.rs`). Commands
//! run on the caller's task through [`crate::send`] and use the same shared state.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;

use chirper_bridge::model::{Ack, BackfillPage, Command, Event, RemoteUser, SessionState};
use chirper_bridge::{Error, LoginCtx, Result, Session};
use chirper_signal_protocol::identity::IdentityKeyPair;

use crate::account::Account;
use crate::convert::ReactionBook;
use crate::keystore::KeyStore;
use crate::{Endpoints, ids, keys, net, receive, send, ws};
use crate::ws::WsClient;

/// Everything the tasks of one connection share.
pub struct Shared {
    pub account: Account,
    pub identity: IdentityKeyPair,
    pub endpoints: Endpoints,
    /// The authenticated websocket.
    pub ws: WsClient,
    /// Pinned HTTP client for the CDN and the upload hosts.
    pub http: reqwest::Client,
    /// The protocol store. Locked only around synchronous protocol calls.
    pub store: Mutex<KeyStore>,
    /// Reactions per message, so each change reports the full set.
    pub reactions: Mutex<ReactionBook>,
    secrets: LoginCtx,
    events: Mutex<Option<mpsc::UnboundedSender<Event>>>,
}

impl Shared {
    /// Sends an event to the engine. Silently dropped once the connection has ended.
    pub fn emit(&self, event: Event) {
        if let Ok(guard) = self.events.lock() {
            if let Some(tx) = guard.as_ref() {
                let _ = tx.send(event);
            }
        }
    }

    /// Ends the event stream: `Session::next` returns `None` after the queued events.
    pub fn close_events(&self) {
        if let Ok(mut guard) = self.events.lock() {
            guard.take();
        }
    }

    /// Runs `f` against the protocol store and writes the store back when it succeeds.
    pub fn with_store<T>(&self, f: impl FnOnce(&mut KeyStore) -> Result<T>) -> Result<T> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| Error::Internal("the key store lock was poisoned".to_owned()))?;
        let out = f(&mut guard)?;
        let bytes = guard
            .to_json()
            .map_err(|_| Error::Internal("the key store could not be encoded".to_owned()))?;
        self.secrets.set_secret("store", &bytes)?;
        Ok(out)
    }

    /// The local account's ACI.
    pub fn me(&self) -> &str {
        &self.account.aci
    }

    /// The local device id.
    pub fn device_id(&self) -> u32 {
        self.account.device_id
    }

    /// The remote user of the local account.
    pub fn me_user(&self) -> RemoteUser {
        RemoteUser {
            id: ids::user_id(self.me()),
            ..RemoteUser::default()
        }
    }
}

/// A connected Signal account.
pub struct SignalSession {
    shared: Arc<Shared>,
    events: mpsc::UnboundedReceiver<Event>,
}

/// Connects a stored account. Missing or unreadable secrets are an authentication failure.
pub async fn connect(ctx: LoginCtx, endpoints: Endpoints) -> Result<Box<dyn Session>> {
    let account: Account = read_json(&ctx, "session")?
        .ok_or_else(|| Error::Auth("no Signal account is stored".to_owned()))?;
    let store: KeyStore = read_json(&ctx, "store")?
        .ok_or_else(|| Error::Auth("no Signal keys are stored".to_owned()))?;
    let identity = store
        .identity()
        .ok_or_else(|| Error::Auth("the stored Signal identity is not valid".to_owned()))?;
    let (tx, rx) = mpsc::unbounded_channel();
    let http = crate::tls::http_client()?;
    let url = format!("{}/v1/websocket/", endpoints.ws);
    let socket = net::open(&url, Some(&account.basic_auth())).await?;
    let (client, pump) = ws::spawn(socket);
    let shared = Arc::new(Shared {
        account,
        identity,
        endpoints,
        ws: client,
        http,
        store: Mutex::new(store),
        reactions: Mutex::new(ReactionBook::default()),
        secrets: ctx,
        events: Mutex::new(Some(tx)),
    });
    shared.emit(Event::State {
        state: SessionState::Connecting,
        error: String::new(),
        message: String::new(),
        profile: None,
    });
    let uploaded = keys::refresh(&shared, now_ms()).await;
    if let Err(err) = uploaded {
        shared.close_events();
        return Err(err);
    }
    let user = shared.me_user();
    tokio::spawn(receive::run(shared.clone(), pump));
    shared.emit(Event::State {
        state: SessionState::Running,
        error: String::new(),
        message: String::new(),
        profile: Some(user),
    });
    Ok(Box::new(SignalSession {
        shared,
        events: rx,
    }))
}

fn read_json<T: serde::de::DeserializeOwned>(ctx: &LoginCtx, name: &str) -> Result<Option<T>> {
    match ctx.get_secret(name)? {
        None => Ok(None),
        Some(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| Error::Auth(format!("the stored Signal {name} is not valid"))),
    }
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[async_trait]
impl Session for SignalSession {
    async fn next(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    async fn command(&mut self, command: Command) -> Result<Ack> {
        send::command(&self.shared, command).await
    }

    async fn backfill(
        &mut self,
        _portal: &chirper_bridge::PortalRef,
        _cursor: Option<String>,
        _limit: usize,
    ) -> Result<BackfillPage> {
        // Signal keeps no server-side history: a device only sees what arrives while it is linked.
        Ok(BackfillPage {
            messages: Vec::new(),
            cursor: None,
            has_more: false,
        })
    }

    async fn logout_remote(&mut self) -> Result<()> {
        let path = format!("/v1/devices/{}", self.shared.device_id());
        let response = self.shared.ws.request("DELETE", &path, None, Vec::new()).await?;
        keys::ensure_ok(response.status)
    }

    fn user(&self) -> Option<RemoteUser> {
        Some(self.shared.me_user())
    }
}
