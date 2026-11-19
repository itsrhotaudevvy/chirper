//! The Noise XX handshake WhatsApp runs on its WebSocket, and the transport ciphers it leaves.
//!
//! The protocol is `Noise_XX_25519_AESGCM_SHA256` with WhatsApp's prologue (the connection header)
//! and certificate check. The pattern name, connection header and root certificate key are protocol
//! data copied from wacore-noise and wacore-binary 0.7.0 (MIT),
//! https://github.com/jlucaso1/whatsapp-rust; the state machine is written from the Noise
//! specification.
//!
//! Flow, from the client: `ClientHello{e}`, then the server's `ServerHello{e, s, payload}`, then
//! `ClientFinish{s, payload}`. After the finish, [`Established`] holds the two AES-GCM transport
//! ciphers, one per direction, with their own counters.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use hkdf::Hkdf;
use prost::Message;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use chirper_bridge::{Error, Result};

use crate::keys::KeyPair;
use crate::proto::{CertChain, HandshakeMessage, cert_chain, handshake_message};
use crate::xeddsa;

/// The Noise pattern name, zero-padded to the 32-byte hash length, which makes it the initial `h`.
/// Protocol data from wacore-binary 0.7.0 (MIT), https://github.com/jlucaso1/whatsapp-rust
pub const PATTERN: [u8; 32] = *b"Noise_XX_25519_AESGCM_SHA256\0\0\0\0";

/// The connection header: `WA`, the protocol magic `6` and the dictionary version `3`. It is the
/// Noise prologue and the first bytes the socket sends.
/// Protocol data from wacore-binary 0.7.0 (MIT), https://github.com/jlucaso1/whatsapp-rust
pub const WA_CONN_HEADER: [u8; 4] = [b'W', b'A', 6, 3];

/// The XEdDSA public key that signs the intermediate certificate of the server's chain.
/// Protocol data from wacore-noise 0.7.0 (MIT), https://github.com/jlucaso1/whatsapp-rust
pub const WA_CERT_PUB_KEY: [u8; 32] = [
    0x14, 0x23, 0x75, 0x57, 0x4d, 0x0a, 0x58, 0x71, 0x66, 0xaa, 0xe7, 0x1e, 0xbe, 0x51, 0x64, 0x37,
    0xc4, 0xa2, 0x8b, 0x73, 0xe3, 0x69, 0x5c, 0x6c, 0xe1, 0xf7, 0xf9, 0x54, 0x5d, 0xa8, 0xee, 0x6b,
];

/// The issuer serial the intermediate certificate must carry (the WhatsApp root).
const WA_CERT_ISSUER_SERIAL: u32 = 0;

/// Length of an encrypted 32-byte key: the key plus the 16-byte GCM tag.
const ENCRYPTED_KEY_LEN: usize = 48;

/// The validity window and keys of a server certificate chain that passed verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedCertChain {
    /// The intermediate certificate's signed details and its XEdDSA signature, as the root signed
    /// them. Kept so the signature can be re-checked independently.
    pub intermediate_details: Vec<u8>,
    pub intermediate_signature: [u8; 64],
    pub intermediate_key: [u8; 32],
    pub intermediate_not_before: u64,
    pub intermediate_not_after: u64,
    pub leaf_key: [u8; 32],
    pub leaf_not_before: u64,
    pub leaf_not_after: u64,
}

/// The two transport ciphers of an established connection.
pub struct Established {
    /// Encrypts what the client sends.
    pub write: TransportCipher,
    /// Decrypts what the server sends.
    pub read: TransportCipher,
    /// The verified server chain.
    pub cert_chain: VerifiedCertChain,
}

/// One AES-256-GCM transport direction. The nonce is 12 bytes whose last 8 are the big-endian
/// counter, and the associated data is empty.
pub struct TransportCipher {
    cipher: Aes256Gcm,
    counter: u64,
}

impl TransportCipher {
    fn new(key: &[u8; 32]) -> Result<Self> {
        let cipher = Aes256Gcm::new_from_slice(key)
            .map_err(|_| Error::Internal("the AES key has the wrong length".to_owned()))?;
        Ok(Self { cipher, counter: 0 })
    }

