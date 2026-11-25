//! End-to-end session tests against the scripted server: start-up, receive, send, receipts,
//! reactions, history, media and app state. Every message is really encrypted and decrypted with
//! the Signal crate on both ends.

mod support;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use chirper_bridge::model::{Content, PortalRef, SessionState};
use chirper_bridge::{Command, Event, Session};
use chirper_net_whatsapp::appstate::{HashState, expand_key, lt_hash, patch_mac, snapshot_mac};
use chirper_net_whatsapp::binary::Node;
use chirper_net_whatsapp::crypto;
use chirper_net_whatsapp::keys::DeviceKeys;
use chirper_net_whatsapp::proto::MessageKey;
use chirper_net_whatsapp::proto::e2e::{
    AppStateSyncKey, AppStateSyncKeyData, AppStateSyncKeyId, AppStateSyncKeyShare, ContextInfo,
    HistorySyncNotification, ImageMessage, Message, ProtocolMessage, ReactionMessage,
};
use chirper_net_whatsapp::proto::history::{
    Conversation, HistorySync, HistorySyncMsg, WebMessageInfo,
};
use chirper_net_whatsapp::proto::sync::{
    ContactAction, KeyId, SyncActionData, SyncActionValue, SyncdBlob, SyncdMutation, SyncdPatch,
    SyncdRecord, SyncdVersion,
};
use chirper_signal_protocol::ProtocolAddress;
use chirper_signal_protocol::group::{
    SenderKeyName, group_decrypt, process_sender_key_distribution_message,
};
use chirper_signal_protocol::message::{SenderKeyDistributionMessage, SenderKeyMessage};
use prost::Message as _;
use support::*;

const KEY_ID: &[u8] = b"KEYID1";
const SYNC_KEY: [u8; 32] = [0x5a; 32];

/// A fresh world, its login context and our device keys.
fn setup() -> (Arc<Mutex<World>>, chirper_bridge::LoginCtx, DeviceKeys) {
    let world = Arc::new(Mutex::new(World::new()));
    let keys = world.lock().expect("world").ours.clone();
    (world, login_ctx(), keys)
}

/// The phone's `message` node carrying `message`. `establish` builds a session from our bundle.
fn phone_node(world: &Arc<Mutex<World>>, id: &str, message: &Message, establish: bool) -> Node {
    let mut w = world.lock().expect("world");
    let bundle = w.our_bundle();
    let ours = World::our_address();
    let bundle_ref = establish.then_some(&bundle);
    let (kind, bytes) = w.phones[0].encrypt_for_us(message, &ours, bundle_ref);
    message_node(id, PHONE, kind, bytes, Vec::new())
}

/// Decrypts a node's pairwise `enc` for the phone, by its `to` address.
fn phone_decrypt(world: &Arc<Mutex<World>>, node: &Node) -> Message {
    let mut w = world.lock().expect("world");
    let participants = node.child("participants").expect("participants");
    let to = participants
        .child_nodes()
        .iter()
        .find(|n| n.get_attr("jid").as_deref() == Some(PHONE))
        .expect("a `to` node for the phone");
    let enc = to.child("enc").expect("enc");
    let kind = enc.get_attr("type").expect("type");
    let bytes = enc.content_bytes().expect("bytes").to_vec();
    w.phones[0].decrypt_from_us(&kind, &bytes, &World::our_address())
}

fn our_sent_message(world: &Arc<Mutex<World>>, to: &str) -> Node {
    let w = world.lock().expect("world");
    w.sent
        .iter()
        .find(|n| n.tag == "message" && n.get_attr("to").as_deref() == Some(to))
        .expect("a message node to the chat")
        .clone()
}

fn text_message(body: &str) -> Message {
    text(body)
}

