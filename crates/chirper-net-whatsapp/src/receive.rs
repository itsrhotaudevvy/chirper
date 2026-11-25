//! Incoming traffic: `message`, `receipt`, `chatstate`, `notification` and `call` nodes.
//!
//! A `message` node carries one or more `enc` children, each encrypted for this device. Each is
//! decrypted with the protocol crate (pairwise for `msg`/`pkmsg`, sender key for `skmsg`), unpadded,
//! and decoded as `waE2E.Message`. A message that cannot be decrypted gets a retry receipt, up to
//! five times, and the server is acknowledged either way so it does not redeliver forever.

use std::sync::Arc;

use chirper_bridge::model::{
    ChatMeta, Content, DeliveryStatus, MediaInfo, Member, Reaction, RemoteEventRef, RemoteMessage,
    RemoteUser,
};
use chirper_bridge::{Error, Event, Result};
use chirper_signal_protocol::message::{
    PreKeySignalMessage, SenderKeyDistributionMessage as SenderKeyDistribution, SenderKeyMessage,
    SignalMessage,
};
use chirper_signal_protocol::prekey::PreKeyRecord;

use crate::binary::{Jid, Node, Server};
use crate::convert::{self, Body, MediaSpec, ReplyTo};
use crate::crypto;
use crate::ids;
use crate::proto::e2e::{Message, ProtocolMessage, ReactionMessage, SenderKeyDistributionMessage};
use crate::session::Client;
use crate::store::{address, sender_key_name};
use crate::transport::Wire;

/// The most retry receipts sent for one message.
pub const MAX_RETRIES: u32 = 5;

/// `ack` error codes the server understands.
const NACK_PARSING: u32 = 487;
const NACK_UNRECOGNIZED: u32 = 493;
const NACK_INVALID_PROTOBUF: u32 = 491;

/// Who sent a message and where it was sent.
#[derive(Clone, Debug)]
pub struct MessageInfo {
    pub id: String,
    pub chat: Jid,
    /// The sender's device JID: `from` of a direct message, `participant` of a group one.
    pub sender: Jid,
    pub is_group: bool,
    pub is_from_me: bool,
    pub timestamp_s: i64,
    pub push_name: String,
}

impl MessageInfo {
    /// Reads the addressing of a `message` node. `own` is this device's JID.
    pub fn parse(node: &Node, own: &Jid) -> Result<Self> {
        let id = node
            .get_attr("id")
            .filter(|id| !id.is_empty())
            .ok_or_else(|| Error::Protocol("a message has no id".to_owned()))?;
        let from = node
            .jid_or_text("from")
            .ok_or_else(|| Error::Protocol("a message has no sender".to_owned()))?;
        let timestamp_s = node
            .get_attr("t")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0);
        let push_name = node.get_attr("notify").unwrap_or_default();
        let (chat, sender, is_group, is_from_me) = match from.server {
            Server::Group | Server::Broadcast => {
                let participant = node.jid_or_text("participant").ok_or_else(|| {
                    Error::Protocol("a group message has no participant".to_owned())
                })?;
                let mine = participant.user == own.user;
                (from.clone(), participant, true, mine)
            }
            _ => {
                if from.user == own.user {
                    let recipient = node
                        .jid_or_text("recipient")
                        .unwrap_or_else(|| from.clone());
                    (recipient.clone(), from.clone(), false, true)
                } else {
                    (from.clone(), from.clone(), false, false)
                }
            }
        };
        Ok(Self {
            id,
            chat: Jid {
                device: 0,
                agent: 0,
                ..chat
            },
            sender,
            is_group,
            is_from_me,
            timestamp_s,
            push_name,
        })
    }

    /// The sender without its device: the user the message is attributed to.
    pub fn sender_user(&self) -> Jid {
        Jid {
            device: 0,
            agent: 0,
            ..self.sender.clone()
        }
    }
}

