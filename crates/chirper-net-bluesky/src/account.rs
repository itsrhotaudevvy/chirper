//! The signed-in account: its stored session, the refresh of an expired access token, the event
//! cursor, and the cache of profiles. One `Account` serves every call of one login.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::{Mutex as StdMutex, PoisonError};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use chirper_bridge::model::RemoteUser;
use chirper_bridge::{Error, LoginCtx, Result};

use crate::Hosts;
use crate::api::actor::{self, PROFILE_BATCH};
use crate::api::identity;
use crate::api::server;
use crate::api::types::{AuthSession, Profile};
use crate::api::{ApiError, Pds};
use crate::convert::{self, Users};
use crate::ids;

/// The secret that holds the session.
const SECRET: &str = "session";

/// The session an account is signed in with. Holds bearer tokens, so `Debug` redacts it.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stored {
    pub did: String,
    pub handle: String,
    /// The origin of the account's PDS.
    pub pds: String,
    pub access_jwt: String,
    pub refresh_jwt: String,
    /// Where the event log was last read to. Opaque, from the network.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

impl Stored {
    /// The stored form of a fresh sign-in at `pds`, or at the PDS its DID document names.
    pub fn from_session(session: AuthSession, pds: String) -> Self {
        Self {
            did: session.did,
            handle: session.handle,
            pds,
            access_jwt: session.access_jwt,
            refresh_jwt: session.refresh_jwt,
            cursor: None,
        }
    }
}

impl fmt::Debug for Stored {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// Read the stored session of a login.
pub fn load(ctx: &LoginCtx) -> Result<Stored> {
    let bytes = ctx
        .get_secret(SECRET)?
        .ok_or_else(|| Error::Auth("no Bluesky session is stored".to_owned()))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| Error::Auth("the stored Bluesky session cannot be read".to_owned()))
}

/// Write the session of a login to the credential store.
pub fn store(ctx: &LoginCtx, stored: &Stored) -> Result<()> {
    let bytes = serde_json::to_vec(stored)
        .map_err(|err| Error::Internal(format!("cannot encode the Bluesky session: {err}")))?;
    ctx.set_secret(SECRET, &bytes)
}

/// The account of one live login.
pub struct Account {
    ctx: LoginCtx,
    hosts: Hosts,
    did: String,
    state: Mutex<Stored>,
    users: StdMutex<HashMap<String, RemoteUser>>,
}

impl Account {
    pub fn new(ctx: LoginCtx, hosts: Hosts, stored: Stored) -> Self {
        Self {
            ctx,
            hosts,
            did: stored.did.clone(),
            state: Mutex::new(stored),
            users: StdMutex::new(HashMap::new()),
        }
    }

    /// The account's own DID. It never changes for a login.
    pub fn did(&self) -> &str {
        &self.did
    }

    /// The event-log cursor, when one was saved.
    pub async fn cursor(&self) -> Option<String> {
        self.state.lock().await.cursor.clone()
    }

    /// Save the event-log cursor. Called only after the entries before it were handed on.
    pub async fn set_cursor(&self, cursor: String) -> Result<()> {
        let mut state = self.state.lock().await;
        state.cursor = Some(cursor);
        store(&self.ctx, &state)
    }

    /// Run one XRPC call with the current access token. An expired token is refreshed once and the
    /// call repeated; every other failure is returned as a bridge error.
    pub async fn send<T, F, Fut>(&self, op: F) -> Result<T>
    where
        F: Fn(Pds, String) -> Fut,
        Fut: Future<Output = std::result::Result<T, ApiError>>,
    {
        let (pds, token) = self.snapshot().await;
        match op(pds, token.clone()).await {
            Err(ApiError::ExpiredToken) => {
                self.refresh(&token).await?;
                let (pds, token) = self.snapshot().await;
                op(pds, token).await.map_err(ApiError::into_error)
            }
            other => other.map_err(ApiError::into_error),
        }
    }