#[tokio::test]
async fn start_reports_connecting_the_group_then_running() {
    let (world, ctx, keys) = setup();
    let mut session = running_session(Arc::clone(&world), ctx, keys, None).await;
    let events = events_until(&mut session, |e| {
        matches!(
            e,
            Event::State {
                state: SessionState::Running,
                ..
            }
        )
    })
    .await;
    assert!(matches!(
        events.first(),
        Some(Event::State {
            state: SessionState::Connecting,
            ..
        })
    ));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Chat(meta) if meta.name == "Friends" && !meta.is_dm))
    );
    match events.last() {
        Some(Event::State {
            state: SessionState::Running,
            profile: Some(profile),
            ..
        }) => {
            assert_eq!(profile.id, OUR_USER);
        }
        other => panic!("not running: {other:?}"),
    }
    let group = events
        .iter()
        .find_map(|e| match e {
            Event::Chat(meta) if meta.name == "Friends" => Some(meta),
            _ => None,
        })
        .expect("the group");
    assert_eq!(group.avatar_url, "https://pics.example/g12.jpg");
    // A server with no prekeys gets the initial batch of 812.
    assert_eq!(world.lock().expect("world").uploaded.len(), 812);
}

#[tokio::test]
async fn an_incoming_prekey_message_is_decrypted_and_receipted() {
    let (world, ctx, keys) = setup();
    let mut session = running_session(Arc::clone(&world), ctx, keys, None).await;
    events_until(&mut session, |e| {
        matches!(
            e,
            Event::State {
                state: SessionState::Running,
                ..
            }
        )
    })
    .await;

    push_inbound(
        &world,
        phone_node(&world, "ABC1", &text_message("hello"), true),
    );
    let events = events_until(&mut session, |e| matches!(e, Event::Message(_))).await;
    let Some(Event::Message(message)) = events.iter().find(|e| matches!(e, Event::Message(_)))
    else {
        panic!("no message event: {events:?}");
    };
    assert_eq!(message.id, format!("{PHONE}:{PHONE}:ABC1"));
    assert_eq!(message.portal.id, PHONE);
    assert_eq!(message.portal.receiver, OUR_USER);
    assert!(!message.from_me);
    assert!(message.live);
    assert!(matches!(&message.content, Content::Text { body, .. } if body == "hello"));

    let w = world.lock().expect("world");
    assert!(w.sent.iter().any(|n| n.tag == "receipt"
        && n.get_attr("id").as_deref() == Some("ABC1")
        && n.get_attr("type").as_deref() == Some("inactive")));
}

#[tokio::test]
async fn a_direct_message_is_encrypted_for_the_phone_and_acknowledged() {
    let (world, ctx, keys) = setup();
    let mut session = running_session(Arc::clone(&world), ctx, keys, None).await;
    events_until(&mut session, |e| {
        matches!(
            e,
            Event::State {
                state: SessionState::Running,
                ..
            }
        )
    })
    .await;

    let ack = session
        .command(Command::SendText {
            portal: PortalRef {
                id: PHONE.to_owned(),
                receiver: OUR_USER.to_owned(),
            },
            text: "hi back".to_owned(),
            reply_to: None,
            txn_id: "tx1".to_owned(),
        })
        .await
        .expect("send");
    let remote = ack.remote_event_id.expect("remote id");
    assert!(
        remote.starts_with(&format!("{PHONE}:{OUR_USER}@s.whatsapp.net:3EB0")),
        "{remote}"
    );

    let delivery = events_until(&mut session, |e| matches!(e, Event::Delivery { .. })).await;
    assert!(matches!(delivery.last(), Some(Event::Delivery { txn_id, .. }) if txn_id == "tx1"));

    let node = our_sent_message(&world, PHONE);
    assert_eq!(node.get_attr("type").as_deref(), Some("text"));
    // Our own other device (9) has no bundle, so it is skipped rather than failing the send.
    let decrypted = phone_decrypt(&world, &node);
    assert_eq!(decrypted.conversation.as_deref(), Some("hi back"));
}

