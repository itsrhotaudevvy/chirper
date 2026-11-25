//! The client layer: one [`Client`] owns the MTProto sessions of the account's data centres and
//! every call goes through it.
//!
//! The rules here are the ones the rest of the crate relies on:
//! * the first call on each connection is wrapped in `invokeWithLayer(initConnection(...))`;
//! * `*_MIGRATE_X` moves the home data centre to `X` and retries the call once;
//! * `FLOOD_WAIT_X` and the credential errors are mapped by [`crate::error`];
//! * files from another data centre are fetched after `auth.exportAuthorization` and
//!   `auth.importAuthorization`, over that data centre's own connection.
//!
//! Login and session code are written against the [`Api`] trait, so tests run them against an
//! in-memory implementation.

mod wrap;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use async_trait::async_trait;
use chirper_bridge::Error;
use tokio::sync::{Mutex, mpsc};

use crate::error::CallError;
use crate::mtproto::{self, MtSession, UpdateFrame};
use crate::store::{DcState, Store};
use crate::tl::RemoteCall;
use crate::tl::enums::auth::ExportedAuthorization;
use crate::tl::functions::auth::{ExportAuthorization, ImportAuthorization};
use wrap::{Device, InitFields, Wrapped};

/// The data centre a new account starts in.
pub const DEFAULT_DC: i32 = 2;

/// A frame or a connection end, from any data centre of the account.
#[derive(Debug)]
pub enum Incoming {
    /// Something the server pushed on the connection to `dc`.
    Frame { dc: i32, frame: UpdateFrame },
    /// The connection to `dc` ended.
    Closed { dc: i32 },
}

/// The calls the rest of the crate makes. [`Client`] implements it over the network; tests use an
/// in-memory implementation.
#[async_trait]
pub trait Api: Send + Sync {
    /// Call the home data centre.
    async fn call<R>(&self, request: &R) -> Result<R::Return, CallError>
    where
        R: RemoteCall + Sync,
        R::Return: Send;

    /// Call a given data centre, creating its connection (and auth key) if needed.
    async fn call_on<R>(&self, dc: i32, request: &R) -> Result<R::Return, CallError>
    where
        R: RemoteCall + Sync,
        R::Return: Send;

    /// Make `dc` accept the account's requests, by exporting the authorization from home.
    async fn authorize_dc(&self, dc: i32) -> Result<(), CallError>;

    /// The home data centre.
    fn home_dc(&self) -> i32;

    /// Make `dc` the home data centre.
    fn set_home_dc(&self, dc: i32);
}

#[async_trait]
impl<A: Api> Api for Arc<A> {
    async fn call<R>(&self, request: &R) -> Result<R::Return, CallError>
    where
        R: RemoteCall + Sync,
        R::Return: Send,
    {
        (**self).call(request).await
    }

    async fn call_on<R>(&self, dc: i32, request: &R) -> Result<R::Return, CallError>
    where
        R: RemoteCall + Sync,
        R::Return: Send,
    {
        (**self).call_on(dc, request).await
    }

    async fn authorize_dc(&self, dc: i32) -> Result<(), CallError> {
        (**self).authorize_dc(dc).await
    }

    fn home_dc(&self) -> i32 {
        (**self).home_dc()
    }

    fn set_home_dc(&self, dc: i32) {
        (**self).set_home_dc(dc);
    }
}

/// One MTProto connection to a data centre.
struct Conn {
    session: MtSession,
    /// Set once a call on this connection went through with `initConnection`.
    initialized: AtomicBool,
}

