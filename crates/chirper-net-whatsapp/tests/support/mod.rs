//! A scripted WhatsApp server and a fake phone, for end-to-end tests without a network.
//!
//! The server is a [`Wire`]: it records what the driver sends, answers each request with the
//! reply a real server gives, and queues whatever the test pushes. The phone is a Signal store
//! from `chirper-signal-protocol`, so messages are really encrypted and decrypted.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chirper_bridge::{Error, LoginCtx, Result, Secrets};
use chirper_net_whatsapp::binary::{Jid, Node};
use chirper_net_whatsapp::keys::{DeviceKeys, KeyPair};
use chirper_net_whatsapp::login::Dialer;
use chirper_net_whatsapp::proto::e2e::Message;
use chirper_net_whatsapp::proto::{
    AdvDeviceIdentity, AdvSignedDeviceIdentity, AdvSignedDeviceIdentityHmac,
};
use chirper_net_whatsapp::transport::Wire;
use chirper_signal_protocol::identity::{IdentityKey, IdentityKeyPair};
use chirper_signal_protocol::message::{CiphertextMessage, PreKeySignalMessage, SignalMessage};
use chirper_signal_protocol::prekey::{PreKeyBundle, PreKeyRecord, SignedPreKeyRecord};
use chirper_signal_protocol::store::{PreKeyStore, SignedPreKeyStore};
use chirper_signal_protocol::{ProtocolAddress, curve::PublicKey, memory::InMemoryProtocolStore};
use prost::Message as _;
use rand::rngs::OsRng;

/// The account and device the driver logs in as.
pub const OUR_USER: &str = "9990";
pub const OUR_DEVICE: u16 = 7;

/// The phone's JID (primary device).
pub const PHONE: &str = "555@s.whatsapp.net";

/// The group the server reports.
pub const GROUP: &str = "12@g.us";

/// Secrets in memory.
#[derive(Default)]
pub struct MemSecrets(Mutex<HashMap<String, Vec<u8>>>);

impl Secrets for MemSecrets {
    fn get(&self, key: &str) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>> {
        Ok(self
            .0
            .lock()
            .map_err(|_| Error::Internal("poisoned".to_owned()))?
            .get(key)
            .map(|v| zeroize::Zeroizing::new(v.clone())))
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| Error::Internal("poisoned".to_owned()))?
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| Error::Internal("poisoned".to_owned()))?
            .remove(key);
        Ok(())
    }
}

/// A login context over fresh in-memory secrets.
pub fn login_ctx() -> LoginCtx {
    LoginCtx {
        network: "whatsapp".to_owned(),
        login_id: OUR_USER.to_owned(),
        data_dir: PathBuf::from("/nonexistent"),
        secrets: Arc::new(MemSecrets::default()),
        http: reqwest::Client::new(),
    }
}

/// Our device keys, linked to [`OUR_USER`]:[`OUR_DEVICE`].
pub fn our_keys() -> DeviceKeys {
    let mut keys = DeviceKeys::generate().expect("keys");
    keys.jid = Some(
        format!("{OUR_USER}:{OUR_DEVICE}@s.whatsapp.net")
            .parse()
            .expect("jid"),
    );
    keys.account_identity = Some(vec![1, 2, 3, 4]);
    keys
}

/// Pads a plaintext as the sender does: `n` copies of `n`, for `n` in 1..=15.
pub fn pad(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    out.extend(std::iter::repeat_n(3u8, 3));
    out
}

/// A phone: a device with its own Signal identity, one signed prekey and one one-time prekey.
pub struct Phone {
    pub jid: Jid,
    pub store: InMemoryProtocolStore,
    pub identity: IdentityKeyPair,
    pub signed: SignedPreKeyRecord,
    pub one_time: PreKeyRecord,
}

impl Phone {
    pub fn new(jid: &str) -> Self {
        let identity = IdentityKeyPair::generate(&mut OsRng);
        let mut store = InMemoryProtocolStore::new(identity.clone(), 222);
        let signed =
            chirper_signal_protocol::prekey::generate_signed_pre_key(&identity, 1, 0, &mut OsRng)
                .expect("signed prekey");
        let one_time = chirper_signal_protocol::prekey::generate_pre_keys(100, 1, &mut OsRng)
            .expect("prekeys")
            .remove(0);
        store.store_signed_pre_key(&signed).expect("store");
        store.store_pre_key(&one_time).expect("store");
        Self {
            jid: jid.parse().expect("jid"),
            store,
            identity,
            signed,
            one_time,
        }
    }

