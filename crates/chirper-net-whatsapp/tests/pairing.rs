//! Both login flows against the scripted server: the 8-character pairing code and the QR code.
//! The primary phone's half is played by the server, so the ADV secret both sides derive is real.

mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use chirper_bridge::LoginCtx;
use chirper_bridge::model::LoginStep;
use chirper_bridge::network::LoginInput;
use chirper_net_whatsapp::binary::Node;
use chirper_net_whatsapp::keys::{DeviceKeys, KeyPair};
use chirper_net_whatsapp::login::{Dialer, Pairing};
use chirper_net_whatsapp::pair_code::PBKDF2_ROUNDS;
use support::*;

const VERSION: (u32, u32, u32) = (2, 3000, 1);

/// The fixed key material of the primary phone and its account.
fn primary() -> (KeyPair, KeyPair, KeyPair) {
    (
        KeyPair::from_private([0x31; 32]),
        KeyPair::from_private([0x32; 32]),
        KeyPair::from_private([0x33; 32]),
    )
}

fn new_pairing(
    world: &Arc<Mutex<World>>,
    ctx: LoginCtx,
    method: fn(LoginCtx, DeviceKeys, (u32, u32, u32)) -> Pairing,
) -> Pairing {
    let dialer: Arc<dyn Dialer> = Arc::new(FakeDialer {
        world: Arc::clone(world),
    });
    method(ctx, DeviceKeys::generate().expect("keys"), VERSION).with_dialer(dialer)
}

fn complete_login_id(step: LoginStep) -> String {
    match step {
        LoginStep::Complete { login_id } => login_id,
        other => panic!("not complete: {other:?}"),
    }
}

#[tokio::test]
async fn the_pairing_code_login_completes_and_stores_the_device() {
    let world = Arc::new(Mutex::new(World::new()));
    let (primary_eph, primary_id, account) = primary();
    world.lock().expect("world").pairing = Some(PairingScript {
        hello: None,
        code: None,
        primary_ephemeral: primary_eph.clone(),
        primary_identity: primary_id.clone(),
        account: account.clone(),
        account_rand: [0x34; 64],
    });
    let ctx = login_ctx();
    let mut pairing = new_pairing(&world, ctx.clone(), Pairing::phone);

    // Step one asks for the phone number.
    let first = pairing
        .advance(LoginInput::start())
        .await
        .expect("first step");
    assert!(matches!(first, LoginStep::UserInput { ref step_id, .. } if step_id == "phone"));

    // Step two sends the hello and shows the code.
    let mut values = BTreeMap::new();
    values.insert("phone".to_owned(), "+1 555 123 4567".to_owned());
    let shown = pairing
        .advance(LoginInput::values(values))
        .await
        .expect("code");
    let data = match shown {
        LoginStep::DisplayAndWait { kind, data, .. } => {
            assert_eq!(kind, "code");
            data
        }
        other => panic!("no code: {other:?}"),
    };
    assert_eq!(data.len(), 9, "XXXX-XXXX");
    let code: String = data.chars().filter(|c| *c != '-').collect();
    world
        .lock()
        .expect("world")
        .pairing
        .as_mut()
        .expect("script")
        .code = Some(code.clone());

    // The primary wraps its ephemeral key under the code and sends its notification.
    let salt = [0x41u8; 32];
    let iv = [0x42u8; 16];
    let key =
        chirper_net_whatsapp::crypto::pbkdf2_sha256(code.as_bytes(), &salt, PBKDF2_ROUNDS, 32);
    let mut key_arr = [0u8; 32];
    key_arr.copy_from_slice(&key);
    let mut encrypted = *primary_eph.public();
    chirper_net_whatsapp::crypto::aes_ctr(&key_arr, &iv, &mut encrypted).expect("ctr");
    let mut wrapped = salt.to_vec();
    wrapped.extend_from_slice(&iv);
    wrapped.extend_from_slice(&encrypted);
    let notification = Node::new("notification")
        .attr("from", "s.whatsapp.net")
        .attr("id", "n1")
        .attr("type", "link_code_companion_reg")
        .children(vec![Node::new("link_code_companion_reg").children(vec![
            Node::new("link_code_pairing_ref").bytes(b"PAIRREF".to_vec()),
            Node::new("link_code_pairing_wrapped_primary_ephemeral_pub").bytes(wrapped),
            Node::new("primary_identity_pub").bytes(primary_id.public().to_vec()),
        ])]);
    // The login's second connection is the one that answers with `success`.
    world.lock().expect("world").next_dial_seed = vec![success_node()];
    push_inbound(&world, notification);

    let step = pairing
        .advance(LoginInput::default())
        .await
        .expect("finish");
    assert_eq!(complete_login_id(step), OUR_USER);
    let stored = ctx
        .get_secret("device")
        .expect("secrets")
        .expect("device record");
    let keys = DeviceKeys::from_record(&stored).expect("record");
    assert_eq!(keys.jid.expect("account").user, OUR_USER);
}

#[tokio::test]
async fn the_qr_login_shows_codes_and_completes_on_pair_success() {
    let world = Arc::new(Mutex::new(World::new()));
    let ctx = login_ctx();
    world.lock().expect("world").next_dial_seed = vec![
        Node::new("iq")
            .attr("id", "pd1")
            .attr("type", "set")
            .attr("from", "s.whatsapp.net")
            .children(vec![Node::new("pair-device").children(vec![
                Node::new("ref").bytes(b"QRREF1".to_vec()),
                Node::new("ref").bytes(b"QRREF2".to_vec()),
            ])]),
    ];
    let mut pairing = new_pairing(&world, ctx.clone(), Pairing::qr);

    let shown = pairing.advance(LoginInput::start()).await.expect("qr");
    let data = match shown {
        LoginStep::DisplayAndWait {
            kind,
            data,
            step_id,
            ..
        } => {
            assert_eq!(kind, "qr");
            assert_eq!(step_id, "qr");
            data
        }
        other => panic!("no qr: {other:?}"),
    };
    // ref,noise,identity,adv,client-type: the phone reads the companion's keys from it.
    let parts: Vec<&str> = data.split(',').collect();
    assert_eq!(parts.len(), 5);
    assert_eq!(parts[0], "QRREF1");
    let adv = decode32(parts[3]);
    let identity = decode32(parts[2]);
    let (_, _, account) = primary();
    let success = pair_success_node(&adv, &identity, &account, &[0x35; 64]);
    world.lock().expect("world").next_dial_seed = vec![success_node()];
    push_inbound(&world, success);

    let step = pairing
        .advance(LoginInput::default())
        .await
        .expect("pair success");
    assert_eq!(complete_login_id(step), OUR_USER);
    assert!(ctx.get_secret("device").expect("secrets").is_some());
}

fn decode32(text: &str) -> [u8; 32] {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .expect("base64");
    <[u8; 32]>::try_from(bytes.as_slice()).expect("32 bytes")
}

#[tokio::test]
async fn a_phone_number_that_is_too_short_is_refused_before_anything_is_sent() {
    let world = Arc::new(Mutex::new(World::new()));
    let mut pairing = new_pairing(&world, login_ctx(), Pairing::phone);
    let mut values = BTreeMap::new();
    values.insert("phone".to_owned(), "12345".to_owned());
    assert!(pairing.advance(LoginInput::values(values)).await.is_err());
    assert!(world.lock().expect("world").sent.is_empty());
}