#[tokio::test]
async fn a_group_message_carries_a_sender_key_the_phone_can_use() {
    let (world, ctx, keys) = setup();
    let mut session = running_session(Arc::clone(&world), ctx, keys, None).await;
    events_until(&mut session, |e| {
        matches!(
            e,
            Event::State {
                state: SessionState::Running,
                ..
            }
        )
    })
    .await;

    session
        .command(Command::SendText {
            portal: PortalRef {
                id: GROUP.to_owned(),
                receiver: String::new(),
            },
            text: "group hi".to_owned(),
            reply_to: None,
            txn_id: "g1".to_owned(),
        })
        .await
        .expect("send");

    let node = our_sent_message(&world, GROUP);
    assert!(node.get_attr("phash").is_some_and(|h| h.starts_with("2:")));
    let distribution = phone_decrypt(&world, &node)
        .sender_key_distribution_message
        .expect("the sender key rides pairwise");
    let our_name = ProtocolAddress::new(OUR_USER, u8::try_from(OUR_DEVICE).expect("device"));
    let name = SenderKeyName::new(GROUP, our_name);
    {
        let mut w = world.lock().expect("world");
        let bytes = distribution
            .axolotl_sender_key_distribution_message
            .expect("bytes");
        let skdm = SenderKeyDistributionMessage::deserialize(&bytes).expect("skdm");
        process_sender_key_distribution_message(&mut w.phones[0].store, &name, &skdm)
            .expect("process");
    }
    let enc = node.child("enc").expect("the group cipher");
    assert_eq!(enc.get_attr("type").as_deref(), Some("skmsg"));
    let message =
        SenderKeyMessage::deserialize(enc.content_bytes().expect("bytes")).expect("skmsg");
    let plain = {
        let mut w = world.lock().expect("world");
        group_decrypt(&mut w.phones[0].store, &name, &message).expect("group decrypt")
    };
    let body = crypto::unpad_message(&plain, 2).expect("unpad");
    let decoded = Message::decode(body.as_slice()).expect("message");
    assert_eq!(decoded.conversation.as_deref(), Some("group hi"));
}

#[tokio::test]
async fn reactions_typing_and_receipts_become_events() {
    let (world, ctx, keys) = setup();
    let mut session = running_session(Arc::clone(&world), ctx, keys, None).await;
    events_until(&mut session, |e| {
        matches!(
            e,
            Event::State {
                state: SessionState::Running,
                ..
            }
        )
    })
    .await;
    push_inbound(
        &world,
        phone_node(&world, "P1", &text_message("first"), true),
    );
    events_until(&mut session, |e| matches!(e, Event::Message(_))).await;

    // The phone reacts to a message of ours.
    let reaction = Message {
        reaction_message: Some(ReactionMessage {
            key: Some(MessageKey {
                remote_jid: Some(PHONE.to_owned()),
                from_me: Some(true),
                id: Some("OURS1".to_owned()),
                participant: None,
            }),
            text: Some("👍".to_owned()),
            sender_timestamp_ms: None,
        }),
        ..Default::default()
    };
    push_inbound(&world, phone_node(&world, "R1", &reaction, false));
    let events = events_until(&mut session, |e| matches!(e, Event::Reactions { .. })).await;
    match events.last() {
        Some(Event::Reactions {
            target, reactions, ..
        }) => {
            assert_eq!(
                target.id,
                format!("{PHONE}:{OUR_USER}@s.whatsapp.net:OURS1")
            );
            assert_eq!(reactions.len(), 1);
            assert_eq!(reactions[0].key, "👍");
            assert_eq!(reactions[0].count, 1);
            assert!(!reactions[0].from_me);
        }
        other => panic!("no reactions: {other:?}"),
    }

    // Typing starts and stops.
    let typing = Node::new("chatstate")
        .attr("from", PHONE)
        .children(vec![Node::new("composing")]);
    push_inbound(&world, typing);
    let events = events_until(&mut session, |e| matches!(e, Event::Typing { .. })).await;
    assert!(matches!(events.last(), Some(Event::Typing { senders, .. }) if senders.len() == 1));

    // A delivery receipt and a read receipt about a message of ours.
    let delivered = Node::new("receipt").attr("from", PHONE).attr("id", "ABC");
    push_inbound(&world, delivered);
    let events = events_until(&mut session, |e| matches!(e, Event::Delivery { .. })).await;
    assert!(matches!(events.last(), Some(Event::Delivery { txn_id, .. })
        if txn_id == &format!("{PHONE}:{OUR_USER}@s.whatsapp.net:ABC")));
    let read = Node::new("receipt")
        .attr("from", PHONE)
        .attr("id", "ABC")
        .attr("type", "read");
    push_inbound(&world, read);
    let events = events_until(&mut session, |e| matches!(e, Event::ReadReceipt { .. })).await;
    assert!(matches!(events.last(), Some(Event::ReadReceipt { user, .. }) if user.id == "555"));
}