    /// The bundle a server hands out for this phone.
    pub fn bundle(&self) -> PreKeyBundle {
        PreKeyBundle {
            registration_id: 222,
            device_id: u32::from(self.jid.device),
            pre_key: Some((self.one_time.id(), self.one_time.key_pair().public_key)),
            signed_pre_key_id: self.signed.id(),
            signed_pre_key_public: self.signed.key_pair().public_key,
            signed_pre_key_signature: *self.signed.signature(),
            identity_key: self.identity.identity_key(),
            kyber_pre_key: None,
        }
    }

    /// The `user` node of a prekey answer for this phone.
    pub fn bundle_node(&self) -> Node {
        let bundle = self.bundle();
        let (pre_id, pre_pub) = bundle.pre_key.expect("one-time key");
        Node::new("user")
            .attr("jid", self.jid.to_string())
            .children(vec![
                Node::new("registration").bytes(222u32.to_be_bytes().to_vec()),
                Node::new("identity")
                    .bytes(bundle.identity_key.public_key().public_key_bytes().to_vec()),
                key_node("key", pre_id, &pre_pub),
                Node::new("skey").children(vec![
                    Node::new("id").bytes(bundle.signed_pre_key_id.to_be_bytes()[1..].to_vec()),
                    Node::new("value")
                        .bytes(bundle.signed_pre_key_public.public_key_bytes().to_vec()),
                    Node::new("signature").bytes(bundle.signed_pre_key_signature.to_vec()),
                ]),
            ])
    }

    /// Encrypts `message` for our device. Returns the enc type and the serialized ciphertext.
    pub fn encrypt_for_us(
        &mut self,
        message: &Message,
        us: &ProtocolAddress,
        bundle: Option<&PreKeyBundle>,
    ) -> (&'static str, Vec<u8>) {
        if let Some(bundle) = bundle {
            chirper_signal_protocol::session_builder::process_prekey_bundle(
                &mut self.store,
                us,
                bundle,
                &mut OsRng,
            )
            .expect("process bundle");
        }
        let plain = pad(&message.encode_to_vec());
        let ct =
            chirper_signal_protocol::cipher::encrypt(&mut self.store, us, &plain).expect("encrypt");
        let kind = match ct {
            CiphertextMessage::PreKey(_) => "pkmsg",
            CiphertextMessage::Signal(_) => "msg",
        };
        (kind, ct.serialized().to_vec())
    }

    /// Decrypts a message our device sent to this phone.
    pub fn decrypt_from_us(&mut self, kind: &str, bytes: &[u8], us: &ProtocolAddress) -> Message {
        let plain = if kind == "pkmsg" {
            let message = PreKeySignalMessage::deserialize(bytes).expect("pkmsg");
            chirper_signal_protocol::cipher::decrypt_prekey_message(
                &mut self.store,
                us,
                &message,
                &mut OsRng,
            )
        } else {
            let message = SignalMessage::deserialize(bytes).expect("msg");
            chirper_signal_protocol::cipher::decrypt_signal_message(
                &mut self.store,
                us,
                &message,
                &mut OsRng,
            )
        }
        .expect("decrypt");
        let body = chirper_net_whatsapp::crypto::unpad_message(&plain, 2).expect("unpad");
        Message::decode(body.as_slice()).expect("message")
    }
}

/// A key node: a 3-byte id and a 32-byte public key.
pub fn key_node(tag: &str, id: u32, public: &PublicKey) -> Node {
    let bytes = id.to_be_bytes();
    Node::new(tag).children(vec![
        Node::new("id").bytes(bytes[1..].to_vec()),
        Node::new("value").bytes(public.public_key_bytes().to_vec()),
    ])
}

/// The shared state of the fake server.
pub struct World {
    pub ours: DeviceKeys,
    /// Prekeys the driver uploaded: id and public key.
    pub uploaded: Vec<(u32, [u8; 32])>,
    /// Prekey count the server reports.
    pub server_count: u32,
    pub phones: Vec<Phone>,
    /// Every node the driver sent, in order.
    pub sent: Vec<Node>,
    /// Device ids per user, for the usync answer.
    pub devices: HashMap<String, Vec<u16>>,
    /// The app-state patch served once, for the `regular` collection.
    pub patch: Option<Node>,
    /// Nodes the server sends after answering the current request.
    pub extra: Vec<Node>,
    /// Inboxes of every wire opened, newest last.
    pub inboxes: Vec<Arc<Mutex<VecDeque<Node>>>>,
    /// Nodes a newly opened wire starts with.
    pub next_dial_seed: Vec<Node>,
    /// The media host answer's base URL is injected by the test, not here.
    pub media_host: String,
    /// The pairing-code state of the primary phone, when a code login runs.
    pub pairing: Option<PairingScript>,
}

