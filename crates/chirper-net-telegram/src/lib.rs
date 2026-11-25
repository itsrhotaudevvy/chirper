//! The Telegram driver for Chirper's unified bridge runtime.
//!
//! The protocol client is our own (MTProto 2.0 over TCP, `mtproto/`, with the schema codegen in
//! `tl/`). Logins are `phone` (code, then two-step password), `qr` and `bot`. A login keeps its
//! auth keys and update state in the secret store and the data directory; the session syncs the
//! update state and every dialog, then delivers live events.

#![forbid(unsafe_code)]

pub mod mtproto;
pub mod tl;

mod backfill;
mod client;
mod commands;
mod convert;
mod error;
mod format;
mod ids;
mod login;
mod media;
mod peers;
mod session;
mod srp;
mod state;
mod store;

#[cfg(test)]
mod testing;

use std::sync::Arc;

use async_trait::async_trait;
use chirper_bridge::model::{Capabilities, LoginFlow};
use chirper_bridge::{
    Ack, BackfillPage, Command, Error, Event, LoginCtx, LoginProcess, Network, PortalRef,
    RemoteUser, Result, Session as BridgeSession,
};

use crate::client::Client;
use crate::store::Store;

/// The network id used in settings, account ids and every stored row. Never changes.
pub const NETWORK_ID: &str = "telegram";

/// The Telegram API id and hash this build was compiled with, if both were set.
pub fn api_credentials() -> Option<(i32, &'static str)> {
    let id = option_env!("CHIRPER_TELEGRAM_API_ID")?.parse().ok()?;
    let hash = option_env!("CHIRPER_TELEGRAM_API_HASH")?;
    Some((id, hash))
}

/// The application credentials a login uses.
#[derive(Clone)]
pub struct Credentials {
    pub api_id: i32,
    pub api_hash: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("api_id", &self.api_id)
            .field("api_hash", &"[redacted]")
            .finish()
    }
}

/// The Telegram driver.
#[derive(Debug, Default, Clone)]
pub struct Telegram {}

impl Telegram {
    pub fn new() -> Self {
        Self {}
    }
}

/// The credentials, or the reason this build cannot log in.
fn credentials() -> Result<Credentials> {
    let (api_id, api_hash) = api_credentials().ok_or(Error::Unsupported(
        "Telegram login in a build without CHIRPER_TELEGRAM_API_ID",
    ))?;
    Ok(Credentials {
        api_id,
        api_hash: api_hash.to_owned(),
    })
}

/// A live Telegram login, as the runtime sees it.
struct TelegramSession {
    inner: session::Session<Client>,
}

#[async_trait]
impl BridgeSession for TelegramSession {
    async fn next(&mut self) -> Option<Event> {
        self.inner.next().await
    }

    async fn command(&mut self, cmd: Command) -> Result<Ack> {
        self.inner.command(cmd).await
    }

    async fn backfill(
        &mut self,
        portal: &PortalRef,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<BackfillPage> {
        self.inner.backfill_page(portal, cursor, limit).await
    }

    async fn logout_remote(&mut self) -> Result<()> {
        self.inner.logout_remote().await
    }

    fn user(&self) -> Option<RemoteUser> {
        Some(self.inner.profile.clone())
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
        "#26A5E4"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::ALL
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        vec![
            LoginFlow {
                id: login::FLOW_PHONE.into(),
                name: "Phone Number".into(),
                description: "Login using your Telegram phone number".into(),
            },
            LoginFlow {
                id: login::FLOW_QR.into(),
                name: "QR Code".into(),
                description: "Login by scanning a QR code from your phone".into(),
            },
            LoginFlow {
                id: login::FLOW_BOT.into(),
                name: "Bot token".into(),
                description: "Log in as a bot using the bot token provided by BotFather".into(),
            },
        ]
    }

    fn unavailable(&self) -> Option<String> {
        if api_credentials().is_none() {
            return Some(
                "This build has no Telegram API id (set CHIRPER_TELEGRAM_API_ID and CHIRPER_TELEGRAM_API_HASH)"
                    .to_owned(),
            );
        }
        None
    }

    async fn start_login(&self, flow_id: &str, ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        let credentials = credentials()?;
        let store = Arc::new(Store::open(ctx.clone()));
        let (client, frames) = Client::new(store, credentials.api_id, None);
        match flow_id {
            login::FLOW_PHONE => Ok(Box::new(login::PhoneLogin::new(client, ctx, credentials))),
            login::FLOW_QR => Ok(Box::new(login::QrLogin::new(
                client,
                ctx,
                credentials,
                frames,
            ))),
            login::FLOW_BOT => Ok(Box::new(login::BotLogin::new(client, ctx, credentials))),
            _ => Err(Error::Unsupported("this Telegram login flow")),
        }
    }

    async fn connect(&self, ctx: LoginCtx) -> Result<Box<dyn BridgeSession>> {
        let credentials = credentials()?;
        let store = Arc::new(Store::open(ctx));
        let home = store.home_dc()?.unwrap_or(client::DEFAULT_DC);
        if store.auth_key(home)?.is_none() {
            return Err(Error::Auth(
                "no saved Telegram login for this account".into(),
            ));
        }
        let (client, frames) = Client::new(Arc::clone(&store), credentials.api_id, Some(home));
        let inner = session::Session::start(client, store, frames).await?;
        Ok(Box::new(TelegramSession { inner }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_driver_describes_itself_as_the_go_build_did() {
        let telegram = Telegram::new();
        assert_eq!(telegram.id(), "telegram");
        assert_eq!(telegram.color(), "#26A5E4");
        let flows: Vec<String> = telegram.login_flows().into_iter().map(|f| f.id).collect();
        assert_eq!(flows, ["phone", "qr", "bot"]);
        assert_eq!(telegram.capabilities(), Capabilities::ALL);
    }

    #[test]
    fn unavailable_follows_the_build_credentials() {
        let telegram = Telegram::new();
        assert_eq!(
            telegram.unavailable().is_none(),
            api_credentials().is_some()
        );
    }
}
