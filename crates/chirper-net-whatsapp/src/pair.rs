//! Linking the device to a phone: the QR codes the phone scans, and the cryptography of the
//! `pair-success` the phone answers with.
//!
//! The QR string and the pairing checks follow wacore 0.7.0 (MIT),
//! https://github.com/jlucaso1/whatsapp-rust (`wacore::pair::PairUtils`), including its
//! `[6,0]`/`[6,1]` signature prefixes and the five-field QR form. The protocol shapes are written
//! from that behaviour; the code is ours.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chirper_bridge::{Error, Result};
use hmac::{Hmac, Mac};
use prost::Message;
use sha2::Sha256;

use crate::binary::{Jid, Node};
use crate::keys::{self, DeviceKeys};
use crate::proto::adv_encryption_type;
use crate::proto::{AdvDeviceIdentity, AdvSignedDeviceIdentity, AdvSignedDeviceIdentityHmac};
use crate::xeddsa;

/// The server's domain, the `to` of every reply to it.
pub const SERVER: &str = "s.whatsapp.net";

/// How long the first QR code stays valid before the next one is shown.
pub const FIRST_QR_TTL: Duration = Duration::from_secs(60);

/// How long each later QR code stays valid.
pub const QR_TTL: Duration = Duration::from_secs(20);

/// The companion client type the QR advertises: `1`, the Chrome browser. The server rejects a QR
/// without this fifth field.
const CLIENT_TYPE: char = '1';

const PREFIX_ACCOUNT: [u8; 2] = [6, 0];
const PREFIX_DEVICE: [u8; 2] = [6, 1];
const PREFIX_HOSTED_ACCOUNT: [u8; 2] = [6, 5];
const PREFIX_HOSTED_DEVICE: [u8; 2] = [6, 6];

/// The QR string for one pairing reference:
/// `ref,noise_pub,identity_pub,adv_secret,client_type`, each key in standard base64.
pub fn qr_code(keys: &DeviceKeys, pairing_ref: &str) -> String {
    format!(
        "{pairing_ref},{},{},{},{CLIENT_TYPE}",
        STANDARD.encode(keys.noise.public()),
        STANDARD.encode(keys.identity.public()),
        STANDARD.encode(keys.adv_secret.as_slice()),
    )
}

/// The acknowledgement of an incoming `iq`, echoing its id back to its sender.
pub fn ack(iq: &Node) -> Option<Node> {
    let to = iq.get_attr("from")?;
    let id = iq.get_attr("id")?;
    Some(
        Node::new("iq")
            .attr("to", to)
            .attr("id", id)
            .attr("type", "result"),
    )
}

/// The pairing references of a `pair-device` node, in the order the server sent them. A reference
/// that is not text is dropped rather than failing the whole batch.
pub fn pairing_refs(pair_device: &Node) -> Vec<String> {
    pair_device
        .child_nodes()
        .iter()
        .filter(|node| node.tag == "ref")
        .filter_map(|node| node.content_bytes())
        .filter_map(|bytes| std::str::from_utf8(bytes).ok().map(str::to_owned))
        .collect()
}

/// The outcome of a successful pairing: the reply to send, and the account that was linked.
#[derive(Debug)]
pub struct Paired {
    /// The `pair-device-sign` iq to send back to the server.
    pub reply: Node,
    /// The linked account's device JID.
    pub jid: Jid,
    /// The platform the phone reported, when it did.
    pub platform: String,
}

