//! The two ways to link a device: the QR code and the 8-character pairing code.
//!
//! Both end the same way: the primary phone sends `pair-success`, we verify it against the ADV
//! secret, store the device record, reconnect with the login payload and wait for `success`.
//! Only the first half differs. QR pairing shows references the server sends in `pair-device`.
//! Code pairing sends `link_code_companion_reg` with stage `companion_hello`, shows the code, and
//! answers the primary's `link_code_companion_reg` notification with stage `companion_finish`.

use std::collections::VecDeque;

use chirper_bridge::model::{InputField, LoginStep};
use chirper_bridge::network::LoginInput;
use chirper_bridge::{Error, LoginCtx, Result};
use prost::Message as _;
use tokio::time::{Duration, Instant};

use crate::binary::Node;
use crate::ids;
use crate::keys::{self, DeviceKeys, KeyPair};
use crate::pair::{self, FIRST_QR_TTL};
use crate::pair_code::{self, CompanionLink, CompanionRandom, FinishRandom, PrimaryNotice};
use crate::payload;
use crate::socket::{Connection, WS_URL};
use crate::transport::Wire;
use async_trait::async_trait;
use std::sync::Arc;

/// How long the server has to accept a login after pairing.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(30);

/// The step id of the QR login.
pub const QR_STEP: &str = "qr";

/// The step id of the pairing-code login.
pub const PHONE_STEP: &str = "phone";

/// The platform id of a web companion, as the phone shows it.
const PLATFORM_ID: &str = "1";

/// Which pairing a login runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Qr,
    Phone,
}

/// How a login opens its connections. The network path is [`SocketDialer`]; tests substitute a
/// scripted server.
#[async_trait]
pub trait Dialer: Send + Sync {
    /// Opens a connection that sends `payload` as its client payload.
    async fn dial(&self, noise: &KeyPair, payload: Vec<u8>) -> Result<Box<dyn Wire>>;
}

/// The real dialer: the Noise WebSocket to WhatsApp.
pub struct SocketDialer;

#[async_trait]
impl Dialer for SocketDialer {
    async fn dial(&self, noise: &KeyPair, payload: Vec<u8>) -> Result<Box<dyn Wire>> {
        Ok(Box::new(
            Connection::connect(WS_URL, noise.clone(), payload).await?,
        ))
    }
}

/// A login in progress. Drives the pairing with the server through a [`Wire`].
pub struct Pairing {
    pub ctx: LoginCtx,
    pub dialer: Arc<dyn Dialer>,
    pub keys: DeviceKeys,
    pub version: (u32, u32, u32),
    pub method: Method,
    /// The phone number the user typed, digits only, for the pairing code.
    pub phone: Option<String>,
    /// The connection to the server, once opened.
    pub wire: Option<Box<dyn Wire>>,
    /// QR pairing references the server sent that have not been shown yet.
    pub refs: VecDeque<String>,
    /// The QR code on screen and when it expires.
    pub current: Option<(String, Instant)>,
    /// The code-pairing state, from `companion_hello` until `companion_finish`.
    pub link: Option<(CompanionLink, String)>,
    /// Counter for request ids.
    pub counter: u64,
}

/// What a pairing waits for next.
enum Event {
    Node(Node),
    /// The QR code on screen expired.
    Rotate,
}

impl Pairing {
    /// A QR pairing with fresh keys.
    pub fn qr(ctx: LoginCtx, keys: DeviceKeys, version: (u32, u32, u32)) -> Self {
        Self::new(ctx, keys, version, Method::Qr)
    }

    /// A pairing-code login with fresh keys. The phone number comes in the first step.
    pub fn phone(ctx: LoginCtx, keys: DeviceKeys, version: (u32, u32, u32)) -> Self {
        Self::new(ctx, keys, version, Method::Phone)
    }

    fn new(ctx: LoginCtx, keys: DeviceKeys, version: (u32, u32, u32), method: Method) -> Self {
        Self {
            ctx,
            dialer: Arc::new(SocketDialer),
            keys,
            version,
            method,
            phone: None,
            wire: None,
            refs: VecDeque::new(),
            current: None,
            link: None,
            counter: 0,
        }
    }