    async fn snapshot(&self) -> (Pds, String) {
        let state = self.state.lock().await;
        (
            Pds::new(self.ctx.http.clone(), state.pds.clone()),
            state.access_jwt.clone(),
        )
    }

    /// Exchange the refresh token for new tokens. If another call already replaced `stale`, there is
    /// nothing to do.
    async fn refresh(&self, stale: &str) -> Result<()> {
        let mut state = self.state.lock().await;
        if state.access_jwt != stale {
            return Ok(());
        }
        let pds = Pds::new(self.ctx.http.clone(), state.pds.clone());
        let session = server::refresh_session(&pds, &state.refresh_jwt)
            .await
            .map_err(ApiError::into_error)?;
        if session.did != state.did {
            return Err(Error::Auth("the Bluesky account changed".to_owned()));
        }
        state.access_jwt = session.access_jwt;
        state.refresh_jwt = session.refresh_jwt;
        state.handle = session.handle;
        if let Some(endpoint) = session
            .did_doc
            .as_ref()
            .and_then(|doc| identity::pds_from_doc(doc, &self.hosts.scheme))
        {
            state.pds = endpoint;
        }
        store(&self.ctx, &state)
    }

    /// The profile of the account itself, or what the session knows of it.
    pub async fn me(&self) -> Result<RemoteUser> {
        let users = self.users(std::slice::from_ref(&self.did)).await?;
        if let Some(user) = users.get(&self.did) {
            return Ok(user.clone());
        }
        let handle = self.state.lock().await.handle.clone();
        Ok(RemoteUser {
            id: ids::user_id(&self.did)?,
            username: handle,
            ..RemoteUser::default()
        })
    }

    /// Remember a profile that came with a conversation or a message.
    pub fn remember(&self, profile: &Profile) {
        match convert::user_of_profile(profile) {
            Ok(user) => {
                self.cache().insert(profile.did.clone(), user);
            }
            Err(err) => log::debug!("bluesky: skipped a profile: {err}"),
        }
    }

    /// The users of the given DIDs. Unknown ones are fetched, in batches, and remembered.
    pub async fn users(&self, dids: &[String]) -> Result<Users> {
        let mut missing: Vec<String> = {
            let cache = self.cache();
            dids.iter()
                .filter(|did| !cache.contains_key(did.as_str()))
                .cloned()
                .collect()
        };
        missing.sort();
        missing.dedup();
        for batch in missing.chunks(PROFILE_BATCH) {
            let profiles = self
                .send(|pds, token| async move { actor::get_profiles(&pds, &token, batch).await })
                .await?;
            for profile in &profiles {
                self.remember(profile);
            }
        }
        let cache = self.cache();
        Ok(dids
            .iter()
            .filter_map(|did| cache.get(did).map(|user| (did.clone(), user.clone())))
            .collect())
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<String, RemoteUser>> {
        self.users.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_session_round_trips_and_hides_its_tokens() {
        let stored = Stored {
            did: "did:plc:abc".to_owned(),
            handle: "ada.test".to_owned(),
            pds: "https://pds.example".to_owned(),
            access_jwt: "access-secret".to_owned(),
            refresh_jwt: "refresh-secret".to_owned(),
            cursor: Some("cur".to_owned()),
        };
        let bytes = serde_json::to_vec(&stored).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.contains("\"accessJwt\""));
        let back: Stored = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.cursor.as_deref(), Some("cur"));
        assert_eq!(format!("{stored:?}"), "[redacted]");
    }

    #[test]
    fn a_stored_session_without_a_cursor_still_loads() {
        let text = r#"{"did":"did:plc:a","handle":"a.test","pds":"https://p","accessJwt":"x","refreshJwt":"y"}"#;
        let stored: Stored = serde_json::from_str(text).unwrap();
        assert!(stored.cursor.is_none());
    }
}
