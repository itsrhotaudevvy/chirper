//! Outgoing messages: device discovery, prekey bundles, per-device encryption and the fan-out.
//!
//! A direct message is encrypted for every device of the recipient and of our own account. Our own
//! other devices get a `DeviceSentMessage` wrapper, so they show the message as sent from here. A
//! group message is encrypted once with our sender key, and the sender key itself (a
//! `SenderKeyDistributionMessage`) is encrypted pairwise for every device of every member, as
//! whatsmeow's `sendGroup` does. The `phash` is WhatsApp's hash of the device list, sorted and
//! base64-encoded.

use base64::Engine as _;
use chirper_bridge::{Error, Result};
use chirper_signal_protocol::cipher::encrypt as signal_encrypt;
use chirper_signal_protocol::curve::PublicKey;
use chirper_signal_protocol::identity::IdentityKey;
use chirper_signal_protocol::message::CiphertextMessage;
use chirper_signal_protocol::prekey::PreKeyBundle;
use chirper_signal_protocol::store::SessionStore;
use prost::Message as _;
use sha2::{Digest, Sha256};

use crate::binary::{Jid, Node, Server};
use crate::crypto::pad_message;
use crate::ids::{self, ParsedMessageId};
use crate::proto::e2e::{DeviceSentMessage, Message, SenderKeyDistributionMessage};
use crate::session::{Client, SERVER_JID};
use crate::store::{address, sender_key_name};
use crate::transport::Wire;

impl<W: Wire> Client<W> {
    /// Encrypts and sends `message` to `chat` under `id`, and waits for the server's ack.
    pub(crate) async fn send_message(
        &mut self,
        chat: &Jid,
        id: &str,
        message: &Message,
        edit: &str,
    ) -> Result<()> {
        let node = if chat.server == Server::Group {
            self.group_message(chat, id, message, edit).await?
        } else {
            self.direct_message(chat, id, message, edit).await?
        };
        self.send(&node).await?;
        self.await_message_ack(id).await
    }

    async fn direct_message(
        &mut self,
        chat: &Jid,
        id: &str,
        message: &Message,
        edit: &str,
    ) -> Result<Node> {
        let plain = pad_message(&message.encode_to_vec(), &mut self.rng);
        let sent = Message {
            device_sent_message: Some(Box::new(DeviceSentMessage {
                destination_jid: Some(portal_jid(chat).to_string()),
                message: Some(Box::new(message.clone())),
                phash: None,
            })),
            ..Default::default()
        };
        let sent_plain = pad_message(&sent.encode_to_vec(), &mut self.rng);
        let users = [user_of(chat), user_of(&self.own)];
        let devices = self.user_devices(&users).await?;
        let own = self.own.clone();
        let (participants, identity) = self
            .encrypt_for(
                &devices,
                |device| {
                    if device.user == own.user {
                        sent_plain.clone()
                    } else {
                        plain.clone()
                    }
                },
                media_label(message),
                edit,
            )
            .await?;
        let mut children = vec![Node::new("participants").children(participants)];
        if identity {
            children.push(self.device_identity()?);
        }
        Ok(message_node(chat, id, message, edit, children, None))
    }

    async fn group_message(
        &mut self,
        chat: &Jid,
        id: &str,
        message: &Message,
        edit: &str,
    ) -> Result<Node> {
        let members = self.group_members(chat).await?;
        let mut users = members;
        users.push(user_of(&self.own));
        let devices = self.user_devices(&users).await?;
        let name = sender_key_name(chat, &self.own)?;
        let distribution = chirper_signal_protocol::group::create_sender_key_distribution_message(
            &mut self.signal,
            &name,
            &mut self.rng,
        )
        .map_err(|_| Error::Protocol("the sender key could not be created".to_owned()))?;
        let distribution_message = Message {
            sender_key_distribution_message: Some(SenderKeyDistributionMessage {
                group_id: Some(portal_jid(chat).to_string()),
                axolotl_sender_key_distribution_message: Some(distribution.serialized().to_vec()),
            }),
            ..Default::default()
        };
        let distribution_plain = pad_message(&distribution_message.encode_to_vec(), &mut self.rng);
        let (participants, identity) = self
            .encrypt_for(&devices, |_| distribution_plain.clone(), None, edit)
            .await?;

        let plain = pad_message(&message.encode_to_vec(), &mut self.rng);
        let cipher = chirper_signal_protocol::group::group_encrypt(
            &mut self.signal,
            &name,
            &plain,
            &mut self.rng,
        )
        .map_err(|_| Error::Protocol("the group message could not be encrypted".to_owned()))?;
        let mut enc = Node::new("enc")
            .attr("v", "2")
            .attr("type", "skmsg")
            .bytes(cipher.serialized().to_vec());
        if let Some(label) = media_label(message) {
            enc = enc.attr("mediatype", label);
        }
        let mut children = vec![Node::new("participants").children(participants)];
        if identity {
            children.push(self.device_identity()?);
        }
        children.push(enc);
        let phash = participant_hash(&devices);
        Ok(message_node(chat, id, message, edit, children, Some(phash)))
    }