impl<W: Wire> Client<W> {
    /// A `message` node: decrypt, process, and acknowledge.
    pub(crate) async fn on_message(&mut self, node: &Node) -> Result<()> {
        let info = match MessageInfo::parse(node, &self.own) {
            Ok(info) => info,
            Err(error) => {
                log::debug!("a message could not be parsed: {error}");
                let ack = Self::ack_for(node, Some(NACK_PARSING));
                return self.send(&ack).await;
            }
        };
        let mut recognized = false;
        let mut protobuf_failed = false;
        for child in node.child_nodes().iter().filter(|child| child.tag == "enc") {
            recognized = true;
            match self.decrypt_child(child, &info).await {
                Ok(Some(plaintext)) => match prost::Message::decode(plaintext.as_slice()) {
                    Ok(message) => self.process_message(&info, message).await?,
                    Err(_) => protobuf_failed = true,
                },
                Ok(None) => {}
                Err(error) => {
                    log::debug!("a message could not be decrypted: {error}");
                    self.send_retry_receipt(node, &info).await?;
                    return self.send(&Self::ack_for(node, None)).await;
                }
            }
        }
        if !recognized {
            return self
                .send(&Self::ack_for(node, Some(NACK_UNRECOGNIZED)))
                .await;
        }
        if protobuf_failed {
            return self
                .send(&Self::ack_for(node, Some(NACK_INVALID_PROTOBUF)))
                .await;
        }
        self.send_message_receipt(node, &info).await
    }

    /// Decrypts one `enc` child. `Ok(None)` for a message already processed.
    async fn decrypt_child(&mut self, child: &Node, info: &MessageInfo) -> Result<Option<Vec<u8>>> {
        let kind = child.get_attr("type").unwrap_or_default();
        let version = child
            .get_attr("v")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(2);
        let bytes = child
            .content_bytes()
            .ok_or_else(|| Error::Protocol("an encrypted message has no content".to_owned()))?;
        let plaintext = match kind.as_str() {
            "pkmsg" | "msg" => {
                let addr = address(&info.sender)?;
                let result = if kind == "pkmsg" {
                    let message = PreKeySignalMessage::deserialize(bytes)
                        .map_err(|_| Error::Protocol("a prekey message is malformed".to_owned()))?;
                    chirper_signal_protocol::cipher::decrypt_prekey_message(
                        &mut self.signal,
                        &addr,
                        &message,
                        &mut self.rng,
                    )
                } else {
                    let message = SignalMessage::deserialize(bytes)
                        .map_err(|_| Error::Protocol("a message is malformed".to_owned()))?;
                    chirper_signal_protocol::cipher::decrypt_signal_message(
                        &mut self.signal,
                        &addr,
                        &message,
                        &mut self.rng,
                    )
                };
                match result {
                    Ok(plaintext) => plaintext,
                    Err(chirper_signal_protocol::Error::DuplicatedMessage(..)) => return Ok(None),
                    Err(_) => {
                        return Err(Error::Protocol("the message does not decrypt".to_owned()));
                    }
                }
            }
            "skmsg" => {
                let name = sender_key_name(&info.chat, &info.sender)?;
                let message = SenderKeyMessage::deserialize(bytes)
                    .map_err(|_| Error::Protocol("a group message is malformed".to_owned()))?;
                match chirper_signal_protocol::group::group_decrypt(
                    &mut self.signal,
                    &name,
                    &message,
                ) {
                    Ok(plaintext) => plaintext,
                    Err(chirper_signal_protocol::Error::DuplicatedMessage(..)) => return Ok(None),
                    Err(_) => {
                        return Err(Error::Protocol(
                            "the group message does not decrypt".to_owned(),
                        ));
                    }
                }
            }
            other => {
                log::debug!("an encrypted message of unknown type {other} was skipped");
                return Ok(None);
            }
        };
        crypto::unpad_message(&plaintext, version).map(Some)
    }

