//! The WhatsApp driver for Chirper's unified bridge runtime: our own multi-device client.
//!
//! Layers, bottom up: the Noise handshake and binary node codec ([`noise`], [`binary`]), the
//! protobuf messages ([`proto`]), the Signal protocol from `chirper-signal-protocol` behind
//! [`store`], pairing by QR code ([`pair`]) or by pairing code ([`pair_code`], via [`login`]), and
//! the live [`session`] with its receive ([`receive`]), send ([`send`]), media ([`media`]), app
//! state ([`appstate`]) and history ([`history`]) paths. Remote ids follow the Go bridge exactly
//! ([`ids`]), so the history the Go build stored stays reachable.
//!
//! Nothing here touches the network except through [`socket::Connection`] and the media HTTP
//! client. Tests run the login and session against an in-memory [`transport::Wire`].

#![forbid(unsafe_code)]

pub mod appstate;
pub mod binary;
pub mod convert;
pub mod crypto;
pub mod history;
pub mod ids;
pub mod keys;
pub mod login;
pub mod media;
pub mod noise;
pub mod pair;
pub mod pair_code;
pub mod payload;
pub mod proto;
pub mod receive;
pub mod send;
pub mod session;
pub mod socket;
pub mod store;
pub mod transport;
pub mod version;
pub mod xeddsa;

use async_trait::async_trait;
use prost::Message;

use chirper_bridge::model::{Capabilities, LoginFlow, LoginStep};
use chirper_bridge::network::LoginInput;
use chirper_bridge::{Error, LoginCtx, LoginProcess, Network, Result, Session};

use crate::keys::DeviceKeys;
use crate::login::{PHONE_STEP, Pairing, QR_STEP};
use crate::session::Client;
use crate::socket::{Connection, WS_URL};

/// The network id used in settings, account ids and every stored row. Never changes.
pub const NETWORK_ID: &str = "whatsapp";

/// The QR login flow id.
pub const QR_FLOW: &str = "qr";

/// The pairing-code login flow id.
pub const PHONE_FLOW: &str = "phone";

/// Why the driver is not usable yet. `None` once login, receive and send are covered by tests.
const UNAVAILABLE: Option<&str> = None;

/// The WhatsApp driver.
#[derive(Debug, Default, Clone)]
pub struct WhatsApp {}

impl WhatsApp {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl Network for WhatsApp {
    fn id(&self) -> &'static str {
        NETWORK_ID
    }

    fn name(&self) -> &'static str {
        "WhatsApp"
    }

    fn color(&self) -> &'static str {
        "#25D366"
    }

    fn capabilities(&self) -> Capabilities {
        session::capabilities()
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        vec![
            LoginFlow {
                id: QR_FLOW.to_owned(),
                name: "QR code".to_owned(),
                description: "On your phone: WhatsApp → Settings → Linked devices → Link a device."
                    .to_owned(),
            },
            LoginFlow {
                id: PHONE_FLOW.to_owned(),
                name: "Phone number".to_owned(),
                description:
                    "On your phone: WhatsApp → Settings → Linked devices → Link with phone number."
                        .to_owned(),
            },
        ]
    }

    fn unavailable(&self) -> Option<String> {
        UNAVAILABLE.map(str::to_owned)
    }

    async fn start_login(&self, flow_id: &str, ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        let version = version::fetch(&ctx.http).await?;
        let keys = DeviceKeys::generate()?;
        let pairing = match flow_id {
            QR_FLOW => Pairing::qr(ctx, keys, version),
            PHONE_FLOW => Pairing::phone(ctx, keys, version),
            _ => return Err(Error::Unsupported("this login flow")),
        };
        Ok(Box::new(PairingProcess {
            pairing,
            finished: false,
        }))
    }

    async fn connect(&self, ctx: LoginCtx) -> Result<Box<dyn Session>> {
        let record = ctx
            .get_secret("device")?
            .ok_or_else(|| Error::Auth("this login has no linked device".to_owned()))?;
        let keys = DeviceKeys::from_record(&record)?;
        let version = version::fetch(&ctx.http).await?;
        let payload = payload::login(&keys, version)?;
        let wire = Connection::connect(WS_URL, keys.noise.clone(), payload.encode_to_vec()).await?;
        let client = Client::start(wire, ctx, keys, None).await?;
        Ok(Box::new(client))
    }
}

/// One pairing run, driven by the user through the wire steps.
struct PairingProcess {
    pairing: Pairing,
    finished: bool,
}

#[async_trait]
impl LoginProcess for PairingProcess {
    async fn next(&mut self, input: LoginInput) -> Result<LoginStep> {
        if self.finished {
            return Err(Error::Protocol("the login already finished".to_owned()));
        }
        let result = self.pairing.advance(input).await;
        let keeps_going = match &result {
            Ok(LoginStep::DisplayAndWait { step_id, .. }) => {
                step_id == QR_STEP || step_id == PHONE_STEP
            }
            Ok(LoginStep::UserInput { .. }) => true,
            _ => false,
        };
        if !keeps_going {
            self.finished = true;
        }
        result
    }

    async fn cancel(&mut self) {
        self.finished = true;
        if let Some(mut wire) = self.pairing.wire.take() {
            wire.close().await;
        }
    }
}