    /// Encrypts `plaintext(device)` for every device except this one, establishing sessions from
    /// prekey bundles where there is none. Returns the `to` nodes and whether any is a prekey
    /// message (which needs our device identity).
    pub(crate) async fn encrypt_for(
        &mut self,
        devices: &[Jid],
        plaintext: impl Fn(&Jid) -> Vec<u8>,
        media: Option<&'static str>,
        edit: &str,
    ) -> Result<(Vec<Node>, bool)> {
        let own = self.own.clone();
        let targets: Vec<Jid> = devices.iter().filter(|d| **d != own).cloned().collect();
        let missing = self.missing_sessions(&targets)?;
        if !missing.is_empty() {
            self.establish_sessions(&missing).await?;
        }
        let mut nodes = Vec::with_capacity(targets.len());
        let mut identity = false;
        for device in &targets {
            let addr = address(device)?;
            if !self.has_session(&addr)? {
                continue;
            }
            let bytes = plaintext(device);
            let message = signal_encrypt(&mut self.signal, &addr, &bytes)
                .map_err(|_| Error::Protocol("a message could not be encrypted".to_owned()))?;
            let kind = match message {
                CiphertextMessage::PreKey(_) => {
                    identity = true;
                    "pkmsg"
                }
                CiphertextMessage::Signal(_) => "msg",
            };
            let mut enc = Node::new("enc")
                .attr("v", "2")
                .attr("type", kind)
                .bytes(message.serialized().to_vec());
            if let Some(label) = media {
                enc = enc.attr("mediatype", label);
            }
            if !edit.is_empty() {
                enc = enc.attr("decrypt-fail", "hide");
            }
            nodes.push(
                Node::new("to")
                    .attr("jid", device.to_string())
                    .children(vec![enc]),
            );
        }
        Ok((nodes, identity))
    }

    /// Whether a usable session with `addr` exists.
    fn has_session(&self, addr: &chirper_signal_protocol::ProtocolAddress) -> Result<bool> {
        let record = self
            .signal
            .load_session(addr)
            .map_err(|_| Error::Internal("the session store failed".to_owned()))?;
        Ok(record.is_some_and(|record| record.state().is_some()))
    }

    /// The devices that have no session yet.
    fn missing_sessions(&self, devices: &[Jid]) -> Result<Vec<Jid>> {
        let mut missing = Vec::new();
        for device in devices {
            if !self.has_session(&address(device)?)? {
                missing.push(device.clone());
            }
        }
        Ok(missing)
    }

    /// Fetches prekey bundles for `devices` and installs a session for each one that has a bundle.
    async fn establish_sessions(&mut self, devices: &[Jid]) -> Result<()> {
        let users: Vec<Node> = devices
            .iter()
            .map(|device| {
                Node::new("user")
                    .attr("jid", device.to_string())
                    .attr("reason", "identity")
            })
            .collect();
        let id = self.next_id();
        let request = Node::new("iq")
            .attr("to", SERVER_JID)
            .attr("type", "get")
            .attr("xmlns", "encrypt")
            .attr("id", id)
            .children(vec![Node::new("key").children(users)]);
        let reply = self.request(request).await?;
        let list = reply
            .child("list")
            .ok_or_else(|| Error::Protocol("prekeys: no list in the answer".to_owned()))?;
        for user in list.child_nodes().iter().filter(|node| node.tag == "user") {
            let Some(jid) = user.jid_or_text("jid") else {
                continue;
            };
            match bundle_of(user, &jid) {
                Ok(bundle) => {
                    let addr = address(&jid)?;
                    chirper_signal_protocol::session_builder::process_prekey_bundle(
                        &mut self.signal,
                        &addr,
                        &bundle,
                        &mut self.rng,
                    )
                    .map_err(|_| Error::Protocol("a prekey bundle does not verify".to_owned()))?;
                }
                Err(error) => log::debug!("a device had no usable prekey bundle: {error}"),
            }
        }
        Ok(())
    }