    /// Handles one decrypted `waE2E.Message`.
    async fn process_message(&mut self, info: &MessageInfo, message: Message) -> Result<()> {
        let (chat, sender, from_me, message) = match message.device_sent_message {
            Some(sent) => {
                let destination = sent
                    .destination_jid
                    .as_deref()
                    .and_then(|jid| jid.parse::<Jid>().ok())
                    .unwrap_or_else(|| info.chat.clone());
                let inner = sent.message.map(|boxed| *boxed).unwrap_or_default();
                (destination, info.sender_user(), true, inner)
            }
            None => (
                info.chat.clone(),
                info.sender_user(),
                info.is_from_me,
                message,
            ),
        };
        let info = MessageInfo {
            chat: chat.clone(),
            sender: sender.clone(),
            is_from_me: from_me,
            ..info.clone()
        };
        if let Some(distribution) = &message.sender_key_distribution_message
            && info.is_group
        {
            self.process_distribution(&info, distribution)?;
        }
        if let Some(protocol) = &message.protocol_message {
            return self.process_protocol(&info, protocol).await;
        }
        if let Some(reaction) = &message.reaction_message {
            return self.process_reaction(&info, reaction);
        }
        self.note_chat(&info);
        let body = convert::classify(&message);
        let Some(content) = self.content_of(body).await? else {
            return Ok(());
        };
        let (content, reply_to) = content;
        let remote = RemoteMessage {
            portal: self.portal(&info.chat),
            id: ids::message_id(&info.chat, &info.sender, &info.id),
            part: String::new(),
            sender: self.remote_user(&info),
            timestamp_ms: info.timestamp_s.saturating_mul(1000),
            content,
            reply_to: reply_to.and_then(|reply| convert::reply_message_id(&info.chat, &reply)),
            edit_of: None,
            live: true,
            from_me: info.is_from_me,
        };
        self.last_seen
            .insert(ids::portal_id(&info.chat), remote.id.clone());
        self.events.push_back(Event::Message(remote));
        Ok(())
    }

    /// The content of a body, with media downloaded. `None` when there is nothing to show.
    async fn content_of(&mut self, body: Body) -> Result<Option<(Content, Option<ReplyTo>)>> {
        Ok(match body {
            Body::Empty => None,
            Body::Text {
                body,
                formatted,
                reply_to,
            } => Some((Content::Text { body, formatted }, reply_to)),
            Body::Location(body) => Some((Content::Location { body }, None)),
            Body::Contact(body) | Body::Unsupported(body) => {
                Some((Content::Unsupported { body }, None))
            }
            Body::Media { spec, reply_to } => {
                let content = match self.download_media(&spec).await {
                    Ok(data) => media_content(&spec, data),
                    Err(error) => {
                        log::debug!("a {} could not be downloaded: {error}", spec.kind);
                        Content::Unsupported {
                            body: format!("{} could not be downloaded", spec.kind),
                        }
                    }
                };
                Some((content, reply_to))
            }
        })
    }

    /// The bridge user of a sender.
    pub(crate) fn remote_user(&self, info: &MessageInfo) -> RemoteUser {
        let user = info.sender_user();
        let user_id = ids::user_id(&user);
        let display_name = if info.push_name.is_empty() {
            self.names.get(&user_id).cloned().unwrap_or_default()
        } else {
            info.push_name.clone()
        };
        RemoteUser {
            id: user_id,
            display_name,
            avatar_url: String::new(),
            username: String::new(),
            phone: user.user,
        }
    }

    /// Announces a chat the first time a message arrives in it.
    fn note_chat(&mut self, info: &MessageInfo) {
        let key = ids::portal_id(&info.chat);
        if !self.seen_chats.insert(key) {
            return;
        }
        let name = if info.is_group {
            String::new()
        } else {
            self.names
                .get(&ids::user_id(&info.chat))
                .cloned()
                .unwrap_or_else(|| info.push_name.clone())
        };
        self.events.push_back(Event::Chat(ChatMeta {
            portal: self.portal(&info.chat),
            name,
            is_dm: !info.is_group,
            dm_user: (!info.is_group).then(|| RemoteUser {
                id: ids::user_id(&info.chat),
                phone: info.chat.user.clone(),
                ..Default::default()
            }),
            ..Default::default()
        }));
    }