    /// Encrypt one frame body. Fails rather than reuse a nonce.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let nonce = self.next_nonce()?;
        self.cipher
            .encrypt(&nonce, plaintext)
            .map_err(|_| Error::Internal("transport encryption failed".to_owned()))
    }

    /// Decrypt one frame body, which must have been produced by the peer's matching counter.
    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let nonce = self.next_nonce()?;
        self.cipher
            .decrypt(&nonce, ciphertext)
            .map_err(|_| Error::Protocol("a frame failed to authenticate".to_owned()))
    }

    fn next_nonce(&mut self) -> Result<Nonce<aes_gcm::aead::consts::U12>> {
        let nonce = nonce_bytes(self.counter);
        self.counter = self
            .counter
            .checked_add(1)
            .ok_or_else(|| Error::Internal("the transport counter is exhausted".to_owned()))?;
        Ok(Nonce::from(nonce))
    }
}

/// The 12-byte nonce for a counter: four zero bytes, then the counter in big-endian.
fn nonce_bytes(counter: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&counter.to_be_bytes());
    nonce
}

/// The Noise symmetric state: the running hash `h`, the chaining key `ck`, and the current AEAD
/// key with its counter.
struct SymmetricState {
    h: [u8; 32],
    ck: Zeroizing<[u8; 32]>,
    key: Option<Zeroizing<[u8; 32]>>,
    counter: u64,
}

impl SymmetricState {
    fn new(prologue: &[u8]) -> Self {
        let mut state = Self {
            h: PATTERN,
            ck: Zeroizing::new(PATTERN),
            key: None,
            counter: 0,
        };
        state.mix_hash(prologue);
        state
    }

    fn mix_hash(&mut self, data: &[u8]) {
        let mut hash = Sha256::new();
        hash.update(self.h);
        hash.update(data);
        self.h = hash.finalize().into();
    }

    /// `MixKey`: fold key material into the chaining key and start a new AEAD key at counter 0.
    fn mix_key(&mut self, input: &[u8]) -> Result<()> {
        let (chaining, key) = expand(&self.ck, input)?;
        self.ck = chaining;
        self.key = Some(key);
        self.counter = 0;
        Ok(())
    }

    fn encrypt_and_hash(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let key = self.key_or_error()?;
        let ciphertext = seal(&key, self.counter, &self.h, plaintext)?;
        self.bump()?;
        self.mix_hash(&ciphertext);
        Ok(ciphertext)
    }

    fn decrypt_and_hash(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let key = self.key_or_error()?;
        let plaintext = open(&key, self.counter, &self.h, ciphertext)?;
        self.bump()?;
        self.mix_hash(ciphertext);
        Ok(plaintext)
    }

    /// `Split`: the two transport keys, first the initiator's sending key.
    fn split(&self) -> Result<(TransportCipher, TransportCipher)> {
        let (first, second) = expand(&self.ck, &[])?;
        Ok((
            TransportCipher::new(&first)?,
            TransportCipher::new(&second)?,
        ))
    }

    fn key_or_error(&self) -> Result<Zeroizing<[u8; 32]>> {
        self.key
            .as_ref()
            .map(|key| Zeroizing::new(**key))
            .ok_or_else(|| {
                Error::Internal("the handshake used a cipher before mixing a key".to_owned())
            })
    }

    fn bump(&mut self) -> Result<()> {
        self.counter = self
            .counter
            .checked_add(1)
            .ok_or_else(|| Error::Internal("the handshake counter is exhausted".to_owned()))?;
        Ok(())
    }
}

/// HKDF-SHA256 with `salt` as the chaining key and `input` as the key material, 64 output bytes
/// split into the new chaining key and the second key.
/// Two 32-byte secrets split out of one HKDF output.
type SecretPair = (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>);