/// The network side of one login.
pub struct Client {
    store: Arc<Store>,
    init: InitFields,
    home: AtomicI32,
    conns: Mutex<BTreeMap<i32, Arc<Conn>>>,
    authorized: Mutex<BTreeSet<i32>>,
    incoming: mpsc::UnboundedSender<Incoming>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("home", &self.home.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// Map a store failure into the call error model.
fn store_error(error: Error) -> CallError {
    CallError::Protocol(error.to_string())
}

impl Client {
    /// A client for the account in `store`, with the Telegram application id. Returns the client
    /// and the receiver that carries every server push of every connection.
    pub fn new(
        store: Arc<Store>,
        api_id: i32,
        home: Option<i32>,
    ) -> (Client, mpsc::UnboundedReceiver<Incoming>) {
        let (incoming, receiver) = mpsc::unbounded_channel();
        let home = home.unwrap_or(DEFAULT_DC);
        let client = Client {
            store,
            init: InitFields::new(api_id, Device::current()),
            home: AtomicI32::new(home),
            conns: Mutex::new(BTreeMap::new()),
            authorized: Mutex::new(BTreeSet::new()),
            incoming,
        };
        (client, receiver)
    }

    /// Find or open the connection to `dc`.
    async fn connection(&self, dc: i32) -> Result<Arc<Conn>, CallError> {
        let mut conns = self.conns.lock().await;
        if let Some(conn) = conns.get(&dc) {
            return Ok(Arc::clone(conn));
        }
        let address = mtproto::production_dc(dc)
            .ok_or_else(|| CallError::Protocol(format!("unknown data centre {dc}")))?;
        let mut transport = mtproto::connect(address)
            .await
            .map_err(|e| CallError::Network(e.to_string()))?;
        let stored = self.store.auth_key(dc).map_err(store_error)?;
        let (key, salt, offset) = match stored {
            Some(key) => {
                let state = self.store.dc_state(dc);
                (key, state.server_salt, state.time_offset)
            }
            None => {
                let made = mtproto::generate_auth_key(&mut transport, dc)
                    .await
                    .map_err(|e| CallError::Protocol(e.to_string()))?;
                self.store
                    .save_auth_key(dc, &made.key)
                    .map_err(store_error)?;
                (made.key, made.server_salt, made.time_offset)
            }
        };
        let (session, mut frames) =
            MtSession::start(transport, key, salt, offset).map_err(CallError::from)?;
        self.store.set_dc_state(
            dc,
            DcState {
                server_salt: salt,
                time_offset: offset,
            },
        );
        let sink = self.incoming.clone();
        tokio::spawn(async move {
            while let Some(frame) = frames.recv().await {
                if sink.send(Incoming::Frame { dc, frame }).is_err() {
                    return;
                }
            }
            let _ = sink.send(Incoming::Closed { dc });
        });
        let conn = Arc::new(Conn {
            session,
            initialized: AtomicBool::new(false),
        });
        conns.insert(dc, Arc::clone(&conn));
        Ok(conn)
    }

    /// One request on `dc`, wrapped in `initConnection` until the connection has been initialised.
    async fn invoke_on<R: RemoteCall + Sync>(
        &self,
        dc: i32,
        request: &R,
    ) -> Result<R::Return, CallError> {
        let conn = self.connection(dc).await?;
        if conn.initialized.load(Ordering::Acquire) {
            return conn.session.invoke(request).await.map_err(CallError::from);
        }
        let wrapped = Wrapped::new(&self.init, request);
        let answer = conn
            .session
            .invoke(&wrapped)
            .await
            .map_err(CallError::from)?;
        conn.initialized.store(true, Ordering::Release);
        Ok(answer)
    }

    /// Record a new home data centre, in memory and on disk.
    fn move_home(&self, dc: i32) {
        self.home.store(dc, Ordering::Release);
        if let Err(error) = self.store.save_home_dc(dc) {
            log::warn!("could not save the home data centre: {error}");
        }
    }
}

#[async_trait]
impl Api for Client {
    async fn call<R>(&self, request: &R) -> Result<R::Return, CallError>
    where
        R: RemoteCall + Sync,
        R::Return: Send,
    {
        let dc = self.home.load(Ordering::Acquire);
        match self.invoke_on(dc, request).await {
            Err(CallError::Migrate { dc: next }) => {
                log::debug!("following a data centre migration to {next}");
                self.move_home(next);
                match self.invoke_on(next, request).await {
                    Err(CallError::Migrate { .. }) => Err(CallError::Protocol(
                        "the data centre moved twice in one call".to_owned(),
                    )),
                    other => other,
                }
            }
            other => other,
        }
    }

    async fn call_on<R>(&self, dc: i32, request: &R) -> Result<R::Return, CallError>
    where
        R: RemoteCall + Sync,
        R::Return: Send,
    {
        self.invoke_on(dc, request).await
    }

    async fn authorize_dc(&self, dc: i32) -> Result<(), CallError> {
        if dc == self.home.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut authorized = self.authorized.lock().await;
        if authorized.contains(&dc) {
            return Ok(());
        }
        let exported = self.call(&ExportAuthorization { dc_id: dc }).await?;
        let ExportedAuthorization::ExportedAuthorization(exported) = exported;
        self.invoke_on(
            dc,
            &ImportAuthorization {
                id: exported.id,
                bytes: exported.bytes,
            },
        )
        .await?;
        authorized.insert(dc);
        Ok(())
    }

    fn home_dc(&self) -> i32 {
        self.home.load(Ordering::Acquire)
    }

    fn set_home_dc(&self, dc: i32) {
        self.move_home(dc);
    }
}