    /// The device JIDs of `users`, from the cache or a usync query. Hosted devices are dropped.
    pub(crate) async fn user_devices(&mut self, users: &[Jid]) -> Result<Vec<Jid>> {
        let mut unknown = Vec::new();
        let mut out = Vec::new();
        for user in users {
            let key = ids::portal_id(user);
            match self.devices.get(&key) {
                Some(cached) => out.extend(cached.iter().cloned()),
                None => unknown.push(user.clone()),
            }
        }
        if !unknown.is_empty() {
            let id = self.next_id();
            let user_nodes: Vec<Node> = unknown
                .iter()
                .map(|user| Node::new("user").attr("jid", ids::portal_id(user)))
                .collect();
            let request = Node::new("iq")
                .attr("to", SERVER_JID)
                .attr("type", "get")
                .attr("xmlns", "usync")
                .attr("id", id)
                .children(vec![
                    Node::new("usync")
                        .attr("sid", self.next_id())
                        .attr("mode", "query")
                        .attr("last", "true")
                        .attr("index", "0")
                        .attr("context", "message")
                        .children(vec![
                            Node::new("query")
                                .children(vec![Node::new("devices").attr("version", "2")]),
                            Node::new("list").children(user_nodes),
                        ]),
                ]);
            let reply = self.request(request).await?;
            let list = reply
                .child("usync")
                .and_then(|usync| usync.child("list"))
                .ok_or_else(|| Error::Protocol("devices: no list in the answer".to_owned()))?;
            for user in list.child_nodes().iter().filter(|node| node.tag == "user") {
                let Some(jid) = user.jid_or_text("jid") else {
                    continue;
                };
                let devices = parse_device_list(&jid, user);
                self.devices.insert(ids::portal_id(&jid), devices.clone());
                out.extend(devices);
            }
        }
        Ok(out)
    }

    /// The members of a group, from the cache or the group query.
    pub(crate) async fn group_members(&mut self, chat: &Jid) -> Result<Vec<Jid>> {
        let key = ids::portal_id(chat);
        if let Some(cached) = self.members.get(&key) {
            return Ok(cached.clone());
        }
        let id = self.next_id();
        let request = Node::new("iq")
            .attr("to", portal_jid(chat).to_string())
            .attr("type", "get")
            .attr("xmlns", "w:g2")
            .attr("id", id)
            .children(vec![Node::new("query").attr("request", "interactive")]);
        let reply = self.request(request).await?;
        let members: Vec<Jid> = reply
            .child("group")
            .map(|group| {
                group
                    .child_nodes()
                    .iter()
                    .filter(|node| node.tag == "participant")
                    .filter_map(|node| node.jid_or_text("jid"))
                    .collect()
            })
            .unwrap_or_default();
        self.members.insert(key, members.clone());
        Ok(members)
    }

    /// Our device identity, sent with prekey messages so the recipient can verify the account.
    pub(crate) fn device_identity(&self) -> Result<Node> {
        let account =
            self.keys.account_identity.clone().ok_or_else(|| {
                Error::Auth("the device record has no account identity".to_owned())
            })?;
        Ok(Node::new("device-identity").bytes(account))
    }

    /// Tells the server we read the message `target` in its chat.
    pub(crate) async fn send_read(&mut self, chat: &Jid, target: &ParsedMessageId) -> Result<()> {
        let mut receipt = Node::new("receipt")
            .attr("to", portal_jid(chat).to_string())
            .attr("type", "read")
            .attr("id", target.id.clone())
            .attr("t", (crate::convert::now_ms() / 1000).to_string());
        if chat.server == Server::Group {
            receipt = receipt.attr("participant", portal_jid(&target.sender).to_string());
        }
        self.send(&receipt).await
    }
}

/// The device list hash: `2:` and the first six bytes of the SHA-256 of the sorted device list.
pub fn participant_hash(devices: &[Jid]) -> String {
    let mut names: Vec<String> = devices.iter().map(ad_string).collect();
    names.sort();
    let digest = Sha256::digest(names.concat().as_bytes());
    format!(
        "2:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(&digest[..6])
    )
}

