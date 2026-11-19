//! Key material of one linked device: the X25519 key pairs, the signed pre-key, the registration
//! id and the ADV secret, plus the record that persists them.
//!
//! Private scalars are kept in [`Zeroizing`] buffers and never appear in `Debug` output. The record
//! is persisted only through `LoginCtx::set_secret`, which the core encrypts.

use std::fmt;

use chirper_bridge::{Error, Result};
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, Zeroizing};

use crate::binary::Jid;
use crate::xeddsa;

/// The id of the signed pre-key a new device registers.
pub const SIGNED_PRE_KEY_ID: u32 = 1;

/// An X25519 key pair: a private scalar and its public Montgomery `u` coordinate.
#[derive(Clone)]
pub struct KeyPair {
    private: Zeroizing<[u8; 32]>,
    public: [u8; 32],
}

impl fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KeyPair([redacted])")
    }
}

impl KeyPair {
    /// A fresh random key pair from the OS random source.
    pub fn generate() -> Result<Self> {
        Ok(Self::from_private(random()?))
    }

    /// The key pair of a stored private scalar.
    pub fn from_private(private: [u8; 32]) -> Self {
        let public = PublicKey::from(&StaticSecret::from(private)).to_bytes();
        Self {
            private: Zeroizing::new(private),
            public,
        }
    }

    /// The private scalar. Callers must not copy it out of a [`Zeroizing`] buffer.
    pub fn private(&self) -> &[u8; 32] {
        &self.private
    }

    /// The public key, 32 bytes.
    pub fn public(&self) -> &[u8; 32] {
        &self.public
    }

    /// The X25519 shared secret with `peer`. A low-order peer key yields no contribution and is
    /// refused, as the Noise specification requires.
    pub fn agree(&self, peer: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>> {
        let shared = StaticSecret::from(*self.private).diffie_hellman(&PublicKey::from(*peer));
        if !shared.was_contributory() {
            return Err(Error::Protocol(
                "a peer key contributed nothing to the key agreement".to_owned(),
            ));
        }
        Ok(Zeroizing::new(shared.to_bytes()))
    }
}

/// `N` bytes from the OS random source.
pub fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes)
        .map_err(|err| Error::Internal(format!("no OS randomness: {err}")))?;
    Ok(bytes)
}

/// Everything a linked device holds. Created once, registered with the server, and reused for
/// every login.
#[derive(Clone)]
pub struct DeviceKeys {
    /// The Noise static key, used in the handshake.
    pub noise: KeyPair,
    /// The Curve25519 identity key: it signs the pre-key and the device's pairing signature.
    pub identity: KeyPair,
    /// The signed pre-key, [`SIGNED_PRE_KEY_ID`].
    pub signed_pre_key: KeyPair,
    /// XEdDSA signature by `identity` over `0x05 || signed_pre_key.public`.
    pub signed_pre_key_signature: [u8; 64],
    /// The Signal registration id, 1 to 16380.
    pub registration_id: u32,
    /// The secret the QR code shares with the phone to authenticate the pairing.
    pub adv_secret: Zeroizing<[u8; 32]>,
    /// The linked account's JID, once pairing has succeeded.
    pub jid: Option<Jid>,
    /// The signed device identity the account issued, once pairing has succeeded.
    pub account_identity: Option<Vec<u8>>,
}

impl fmt::Debug for DeviceKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeviceKeys([redacted])")
    }
}

impl DeviceKeys {
    /// Fresh keys for a device that has not been paired.
    pub fn generate() -> Result<Self> {
        let identity = KeyPair::generate()?;
        let signed_pre_key = KeyPair::generate()?;
        let prefix = [0x05u8];
        let signed = [&prefix[..], signed_pre_key.public()];
        let signed_pre_key_signature =
            xeddsa::sign_with_random(identity.private(), &signed, &random::<64>()?);
        // 14-bit registration id, never zero.
        let registration_id = u32::from(u16::from_be_bytes(random::<2>()?) % 16_380) + 1;
        Ok(Self {
            noise: KeyPair::generate()?,
            identity,
            signed_pre_key,
            signed_pre_key_signature,
            registration_id,
            adv_secret: Zeroizing::new(random()?),
            jid: None,
            account_identity: None,
        })
    }

    /// The record persisted through `set_secret`, as JSON with hex-encoded secrets.
    pub fn to_record(&self) -> Result<Zeroizing<Vec<u8>>> {
        let record = StoredDevice {
            noise: hex::encode(self.noise.private().as_slice()),
            identity: hex::encode(self.identity.private().as_slice()),
            signed_pre_key: hex::encode(self.signed_pre_key.private().as_slice()),
            signed_pre_key_signature: hex::encode(self.signed_pre_key_signature),
            registration_id: self.registration_id,
            adv_secret: hex::encode(self.adv_secret.as_slice()),
            jid: self.jid.as_ref().map(ToString::to_string),
            account_identity: self.account_identity.as_ref().map(hex::encode),
        };
        let bytes = serde_json::to_vec(&record)
            .map_err(|_| Error::Internal("the device record could not be encoded".to_owned()))?;
        Ok(Zeroizing::new(bytes))
    }

