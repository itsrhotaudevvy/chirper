//! Linking a device with an 8-character code instead of a QR code (whatsmeow's `pair-code.go`,
//! Mozilla Public License 2.0).
//!
//! The companion (us) generates an ephemeral X25519 key and a random 5-byte code, shown to the
//! user as `XXXX-XXXX` in the base32 alphabet `123456789ABCDEFGHJKLMNPQRSTVWXYZ`. The ephemeral
//! public key is sent wrapped with AES-256-CTR under PBKDF2(code). The primary phone answers with
//! its own wrapped ephemeral key and its identity key; both sides derive the same shared secrets
//! and the companion hands the primary an encrypted key bundle. The ADV secret that comes out is
//! the one the `pair-success` HMAC is checked against, so the QR flow's verification applies
//! unchanged.

use chirper_bridge::{Error, Result};
use zeroize::Zeroizing;

use crate::crypto::{aes_ctr, gcm_seal, hkdf, hkdf_salted, pbkdf2_sha256};
use crate::keys::{KeyPair, random};

/// The base32 alphabet of the linking code: no `0`, `O`, `I` or `U`.
pub const LINKING_ALPHABET: &[u8; 32] = b"123456789ABCDEFGHJKLMNPQRSTVWXYZ";

/// PBKDF2 rounds that wrap the ephemeral key under the linking code.
pub const PBKDF2_ROUNDS: u32 = 2 << 16;

/// Length of the wrapped ephemeral key the companion sends: salt, IV and the encrypted key.
pub const WRAPPED_LEN: usize = 80;

/// Encodes 5 bytes as 8 characters of the linking alphabet, most significant bits first.
pub fn encode_linking_code(bytes: &[u8; 5]) -> String {
    let mut value = bytes
        .iter()
        .fold(0u64, |acc, byte| (acc << 8) | u64::from(*byte));
    let mut out = String::with_capacity(8);
    for _ in 0..8 {
        let index = ((value >> 35) & 31) as usize;
        out.push(char::from(LINKING_ALPHABET[index]));
        value = (value << 5) & ((1 << 40) - 1);
    }
    out
}

/// The code as the user types it: `XXXX-XXXX`.
pub fn display_code(code: &str) -> String {
    match code.is_char_boundary(4) {
        true => format!("{}-{}", &code[..4], &code[4..]),
        false => code.to_owned(),
    }
}

/// The random inputs of the companion's first step.
pub struct CompanionRandom {
    pub ephemeral_private: [u8; 32],
    pub salt: [u8; 32],
    pub iv: [u8; 16],
    pub code: [u8; 5],
}

impl CompanionRandom {
    /// Fresh OS randomness for one pairing attempt.
    pub fn generate() -> Result<Self> {
        Ok(Self {
            ephemeral_private: random()?,
            salt: random()?,
            iv: random()?,
            code: random()?,
        })
    }
}

/// The companion's state after the first step: what to send and what to keep.
pub struct CompanionLink {
    /// The linking code, 8 characters.
    pub code: String,
    /// The ephemeral key pair, kept until the primary answers.
    pub ephemeral: KeyPair,
    /// Salt, IV and the encrypted ephemeral public key: sent as `link_code_pairing_wrapped_companion_ephemeral_pub`.
    pub wrapped_ephemeral: Vec<u8>,
}

/// The first step, from given randomness.
pub fn companion_link(input: &CompanionRandom) -> Result<CompanionLink> {
    let ephemeral = KeyPair::from_private(input.ephemeral_private);
    let code = encode_linking_code(&input.code);
    let key = wrap_key(code.as_bytes(), &input.salt);
    let mut encrypted = *ephemeral.public();
    aes_ctr(&key, &input.iv, &mut encrypted)?;
    let mut wrapped = Vec::with_capacity(WRAPPED_LEN);
    wrapped.extend_from_slice(&input.salt);
    wrapped.extend_from_slice(&input.iv);
    wrapped.extend_from_slice(&encrypted);
    Ok(CompanionLink {
        code,
        ephemeral,
        wrapped_ephemeral: wrapped,
    })
}

/// What the primary sent in its `link_code_companion_reg` notification.
pub struct PrimaryNotice {
    /// `link_code_pairing_wrapped_primary_ephemeral_pub`: at least 80 bytes.
    pub wrapped_ephemeral: Vec<u8>,
    /// `primary_identity_pub`: the primary's identity key, 32 bytes.
    pub identity_pub: [u8; 32],
}