impl World {
    pub fn new() -> Self {
        let mut devices = HashMap::new();
        devices.insert("555".to_owned(), vec![0]);
        devices.insert(OUR_USER.to_owned(), vec![OUR_DEVICE, 9]);
        Self {
            ours: our_keys(),
            uploaded: Vec::new(),
            server_count: 0,
            phones: vec![Phone::new(PHONE)],
            sent: Vec::new(),
            devices,
            patch: None,
            extra: Vec::new(),
            inboxes: Vec::new(),
            next_dial_seed: Vec::new(),
            media_host: "127.0.0.1".to_owned(),
            pairing: None,
        }
    }

    /// Our address as a Signal peer sees it.
    pub fn our_address() -> ProtocolAddress {
        ProtocolAddress::new(OUR_USER, u8::try_from(OUR_DEVICE).expect("device"))
    }

    /// The prekey bundle a phone uses to message our device.
    pub fn our_bundle(&self) -> PreKeyBundle {
        let (id, public) = self.uploaded[0];
        let identity = self.ours.identity.public();
        PreKeyBundle {
            registration_id: self.ours.registration_id,
            device_id: u32::from(OUR_DEVICE),
            pre_key: Some((
                id,
                PublicKey::from_djb_public_key_bytes(&public).expect("key"),
            )),
            signed_pre_key_id: 1,
            signed_pre_key_public: PublicKey::from_djb_public_key_bytes(
                self.ours.signed_pre_key.public(),
            )
            .expect("key"),
            signed_pre_key_signature: self.ours.signed_pre_key_signature,
            identity_key: IdentityKey::new(
                PublicKey::from_djb_public_key_bytes(identity).expect("key"),
            ),
            kyber_pre_key: None,
        }
    }

    /// Answers one node the driver sent.
    pub fn answer(&mut self, node: &Node) -> Vec<Node> {
        self.sent.push(node.clone());
        match node.tag.as_str() {
            "message" => {
                let id = node.get_attr("id").unwrap_or_default();
                let to = node.get_attr("to").unwrap_or_default();
                vec![
                    Node::new("ack")
                        .attr("class", "message")
                        .attr("id", id)
                        .attr("from", to),
                ]
            }
            "iq" => {
                let mut out = self.answer_iq(node);
                out.extend(std::mem::take(&mut self.extra));
                out
            }
            _ => Vec::new(),
        }
    }