/// Verify a `pair-success` and build the reply. On success the keys record the account and the
/// signed identity; on any failure they are left untouched.
pub fn complete(keys: &mut DeviceKeys, iq: &Node, success: &Node) -> Result<Paired> {
    let protocol = |what: &str| Error::Protocol(format!("pairing: {what}"));
    let request_id = iq.get_attr("id").ok_or_else(|| protocol("no request id"))?;
    let container_bytes = success
        .child("device-identity")
        .and_then(Node::content_bytes)
        .ok_or_else(|| protocol("no device identity"))?;
    let container = AdvSignedDeviceIdentityHmac::decode(container_bytes)
        .map_err(|_| protocol("the device identity is malformed"))?;
    let hosted = container.account_type == Some(adv_encryption_type::HOSTED);
    let details = container
        .details
        .as_deref()
        .ok_or_else(|| protocol("the identity has no details"))?;
    let mac_bytes = container
        .hmac
        .as_deref()
        .ok_or_else(|| protocol("the identity has no HMAC"))?;

    // The HMAC proves the phone holds the ADV secret the QR code carried.
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(keys.adv_secret.as_slice())
        .map_err(|_| Error::Internal("the ADV secret has no valid HMAC key".to_owned()))?;
    if hosted {
        mac.update(&PREFIX_HOSTED_ACCOUNT);
    }
    mac.update(details);
    mac.verify_slice(mac_bytes)
        .map_err(|_| protocol("the HMAC does not match the ADV secret"))?;

    let mut signed = AdvSignedDeviceIdentity::decode(details)
        .map_err(|_| protocol("the signed identity is malformed"))?;
    let account_key = fixed::<32>(signed.account_signature_key.as_deref(), "account key")?;
    let account_signature = fixed::<64>(signed.account_signature.as_deref(), "account signature")?;
    let inner = signed
        .details
        .clone()
        .ok_or_else(|| protocol("the signed identity has no details"))?;

    let account_prefix = if hosted {
        PREFIX_HOSTED_ACCOUNT
    } else {
        PREFIX_ACCOUNT
    };
    let identity = keys.identity.public();
    if !xeddsa::verify(
        &account_key,
        &[&account_prefix[..], &inner, identity],
        &account_signature,
    ) {
        return Err(protocol("the account signature does not verify"));
    }

    let device_prefix = if hosted {
        PREFIX_HOSTED_DEVICE
    } else {
        PREFIX_DEVICE
    };
    let device_signature = xeddsa::sign_with_random(
        keys.identity.private(),
        &[&device_prefix[..], &inner, identity, &account_key],
        &keys::random::<64>()?,
    );
    signed.device_signature = Some(device_signature.to_vec());

    let key_index = AdvDeviceIdentity::decode(inner.as_slice())
        .map_err(|_| protocol("the device identity details are malformed"))?
        .key_index
        .ok_or_else(|| protocol("the device identity has no key index"))?;
    let signed_bytes = signed.encode_to_vec();

    let jid = device_jid(success)?;
    let platform = success
        .child("platform")
        .and_then(|node| node.get_attr("name"))
        .unwrap_or_default();
    let reply = Node::new("iq")
        .attr("to", SERVER)
        .attr("id", request_id)
        .attr("type", "result")
        .children(vec![Node::new("pair-device-sign").children(vec![
            Node::new("device-identity")
                .attr("key-index", key_index.to_string())
                .bytes(signed_bytes.clone()),
        ])]);

    keys.jid = Some(jid.clone());
    keys.account_identity = Some(signed_bytes);
    Ok(Paired {
        reply,
        jid,
        platform,
    })
}

fn device_jid(success: &Node) -> Result<Jid> {
    let device = success
        .child("device")
        .ok_or_else(|| Error::Protocol("pairing: no device in pair-success".to_owned()))?;
    match device.jid_or_text("jid") {
        Some(jid) => Ok(jid.clone()),
        None => device
            .get_attr("jid")
            .ok_or_else(|| Error::Protocol("pairing: the device has no JID".to_owned()))?
            .parse()
            .map_err(|_| Error::Protocol("pairing: the device JID is malformed".to_owned())),
    }
}