/// The random inputs of the companion's last step.
pub struct FinishRandom {
    pub key_bundle_salt: [u8; 32],
    pub key_bundle_nonce: [u8; 12],
    pub adv_random: [u8; 32],
}

impl FinishRandom {
    /// Fresh OS randomness for the key bundle.
    pub fn generate() -> Result<Self> {
        Ok(Self {
            key_bundle_salt: random()?,
            key_bundle_nonce: random()?,
            adv_random: random()?,
        })
    }
}

/// The result of the last step.
pub struct Finish {
    /// `link_code_pairing_wrapped_key_bundle`: salt, nonce and the sealed key bundle.
    pub wrapped_key_bundle: Vec<u8>,
    /// The ADV secret the `pair-success` HMAC is checked against.
    pub adv_secret: Zeroizing<[u8; 32]>,
}

/// The last step: unwrap the primary's ephemeral key, derive the shared secrets, seal the key
/// bundle for the primary and derive the ADV secret.
pub fn finish_link(
    link: &CompanionLink,
    identity: &KeyPair,
    notice: &PrimaryNotice,
    input: &FinishRandom,
) -> Result<Finish> {
    let wrapped = notice.wrapped_ephemeral.get(..WRAPPED_LEN).ok_or_else(|| {
        Error::Protocol("pairing: the primary ephemeral key is too short".to_owned())
    })?;
    let (salt, rest) = wrapped.split_at(32);
    let (iv, encrypted) = rest.split_at(16);
    let iv: [u8; 16] = iv
        .try_into()
        .map_err(|_| Error::Protocol("pairing: the primary IV is malformed".to_owned()))?;
    let mut primary_pub: [u8; 32] = encrypted
        .try_into()
        .map_err(|_| Error::Protocol("pairing: the primary key is malformed".to_owned()))?;
    let key = wrap_key(link.code.as_bytes(), salt);
    aes_ctr(&key, &iv, &mut primary_pub)?;

    let ephemeral_shared = link.ephemeral.agree(&primary_pub)?;
    let bundle_key = hkdf_salted(
        ephemeral_shared.as_slice(),
        &input.key_bundle_salt,
        b"link_code_pairing_key_bundle_encryption_key",
        32,
    )?;
    let bundle_key: [u8; 32] = bundle_key
        .try_into()
        .map_err(|_| Error::Internal("pairing: key length".to_owned()))?;
    let mut plaintext = Vec::with_capacity(96);
    plaintext.extend_from_slice(identity.public());
    plaintext.extend_from_slice(&notice.identity_pub);
    plaintext.extend_from_slice(&input.adv_random);
    let sealed = gcm_seal(&bundle_key, &input.key_bundle_nonce, &plaintext)?;
    let mut wrapped_key_bundle = Vec::with_capacity(44 + sealed.len());
    wrapped_key_bundle.extend_from_slice(&input.key_bundle_salt);
    wrapped_key_bundle.extend_from_slice(&input.key_bundle_nonce);
    wrapped_key_bundle.extend_from_slice(&sealed);

    let identity_shared = identity.agree(&notice.identity_pub)?;
    let mut adv_input = Vec::with_capacity(96);
    adv_input.extend_from_slice(ephemeral_shared.as_slice());
    adv_input.extend_from_slice(identity_shared.as_slice());
    adv_input.extend_from_slice(&input.adv_random);
    let adv = hkdf(&adv_input, b"adv_secret", 32)?;
    let adv_secret = Zeroizing::new(
        <[u8; 32]>::try_from(adv.as_slice())
            .map_err(|_| Error::Internal("pairing: adv secret length".to_owned()))?,
    );
    Ok(Finish {
        wrapped_key_bundle,
        adv_secret,
    })
}