fn expand(salt: &[u8; 32], input: &[u8]) -> Result<SecretPair> {
    let hkdf = Hkdf::<Sha256>::new(Some(salt), input);
    let mut okm = Zeroizing::new([0u8; 64]);
    hkdf.expand(&[], okm.as_mut())
        .map_err(|_| Error::Internal("HKDF expansion failed".to_owned()))?;
    let mut first = Zeroizing::new([0u8; 32]);
    let mut second = Zeroizing::new([0u8; 32]);
    first.copy_from_slice(&okm[..32]);
    second.copy_from_slice(&okm[32..]);
    Ok((first, second))
}

fn seal(key: &[u8; 32], counter: u64, aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| Error::Internal("the AES key has the wrong length".to_owned()))?;
    cipher
        .encrypt(
            &Nonce::from(nonce_bytes(counter)),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| Error::Internal("handshake encryption failed".to_owned()))
}

fn open(key: &[u8; 32], counter: u64, aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| Error::Internal("the AES key has the wrong length".to_owned()))?;
    cipher
        .decrypt(
            &Nonce::from(nonce_bytes(counter)),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| Error::Protocol("the server's handshake did not authenticate".to_owned()))
}

/// A client-side Noise XX handshake in progress.
pub struct Handshake {
    symmetric: SymmetricState,
    ephemeral: KeyPair,
    static_key: KeyPair,
    payload: Vec<u8>,
}

impl Handshake {
    /// Start a handshake with a fresh ephemeral key. `payload` is the encoded `ClientPayload` the
    /// client sends encrypted in its finish message.
    pub fn new(static_key: KeyPair, payload: Vec<u8>) -> Result<Self> {
        let ephemeral = KeyPair::generate()?;
        let mut symmetric = SymmetricState::new(&WA_CONN_HEADER);
        symmetric.mix_hash(ephemeral.public());
        Ok(Self {
            symmetric,
            ephemeral,
            static_key,
            payload,
        })
    }

    /// The encoded `ClientHello` carrying the ephemeral key.
    pub fn client_hello(&self) -> Vec<u8> {
        HandshakeMessage {
            client_hello: Some(handshake_message::ClientHello {
                ephemeral: Some(self.ephemeral.public().to_vec()),
                ..Default::default()
            }),
            ..Default::default()
        }
        .encode_to_vec()
    }

    /// Consume the server's `ServerHello` and produce the encoded `ClientFinish` to send, plus the
    /// established transport. Any failure means the server is not who the certificate says.
    pub fn finish(self, server_hello: &[u8]) -> Result<(Vec<u8>, Established)> {
        self.finish_with_root(server_hello, &WA_CERT_PUB_KEY)
    }

    /// [`Self::finish`] against an explicit certificate root. Tests use it with a root whose
    /// private key they hold.
    fn finish_with_root(
        mut self,
        server_hello: &[u8],
        root: &[u8; 32],
    ) -> Result<(Vec<u8>, Established)> {
        let hello = HandshakeMessage::decode(server_hello)
            .map_err(|_| Error::Protocol("the server's handshake message is malformed".to_owned()))?
            .server_hello
            .ok_or_else(|| {
                Error::Protocol("the server's handshake has no ServerHello".to_owned())
            })?;
        let server_ephemeral = fixed_32(hello.ephemeral.as_deref(), "server ephemeral key")?;
        let static_ciphertext = hello
            .r#static
            .ok_or_else(|| Error::Protocol("the ServerHello has no static key".to_owned()))?;
        let payload_ciphertext = hello
            .payload
            .ok_or_else(|| Error::Protocol("the ServerHello has no certificate".to_owned()))?;
        if static_ciphertext.len() != ENCRYPTED_KEY_LEN {
            return Err(Error::Protocol(
                "the ServerHello static key has the wrong length".to_owned(),
            ));
        }

        self.symmetric.mix_hash(&server_ephemeral);
        let shared = self.ephemeral.agree(&server_ephemeral)?;
        self.symmetric.mix_key(shared.as_slice())?;

        let server_static_bytes = self.symmetric.decrypt_and_hash(&static_ciphertext)?;
        let server_static = fixed_32(Some(&server_static_bytes), "server static key")?;

        let shared = self.ephemeral.agree(&server_static)?;
        self.symmetric.mix_key(shared.as_slice())?;

        let chain_bytes = self.symmetric.decrypt_and_hash(&payload_ciphertext)?;
        let cert_chain = verify_cert_chain(&chain_bytes, &server_static, root)?;

        let encrypted_static = self.symmetric.encrypt_and_hash(self.static_key.public())?;
        let shared = self.static_key.agree(&server_ephemeral)?;
        self.symmetric.mix_key(shared.as_slice())?;
        let encrypted_payload = self.symmetric.encrypt_and_hash(&self.payload)?;

        let finish = HandshakeMessage {
            client_finish: Some(handshake_message::ClientFinish {
                r#static: Some(encrypted_static),
                payload: Some(encrypted_payload),
            }),
            ..Default::default()
        }
        .encode_to_vec();
        let (write, read) = self.symmetric.split()?;
        Ok((
            finish,
            Established {
                write,
                read,
                cert_chain,
            },
        ))
    }
}