fn fixed<const N: usize>(bytes: Option<&[u8]>, what: &str) -> Result<[u8; N]> {
    let bytes = bytes.ok_or_else(|| Error::Protocol(format!("pairing: no {what}")))?;
    <[u8; N]>::try_from(bytes)
        .map_err(|_| Error::Protocol(format!("pairing: the {what} has the wrong length")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in phone: its account key and the identity it issues for a device.
    fn primary_success(keys: &DeviceKeys, account_private: [u8; 32], key_index: u32) -> Node {
        let account_public = xeddsa::public_key(&account_private);
        let inner = AdvDeviceIdentity {
            raw_id: Some(1),
            timestamp: Some(1_700_000_000),
            key_index: Some(key_index),
            account_type: Some(adv_encryption_type::E2EE),
            device_type: Some(adv_encryption_type::E2EE),
        }
        .encode_to_vec();
        let account_signature = xeddsa::sign_with_random(
            &account_private,
            &[&PREFIX_ACCOUNT[..], &inner, keys.identity.public()],
            &[3; 64],
        );
        let signed = AdvSignedDeviceIdentity {
            details: Some(inner),
            account_signature_key: Some(account_public.to_vec()),
            account_signature: Some(account_signature.to_vec()),
            device_signature: None,
        }
        .encode_to_vec();
        let mut mac =
            <Hmac<Sha256> as Mac>::new_from_slice(keys.adv_secret.as_slice()).expect("key");
        mac.update(&signed);
        let container = AdvSignedDeviceIdentityHmac {
            details: Some(signed),
            hmac: Some(mac.finalize().into_bytes().to_vec()),
            account_type: Some(adv_encryption_type::E2EE),
        }
        .encode_to_vec();
        Node::new("pair-success").children(vec![
            Node::new("device-identity").bytes(container),
            Node::new("device")
                .jid_attr("jid", "15551234567:4@s.whatsapp.net".parse().expect("jid")),
            Node::new("platform").attr("name", "chrome"),
        ])
    }

    fn request() -> Node {
        Node::new("iq")
            .attr("from", SERVER)
            .attr("id", "req-1")
            .attr("type", "set")
    }

    #[test]
    fn qr_has_five_fields_with_standard_base64_keys() {
        let keys = DeviceKeys::generate().expect("keys");
        let qr = qr_code(&keys, "ref-abc");
        let fields: Vec<&str> = qr.split(',').collect();
        assert_eq!(fields.len(), 5);
        assert_eq!(fields[0], "ref-abc");
        assert_eq!(
            STANDARD.decode(fields[1]).expect("b64"),
            keys.noise.public()
        );
        assert_eq!(
            STANDARD.decode(fields[2]).expect("b64"),
            keys.identity.public()
        );
        assert_eq!(
            STANDARD.decode(fields[3]).expect("b64"),
            keys.adv_secret.as_slice()
        );
        assert_eq!(fields[4], "1");
    }

    #[test]
    fn ack_echoes_the_id_and_sender() {
        let reply = ack(&request()).expect("has from and id");
        assert_eq!(reply.get_attr("to").as_deref(), Some(SERVER));
        assert_eq!(reply.get_attr("id").as_deref(), Some("req-1"));
        assert_eq!(reply.get_attr("type").as_deref(), Some("result"));
        assert!(ack(&Node::new("iq")).is_none());
    }

    #[test]
    fn refs_come_out_in_order_and_skip_non_text() {
        let node = Node::new("pair-device").children(vec![
            Node::new("ref").bytes(b"one".to_vec()),
            Node::new("ref").bytes(vec![0xFF, 0xFE]),
            Node::new("other").bytes(b"skip".to_vec()),
            Node::new("ref").bytes(b"two".to_vec()),
        ]);
        assert_eq!(
            pairing_refs(&node),
            vec!["one".to_owned(), "two".to_owned()]
        );
    }

    #[test]
    fn a_valid_pair_success_links_the_device() {
        let mut keys = DeviceKeys::generate().expect("keys");
        let success = primary_success(&keys, [0x61; 32], 4);
        let paired = complete(&mut keys, &request(), &success).expect("pairs");

        assert_eq!(paired.jid.to_string(), "15551234567:4@s.whatsapp.net");
        assert_eq!(paired.platform, "chrome");
        assert_eq!(keys.jid.as_ref(), Some(&paired.jid));
        assert!(keys.account_identity.is_some());

        // The reply carries the identity with a device signature we can check.
        assert_eq!(paired.reply.get_attr("id").as_deref(), Some("req-1"));
        let sign = paired.reply.child("pair-device-sign").expect("sign node");
        let device_identity = sign.child("device-identity").expect("identity");
        assert_eq!(device_identity.get_attr("key-index").as_deref(), Some("4"));
        let signed =
            AdvSignedDeviceIdentity::decode(device_identity.content_bytes().expect("bytes"))
                .expect("decodes");
        let inner = signed.details.expect("details");
        let account_key = signed.account_signature_key.expect("account key");
        let device_signature: [u8; 64] = signed
            .device_signature
            .expect("device signature")
            .try_into()
            .expect("64 bytes");
        assert!(xeddsa::verify(
            keys.identity.public(),
            &[
                &PREFIX_DEVICE[..],
                &inner,
                keys.identity.public(),
                &account_key
            ],
            &device_signature,
        ));
    }

    #[test]
    fn a_forged_hmac_is_refused_and_keys_stay_untouched() {
        let mut keys = DeviceKeys::generate().expect("keys");
        let other_secret = DeviceKeys::generate().expect("keys");
        let success = primary_success(&other_secret, [0x61; 32], 4);
        assert!(complete(&mut keys, &request(), &success).is_err());
        assert!(keys.jid.is_none());
        assert!(keys.account_identity.is_none());
    }

    #[test]
    fn a_bad_account_signature_is_refused() {
        let mut keys = DeviceKeys::generate().expect("keys");
        // Sign the account with a key that does not match the advertised account key.
        let mut success = primary_success(&keys, [0x61; 32], 4);
        let container_bytes = success
            .child("device-identity")
            .and_then(Node::content_bytes)
            .expect("bytes")
            .to_vec();
        let mut container =
            AdvSignedDeviceIdentityHmac::decode(container_bytes.as_slice()).expect("decodes");
        let mut signed =
            AdvSignedDeviceIdentity::decode(container.details.as_deref().expect("d")).expect("d");
        signed.account_signature = Some(vec![0; 64]);
        let details = signed.encode_to_vec();
        let mut mac =
            <Hmac<Sha256> as Mac>::new_from_slice(keys.adv_secret.as_slice()).expect("key");
        mac.update(&details);
        container.details = Some(details);
        container.hmac = Some(mac.finalize().into_bytes().to_vec());
        success = Node::new("pair-success").children(vec![
            Node::new("device-identity").bytes(container.encode_to_vec()),
            Node::new("device").attr("jid", "15551234567:4@s.whatsapp.net"),
        ]);
        assert!(complete(&mut keys, &request(), &success).is_err());
        assert!(keys.jid.is_none());
    }

    #[test]
    fn missing_device_identity_is_refused() {
        let mut keys = DeviceKeys::generate().expect("keys");
        let success = Node::new("pair-success");
        assert!(complete(&mut keys, &request(), &success).is_err());
    }
}