    fn answer_iq(&mut self, node: &Node) -> Vec<Node> {
        let id = node.get_attr("id").unwrap_or_default();
        let xmlns = node.get_attr("xmlns").unwrap_or_default();
        let to = node.get_attr("to").unwrap_or_default();
        let kind = node.get_attr("type").unwrap_or_default();
        let children = node.child_nodes();
        let body: Vec<Node> = match xmlns.as_str() {
            "encrypt" if kind == "get" => {
                if children.iter().any(|c| c.tag == "count") {
                    vec![Node::new("count").attr("value", self.server_count.to_string())]
                } else {
                    let users = node
                        .child("key")
                        .map(|key| key.child_nodes().to_vec())
                        .unwrap_or_default();
                    let mut list = Vec::new();
                    for user in users {
                        let Some(jid) = user
                            .get_jid("jid")
                            .cloned()
                            .or_else(|| user.get_attr("jid").and_then(|j| j.parse().ok()))
                        else {
                            continue;
                        };
                        if let Some(phone) = self
                            .phones
                            .iter()
                            .find(|p| p.jid.user == jid.user && p.jid.device == jid.device)
                        {
                            list.push(phone.bundle_node());
                        }
                    }
                    vec![Node::new("list").children(list)]
                }
            }
            "encrypt" => {
                // Upload of our prekeys: `list > key` with id and value.
                if let Some(list) = node.child("list") {
                    for key in list.child_nodes().iter().filter(|k| k.tag == "key") {
                        let idb = key
                            .child("id")
                            .and_then(Node::content_bytes)
                            .unwrap_or(&[0, 0, 0]);
                        let value = key
                            .child("value")
                            .and_then(Node::content_bytes)
                            .unwrap_or(&[0; 32]);
                        let id = u32::from_be_bytes([0, idb[0], idb[1], idb[2]]);
                        let mut public = [0u8; 32];
                        public.copy_from_slice(&value[..32]);
                        self.uploaded.push((id, public));
                    }
                }
                Vec::new()
            }
            "w:g2" => {
                if children.iter().any(|c| c.tag == "participating") {
                    vec![Node::new("participating").children(vec![Node::new("group")
                        .attr("id", "12")
                        .attr("subject", "Friends")
                        .children(vec![Node::new("participant").attr("jid", PHONE)])])]
                } else {
                    vec![
                        Node::new("group")
                            .children(vec![Node::new("participant").attr("jid", PHONE)]),
                    ]
                }
            }
            "w:sync:app:state" => {
                let collection = node
                    .child("sync")
                    .and_then(|s| s.child("collection"))
                    .and_then(|c| c.get_attr("name"))
                    .unwrap_or_default();
                let mut collection_node = Node::new("collection").attr("name", collection.clone());
                if collection == "regular"
                    && let Some(patch) = self.patch.take()
                {
                    collection_node =
                        collection_node.children(vec![Node::new("patches").children(vec![patch])]);
                }
                vec![Node::new("sync").children(vec![collection_node])]
            }
            "w:profile:picture" => {
                vec![Node::new("picture").attr("url", "https://pics.example/g12.jpg")]
            }
            "usync" => {
                let users: Vec<Node> = node
                    .child("usync")
                    .and_then(|u| u.child("list"))
                    .map(|list| list.child_nodes().to_vec())
                    .unwrap_or_default();
                let mut out = Vec::new();
                for user in users {
                    let Some(jid) = user.get_attr("jid") else {
                        continue;
                    };
                    let parsed: Jid = jid.parse().expect("jid");
                    let devices = self.devices.get(&parsed.user).cloned().unwrap_or_default();
                    let device_nodes: Vec<Node> = devices
                        .iter()
                        .map(|d| Node::new("device").attr("id", d.to_string()))
                        .collect();
                    out.push(Node::new("user").attr("jid", jid).children(vec![
                            Node::new("devices")
                                .children(vec![Node::new("device-list").children(device_nodes)]),
                        ]));
                }
                vec![Node::new("usync").children(vec![Node::new("list").children(out)])]
            }
            "w:m" => vec![
                Node::new("media_conn")
                    .attr("auth", "token-a")
                    .attr("ttl", "300")
                    .children(vec![
                        Node::new("host").attr("hostname", self.media_host.clone()),
                    ]),
            ],
            "md" if children.iter().any(|c| c.tag == "link_code_companion_reg") => {
                self.answer_pairing(node)
            }
            _ => Vec::new(),
        };
        let mut reply = Node::new("iq")
            .attr("id", id)
            .attr("type", "result")
            .attr("from", "s.whatsapp.net")
            .children(body);
        if !to.is_empty() {
            reply = reply.attr("to", to);
        }
        vec![reply]
    }
}

/// The server's half of a pairing-code login (see `tests/pairing.rs`).
#[derive(Clone)]
pub struct PairingScript {
    /// The companion's hello, as it arrived.
    pub hello: Option<Node>,
    /// The linking code the user typed, once known.
    pub code: Option<String>,
    /// The primary phone's ephemeral key.
    pub primary_ephemeral: KeyPair,
    /// The primary phone's identity key.
    pub primary_identity: KeyPair,
    /// The account key that signs the device identity.
    pub account: KeyPair,
    /// The account key pair's signing randomness, fixed for the test.
    pub account_rand: [u8; 64],
}