/// PBKDF2 of the linking code under the wrapping salt, the AES key of the wrapped ephemeral key.
fn wrap_key(code: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    key.copy_from_slice(&pbkdf2_sha256(code, salt, PBKDF2_ROUNDS, 32));
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::aes_ctr as ctr;

    #[test]
    fn linking_codes_use_the_alphabet_msb_first() {
        assert_eq!(encode_linking_code(&[0; 5]), "11111111");
        assert_eq!(encode_linking_code(&[0xff; 5]), "ZZZZZZZZ");
        // 0x0000000001 -> the last symbol is the value 1, which is '2'.
        assert_eq!(encode_linking_code(&[0, 0, 0, 0, 1]), "11111112");
        assert_eq!(display_code("ABCDEFGH"), "ABCD-EFGH");
    }

    #[test]
    fn companion_link_wraps_the_ephemeral_key_so_the_primary_can_unwrap_it() {
        let input = CompanionRandom {
            ephemeral_private: [5; 32],
            salt: [6; 32],
            iv: [7; 16],
            code: [8; 5],
        };
        let link = companion_link(&input).expect("link");
        assert_eq!(link.wrapped_ephemeral.len(), WRAPPED_LEN);
        // The primary knows the code, so it can recover the companion's ephemeral public key.
        let key = wrap_key(link.code.as_bytes(), &link.wrapped_ephemeral[..32]);
        let mut recovered = link.wrapped_ephemeral[48..80].to_vec();
        ctr(&key, &[7; 16], &mut recovered).expect("ctr");
        assert_eq!(recovered, link.ephemeral.public());
    }

    /// Plays the primary phone: both sides must arrive at the same ADV secret.
    #[test]
    fn both_sides_derive_the_same_adv_secret() {
        let companion_identity = KeyPair::from_private([11; 32]);
        let primary_identity = KeyPair::from_private([12; 32]);
        let link = companion_link(&CompanionRandom {
            ephemeral_private: [13; 32],
            salt: [14; 32],
            iv: [15; 16],
            code: [16; 5],
        })
        .expect("link");

        // The primary wraps its own ephemeral key under the same code.
        let primary_ephemeral = KeyPair::from_private([17; 32]);
        let primary_salt = [18u8; 32];
        let primary_iv = [19u8; 16];
        let key = wrap_key(link.code.as_bytes(), &primary_salt);
        let mut encrypted = *primary_ephemeral.public();
        ctr(&key, &primary_iv, &mut encrypted).expect("ctr");
        let mut wrapped = Vec::new();
        wrapped.extend_from_slice(&primary_salt);
        wrapped.extend_from_slice(&primary_iv);
        wrapped.extend_from_slice(&encrypted);
        let notice = PrimaryNotice {
            wrapped_ephemeral: wrapped,
            identity_pub: *primary_identity.public(),
        };
        let input = FinishRandom {
            key_bundle_salt: [20; 32],
            key_bundle_nonce: [21; 12],
            adv_random: [22; 32],
        };
        let finish = finish_link(&link, &companion_identity, &notice, &input).expect("finish");

        // Primary side: ephemeral DH with the companion's public key, identity DH with the
        // companion's identity key, the advRandom from the bundle it opens.
        let eph_shared = primary_ephemeral
            .agree(link.ephemeral.public())
            .expect("dh");
        let id_shared = primary_identity
            .agree(companion_identity.public())
            .expect("dh");
        let mut adv_input = Vec::new();
        adv_input.extend_from_slice(eph_shared.as_slice());
        adv_input.extend_from_slice(id_shared.as_slice());
        adv_input.extend_from_slice(&input.adv_random);
        let expected = hkdf(&adv_input, b"adv_secret", 32).expect("hkdf");
        assert_eq!(finish.adv_secret.as_slice(), expected.as_slice());
        // The bundle opens with the primary's shared key and carries our identity key first.
        let bundle_key = hkdf_salted(
            eph_shared.as_slice(),
            &input.key_bundle_salt,
            b"link_code_pairing_key_bundle_encryption_key",
            32,
        )
        .expect("hkdf");
        let bundle_key: [u8; 32] = bundle_key.try_into().expect("len");
        let opened = crate::crypto::gcm_open(
            &bundle_key,
            &input.key_bundle_nonce,
            &finish.wrapped_key_bundle[44..],
        )
        .expect("open");
        assert_eq!(&opened[..32], companion_identity.public());
        assert_eq!(&opened[64..], &input.adv_random);
    }

    #[test]
    fn a_short_notice_is_a_protocol_error() {
        let link = companion_link(&CompanionRandom {
            ephemeral_private: [1; 32],
            salt: [2; 32],
            iv: [3; 16],
            code: [4; 5],
        })
        .expect("link");
        let notice = PrimaryNotice {
            wrapped_ephemeral: vec![0; 10],
            identity_pub: [0; 32],
        };
        let input = FinishRandom {
            key_bundle_salt: [0; 32],
            key_bundle_nonce: [0; 12],
            adv_random: [0; 32],
        };
        let identity = KeyPair::from_private([9; 32]);
        assert!(finish_link(&link, &identity, &notice, &input).is_err());
    }
}