fn fixed_32(bytes: Option<&[u8]>, what: &str) -> Result<[u8; 32]> {
    let bytes = bytes.ok_or_else(|| Error::Protocol(format!("the handshake has no {what}")))?;
    <[u8; 32]>::try_from(bytes)
        .map_err(|_| Error::Protocol(format!("the handshake's {what} has the wrong length")))
}

/// Check the server's certificate chain: the intermediate is signed by `root`, the leaf by the
/// intermediate, and the leaf's key is the static key the handshake just decrypted.
pub fn verify_cert_chain(
    chain_bytes: &[u8],
    server_static: &[u8; 32],
    root: &[u8; 32],
) -> Result<VerifiedCertChain> {
    let protocol = |what: &str| Error::Protocol(format!("the server certificate chain: {what}"));
    let chain = CertChain::decode(chain_bytes).map_err(|_| protocol("malformed"))?;
    let intermediate = chain
        .intermediate
        .ok_or_else(|| protocol("no intermediate"))?;
    let leaf = chain.leaf.ok_or_else(|| protocol("no leaf"))?;

    let intermediate_details_bytes = intermediate
        .details
        .as_deref()
        .ok_or_else(|| protocol("intermediate has no details"))?;
    let intermediate_details =
        cert_chain::noise_certificate::Details::decode(intermediate_details_bytes)
            .map_err(|_| protocol("intermediate details are malformed"))?;
    if intermediate_details.issuer_serial.unwrap_or(0) != WA_CERT_ISSUER_SERIAL {
        return Err(protocol("unexpected issuer of the intermediate"));
    }
    let intermediate_key = fixed_32(intermediate_details.key.as_deref(), "intermediate key")?;
    let intermediate_signature = signature_of(intermediate.signature.as_deref(), "intermediate")?;
    if !xeddsa::verify(root, &[intermediate_details_bytes], &intermediate_signature) {
        return Err(protocol(
            "the intermediate is not signed by the WhatsApp root",
        ));
    }

    let leaf_details_bytes = leaf
        .details
        .as_deref()
        .ok_or_else(|| protocol("leaf has no details"))?;
    let leaf_details = cert_chain::noise_certificate::Details::decode(leaf_details_bytes)
        .map_err(|_| protocol("leaf details are malformed"))?;
    if leaf_details.issuer_serial != intermediate_details.serial {
        return Err(protocol("the leaf names another intermediate"));
    }
    let leaf_key = fixed_32(leaf_details.key.as_deref(), "leaf key")?;
    if &leaf_key != server_static {
        return Err(protocol("the leaf key is not the server's static key"));
    }
    let leaf_signature = signature_of(leaf.signature.as_deref(), "leaf")?;
    if !xeddsa::verify(&intermediate_key, &[leaf_details_bytes], &leaf_signature) {
        return Err(protocol("the leaf is not signed by the intermediate"));
    }

    Ok(VerifiedCertChain {
        intermediate_details: intermediate_details_bytes.to_vec(),
        intermediate_signature,
        intermediate_key,
        intermediate_not_before: intermediate_details.not_before.unwrap_or(0),
        intermediate_not_after: intermediate_details.not_after.unwrap_or(0),
        leaf_key,
        leaf_not_before: leaf_details.not_before.unwrap_or(0),
        leaf_not_after: leaf_details.not_after.unwrap_or(0),
    })
}