    /// Read a record written by [`Self::to_record`]. A corrupt record means the login must be
    /// redone, so it is reported as an authentication failure.
    pub fn from_record(bytes: &[u8]) -> Result<Self> {
        let corrupt = || Error::Auth("the stored device record is corrupt".to_owned());
        let record: StoredDevice = serde_json::from_slice(bytes).map_err(|_| corrupt())?;
        let key = |text: &str| -> Result<[u8; 32]> {
            let raw = hex::decode(text).map_err(|_| corrupt())?;
            <[u8; 32]>::try_from(raw.as_slice()).map_err(|_| corrupt())
        };
        let signature = {
            let raw = hex::decode(&record.signed_pre_key_signature).map_err(|_| corrupt())?;
            <[u8; 64]>::try_from(raw.as_slice()).map_err(|_| corrupt())?
        };
        let jid = match &record.jid {
            Some(text) => Some(text.parse::<Jid>().map_err(|_| corrupt())?),
            None => None,
        };
        let account_identity = match &record.account_identity {
            Some(text) => Some(hex::decode(text).map_err(|_| corrupt())?),
            None => None,
        };
        Ok(Self {
            noise: KeyPair::from_private(key(&record.noise)?),
            identity: KeyPair::from_private(key(&record.identity)?),
            signed_pre_key: KeyPair::from_private(key(&record.signed_pre_key)?),
            signed_pre_key_signature: signature,
            registration_id: record.registration_id,
            adv_secret: Zeroizing::new(key(&record.adv_secret)?),
            jid,
            account_identity,
        })
    }
}

/// The JSON shape of a stored device. Its strings hold secrets, so they are wiped on drop.
#[derive(Serialize, Deserialize)]
struct StoredDevice {
    noise: String,
    identity: String,
    signed_pre_key: String,
    signed_pre_key_signature: String,
    registration_id: u32,
    adv_secret: String,
    jid: Option<String>,
    account_identity: Option<String>,
}

impl Drop for StoredDevice {
    fn drop(&mut self) {
        self.noise.zeroize();
        self.identity.zeroize();
        self.signed_pre_key.zeroize();
        self.adv_secret.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agreement_is_symmetric() {
        let alice = KeyPair::generate().expect("random key");
        let bob = KeyPair::generate().expect("random key");
        let ab = alice.agree(bob.public()).expect("contributory");
        let ba = bob.agree(alice.public()).expect("contributory");
        assert_eq!(*ab, *ba);
    }

    #[test]
    fn rfc7748_public_key_vector() {
        // RFC 7748 section 6.1: Alice's private key and public key.
        let private =
            hex::decode("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a")
                .expect("hex");
        let mut key = [0u8; 32];
        key.copy_from_slice(&private);
        let pair = KeyPair::from_private(key);
        assert_eq!(
            hex::encode(pair.public()),
            "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a"
        );
    }

    #[test]
    fn low_order_peer_is_refused() {
        let pair = KeyPair::generate().expect("random key");
        assert!(pair.agree(&[0u8; 32]).is_err());
    }

    #[test]
    fn debug_output_hides_secrets() {
        let pair = KeyPair::from_private([7; 32]);
        let text = format!("{pair:?}");
        assert!(text.contains("redacted"));
        assert!(!text.contains("07"));
        let keys = DeviceKeys::generate().expect("keys");
        assert_eq!(format!("{keys:?}"), "DeviceKeys([redacted])");
    }

    #[test]
    fn generated_keys_are_consistent() {
        let keys = DeviceKeys::generate().expect("keys");
        assert!((1..=16_380).contains(&keys.registration_id));
        let prefix = [0x05u8];
        let signed = [&prefix[..], keys.signed_pre_key.public()];
        assert!(xeddsa::verify(
            keys.identity.public(),
            &signed,
            &keys.signed_pre_key_signature
        ));
    }

    #[test]
    fn record_round_trips_without_losing_secrets() {
        let mut keys = DeviceKeys::generate().expect("keys");
        keys.jid = Some("15551234567:2@s.whatsapp.net".parse().expect("jid"));
        keys.account_identity = Some(vec![1, 2, 3]);
        let record = keys.to_record().expect("encodes");
        let back = DeviceKeys::from_record(&record).expect("decodes");
        assert_eq!(back.noise.private(), keys.noise.private());
        assert_eq!(back.identity.public(), keys.identity.public());
        assert_eq!(back.signed_pre_key.public(), keys.signed_pre_key.public());
        assert_eq!(back.registration_id, keys.registration_id);
        assert_eq!(*back.adv_secret, *keys.adv_secret);
        assert_eq!(back.jid, keys.jid);
        assert_eq!(back.account_identity, keys.account_identity);
    }

    #[test]
    fn corrupt_records_are_auth_failures() {
        assert!(matches!(
            DeviceKeys::from_record(b"not json"),
            Err(Error::Auth(_))
        ));
        assert!(matches!(
            DeviceKeys::from_record(br#"{"noise":"00"}"#),
            Err(Error::Auth(_))
        ));
    }
}