/// A device in the `user.agent:device@server` form the hash uses.
fn ad_string(device: &Jid) -> String {
    format!(
        "{}{}{}@{}",
        device.user,
        if device.agent > 0 {
            format!(".{}", device.agent)
        } else {
            String::new()
        },
        if device.device > 0 {
            format!(":{}", device.device)
        } else {
            String::new()
        },
        device.server.as_str()
    )
}

/// The user of a JID, without its device.
fn user_of(jid: &Jid) -> Jid {
    Jid {
        device: 0,
        agent: 0,
        ..jid.clone()
    }
}

/// The chat JID without a device, as the bridge's portal ids are.
fn portal_jid(chat: &Jid) -> Jid {
    user_of(chat)
}

/// The `mediatype` of a message, for the encrypted nodes.
fn media_label(message: &Message) -> Option<&'static str> {
    if message.image_message.is_some() {
        Some("image")
    } else if let Some(video) = &message.video_message {
        Some(if video.gif_playback.unwrap_or(false) {
            "gif"
        } else {
            "video"
        })
    } else if let Some(audio) = &message.audio_message {
        Some(if audio.ptt.unwrap_or(false) {
            "ptt"
        } else {
            "audio"
        })
    } else if message.document_message.is_some() {
        Some("document")
    } else if message.sticker_message.is_some() {
        Some("sticker")
    } else {
        None
    }
}

/// The `type` attribute of a message node.
fn message_label(message: &Message) -> &'static str {
    if message.reaction_message.is_some() {
        "reaction"
    } else if media_label(message).is_some() {
        "media"
    } else {
        "text"
    }
}

/// The `message` node around the children of a send.
fn message_node(
    chat: &Jid,
    id: &str,
    message: &Message,
    edit: &str,
    children: Vec<Node>,
    phash: Option<String>,
) -> Node {
    let mut node = Node::new("message")
        .attr("id", id)
        .attr("type", message_label(message))
        .attr("to", portal_jid(chat).to_string());
    if !edit.is_empty() {
        node = node.attr("edit", edit);
    }
    if let Some(phash) = phash {
        node = node.attr("phash", phash);
    }
    node.children(children)
}

/// The prekey bundle of one `user` node of a prekey answer.
fn bundle_of(user: &Node, jid: &Jid) -> Result<PreKeyBundle> {
    let bad = |what: &str| Error::Protocol(format!("prekeys: {what}"));
    let registration = user
        .child("registration")
        .and_then(Node::content_bytes)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .ok_or_else(|| bad("no registration id"))?;
    let identity_bytes = user
        .child("identity")
        .and_then(Node::content_bytes)
        .ok_or_else(|| bad("no identity key"))?;
    let identity = IdentityKey::new(
        PublicKey::from_djb_public_key_bytes(identity_bytes)
            .map_err(|_| bad("a bad identity key"))?,
    );
    let skey = user.child("skey").ok_or_else(|| bad("no signed prekey"))?;
    let (signed_id, signed_public) = key_node(skey)?;
    let signature: [u8; 64] = skey
        .child("signature")
        .and_then(Node::content_bytes)
        .and_then(|bytes| <[u8; 64]>::try_from(bytes).ok())
        .ok_or_else(|| bad("a bad signed prekey signature"))?;
    let pre_key = match user.child("key") {
        Some(key) => {
            let (id, public) = key_node(key)?;
            Some((id, public))
        }
        None => None,
    };
    Ok(PreKeyBundle {
        registration_id: u32::from_be_bytes(registration),
        device_id: u32::from(jid.device),
        pre_key,
        signed_pre_key_id: signed_id,
        signed_pre_key_public: signed_public,
        signed_pre_key_signature: signature,
        identity_key: identity,
        kyber_pre_key: None,
    })
}

/// The id (3 bytes) and the public key of a `key` or `skey` node.
fn key_node(node: &Node) -> Result<(u32, PublicKey)> {
    let bad = || Error::Protocol("prekeys: a key node is malformed".to_owned());
    let id = node
        .child("id")
        .and_then(Node::content_bytes)
        .and_then(|bytes| <[u8; 3]>::try_from(bytes).ok())
        .ok_or_else(bad)?;
    let value = node
        .child("value")
        .and_then(Node::content_bytes)
        .ok_or_else(bad)?;
    let public = PublicKey::from_djb_public_key_bytes(value).map_err(|_| bad())?;
    Ok((u32::from_be_bytes([0, id[0], id[1], id[2]]), public))
}