fn signature_of(bytes: Option<&[u8]>, what: &str) -> Result<[u8; 64]> {
    let bytes = bytes.ok_or_else(|| {
        Error::Protocol(format!(
            "the server certificate chain: {what} has no signature"
        ))
    })?;
    <[u8; 64]>::try_from(bytes).map_err(|_| {
        Error::Protocol(format!(
            "the server certificate chain: the {what} signature has the wrong length"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::cert_chain::NoiseCertificate;
    use crate::proto::cert_chain::noise_certificate::Details;

    /// A test root key. Its private scalar is known here, so the chain builder can sign.
    const ROOT_PRIVATE: [u8; 32] = [0x41; 32];

    fn cert(details: &Details, signer: &[u8; 32], random: u8) -> NoiseCertificate {
        let details_bytes = details.encode_to_vec();
        let signature = xeddsa::sign_with_random(signer, &[&details_bytes], &[random; 64]);
        NoiseCertificate {
            details: Some(details_bytes),
            signature: Some(signature.to_vec()),
        }
    }

    /// Build a valid chain for `server_static`, returning the chain bytes and the intermediate key.
    fn chain_for(server_static: &[u8; 32]) -> (Vec<u8>, [u8; 32]) {
        let intermediate_private = [0x52; 32];
        let intermediate_key = xeddsa::public_key(&intermediate_private);
        let intermediate = cert(
            &Details {
                serial: Some(1),
                issuer_serial: Some(0),
                key: Some(intermediate_key.to_vec()),
                not_before: Some(1_700_000_000),
                not_after: Some(1_900_000_000),
            },
            &ROOT_PRIVATE,
            1,
        );
        let leaf = cert(
            &Details {
                serial: Some(2),
                issuer_serial: Some(1),
                key: Some(server_static.to_vec()),
                not_before: Some(1_700_000_500),
                not_after: Some(1_899_999_500),
            },
            &intermediate_private,
            2,
        );
        let chain = CertChain {
            leaf: Some(leaf),
            intermediate: Some(intermediate),
        };
        (chain.encode_to_vec(), intermediate_key)
    }

    #[test]
    fn pattern_and_header_have_the_documented_bytes() {
        assert_eq!(PATTERN.len(), 32);
        assert_eq!(&PATTERN[..28], b"Noise_XX_25519_AESGCM_SHA256");
        assert_eq!(&PATTERN[28..], &[0, 0, 0, 0]);
        assert_eq!(WA_CONN_HEADER, [b'W', b'A', 6, 3]);
    }

    #[test]
    fn nonce_puts_the_counter_in_the_last_eight_bytes() {
        assert_eq!(nonce_bytes(0), [0; 12]);
        assert_eq!(nonce_bytes(1), [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(
            nonce_bytes(0x0102_0304_0506_0708),
            [0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]
        );
    }

    #[test]
    fn transport_round_trips_and_counters_are_per_direction() {
        let key = [9u8; 32];
        let mut sender = TransportCipher::new(&key).expect("key");
        let mut receiver = TransportCipher::new(&key).expect("key");
        let first = sender.encrypt(b"one").expect("encrypt");
        let second = sender.encrypt(b"two").expect("encrypt");
        assert_ne!(first, second, "a counter change changes the ciphertext");
        assert_eq!(receiver.decrypt(&first).expect("decrypt"), b"one");
        assert_eq!(receiver.decrypt(&second).expect("decrypt"), b"two");
        // Out of order: the second frame cannot authenticate under the first counter.
        let mut other = TransportCipher::new(&key).expect("key");
        assert!(other.decrypt(&second).is_err());
    }

    #[test]
    fn transport_rejects_tampering() {
        let key = [3u8; 32];
        let mut sender = TransportCipher::new(&key).expect("key");
        let mut receiver = TransportCipher::new(&key).expect("key");
        let mut frame = sender.encrypt(b"payload").expect("encrypt");
        frame[0] ^= 1;
        assert!(receiver.decrypt(&frame).is_err());
    }

    #[test]
    fn cert_chain_verifies_against_its_root() {
        let server_static = KeyPair::from_private([0x33; 32]);
        let (bytes, intermediate_key) = chain_for(server_static.public());
        let root = xeddsa::public_key(&ROOT_PRIVATE);
        let verified =
            verify_cert_chain(&bytes, server_static.public(), &root).expect("valid chain");
        assert_eq!(verified.intermediate_key, intermediate_key);
        assert_eq!(verified.leaf_key, *server_static.public());
        assert_eq!(verified.intermediate_not_after, 1_900_000_000);
    }

    #[test]
    fn cert_chain_rejects_the_wrong_root_or_static_key() {
        let server_static = KeyPair::from_private([0x33; 32]);
        let (bytes, _) = chain_for(server_static.public());
        let other_root = xeddsa::public_key(&[0x99; 32]);
        assert!(verify_cert_chain(&bytes, server_static.public(), &other_root).is_err());

        let root = xeddsa::public_key(&ROOT_PRIVATE);
        let impostor = KeyPair::from_private([0x34; 32]);
        assert!(verify_cert_chain(&bytes, impostor.public(), &root).is_err());
    }

    #[test]
    fn cert_chain_rejects_a_forged_leaf_signature() {
        let server_static = KeyPair::from_private([0x33; 32]);
        let (bytes, _) = chain_for(server_static.public());
        let mut chain = CertChain::decode(bytes.as_slice()).expect("decodes");
        // Re-sign the leaf with the wrong key: the intermediate check must catch it.
        let leaf = chain.leaf.as_mut().expect("leaf");
        let details = leaf.details.clone().expect("details");
        leaf.signature =
            Some(xeddsa::sign_with_random(&[0x77; 32], &[&details], &[4; 64]).to_vec());
        let root = xeddsa::public_key(&ROOT_PRIVATE);
        assert!(verify_cert_chain(&chain.encode_to_vec(), server_static.public(), &root).is_err());
    }

    #[test]
    fn cert_chain_rejects_malformed_input() {
        let root = xeddsa::public_key(&ROOT_PRIVATE);
        assert!(verify_cert_chain(&[0xFF, 0xFF], &[0; 32], &root).is_err());
        assert!(verify_cert_chain(&[], &[0; 32], &root).is_err());
    }

    /// The responder half of the XX pattern, written independently in the test so the initiator is
    /// checked against a second implementation of the same message flow.
    struct Responder {
        symmetric: SymmetricState,
        ephemeral: KeyPair,
        static_key: KeyPair,
    }

    impl Responder {
        fn new(static_key: KeyPair) -> Self {
            Self {
                symmetric: SymmetricState::new(&WA_CONN_HEADER),
                ephemeral: KeyPair::from_private([0x66; 32]),
                static_key,
            }
        }

        /// Read the ClientHello and answer with a ServerHello carrying `chain`.
        fn server_hello(&mut self, client_hello: &[u8], chain: &[u8]) -> Vec<u8> {
            let client_ephemeral = HandshakeMessage::decode(client_hello)
                .expect("hello")
                .client_hello
                .expect("client hello")
                .ephemeral
                .expect("ephemeral");
            let client_ephemeral = <[u8; 32]>::try_from(client_ephemeral.as_slice()).expect("32");
            self.symmetric.mix_hash(&client_ephemeral);

            self.symmetric.mix_hash(self.ephemeral.public());
            let shared = self.ephemeral.agree(&client_ephemeral).expect("dh");
            self.symmetric.mix_key(shared.as_slice()).expect("mix");
            let encrypted_static = self
                .symmetric
                .encrypt_and_hash(self.static_key.public())
                .expect("static");
            let shared = self.static_key.agree(&client_ephemeral).expect("dh");
            self.symmetric.mix_key(shared.as_slice()).expect("mix");
            let encrypted_chain = self.symmetric.encrypt_and_hash(chain).expect("chain");

            HandshakeMessage {
                server_hello: Some(handshake_message::ServerHello {
                    ephemeral: Some(self.ephemeral.public().to_vec()),
                    r#static: Some(encrypted_static),
                    payload: Some(encrypted_chain),
                }),
                ..Default::default()
            }
            .encode_to_vec()
        }

        /// Read the ClientFinish. Returns the client's static key, its payload, and the transport
        /// ciphers as the responder sees them: (from the client, to the client).
        fn client_finish(
            &mut self,
            finish: &[u8],
        ) -> ([u8; 32], Vec<u8>, TransportCipher, TransportCipher) {
            let message = HandshakeMessage::decode(finish)
                .expect("finish")
                .client_finish
                .expect("client finish");
            let client_static = self
                .symmetric
                .decrypt_and_hash(&message.r#static.expect("static"))
                .expect("client static");
            let client_static = <[u8; 32]>::try_from(client_static.as_slice()).expect("32");
            let shared = self.ephemeral.agree(&client_static).expect("dh");
            self.symmetric.mix_key(shared.as_slice()).expect("mix");
            let payload = self
                .symmetric
                .decrypt_and_hash(&message.payload.expect("payload"))
                .expect("payload");
            // split() yields the initiator's sending key first, then its receiving key.
            let (from_client, to_client) = self.symmetric.split().expect("split");
            (client_static, payload, from_client, to_client)
        }
    }

    #[test]
    fn full_handshake_against_an_independent_responder() {
        let server_static = KeyPair::from_private([0x33; 32]);
        let (chain, _) = chain_for(server_static.public());
        let client_static = KeyPair::from_private([0x21; 32]);
        let payload = b"client payload bytes".to_vec();

        let handshake = Handshake::new(client_static.clone(), payload.clone()).expect("start");
        let hello = handshake.client_hello();
        let mut responder = Responder::new(server_static.clone());
        let server_hello = responder.server_hello(&hello, &chain);

        let root = xeddsa::public_key(&ROOT_PRIVATE);
        let (finish, mut established) = handshake
            .finish_with_root(&server_hello, &root)
            .expect("the initiator accepts the chain");

        let (seen_static, seen_payload, mut from_client, mut to_client) =
            responder.client_finish(&finish);
        assert_eq!(seen_static, *client_static.public());
        assert_eq!(seen_payload, payload);

        let sent = established.write.encrypt(b"hello server").expect("encrypt");
        assert_eq!(
            from_client.decrypt(&sent).expect("decrypt"),
            b"hello server"
        );
        let reply = to_client.encrypt(b"hello client").expect("encrypt");
        assert_eq!(
            established.read.decrypt(&reply).expect("decrypt"),
            b"hello client"
        );
        assert_eq!(established.cert_chain.leaf_key, *server_static.public());
    }

    #[test]
    fn handshake_refuses_a_chain_under_another_root() {
        let server_static = KeyPair::from_private([0x33; 32]);
        let (chain, _) = chain_for(server_static.public());
        let handshake =
            Handshake::new(KeyPair::from_private([0x21; 32]), b"p".to_vec()).expect("start");
        let hello = handshake.client_hello();
        let server_hello = Responder::new(server_static).server_hello(&hello, &chain);
        // The production root is not the test root, so the chain must be refused.
        assert!(handshake.finish(&server_hello).is_err());
    }

    #[test]
    fn handshake_refuses_a_malformed_server_hello() {
        let fresh =
            || Handshake::new(KeyPair::generate().expect("key"), Vec::new()).expect("start");
        assert!(fresh().finish(&[0xFF; 8]).is_err());
        assert!(
            fresh()
                .finish(&HandshakeMessage::default().encode_to_vec())
                .is_err()
        );
    }
}
