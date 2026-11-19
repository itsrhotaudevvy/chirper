//! The WhatsApp driver for Chirper's unified bridge runtime: our own multi-device client.
//!
//! Phase one covers the transport and linking: the Noise XX handshake over the WebSocket, the
//! binary node codec, and the QR login that links a device. Messaging, the Signal protocol, media
//! and history sync come next, so the network is still listed as unavailable.

#![forbid(unsafe_code)]

pub mod binary;
pub mod keys;
pub mod noise;
pub mod pair;
pub mod payload;
pub mod proto;
pub mod socket;
pub mod version;
pub mod xeddsa;

use std::collections::VecDeque;

use async_trait::async_trait;
use prost::Message;
use tokio::time::{Duration, Instant};

use chirper_bridge::model::{Capabilities, LoginFlow, LoginStep};
use chirper_bridge::network::LoginInput;
use chirper_bridge::{Error, LoginCtx, LoginProcess, Network, Result, Session};

use crate::binary::Node;
use crate::keys::DeviceKeys;
use crate::socket::{Connection, WS_URL};

/// The network id used in settings, account ids and every stored row. Never changes.
pub const NETWORK_ID: &str = "whatsapp";

/// The one login flow: link a device by scanning a QR code with the phone.
const QR_FLOW: &str = "qr";

/// How long the server has to accept a login after pairing.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(30);

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
        Capabilities::default()
    }

    fn login_flows(&self) -> Vec<LoginFlow> {
        vec![LoginFlow {
            id: QR_FLOW.to_owned(),
            name: "QR code".to_owned(),
            description: "On your phone: WhatsApp → Settings → Linked devices → Link a device."
                .to_owned(),
        }]
    }

    fn unavailable(&self) -> Option<String> {
        Some("WhatsApp messaging lands in the next phase".to_owned())
    }

    async fn start_login(&self, flow_id: &str, ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
        if flow_id != QR_FLOW {
            return Err(Error::Unsupported("this login flow"));
        }
        let version = version::fetch(&chirper_bridge::http_client()).await?;
        Ok(Box::new(QrLogin {
            ctx,
            keys: DeviceKeys::generate()?,
            version,
            connection: None,
            refs: VecDeque::new(),
            current: None,
            finished: false,
        }))
    }

    async fn connect(&self, _ctx: LoginCtx) -> Result<Box<dyn Session>> {
        Err(Error::Unsupported("connecting to WhatsApp"))
    }
}

/// The QR login: register a device, show its QR codes until the phone pairs, then log in.
struct QrLogin {
    ctx: LoginCtx,
    keys: DeviceKeys,
    /// The client version the server accepts, read at the start of the login.
    version: (u32, u32, u32),
    connection: Option<Connection>,
    /// Pairing references the server sent that have not been shown yet.
    refs: VecDeque<String>,
    /// The QR code on screen and when it expires. `None` until the first one is shown.
    current: Option<(String, Instant)>,
    finished: bool,
}

/// What the QR login waits for next.
enum Event {
    Node(Node),
    /// The QR on screen expired.
    Rotate,
}

#[async_trait]
impl LoginProcess for QrLogin {
    async fn next(&mut self, _input: LoginInput) -> Result<LoginStep> {
        if self.finished {
            return Err(Error::Protocol("the login already finished".to_owned()));
        }
        let result = self.advance().await;
        match &result {
            Ok(LoginStep::DisplayAndWait { .. }) => {}
            _ => self.finished = true,
        }
        result
    }

    async fn cancel(&mut self) {
        self.finished = true;
        if let Some(mut connection) = self.connection.take() {
            connection.close().await;
        }
    }
}

impl QrLogin {
    /// Run until the next thing the user must see, or until the login completes.
    async fn advance(&mut self) -> Result<LoginStep> {
        if self.connection.is_none() {
            let payload = payload::registration(&self.keys, self.version)?;
            let connection =
                Connection::connect(WS_URL, self.keys.noise.clone(), payload.encode_to_vec())
                    .await?;
            self.connection = Some(connection);
        }
        loop {
            match self.next_event().await? {
                Event::Rotate => return self.rotate(),
                Event::Node(node) => {
                    if let Some(step) = self.handle(&node).await? {
                        return Ok(step);
                    }
                }
            }
        }
    }