/// The `pair-success` iq a primary phone sends once the companion's ADV secret is known: a device
/// identity signed by the account key, and an HMAC of it under the ADV secret.
pub fn pair_success_node(
    adv: &[u8; 32],
    companion_identity: &[u8; 32],
    account: &KeyPair,
    account_rand: &[u8; 64],
) -> Node {
    let inner = AdvDeviceIdentity {
        raw_id: Some(1),
        timestamp: Some(0),
        key_index: Some(1),
        account_type: None,
        device_type: None,
    }
    .encode_to_vec();
    let prefix = [6u8, 0];
    let signature = chirper_net_whatsapp::xeddsa::sign_with_random(
        account.private(),
        &[&prefix[..], &inner, companion_identity],
        account_rand,
    );
    let signed = AdvSignedDeviceIdentity {
        details: Some(inner),
        account_signature_key: Some(account.public().to_vec()),
        account_signature: Some(signature.to_vec()),
        device_signature: None,
    }
    .encode_to_vec();
    let mac = chirper_net_whatsapp::crypto::hmac256(adv, &[&signed]).expect("hmac");
    let container = AdvSignedDeviceIdentityHmac {
        details: Some(signed),
        hmac: Some(mac.to_vec()),
        account_type: None,
    }
    .encode_to_vec();
    Node::new("iq")
        .attr("id", "ps1")
        .attr("type", "set")
        .attr("from", "s.whatsapp.net")
        .children(vec![Node::new("pair-success").children(vec![
            Node::new("device-identity").bytes(container),
            Node::new("device").attr("jid", format!("{OUR_USER}:{OUR_DEVICE}@s.whatsapp.net")),
            Node::new("platform").attr("name", "android"),
        ])])
}