    /// A sender key distribution message: records the group sender's chain.
    fn process_distribution(
        &mut self,
        info: &MessageInfo,
        distribution: &SenderKeyDistributionMessage,
    ) -> Result<()> {
        let bytes = distribution
            .axolotl_sender_key_distribution_message
            .as_deref()
            .ok_or_else(|| Error::Protocol("a sender key distribution has no key".to_owned()))?;
        let message = SenderKeyDistribution::deserialize(bytes)
            .map_err(|_| Error::Protocol("a sender key distribution is malformed".to_owned()))?;
        let name = sender_key_name(&info.chat, &info.sender)?;
        chirper_signal_protocol::group::process_sender_key_distribution_message(
            &mut self.signal,
            &name,
            &message,
        )
        .map_err(|_| Error::Protocol("a sender key distribution does not apply".to_owned()))
    }

    /// Protocol messages: revokes, edits, history sync, app-state keys.
    async fn process_protocol(
        &mut self,
        info: &MessageInfo,
        protocol: &ProtocolMessage,
    ) -> Result<()> {
        use crate::proto::e2e::protocol_type as t;
        let kind = protocol.r#type.unwrap_or(-1);
        let target_key = protocol.key.clone();
        match kind {
            t::REVOKE => {
                if let Some(target) = self.target_id(info, target_key.as_ref()) {
                    let portal = self.portal(&info.chat);
                    self.events.push_back(Event::Redact {
                        portal,
                        event: RemoteEventRef::new(target),
                    });
                }
            }
            t::MESSAGE_EDIT => {
                let (Some(target), Some(edited)) = (
                    self.target_id(info, target_key.as_ref()),
                    protocol.edited_message.as_deref(),
                ) else {
                    return Ok(());
                };
                if let Some((content, reply_to)) =
                    self.content_of(convert::classify(edited)).await?
                {
                    let remote = RemoteMessage {
                        portal: self.portal(&info.chat),
                        id: String::new(),
                        part: String::new(),
                        sender: self.remote_user(info),
                        timestamp_ms: info.timestamp_s.saturating_mul(1000),
                        content,
                        reply_to: reply_to
                            .and_then(|reply| convert::reply_message_id(&info.chat, &reply)),
                        edit_of: Some(target),
                        live: true,
                        from_me: info.is_from_me,
                    };
                    self.events.push_back(Event::Message(remote));
                }
            }
            t::HISTORY_SYNC_NOTIFICATION => {
                if let Some(notification) = &protocol.history_sync_notification {
                    self.process_history_notification(notification).await;
                }
            }
            t::APP_STATE_SYNC_KEY_SHARE => {
                if let Some(share) = &protocol.app_state_sync_key_share {
                    self.store_app_state_keys(share)?;
                    self.sync_app_state().await;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The remote id of the message a protocol key or reaction key names.
    fn target_id(
        &self,
        info: &MessageInfo,
        key: Option<&crate::proto::MessageKey>,
    ) -> Option<String> {
        let key = key?;
        let id = key.id.clone()?;
        let chat = info.chat.clone();
        let sender = if key.from_me.unwrap_or(false) {
            self.own.clone()
        } else if info.is_group {
            key.participant.as_deref()?.parse().ok()?
        } else {
            chat.clone()
        };
        Some(ids::message_id(
            &chat,
            &Jid {
                device: 0,
                agent: 0,
                ..sender
            },
            &id,
        ))
    }

    /// A reaction: keep the reactor's emoji and report the full set for the message.
    fn process_reaction(&mut self, info: &MessageInfo, reaction: &ReactionMessage) -> Result<()> {
        let target = self
            .target_id(info, reaction.key.as_ref())
            .ok_or_else(|| Error::Protocol("a reaction names no message".to_owned()))?;
        let emoji = reaction.text.clone().unwrap_or_default();
        let reactor = ids::user_id(&info.sender_user());
        let set = self.reactions.set(&target, &reactor, &emoji);
        let reactions = reactions_of(&set, &ids::user_id(&self.own));
        let portal = self.portal(&info.chat);
        self.events.push_back(Event::Reactions {
            portal,
            target: RemoteEventRef::new(target),
            reactions,
        });
        Ok(())
    }

    /// A delivery or read receipt, or a retry request. Receipts from the other side are about our
    /// own messages; `read-self` is our other device reading a chat, so it is about the peer's.
    pub(crate) async fn on_receipt(&mut self, node: &Node) -> Result<()> {
        let ack = Self::ack_for(node, None);
        let kind = node.get_attr("type").unwrap_or_default();
        let Some(from) = node.jid_or_text("from") else {
            return self.send(&ack).await;
        };
        let group = from.server == Server::Group;
        let chat = Jid {
            device: 0,
            agent: 0,
            ..from.clone()
        };
        let reader = if group {
            node.jid_or_text("participant")
                .unwrap_or_else(|| from.clone())
        } else {
            from.clone()
        };
        let reader = Jid {
            device: 0,
            agent: 0,
            ..reader
        };
        let mut ids_list: Vec<String> = node.get_attr("id").into_iter().collect();
        for list in node
            .child_nodes()
            .iter()
            .filter(|child| child.tag == "list")
        {
            ids_list.extend(
                list.child_nodes()
                    .iter()
                    .filter_map(|item| item.get_attr("id")),
            );
        }
        let message_sender = match kind.as_str() {
            "read-self" if group => reader.clone(),
            "read-self" => chat.clone(),
            _ => self.own_user(),
        };
        let reader = if kind == "read-self" {
            self.own_user()
        } else {
            reader
        };
        match kind.as_str() {
            "" | "delivery" => {
                for id in ids_list {
                    let remote = ids::message_id(&chat, &message_sender, &id);
                    self.events.push_back(Event::Delivery {
                        portal: self.portal(&chat),
                        txn_id: remote,
                        status: DeliveryStatus::Delivered,
                        error: String::new(),
                    });
                }
            }
            "read" | "read-self" => {
                for id in ids_list {
                    let remote = ids::message_id(&chat, &message_sender, &id);
                    let user = RemoteUser {
                        id: ids::user_id(&reader),
                        phone: reader.user.clone(),
                        ..Default::default()
                    };
                    self.events.push_back(Event::ReadReceipt {
                        portal: self.portal(&chat),
                        user,
                        event: RemoteEventRef::new(remote),
                    });
                }
            }
            "retry" => log::debug!("a peer asked for a message again; resending is not supported"),
            _ => {}
        }
        self.send(&ack).await
    }

    /// Typing indicators. `composing` starts one; `paused` clears it.
    pub(crate) async fn on_chatstate(&mut self, node: &Node) -> Result<()> {
        let ack = Self::ack_for(node, None);
        if let Some(from) = node.jid_or_text("from") {
            let (chat, sender) = if from.server == Server::Group {
                let participant = node
                    .jid_or_text("participant")
                    .unwrap_or_else(|| from.clone());
                (from.clone(), participant)
            } else {
                (from.clone(), from.clone())
            };
            let typing = node.child("composing").is_some();
            let senders = if typing {
                vec![RemoteUser {
                    id: ids::user_id(&sender),
                    phone: sender.user.clone(),
                    ..Default::default()
                }]
            } else {
                Vec::new()
            };
            let portal = self.portal(&chat);
            self.events.push_back(Event::Typing { portal, senders });
        }
        self.send(&ack).await
    }

    /// Notifications: prekey count, device lists, group changes, app-state changes.
    pub(crate) async fn on_notification(&mut self, node: &Node) -> Result<()> {
        if let Some(ack) = crate::login::notification_ack(node) {
            self.send(&ack).await?;
        }
        let kind = node.get_attr("type").unwrap_or_default();
        match kind.as_str() {
            "encrypt" => {
                let count = node
                    .child("count")
                    .and_then(|count| count.get_attr("value"))
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(u32::MAX);
                if count < crate::session::MIN_PRE_KEY_COUNT {
                    self.upload_pre_keys(crate::session::WANTED_PRE_KEY_UPLOAD)
                        .await?;
                }
            }
            "devices" => {
                if let Some(from) = node.jid_or_text("from") {
                    self.devices.remove(&ids::portal_id(&from));
                }
            }
            "w:gp2" => self.on_group_notification(node),
            "server_sync" => {
                for collection in node
                    .child_nodes()
                    .iter()
                    .filter(|child| child.tag == "collection")
                {
                    if let Some(name) = collection.get_attr("name")
                        && let Some(known) =
                            crate::appstate::COLLECTIONS.iter().find(|c| **c == name)
                    {
                        self.sync_collection(known).await?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn on_group_notification(&mut self, node: &Node) {
        let Some(group) = node.jid_or_text("from") else {
            return;
        };
        let portal = self.portal(&group);
        for child in node.child_nodes() {
            match child.tag.as_str() {
                "subject" => {
                    let name = child.get_attr("subject").unwrap_or_default();
                    self.events.push_back(Event::Chat(ChatMeta {
                        portal: portal.clone(),
                        name,
                        ..Default::default()
                    }));
                }
                "add" | "remove" | "leave" => {
                    let membership = match child.tag.as_str() {
                        "add" => "join",
                        _ => "leave",
                    };
                    for participant in child
                        .child_nodes()
                        .iter()
                        .filter(|p| p.tag == "participant")
                    {
                        let Some(jid) = participant.jid_or_text("jid") else {
                            continue;
                        };
                        // Keep the cached member list the next group send reads in step.
                        let members = self.members.entry(ids::portal_id(&group)).or_default();
                        if membership == "join" {
                            if !members.iter().any(|m| m.user == jid.user) {
                                members.push(Jid {
                                    device: 0,
                                    agent: 0,
                                    ..jid.clone()
                                });
                            }
                        } else {
                            members.retain(|m| m.user != jid.user);
                        }
                        self.events.push_back(Event::Member {
                            portal: portal.clone(),
                            member: Member {
                                user: RemoteUser {
                                    id: ids::user_id(&jid),
                                    phone: jid.user.clone(),
                                    ..Default::default()
                                },
                                membership: membership.to_owned(),
                            },
                        });
                    }
                }
                _ => {}
            }
        }
    }

    /// A call: the only trace we keep is a missed call, shown as a notice in the chat.
    pub(crate) async fn on_call(&mut self, node: &Node) -> Result<()> {
        let ack = Self::ack_for(node, None);
        let terminate = node
            .child_nodes()
            .iter()
            .find(|child| child.tag == "terminate");
        if let (Some(terminate), Some(from)) = (terminate, node.jid_or_text("from"))
            && matches!(
                terminate.get_attr("reason").as_deref(),
                Some("timeout") | Some("unavailable")
            )
        {
            let chat = Jid {
                device: 0,
                agent: 0,
                ..from.clone()
            };
            let info = MessageInfo {
                id: node.get_attr("id").unwrap_or_default(),
                chat: chat.clone(),
                sender: from,
                is_group: false,
                is_from_me: false,
                timestamp_s: 0,
                push_name: String::new(),
            };
            let remote = RemoteMessage {
                portal: self.portal(&chat),
                id: ids::message_id(&chat, &info.sender, &info.id),
                part: String::new(),
                sender: self.remote_user(&info),
                timestamp_ms: 0,
                content: Content::Notice {
                    body: "Missed call".to_owned(),
                },
                reply_to: None,
                edit_of: None,
                live: true,
                from_me: false,
            };
            self.events.push_back(Event::Message(remote));
        }
        self.send(&ack).await
    }

    /// The receipt that tells the server a message arrived (`inactive`: we never went active).
    async fn send_message_receipt(&mut self, node: &Node, info: &MessageInfo) -> Result<()> {
        let mut receipt = Node::new("receipt")
            .attr("id", info.id.clone())
            .attr("to", node.get_attr("from").unwrap_or_default());
        if let Some(participant) = node.get_attr("participant") {
            receipt = receipt.attr("participant", participant);
        }
        if let Some(recipient) = node.get_attr("recipient") {
            receipt = receipt.attr("recipient", recipient);
        }
        if info.is_from_me {
            receipt = receipt.attr("type", "sender");
        } else {
            receipt = receipt.attr("type", "inactive");
        }
        self.send(&receipt).await
    }

    /// A retry receipt: the sender should encrypt again. From the second try on, the receipt
    /// carries a prekey bundle so the sender can start a fresh session.
    async fn send_retry_receipt(&mut self, node: &Node, info: &MessageInfo) -> Result<()> {
        let count = self.retries.entry(info.id.clone()).or_insert(0);
        *count += 1;
        let count = *count;
        if self.retries.len() > 4096 {
            self.retries.clear();
        }
        if count >= MAX_RETRIES {
            return Ok(());
        }
        let retry = Node::new("retry")
            .attr("count", count.to_string())
            .attr("id", info.id.clone())
            .attr("t", info.timestamp_s.to_string())
            .attr("v", "1");
        let registration = self.signal.registration_id().to_be_bytes().to_vec();
        let mut children = vec![retry, Node::new("registration").bytes(registration)];
        if count > 1 {
            children.push(self.key_bundle_node()?);
        }
        let mut receipt = Node::new("receipt")
            .attr("id", info.id.clone())
            .attr("to", node.get_attr("from").unwrap_or_default())
            .attr("type", "retry")
            .children(children);
        if let Some(participant) = node.get_attr("participant") {
            receipt = receipt.attr("participant", participant);
        }
        self.send(&receipt).await
    }

    /// The `keys` node of a retry receipt: a fresh one-time prekey, the signed prekey and our
    /// account identity.
    fn key_bundle_node(&mut self) -> Result<Node> {
        use chirper_signal_protocol::store::PreKeyStore as _;
        let id: u32 = self
            .ctx
            .get_secret("prekey-next")?
            .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok())
            .and_then(|text| text.parse().ok())
            .unwrap_or(1)
            .max(1);
        let record = PreKeyRecord::new(
            id,
            chirper_signal_protocol::curve::KeyPair::generate(&mut self.rng),
        );
        self.signal
            .store_pre_key(&record)
            .map_err(|_| Error::Internal("a prekey could not be stored".to_owned()))?;
        self.ctx
            .set_secret("prekey-next", id.wrapping_add(1).to_string().as_bytes())?;
        let account =
            self.keys.account_identity.clone().ok_or_else(|| {
                Error::Auth("the device record has no account identity".to_owned())
            })?;
        Ok(Node::new("keys").children(vec![
            Node::new("type").bytes(vec![0x05]),
            Node::new("identity").bytes(self.signal.identity_public().to_vec()),
            crate::session::pre_key_node(&record),
            crate::session::signed_pre_key_node(self.signal.signed_pre_key_record()),
            Node::new("device-identity").bytes(account),
        ]))
    }
}

/// The bridge's reaction summary from one reactor per emoji.
pub fn reactions_of(set: &[(String, String)], own_user: &str) -> Vec<Reaction> {
    let mut out: Vec<Reaction> = Vec::new();
    for (reactor, emoji) in set {
        let from_me = reactor == own_user;
        let sender = RemoteUser {
            id: reactor.clone(),
            ..Default::default()
        };
        match out.iter_mut().find(|r| &r.key == emoji) {
            Some(existing) => {
                existing.count += 1;
                existing.from_me |= from_me;
                existing.senders.push(sender);
            }
            None => out.push(Reaction {
                key: emoji.clone(),
                count: 1,
                from_me,
                senders: vec![sender],
            }),
        }
    }
    out
}

/// The content of downloaded media.
pub fn media_content(spec: &MediaSpec, data: Vec<u8>) -> Content {
    let thumbnail = spec.thumbnail.clone().map(Arc::new);
    Content::Media {
        kind: spec.kind.to_owned(),
        caption: spec.caption.clone(),
        info: MediaInfo {
            mime: spec.mime.clone(),
            file_name: spec.file_name.clone(),
            size: data.len(),
            width: spec.width,
            height: spec.height,
            duration_ms: spec.duration_ms,
            blurhash: String::new(),
        },
        data: Arc::new(data),
        thumbnail,
    }
}