/// The devices of one `user` node of a usync answer, excluding hosted ones.
fn parse_device_list(user: &Jid, node: &Node) -> Vec<Jid> {
    let Some(list) = node
        .child("devices")
        .and_then(|devices| devices.child("device-list"))
    else {
        return vec![user_of(user)];
    };
    list.child_nodes()
        .iter()
        .filter(|device| device.tag == "device")
        .filter(|device| device.get_attr("is_hosted").as_deref() != Some("true"))
        .filter_map(|device| device.get_attr("id").and_then(|id| id.parse::<u16>().ok()))
        .map(|device| Jid {
            device,
            ..user_of(user)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jid(text: &str) -> Jid {
        text.parse().expect("test JID")
    }

    #[test]
    fn participant_hash_is_order_independent_and_pinned() {
        let a = vec![jid("555:0@s.whatsapp.net"), jid("555:2@s.whatsapp.net")];
        let b = vec![jid("555:2@s.whatsapp.net"), jid("555:0@s.whatsapp.net")];
        assert_eq!(participant_hash(&a), participant_hash(&b));
        let hash = participant_hash(&a);
        assert!(hash.starts_with("2:"));
        assert_eq!(hash.len(), 2 + 8);
        // The hash of the sorted names, pinned so a change to the encoding is caught.
        // Sorted: "555:2@" sorts before "555@", and device 0 has no device suffix.
        let names = "555:2@s.whatsapp.net555@s.whatsapp.net";
        let digest = Sha256::digest(names.as_bytes());
        let expected = format!(
            "2:{}",
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(&digest[..6])
        );
        assert_eq!(hash, expected);
    }

    #[test]
    fn device_lists_skip_hosted_devices() {
        let user = jid("555@s.whatsapp.net");
        let node = Node::new("user").children(vec![Node::new("devices").children(vec![
            Node::new("device-list").children(vec![
                Node::new("device").attr("id", "0"),
                Node::new("device").attr("id", "3"),
                Node::new("device").attr("id", "99").attr("is_hosted", "true"),
            ]),
        ])]);
        let devices = parse_device_list(&user, &node);
        let ids: Vec<u16> = devices.iter().map(|d| d.device).collect();
        assert_eq!(ids, vec![0, 3]);
    }

    #[test]
    fn media_and_type_labels_follow_the_content() {
        let voice = Message {
            audio_message: Some(crate::proto::e2e::AudioMessage {
                ptt: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(media_label(&voice), Some("ptt"));
        assert_eq!(message_label(&voice), "media");
        let text = Message {
            conversation: Some("hi".to_owned()),
            ..Default::default()
        };
        assert_eq!(message_label(&text), "text");
        assert_eq!(media_label(&text), None);
    }

    #[test]
    fn prekey_bundle_nodes_decode_their_three_byte_ids() {
        let key = PublicKey::from_djb_public_key_bytes(&[9u8; 32]).expect("key");
        let node = Node::new("key").children(vec![
            Node::new("id").bytes(vec![0x01, 0x02, 0x03]),
            Node::new("value").bytes(key.public_key_bytes().to_vec()),
        ]);
        let (id, public) = key_node(&node).expect("key node");
        assert_eq!(id, 0x0001_0203);
        assert_eq!(public, key);
        let short = Node::new("key").children(vec![
            Node::new("id").bytes(vec![1, 2]),
            Node::new("value").bytes(vec![9; 32]),
        ]);
        assert!(key_node(&short).is_err());
    }

    #[test]
    fn a_direct_message_to_a_device_list_builds_one_to_node_per_device() {
        // The node shape the fan-out produces: `message` > `participants` > `to` > `enc`.
        let chat = jid("555@s.whatsapp.net");
        let node = message_node(
            &chat,
            "3EB0ABCDEF",
            &Message {
                conversation: Some("hi".to_owned()),
                ..Default::default()
            },
            "",
            vec![Node::new("participants").children(vec![Node::new("to")
                .attr("jid", "555:3@s.whatsapp.net")
                .children(vec![Node::new("enc").attr("v", "2").attr("type", "msg")])])],
            None,
        );
        assert_eq!(node.tag, "message");
        assert_eq!(node.get_attr("to").as_deref(), Some("555@s.whatsapp.net"));
        assert_eq!(node.get_attr("type").as_deref(), Some("text"));
        let participants = node.child("participants").expect("participants");
        assert_eq!(participants.child_nodes().len(), 1);
        assert_eq!(portal_jid(&chat).to_string(), "555@s.whatsapp.net");
    }
}