    /// Opens connections through `dialer` instead of the network.
    pub fn with_dialer(mut self, dialer: Arc<dyn Dialer>) -> Self {
        self.dialer = dialer;
        self
    }

    /// The step that asks for the phone number of a pairing-code login.
    pub fn phone_step() -> LoginStep {
        LoginStep::UserInput {
            step_id: PHONE_STEP.to_owned(),
            instructions: "Enter the phone number of the account, with its country code."
                .to_owned(),
            fields: vec![InputField {
                id: "phone".to_owned(),
                name: "Phone number".to_owned(),
                kind: "phone".to_owned(),
                description: "For example +15551234567".to_owned(),
                options: Vec::new(),
                pattern: String::new(),
            }],
        }
    }

    /// The next id for a request.
    pub fn next_id(&mut self) -> String {
        self.counter += 1;
        format!("chirper.{}", self.counter)
    }

    /// Opens the connection with the registration payload, unless it is open.
    pub async fn ensure_wire(&mut self) -> Result<()> {
        if self.wire.is_some() {
            return Ok(());
        }
        let payload = payload::registration(&self.keys, self.version)?;
        self.wire = Some(
            self.dialer
                .dial(&self.keys.noise, payload.encode_to_vec())
                .await?,
        );
        Ok(())
    }

    /// Run the pairing until the next thing the user must see, or until the login completes.
    pub async fn advance(&mut self, input: LoginInput) -> Result<LoginStep> {
        match self.method {
            Method::Qr => {
                self.ensure_wire().await?;
                self.advance_loop().await
            }
            Method::Phone => {
                if self.link.is_none() {
                    let Some(phone) = input.values.get("phone") else {
                        return Ok(Self::phone_step());
                    };
                    self.start_code(phone).await
                } else {
                    self.advance_loop().await
                }
            }
        }
    }

    async fn advance_loop(&mut self) -> Result<LoginStep> {
        loop {
            match self.next_event().await? {
                Event::Rotate => return self.rotate(),
                Event::Node(node) => {
                    if let Some(step) = self.handle(&node).await? {
                        return Ok(step);
                    }
                    if self.is_done() {
                        return Err(Error::Protocol("the login already finished".to_owned()));
                    }
                }
            }
        }
    }

    fn is_done(&self) -> bool {
        false
    }