#[tokio::test]
async fn history_is_buffered_and_served_by_backfill() {
    let (world, ctx, keys) = setup();
    let mut session = running_session(Arc::clone(&world), ctx, keys, None).await;
    events_until(&mut session, |e| {
        matches!(
            e,
            Event::State {
                state: SessionState::Running,
                ..
            }
        )
    })
    .await;

    let old = |id: &str, body: &str, at: u64| HistorySyncMsg {
        message: Some(WebMessageInfo {
            key: Some(MessageKey {
                remote_jid: Some(PHONE.to_owned()),
                from_me: Some(false),
                id: Some(id.to_owned()),
                participant: None,
            }),
            message: Some(text_message(body)),
            message_timestamp: Some(at),
            ..Default::default()
        }),
        msg_order_id: None,
    };
    let history = HistorySync {
        sync_type: Some(2),
        conversations: vec![Conversation {
            id: Some(PHONE.to_owned()),
            name: Some("Phone Friend".to_owned()),
            messages: vec![old("H2", "old two", 2000), old("H1", "old one", 1000)],
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut encoder, &history.encode_to_vec()).expect("zlib");
    let blob = encoder.finish().expect("zlib");
    let notification = Message {
        protocol_message: Some(ProtocolMessage {
            r#type: Some(5),
            history_sync_notification: Some(HistorySyncNotification {
                initial_hist_bootstrap_inline_payload: Some(blob),
                sync_type: Some(2),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    push_inbound(&world, phone_node(&world, "HS1", &notification, true));
    let events = events_until(
        &mut session,
        |e| matches!(e, Event::Chat(meta) if meta.name == "Phone Friend"),
    )
    .await;
    assert!(matches!(events.last(), Some(Event::Chat(_))));

    let page = session
        .backfill(
            &PortalRef {
                id: PHONE.to_owned(),
                receiver: OUR_USER.to_owned(),
            },
            None,
            50,
        )
        .await
        .expect("backfill");
    let bodies: Vec<&str> = page
        .messages
        .iter()
        .map(|m| match &m.content {
            Content::Text { body, .. } => body.as_str(),
            _ => "",
        })
        .collect();
    assert_eq!(bodies, vec!["old one", "old two"]);
    assert!(page.messages.iter().all(|m| !m.live));
    assert!(!page.has_more);
}

#[tokio::test]
async fn an_image_is_downloaded_and_decrypted() {
    let plain = b"pixels of a small picture".to_vec();
    let media_key = [0x42u8; 32];
    let encrypted =
        crypto::encrypt_media(&plain, &media_key, "WhatsApp Image Keys").expect("encrypt");
    let body = encrypted.data.clone();

    // A local media host: answers every GET with the encrypted file.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    break;
                }
                request.push(byte[0]);
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    });

    let (world, ctx, keys) = setup();
    let mut session = running_session(
        Arc::clone(&world),
        ctx,
        keys,
        Some(format!("http://127.0.0.1:{port}")),
    )
    .await;
    events_until(&mut session, |e| {
        matches!(
            e,
            Event::State {
                state: SessionState::Running,
                ..
            }
        )
    })
    .await;

    let image = Message {
        image_message: Some(ImageMessage {
            mimetype: Some("image/jpeg".to_owned()),
            caption: Some("a picture".to_owned()),
            file_sha256: Some(encrypted.file_sha256.to_vec()),
            file_length: Some(plain.len() as u64),
            media_key: Some(media_key.to_vec()),
            file_enc_sha256: Some(encrypted.file_enc_sha256.to_vec()),
            direct_path: Some("/v/t1/abc?ccb=1".to_owned()),
            context_info: Some(ContextInfo::default()),
            ..Default::default()
        }),
        ..Default::default()
    };
    push_inbound(&world, phone_node(&world, "IMG1", &image, true));
    let events = events_until(&mut session, |e| matches!(e, Event::Message(_))).await;
    let Some(Event::Message(message)) = events.iter().find(|e| matches!(e, Event::Message(_)))
    else {
        panic!("no message: {events:?}");
    };
    match &message.content {
        Content::Media {
            kind,
            caption,
            data,
            ..
        } => {
            assert_eq!(kind, "image");
            assert_eq!(caption, "a picture");
            assert_eq!(data.as_slice(), plain.as_slice());
        }
        other => panic!("not media: {other:?}"),
    }
}

/// One regular patch that adds a contact named "Phone Friend", with every MAC the way WhatsApp
/// computes it.
fn contact_patch() -> Vec<u8> {
    let keys = expand_key(&SYNC_KEY).expect("keys");
    let index_json = br#"["contact","555@s.whatsapp.net"]"#.to_vec();
    let action = SyncActionData {
        index: Some(index_json.clone()),
        value: Some(SyncActionValue {
            timestamp: Some(1),
            contact_action: Some(ContactAction {
                full_name: Some("Phone Friend".to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        }),
        version: Some(2),
    };
    let iv = [3u8; 16];
    let ciphertext = chirper_signal_protocol::cipher::aes_cbc_encrypt(
        &keys.value_encryption,
        &iv,
        &action.encode_to_vec(),
    )
    .expect("encrypt");
    let mut content = iv.to_vec();
    content.extend_from_slice(&ciphertext);
    let key_len = u64::try_from(KEY_ID.len() + 1).expect("len").to_be_bytes();
    let value_mac = hmac512_prefix(&keys.value_mac, &content, &key_len);
    let mut value_blob = content;
    value_blob.extend_from_slice(&value_mac);
    let index_mac = crypto::hmac256(&keys.index, &[&index_json]).expect("index mac");

    let mutation = SyncdMutation {
        operation: Some(0),
        record: Some(SyncdRecord {
            index: Some(SyncdBlob {
                blob: Some(index_mac.to_vec()),
            }),
            value: Some(SyncdBlob {
                blob: Some(value_blob),
            }),
            key_id: Some(KeyId {
                id: Some(KEY_ID.to_vec()),
            }),
        }),
    };
    let mut state = HashState {
        version: 1,
        ..Default::default()
    };
    lt_hash(&mut state.hash, &[], &[value_mac.to_vec()]).expect("lt");
    let mut patch = SyncdPatch {
        version: Some(SyncdVersion { version: Some(1) }),
        mutations: vec![mutation],
        external_mutations: None,
        snapshot_mac: Some(
            snapshot_mac(&keys, &state, "regular")
                .expect("snapshot mac")
                .to_vec(),
        ),
        patch_mac: None,
        key_id: Some(KeyId {
            id: Some(KEY_ID.to_vec()),
        }),
    };
    patch.patch_mac = Some(
        patch_mac(&keys, &patch, 1, "regular")
            .expect("patch mac")
            .to_vec(),
    );
    patch.encode_to_vec()
}

fn hmac512_prefix(key: &[u8; 32], content: &[u8], key_len: &[u8; 8]) -> [u8; 32] {
    let full = crypto::hmac512(key, &[&[1u8], KEY_ID, content, key_len]).expect("hmac");
    let mut out = [0u8; 32];
    out.copy_from_slice(&full[..32]);
    out
}

#[tokio::test]
async fn app_state_keys_from_the_phone_unlock_a_contact_patch() {
    let (world, ctx, keys) = setup();
    let mut session = running_session(Arc::clone(&world), ctx, keys, None).await;
    events_until(&mut session, |e| {
        matches!(
            e,
            Event::State {
                state: SessionState::Running,
                ..
            }
        )
    })
    .await;

    world.lock().expect("world").patch = Some(Node::new("patch").bytes(contact_patch()));
    let share = Message {
        protocol_message: Some(ProtocolMessage {
            r#type: Some(6),
            app_state_sync_key_share: Some(AppStateSyncKeyShare {
                keys: vec![AppStateSyncKey {
                    key_id: Some(AppStateSyncKeyId {
                        key_id: Some(KEY_ID.to_vec()),
                    }),
                    key_data: Some(AppStateSyncKeyData {
                        key_data: Some(SYNC_KEY.to_vec()),
                        timestamp: Some(1),
                    }),
                }],
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    push_inbound(&world, phone_node(&world, "K1", &share, true));
    let events = events_until(
        &mut session,
        |e| matches!(e, Event::Chat(meta) if meta.name == "Phone Friend"),
    )
    .await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Chat(meta) if meta.is_dm && meta.portal.id == PHONE))
    );
}

#[tokio::test]
async fn a_message_that_does_not_decrypt_gets_a_retry_receipt() {
    let (world, ctx, keys) = setup();
    let mut session = running_session(Arc::clone(&world), ctx, keys, None).await;
    events_until(&mut session, |e| {
        matches!(
            e,
            Event::State {
                state: SessionState::Running,
                ..
            }
        )
    })
    .await;

    // Garbage that parses as nothing: the phone's message cannot be read.
    push_inbound(
        &world,
        message_node("BAD1", PHONE, "msg", vec![1, 2, 3], Vec::new()),
    );
    let events = events_until(&mut session, |e| {
        matches!(e, Event::Message(_) | Event::Error(_))
    })
    .await;
    assert!(!events.iter().any(|e| matches!(e, Event::Message(_))));

    let w = world.lock().expect("world");
    let retry = w
        .sent
        .iter()
        .find(|n| n.tag == "receipt" && n.get_attr("type").as_deref() == Some("retry"))
        .expect("a retry receipt");
    assert_eq!(retry.get_attr("id").as_deref(), Some("BAD1"));
    let counter = retry.child("retry").expect("retry child");
    assert_eq!(counter.get_attr("count").as_deref(), Some("1"));
    assert_eq!(
        retry
            .child("registration")
            .and_then(|r| r.content_bytes())
            .map(<[u8]>::len),
        Some(4)
    );
}

#[tokio::test]
async fn an_image_is_uploaded_encrypted_and_the_phone_gets_its_key() {
    // A local media host: captures the upload and answers with where the file lives.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let uploaded = Arc::new(Mutex::new(Vec::<u8>::new()));
    let captured = Arc::clone(&uploaded);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    break;
                }
                head.push(byte[0]);
            }
            let text = String::from_utf8_lossy(&head).to_string();
            let length: usize = text
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse().unwrap_or(0))
                })
                .unwrap_or(0);
            let mut body = vec![0u8; length];
            let _ = stream.read_exact(&mut body);
            *captured.lock().expect("capture") = body;
            let answer = r#"{"url":"https://mmg.example/up1","direct_path":"/v/up1"}"#;
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                )
                .as_bytes(),
            );
        }
    });

    let (world, ctx, keys) = setup();
    let mut session = running_session(
        Arc::clone(&world),
        ctx,
        keys,
        Some(format!("http://127.0.0.1:{port}")),
    )
    .await;
    events_until(&mut session, |e| {
        matches!(
            e,
            Event::State {
                state: SessionState::Running,
                ..
            }
        )
    })
    .await;

    let picture = b"jpeg bytes of a holiday".to_vec();
    session
        .command(Command::SendMedia {
            portal: PortalRef {
                id: PHONE.to_owned(),
                receiver: OUR_USER.to_owned(),
            },
            data: Arc::new(picture.clone()),
            mime: "image/jpeg".to_owned(),
            file_name: "holiday.jpg".to_owned(),
            caption: "the sea".to_owned(),
            kind: "image".to_owned(),
            reply_to: None,
            txn_id: "m1".to_owned(),
        })
        .await
        .expect("send media");

    let node = our_sent_message(&world, PHONE);
    let message = phone_decrypt(&world, &node);
    let image = message.image_message.expect("an image message");
    assert_eq!(image.direct_path.as_deref(), Some("/v/up1"));
    assert_eq!(image.caption.as_deref(), Some("the sea"));
    let body = uploaded.lock().expect("capture").clone();
    let plain = crypto::decrypt_media(
        &body,
        image.media_key.as_deref().expect("key"),
        "WhatsApp Image Keys",
        image.file_sha256.as_deref().expect("sha"),
        image.file_enc_sha256.as_deref().expect("enc sha"),
    )
    .expect("the uploaded file decrypts with the key in the message");
    assert_eq!(plain, picture);
}

#[tokio::test]
async fn logging_out_tells_the_server_to_remove_this_device() {
    let (world, ctx, keys) = setup();
    let mut session = running_session(Arc::clone(&world), ctx, keys, None).await;
    events_until(&mut session, |e| {
        matches!(
            e,
            Event::State {
                state: SessionState::Running,
                ..
            }
        )
    })
    .await;
    session.logout_remote().await.expect("logout");
    let w = world.lock().expect("world");
    let removal = w
        .sent
        .iter()
        .find(|n| n.get_attr("xmlns").as_deref() == Some("md"))
        .and_then(|n| n.child("remove-companion-device"))
        .expect("the removal request");
    assert_eq!(
        removal.get_attr("reason").as_deref(),
        Some("user_initiated")
    );
}