    /// The next node from the server, or the expiry of the QR on screen, whichever comes first.
    async fn next_event(&mut self) -> Result<Event> {
        let deadline = self.current.as_ref().map(|(_, deadline)| *deadline);
        let Some(connection) = self.connection.as_mut() else {
            return Err(Error::NotConnected);
        };
        match deadline {
            None => Ok(Event::Node(connection.next_node().await?)),
            Some(deadline) => tokio::select! {
                node = connection.next_node() => Ok(Event::Node(node?)),
                _ = tokio::time::sleep_until(deadline) => Ok(Event::Rotate),
            },
        }
    }

    /// Handle one server node. Returns a step when the user must see something now.
    async fn handle(&mut self, node: &Node) -> Result<Option<LoginStep>> {
        match node.tag.as_str() {
            "iq" => {
                if let Some(pair_device) = node.child("pair-device") {
                    self.send_ack(node).await?;
                    self.refs.extend(pair::pairing_refs(pair_device));
                    if self.current.is_none()
                        && let Some(reference) = self.refs.pop_front()
                    {
                        let step = self.show(&reference, pair::FIRST_QR_TTL);
                        return Ok(Some(step));
                    }
                    return Ok(None);
                }
                if node.child("pair-success").is_some() {
                    return self.finish_pairing(node).await.map(Some);
                }
                if node.get_attr("type").as_deref() == Some("get") {
                    self.send_ack(node).await?;
                }
                Ok(None)
            }
            "failure" => Err(Error::Auth("the server refused this device".to_owned())),
            "stream:error" => Err(Error::Network("the server ended the connection".to_owned())),
            _ => Ok(None),
        }
    }

    async fn send_ack(&mut self, iq: &Node) -> Result<()> {
        let Some(ack) = pair::ack(iq) else {
            return Ok(());
        };
        let Some(connection) = self.connection.as_mut() else {
            return Err(Error::NotConnected);
        };
        connection.send_node(&ack).await
    }

    /// Show the next pairing reference when the current one expired.
    fn rotate(&mut self) -> Result<LoginStep> {
        let reference = self
            .refs
            .pop_front()
            .ok_or_else(|| Error::Auth("the QR code expired".to_owned()))?;
        Ok(self.show(&reference, pair::QR_TTL))
    }

    fn show(&mut self, reference: &str, ttl: Duration) -> LoginStep {
        let code = pair::qr_code(&self.keys, reference);
        self.current = Some((code.clone(), Instant::now() + ttl));
        LoginStep::DisplayAndWait {
            step_id: QR_FLOW.to_owned(),
            instructions: "Scan this code with WhatsApp on your phone.".to_owned(),
            kind: "qr".to_owned(),
            data: code,
            image_url: String::new(),
        }
    }

    /// Verify the phone's `pair-success`, store the device, reconnect with the login payload and
    /// wait for the server to accept it.
    async fn finish_pairing(&mut self, iq: &Node) -> Result<LoginStep> {
        let success = iq
            .child("pair-success")
            .ok_or_else(|| Error::Protocol("pairing: no pair-success".to_owned()))?;
        let mut connection = self.connection.take().ok_or(Error::NotConnected)?;
        let paired = pair::complete(&mut self.keys, iq, success)?;
        connection.send_node(&paired.reply).await?;
        connection.close().await;
        self.current = None;
        self.refs.clear();
        self.ctx.set_secret("device", &self.keys.to_record()?)?;

        let login_payload = payload::login(&self.keys, self.version)?;
        let mut login = Connection::connect(
            WS_URL,
            self.keys.noise.clone(),
            login_payload.encode_to_vec(),
        )
        .await?;
        let accepted = tokio::time::timeout(LOGIN_TIMEOUT, async {
            loop {
                let node = login.next_node().await?;
                match node.tag.as_str() {
                    "success" => return Ok(()),
                    "failure" => {
                        return Err(Error::Auth(
                            "the server refused the linked device".to_owned(),
                        ));
                    }
                    "stream:error" => {
                        return Err(Error::Network("the server ended the login".to_owned()));
                    }
                    "iq" if node.get_attr("type").as_deref() == Some("get") => {
                        if let Some(ack) = pair::ack(&node) {
                            login.send_node(&ack).await?;
                        }
                    }
                    _ => {}
                }
            }
        })
        .await
        .map_err(|_| Error::Network("the server did not accept the login in time".to_owned()))?;
        login.close().await;
        accepted?;

        let login_id = paired
            .jid
            .user
            .split(['.', ':', '@'])
            .next()
            .unwrap_or_default()
            .to_owned();
        Ok(LoginStep::Complete { login_id })
    }
}