impl World {
    fn answer_pairing(&mut self, node: &Node) -> Vec<Node> {
        let Some(link) = node.child("link_code_companion_reg") else {
            return Vec::new();
        };
        let stage = link.get_attr("stage").unwrap_or_default();
        let Some(script) = self.pairing.clone() else {
            return Vec::new();
        };
        match stage.as_str() {
            "companion_hello" => {
                if let Some(state) = self.pairing.as_mut() {
                    state.hello = Some(node.clone());
                }
                vec![Node::new("link_code_companion_reg").children(vec![
                    Node::new("link_code_pairing_ref").bytes(b"PAIRREF".to_vec()),
                ])]
            }
            "companion_finish" => {
                // The primary's half: unwrap the companion's ephemeral key with the code, open the
                // key bundle, and derive the ADV secret both sides share.
                let (Some(code), Some(hello)) = (script.code.clone(), script.hello.clone()) else {
                    return Vec::new();
                };
                let Some(hello_link) = hello.child("link_code_companion_reg") else {
                    return Vec::new();
                };
                let wrapped = hello_link
                    .child("link_code_pairing_wrapped_companion_ephemeral_pub")
                    .and_then(Node::content_bytes)
                    .unwrap_or_default()
                    .to_vec();
                let wrap_key = chirper_net_whatsapp::crypto::pbkdf2_sha256(
                    code.as_bytes(),
                    &wrapped[..32],
                    chirper_net_whatsapp::pair_code::PBKDF2_ROUNDS,
                    32,
                );
                let mut key = [0u8; 32];
                key.copy_from_slice(&wrap_key);
                let mut iv = [0u8; 16];
                iv.copy_from_slice(&wrapped[32..48]);
                let mut companion_pub = [0u8; 32];
                companion_pub.copy_from_slice(&wrapped[48..80]);
                chirper_net_whatsapp::crypto::aes_ctr(&key, &iv, &mut companion_pub).expect("ctr");

                let bundle = link
                    .child("link_code_pairing_wrapped_key_bundle")
                    .and_then(Node::content_bytes)
                    .unwrap_or_default()
                    .to_vec();
                let mut salt = [0u8; 32];
                salt.copy_from_slice(&bundle[..32]);
                let mut nonce = [0u8; 12];
                nonce.copy_from_slice(&bundle[32..44]);
                let mut identity = [0u8; 32];
                identity.copy_from_slice(
                    link.child("companion_identity_public")
                        .and_then(Node::content_bytes)
                        .unwrap_or(&[0; 32]),
                );
                let eph = script.primary_ephemeral.agree(&companion_pub).expect("dh");
                let bundle_key = chirper_net_whatsapp::crypto::hkdf_salted(
                    eph.as_slice(),
                    &salt,
                    b"link_code_pairing_key_bundle_encryption_key",
                    32,
                )
                .expect("hkdf");
                let mut bundle_key_arr = [0u8; 32];
                bundle_key_arr.copy_from_slice(&bundle_key);
                let opened =
                    chirper_net_whatsapp::crypto::gcm_open(&bundle_key_arr, &nonce, &bundle[44..])
                        .expect("bundle opens");
                let adv_random = &opened[64..96];
                let id_shared = script.primary_identity.agree(&identity).expect("dh");
                let mut adv_input = Vec::new();
                adv_input.extend_from_slice(eph.as_slice());
                adv_input.extend_from_slice(id_shared.as_slice());
                adv_input.extend_from_slice(adv_random);
                let adv = chirper_net_whatsapp::crypto::hkdf(&adv_input, b"adv_secret", 32)
                    .expect("hkdf");
                let mut adv_arr = [0u8; 32];
                adv_arr.copy_from_slice(&adv);
                self.extra.push(pair_success_node(
                    &adv_arr,
                    &identity,
                    &script.account,
                    &script.account_rand,
                ));
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

/// The wire the driver talks to: a shared inbox and the server behind it.
pub struct FakeWire {
    pub world: Arc<Mutex<World>>,
    pub inbox: Arc<Mutex<VecDeque<Node>>>,
}

#[async_trait]
impl Wire for FakeWire {
    async fn send_node(&mut self, node: &Node) -> Result<()> {
        let replies = self
            .world
            .lock()
            .map_err(|_| Error::Internal("poisoned".to_owned()))?
            .answer(node);
        let mut inbox = self
            .inbox
            .lock()
            .map_err(|_| Error::Internal("poisoned".to_owned()))?;
        inbox.extend(replies);
        Ok(())
    }

    async fn next_node(&mut self) -> Result<Node> {
        self.inbox
            .lock()
            .map_err(|_| Error::Internal("poisoned".to_owned()))?
            .pop_front()
            .ok_or_else(|| Error::Network("the fake server has nothing more to say".to_owned()))
    }

    async fn close(&mut self) {}
}

/// Opens a [`FakeWire`] for every dial, seeded from the world.
pub struct FakeDialer {
    pub world: Arc<Mutex<World>>,
}

#[async_trait]
impl Dialer for FakeDialer {
    async fn dial(&self, _noise: &KeyPair, _payload: Vec<u8>) -> Result<Box<dyn Wire>> {
        let mut world = self
            .world
            .lock()
            .map_err(|_| Error::Internal("poisoned".to_owned()))?;
        let queue: VecDeque<Node> = world.next_dial_seed.drain(..).collect();
        let inbox = Arc::new(Mutex::new(queue));
        world.inboxes.push(Arc::clone(&inbox));
        Ok(Box::new(FakeWire {
            world: Arc::clone(&self.world),
            inbox,
        }))
    }
}

/// The server's `success` for a login.
pub fn success_node() -> Node {
    Node::new("success").attr("t", "0")
}

/// A `message` node from `from` with one encrypted child.
pub fn message_node(id: &str, from: &str, kind: &str, bytes: Vec<u8>, extra: Vec<Node>) -> Node {
    let mut children = vec![
        Node::new("enc")
            .attr("v", "2")
            .attr("type", kind)
            .bytes(bytes),
    ];
    children.extend(extra);
    Node::new("message")
        .attr("id", id)
        .attr("from", from)
        .attr("t", "1700000000")
        .attr("type", "text")
        .attr("notify", "Phone Friend")
        .children(children)
}

/// The server's wire-level node of a text message: `conversation`.
pub fn text(body: &str) -> Message {
    Message {
        conversation: Some(body.to_owned()),
        ..Default::default()
    }
}

/// Collects the events of a session until `done` returns true, or the session stops.
pub async fn events_until<S: chirper_bridge::Session>(
    session: &mut S,
    mut done: impl FnMut(&chirper_bridge::Event) -> bool,
) -> Vec<chirper_bridge::Event> {
    let mut out = Vec::new();
    for _ in 0..64 {
        let Some(event) = session.next().await else {
            break;
        };
        let finished = done(&event);
        out.push(event);
        if finished {
            break;
        }
    }
    out
}

/// Brings a session up on a fresh world and returns it with the world handle.
pub async fn running_session(
    world: Arc<Mutex<World>>,
    ctx: LoginCtx,
    keys: DeviceKeys,
    media_base: Option<String>,
) -> chirper_net_whatsapp::session::Client<FakeWire> {
    let inbox = Arc::new(Mutex::new(VecDeque::from(vec![success_node()])));
    world
        .lock()
        .expect("world")
        .inboxes
        .push(Arc::clone(&inbox));
    let wire = FakeWire {
        world: Arc::clone(&world),
        inbox,
    };
    chirper_net_whatsapp::session::Client::start(wire, ctx, keys, media_base)
        .await
        .expect("start")
}

/// Queues a node for the newest wire the driver opened.
pub fn push_inbound(world: &Arc<Mutex<World>>, node: Node) {
    let world = world.lock().expect("world");
    let inbox = world.inboxes.last().expect("a wire is open");
    inbox.lock().expect("inbox").push_back(node);
}