    /// The next node from the server, or the expiry of the QR on screen, whichever comes first.
    async fn next_event(&mut self) -> Result<Event> {
        let deadline = self.current.as_ref().map(|(_, deadline)| *deadline);
        let Some(wire) = self.wire.as_mut() else {
            return Err(Error::NotConnected);
        };
        match deadline {
            None => Ok(Event::Node(wire.next_node().await?)),
            Some(deadline) => tokio::select! {
                node = wire.next_node() => Ok(Event::Node(node?)),
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
                        let step = self.show_qr(&reference, FIRST_QR_TTL);
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
            "notification" => {
                self.send_notification_ack(node).await?;
                if node.child("link_code_companion_reg").is_some() {
                    self.finish_code(node).await?;
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
        let Some(wire) = self.wire.as_mut() else {
            return Err(Error::NotConnected);
        };
        wire.send_node(&ack).await
    }

    async fn send_notification_ack(&mut self, node: &Node) -> Result<()> {
        let Some(ack) = notification_ack(node) else {
            return Ok(());
        };
        let Some(wire) = self.wire.as_mut() else {
            return Err(Error::NotConnected);
        };
        wire.send_node(&ack).await
    }

    /// Show the next pairing reference when the current one expired.
    fn rotate(&mut self) -> Result<LoginStep> {
        let reference = self
            .refs
            .pop_front()
            .ok_or_else(|| Error::Auth("the QR code expired".to_owned()))?;
        Ok(self.show_qr(&reference, pair::QR_TTL))
    }

    fn show_qr(&mut self, reference: &str, ttl: Duration) -> LoginStep {
        let code = pair::qr_code(&self.keys, reference);
        self.current = Some((code.clone(), Instant::now() + ttl));
        LoginStep::DisplayAndWait {
            step_id: QR_STEP.to_owned(),
            instructions: "Scan this code with WhatsApp on your phone.".to_owned(),
            kind: "qr".to_owned(),
            data: code,
            image_url: String::new(),
        }
    }

    /// Sends `companion_hello` for `phone` and returns the code to show.
    async fn start_code(&mut self, phone: &str) -> Result<LoginStep> {
        let digits: String = phone.chars().filter(char::is_ascii_digit).collect();
        if digits.len() <= 6 {
            return Err(Error::Auth("the phone number is too short".to_owned()));
        }
        if digits.starts_with('0') {
            return Err(Error::Auth(
                "the phone number must start with the country code".to_owned(),
            ));
        }
        self.ensure_wire().await?;
        let random = CompanionRandom::generate()?;
        let link = pair_code::companion_link(&random)?;
        let jid = format!("{digits}@s.whatsapp.net");
        let id = self.next_id();
        let hello = Node::new("iq")
            .attr("to", pair::SERVER)
            .attr("type", "set")
            .attr("xmlns", "md")
            .attr("id", id.clone())
            .children(vec![
                Node::new("link_code_companion_reg")
                    .attr("jid", jid)
                    .attr("stage", "companion_hello")
                    .attr("should_show_push_notification", "true")
                    .children(vec![
                        Node::new("link_code_pairing_wrapped_companion_ephemeral_pub")
                            .bytes(link.wrapped_ephemeral.clone()),
                        Node::new("companion_server_auth_key_pub")
                            .bytes(self.keys.noise.public().to_vec()),
                        Node::new("companion_platform_id").bytes(PLATFORM_ID.as_bytes().to_vec()),
                        Node::new("companion_platform_display")
                            .bytes(payload::DEVICE_OS.as_bytes().to_vec()),
                        Node::new("link_code_pairing_nonce").bytes(vec![0]),
                    ]),
            ]);
        self.send(&hello).await?;
        let reply = self.await_result(&id).await?;
        let reference = reply
            .child("link_code_companion_reg")
            .and_then(|node| node.child("link_code_pairing_ref"))
            .and_then(Node::content_bytes)
            .ok_or_else(|| {
                Error::Protocol("pairing: the server sent no pairing reference".to_owned())
            })?
            .to_vec();
        let reference = String::from_utf8(reference).map_err(|_| {
            Error::Protocol("pairing: the pairing reference is not text".to_owned())
        })?;
        let display = pair_code::display_code(&link.code);
        self.link = Some((link, reference));
        self.phone = Some(digits);
        Ok(LoginStep::DisplayAndWait {
            step_id: PHONE_STEP.to_owned(),
            instructions: "On your phone: WhatsApp → Settings → Linked devices → Link with phone number instead, then enter this code.".to_owned(),
            kind: "code".to_owned(),
            data: display,
            image_url: String::new(),
        })
    }

    /// Answers the primary's notification with `companion_finish`.
    async fn finish_code(&mut self, notification: &Node) -> Result<()> {
        let node = notification
            .child("link_code_companion_reg")
            .ok_or_else(|| Error::Protocol("pairing: no link_code_companion_reg".to_owned()))?;
        let Some((link, reference)) = self.link.as_ref() else {
            return Err(Error::Protocol(
                "pairing: a code notification without a code".to_owned(),
            ));
        };
        let received_ref = node
            .child("link_code_pairing_ref")
            .and_then(Node::content_bytes)
            .unwrap_or_default();
        if received_ref != reference.as_bytes() {
            return Err(Error::Protocol(
                "pairing: the pairing reference does not match".to_owned(),
            ));
        }
        let wrapped = node
            .child("link_code_pairing_wrapped_primary_ephemeral_pub")
            .and_then(Node::content_bytes)
            .ok_or_else(|| Error::Protocol("pairing: no primary ephemeral key".to_owned()))?
            .to_vec();
        let identity_pub: [u8; 32] = node
            .child("primary_identity_pub")
            .and_then(Node::content_bytes)
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .ok_or_else(|| Error::Protocol("pairing: no primary identity key".to_owned()))?;
        let notice = PrimaryNotice {
            wrapped_ephemeral: wrapped,
            identity_pub,
        };
        let finish = pair_code::finish_link(
            link,
            &identity_keypair(&self.keys),
            &notice,
            &FinishRandom::generate()?,
        )?;
        let reference = reference.clone();
        self.keys.adv_secret = finish.adv_secret.clone();
        let id = self.next_id();
        let request = Node::new("iq")
            .attr("to", pair::SERVER)
            .attr("type", "set")
            .attr("xmlns", "md")
            .attr("id", id.clone())
            .children(vec![
                Node::new("link_code_companion_reg")
                    .attr(
                        "jid",
                        format!("{}@s.whatsapp.net", self.phone.clone().unwrap_or_default()),
                    )
                    .attr("stage", "companion_finish")
                    .children(vec![
                        Node::new("link_code_pairing_wrapped_key_bundle")
                            .bytes(finish.wrapped_key_bundle),
                        Node::new("companion_identity_public")
                            .bytes(self.keys.identity.public().to_vec()),
                        Node::new("link_code_pairing_ref").bytes(reference.into_bytes()),
                    ]),
            ]);
        self.send(&request).await?;
        self.link = None;
        Ok(())
    }

    async fn send(&mut self, node: &Node) -> Result<()> {
        let Some(wire) = self.wire.as_mut() else {
            return Err(Error::NotConnected);
        };
        wire.send_node(node).await
    }

    /// Reads nodes until the result of request `id` arrives. Other nodes are ignored here.
    async fn await_result(&mut self, id: &str) -> Result<Node> {
        let Some(wire) = self.wire.as_mut() else {
            return Err(Error::NotConnected);
        };
        tokio::time::timeout(LOGIN_TIMEOUT, async {
            loop {
                let node = wire.next_node().await?;
                if node.tag == "iq" && node.get_attr("id").as_deref() == Some(id) {
                    if node.get_attr("type").as_deref() == Some("error") {
                        return Err(Error::Protocol(
                            "pairing: the server refused a request".to_owned(),
                        ));
                    }
                    return Ok(node);
                }
            }
        })
        .await
        .map_err(|_| Error::Network("the server did not answer in time".to_owned()))?
    }

    /// Verify the phone's `pair-success`, store the device, reconnect with the login payload and
    /// wait for the server to accept it.
    async fn finish_pairing(&mut self, iq: &Node) -> Result<LoginStep> {
        let success = iq
            .child("pair-success")
            .ok_or_else(|| Error::Protocol("pairing: no pair-success".to_owned()))?;
        let paired = pair::complete(&mut self.keys, iq, success)?;
        let mut wire = self.wire.take().ok_or(Error::NotConnected)?;
        wire.send_node(&paired.reply).await?;
        wire.close().await;
        self.current = None;
        self.refs.clear();
        self.ctx.set_secret("device", &self.keys.to_record()?)?;

        let login_payload = payload::login(&self.keys, self.version)?;
        let mut login = self
            .dialer
            .dial(&self.keys.noise, login_payload.encode_to_vec())
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

        let login_id = ids::user_login_id(&paired.jid);
        Ok(LoginStep::Complete { login_id })
    }
}

/// The identity key pair in the form the pairing-code math uses.
fn identity_keypair(keys: &DeviceKeys) -> keys::KeyPair {
    keys::KeyPair::from_private(*keys.identity.private())
}

/// The acknowledgement of a `notification` node.
pub fn notification_ack(node: &Node) -> Option<Node> {
    let to = node.get_attr("from")?;
    let id = node.get_attr("id")?;
    let mut ack = Node::new("ack")
        .attr("class", "notification")
        .attr("to", to)
        .attr("id", id);
    if let Some(kind) = node.get_attr("type") {
        ack = ack.attr("type", kind);
    }
    Some(ack)
}
