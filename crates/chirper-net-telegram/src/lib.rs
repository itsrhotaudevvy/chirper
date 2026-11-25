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

/// The reason shown when neither the user's settings nor the build carry an API id.
const NO_CREDENTIALS_REASON: &str = "Telegram needs an API id. Enter yours in Settings → Advanced";

/// The Telegram API id and hash this build was compiled with, if both were set.
fn compiled_credentials() -> Option<(i32, &'static str)> {
    let id = option_env!("CHIRPER_TELEGRAM_API_ID")?.parse().ok()?;
    let hash = option_env!("CHIRPER_TELEGRAM_API_HASH")?;
    Some((id, hash))
}

/// Whether `hash` has the shape of a Telegram API hash: 32 hexadecimal characters.
pub fn is_valid_api_hash(hash: &str) -> bool {
    hash.len() == 32 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
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

/// The Telegram driver. It logs in with the API id and hash the user entered in Settings when those
/// are valid, and with the pair the build was compiled with otherwise.
#[derive(Debug, Default, Clone)]
pub struct Telegram {
    /// The user's pair, present only when it is valid.
    user: Option<Credentials>,
}

impl Telegram {
    /// A driver that uses the build's compiled-in credentials.
    pub fn new() -> Self {
        Self { user: None }
    }

    /// A driver that prefers the user's API id and hash. A pair that is not valid (an id that is
    /// not positive, or a hash that is not 32 hexadecimal characters) is ignored, and the
    /// compiled-in pair is used instead.
    pub fn with_credentials(api_id: i32, api_hash: String) -> Self {
        Self {
            user: user_credentials(api_id, &api_hash),
        }
    }

    /// The credentials of this instance, if any are available.
    fn active_credentials(&self) -> Option<Credentials> {
        resolve(self.user.clone(), compiled_credentials())
    }

    /// The credentials, or the reason this build cannot log in.
    fn credentials(&self) -> Result<Credentials> {
        self.active_credentials()
            .ok_or(Error::Unsupported(NO_CREDENTIALS_REASON))
    }
}

/// The user's pair as credentials, when it is valid.
fn user_credentials(api_id: i32, api_hash: &str) -> Option<Credentials> {
    (api_id > 0 && is_valid_api_hash(api_hash)).then(|| Credentials {
        api_id,
        api_hash: api_hash.to_owned(),
    })
}

/// The user's credentials when present, otherwise the compiled-in pair.
fn resolve(user: Option<Credentials>, compiled: Option<(i32, &str)>) -> Option<Credentials> {
    user.or_else(|| {
        compiled.map(|(api_id, api_hash)| Credentials {
            api_id,
            api_hash: api_hash.to_owned(),
        })
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
        self.active_credentials()
            .is_none()
            .then(|| NO_CREDENTIALS_REASON.to_owned())
    }

    async fn start_login(&self, flow_id: &str, ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        let credentials = self.credentials()?;
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
        let credentials = self.credentials()?;
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
            compiled_credentials().is_some()
        );
    }

    const HASH: &str = "0123456789abcdef0123456789ABCDEF";

    fn user(id: i32, hash: &str) -> Credentials {
        Credentials {
            api_id: id,
            api_hash: hash.to_owned(),
        }
    }

    #[test]
    fn the_user_pair_overrides_the_compiled_in_pair() {
        let resolved = resolve(
            Some(user(7, HASH)),
            Some((2, "ffffffffffffffffffffffffffffffff")),
        );
        let resolved = resolved.expect("a credential");
        assert_eq!((resolved.api_id, resolved.api_hash.as_str()), (7, HASH));
    }

    #[test]
    fn the_compiled_in_pair_is_used_when_the_user_has_none() {
        let compiled = "ffffffffffffffffffffffffffffffff";
        let resolved = resolve(None, Some((2, compiled))).expect("a credential");
        assert_eq!((resolved.api_id, resolved.api_hash.as_str()), (2, compiled));
    }

    #[test]
    fn there_are_no_credentials_when_neither_source_has_them() {
        assert!(resolve(None, None).is_none());
    }

    #[test]
    fn invalid_user_values_are_ignored() {
        assert!(user_credentials(0, HASH).is_none(), "an id of zero");
        assert!(user_credentials(-5, HASH).is_none(), "a negative id");
        assert!(user_credentials(12, "").is_none(), "an empty hash");
        assert!(user_credentials(12, &HASH[1..]).is_none(), "a short hash");
        assert!(
            user_credentials(12, &format!("{HASH}0")).is_none(),
            "a long hash"
        );
        let not_hex = "g123456789abcdef0123456789abcdef";
        assert!(user_credentials(12, not_hex).is_none(), "a non-hex hash");
        assert!(user_credentials(12, HASH).is_some());
    }

    #[test]
    fn a_valid_user_pair_makes_the_driver_available() {
        let telegram = Telegram::with_credentials(12345, HASH.to_owned());
        assert!(telegram.unavailable().is_none());
        let credentials = telegram.credentials().expect("a credential");
        assert_eq!(credentials.api_id, 12345);
        assert_eq!(credentials.api_hash, HASH);
    }

    #[test]
    fn an_invalid_user_pair_falls_back_to_the_build() {
        let telegram = Telegram::with_credentials(0, String::new());
        assert_eq!(
            telegram.active_credentials().map(|c| c.api_id),
            compiled_credentials().map(|(id, _)| id)
        );
        assert_eq!(
            telegram.unavailable().is_none(),
            compiled_credentials().is_some()
        );
    }

    #[test]
    fn the_hash_shape_is_32_hex_characters() {
        assert!(is_valid_api_hash(HASH));
        assert!(!is_valid_api_hash(""));
        assert!(!is_valid_api_hash(&HASH[..31]));
        assert!(!is_valid_api_hash("0123456789abcdef0123456789abcdeg"));
    }

    #[test]
    fn the_missing_credentials_reason_names_the_settings_page() {
        assert!(NO_CREDENTIALS_REASON.contains("Settings → Advanced"));
    }
}
