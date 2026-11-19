//! Creating an auth key with a data centre, following the MTProto "Authorization Key" steps:
//! `req_pq_multi`, `req_DH_params`, the server's DH parameters, `set_client_DH_params` and the
//! final nonce check. Every step is checked against the nonces and hashes the protocol defines.

use std::fmt;

use num_bigint::BigUint;
use zeroize::Zeroizing;

use super::constants::RSA_PROD_FINGERPRINT;
use super::crypto::{self, CryptoError, RsaPublicKey};
use super::plain;
use super::session::{msg_id_now, now_micros};
use super::transport::{Transport, TransportError};
use crate::tl::{Cursor, Deserializable, DeserializeError, Serializable, enums, functions, types};

/// How many times a `dh_gen_retry` is followed before the handshake gives up.
const MAX_DH_RETRIES: usize = 3;

/// A 2048-bit auth key. The bytes are wiped on drop and never shown by `Debug`.
#[derive(Clone)]
pub struct AuthKey {
    bytes: Zeroizing<[u8; 256]>,
}

impl AuthKey {
    pub fn from_bytes(bytes: [u8; 256]) -> Self {
        Self {
            bytes: Zeroizing::new(bytes),
        }
    }

    pub fn as_bytes(&self) -> &[u8; 256] {
        &self.bytes
    }

    /// The public `auth_key_id`, which identifies the key in every encrypted message.
    pub fn id(&self) -> u64 {
        crypto::auth_key_id(&self.bytes)
    }
}

impl fmt::Debug for AuthKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AuthKey([redacted])")
    }
}

/// The outcome of a successful handshake.
#[derive(Debug)]
pub struct AuthKeyResult {
    pub key: AuthKey,
    /// Server time minus local time, in seconds.
    pub time_offset: i64,
    /// The first server salt, derived from the nonces.
    pub server_salt: i64,
}

/// Why the handshake failed.
#[derive(Debug)]
pub enum AuthKeyError {
    Transport(TransportError),
    Crypto(CryptoError),
    Deserialize(DeserializeError),
    /// A nonce, hash or value did not match what the protocol requires.
    Protocol(&'static str),
    /// None of the server's advertised key fingerprints is one we hold.
    UnknownFingerprint,
}

impl fmt::Display for AuthKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "{e}"),
            Self::Crypto(e) => write!(f, "{e}"),
            Self::Deserialize(e) => write!(f, "{e}"),
            Self::Protocol(m) => write!(f, "auth key handshake: {m}"),
            Self::UnknownFingerprint => write!(f, "no known server key fingerprint"),
        }
    }
}

impl std::error::Error for AuthKeyError {}

impl From<TransportError> for AuthKeyError {
    fn from(e: TransportError) -> Self {
        Self::Transport(e)
    }
}

impl From<CryptoError> for AuthKeyError {
    fn from(e: CryptoError) -> Self {
        Self::Crypto(e)
    }
}

impl From<DeserializeError> for AuthKeyError {
    fn from(e: DeserializeError) -> Self {
        Self::Deserialize(e)
    }
}

/// Send one unencrypted request and read the unencrypted answer.
async fn plain_call(transport: &mut Transport, body: &[u8]) -> Result<Vec<u8>, AuthKeyError> {
    let msg_id = msg_id_now(0);
    transport.send(&plain::encode(msg_id, body)).await?;
    let frame = transport.recv().await?;
    let message = plain::decode(&frame).map_err(|_| AuthKeyError::Protocol("bad plain answer"))?;
    Ok(message.body)
}

/// The minimal big-endian bytes of `value`, as TL `bytes` expects them.
fn be_trimmed(value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let first = bytes
        .iter()
        .position(|b| *b != 0)
        .unwrap_or(bytes.len() - 1);
    bytes[first..].to_vec()
}

fn sha1_of(parts: &[&[u8]]) -> [u8; 20] {
    crypto::sha1(parts)
}

/// Run the whole handshake against `transport`, which must be connected to a production data
/// centre. `dc_id` is sent in `p_q_inner_data_dc`.
pub async fn generate_auth_key(
    transport: &mut Transport,
    dc_id: i32,
) -> Result<AuthKeyResult, AuthKeyError> {
    let nonce: [u8; 16] = crypto::random_array()?;

    // Step 1: req_pq_multi -> resPQ.
    let mut body = Vec::new();
    functions::ReqPqMulti { nonce }.serialize(&mut body);
    let answer = plain_call(transport, &body).await?;
    let enums::ResPQ::ResPQ(res_pq) =
        <enums::ResPQ as Deserializable>::deserialize(&mut Cursor::new(&answer))?;
    if res_pq.nonce != nonce {
        return Err(AuthKeyError::Protocol("resPQ nonce mismatch"));
    }
    if res_pq.pq.len() > 8 {
        return Err(AuthKeyError::Protocol("pq is too large"));
    }
    let mut pq_bytes = [0u8; 8];
    pq_bytes[8 - res_pq.pq.len()..].copy_from_slice(&res_pq.pq);
    let pq = u64::from_be_bytes(pq_bytes);
    let (p, q) = crypto::factorize(pq).ok_or(AuthKeyError::Protocol("pq does not factor"))?;
    if !res_pq
        .server_public_key_fingerprints
        .contains(&RSA_PROD_FINGERPRINT)
    {
        return Err(AuthKeyError::UnknownFingerprint);
    }
    let server_nonce = res_pq.server_nonce;
    let key = RsaPublicKey::production();
    let (p_bytes, q_bytes) = (be_trimmed(p), be_trimmed(q));

    // Step 2: req_DH_params with the RSA-padded p_q_inner_data_dc.
    let new_nonce: [u8; 32] = crypto::random_array()?;
    let mut inner = Vec::new();
    types::PQInnerDataDc {
        pq: res_pq.pq.clone(),
        p: p_bytes.clone(),
        q: q_bytes.clone(),
        nonce,
        server_nonce,
        new_nonce,
        dc: dc_id,
    }
    .serialize(&mut inner);
    let encrypted_data = crypto::rsa_pad_encrypt(&inner, &key)?;
    let mut body = Vec::new();
    functions::ReqDHParams {
        nonce,
        server_nonce,
        p: p_bytes,
        q: q_bytes,
        public_key_fingerprint: RSA_PROD_FINGERPRINT,
        encrypted_data,
    }
    .serialize(&mut body);
    let answer = plain_call(transport, &body).await?;
    let ok =
        match <enums::ServerDHParams as Deserializable>::deserialize(&mut Cursor::new(&answer))? {
            enums::ServerDHParams::ServerDHParamsOk(ok) => ok,
            enums::ServerDHParams::ServerDHParamsFail(_) => {
                return Err(AuthKeyError::Protocol("server refused the DH parameters"));
            }
        };
    if ok.nonce != nonce || ok.server_nonce != server_nonce {
        return Err(AuthKeyError::Protocol("server_DH_params nonce mismatch"));
    }

    // Step 3: decrypt encrypted_answer with the temporary key and IV derived from the nonces.
    let s_new_server = sha1_of(&[&new_nonce, &server_nonce]);
    let s_server_new = sha1_of(&[&server_nonce, &new_nonce]);
    let s_new_new = sha1_of(&[&new_nonce, &new_nonce]);
    let mut tmp_key = [0u8; 32];
    tmp_key[..20].copy_from_slice(&s_new_server);
    tmp_key[20..].copy_from_slice(&s_server_new[..12]);
    let mut tmp_iv = [0u8; 32];
    tmp_iv[..8].copy_from_slice(&s_server_new[12..20]);
    tmp_iv[8..28].copy_from_slice(&s_new_new);
    tmp_iv[28..].copy_from_slice(&new_nonce[..4]);
    let mut answer = ok.encrypted_answer.clone();
    crypto::ige_decrypt(&tmp_key, &tmp_iv, &mut answer)?;

    // The answer is SHA1(inner) || inner || padding (under 16 bytes).
    let digest = answer
        .get(..20)
        .ok_or(AuthKeyError::Protocol("short answer"))?
        .to_vec();
    let tail = answer
        .get(20..)
        .ok_or(AuthKeyError::Protocol("short answer"))?;
    let mut cursor = Cursor::new(tail);
    let dh = <types::ServerDHInnerData as Deserializable>::deserialize(&mut cursor)?;
    let consumed = cursor.position();
    let inner_bytes = tail
        .get(..consumed)
        .ok_or(AuthKeyError::Protocol("short answer"))?;
    if sha1_of(&[inner_bytes])[..] != digest[..] {
        return Err(AuthKeyError::Protocol("server_DH_inner_data hash mismatch"));
    }
    if tail.len() - consumed >= 16 {
        return Err(AuthKeyError::Protocol("too much padding in server answer"));
    }
    if dh.nonce != nonce || dh.server_nonce != server_nonce {
        return Err(AuthKeyError::Protocol(
            "server_DH_inner_data nonce mismatch",
        ));
    }
    let prime = crypto::check_dh_params(&dh.dh_prime, dh.g)?;
    let g_a = BigUint::from_bytes_be(&dh.g_a);
    crypto::check_dh_value(&g_a, &prime)?;
    let time_offset = i64::from(dh.server_time) - (now_micros() / 1_000_000) as i64;

    // Steps 4 and 5: pick b, compute g_b and the auth key, send client_DH_inner_data. A
    // dh_gen_retry repeats this with retry_id set to the previous auth_key_aux_hash.
    let mut retry_id: i64 = 0;
    for _ in 0..=MAX_DH_RETRIES {
        let b_bytes: [u8; 256] = crypto::random_array()?;
        let b = BigUint::from_bytes_be(&b_bytes);
        let g_b = BigUint::from(dh.g as u32).modpow(&b, &prime);
        crypto::check_dh_value(&g_b, &prime)?;
        let auth = Zeroizing::new(crypto::to_fixed_be(&g_a.modpow(&b, &prime), 256)?);
        let mut auth_key = Zeroizing::new([0u8; 256]);
        auth_key.copy_from_slice(&auth);
        let aux = sha1_of(&[&auth_key[..]]);
        let aux_hash: [u8; 8] = aux[..8]
            .try_into()
            .map_err(|_| AuthKeyError::Protocol("hash"))?;

        let mut client_inner = Vec::new();
        types::ClientDHInnerData {
            nonce,
            server_nonce,
            retry_id,
            g_b: g_b.to_bytes_be(),
        }
        .serialize(&mut client_inner);
        let mut data_with_hash = Vec::with_capacity(client_inner.len() + 36);
        data_with_hash.extend_from_slice(&sha1_of(&[&client_inner]));
        data_with_hash.extend_from_slice(&client_inner);
        let pad = (16 - data_with_hash.len() % 16) % 16;
        let mut random_pad = vec![0u8; pad];
        getrandom::fill(&mut random_pad).map_err(|_| CryptoError::Random)?;
        data_with_hash.extend_from_slice(&random_pad);
        crypto::ige_encrypt(&tmp_key, &tmp_iv, &mut data_with_hash)?;

        let mut body = Vec::new();
        functions::SetClientDHParams {
            nonce,
            server_nonce,
            encrypted_data: data_with_hash,
        }
        .serialize(&mut body);
        let answer = plain_call(transport, &body).await?;
        let hash_for = |n: u8| -> [u8; 16] {
            let digest = sha1_of(&[&new_nonce, &[n], &aux_hash]);
            let mut out = [0u8; 16];
            out.copy_from_slice(&digest[4..20]);
            out
        };
        match <enums::SetClientDHParamsAnswer as Deserializable>::deserialize(&mut Cursor::new(
            &answer,
        ))? {
            enums::SetClientDHParamsAnswer::DhGenOk(ok) => {
                if ok.nonce != nonce || ok.server_nonce != server_nonce {
                    return Err(AuthKeyError::Protocol("dh_gen_ok nonce mismatch"));
                }
                if ok.new_nonce_hash1 != hash_for(1) {
                    return Err(AuthKeyError::Protocol("new_nonce_hash1 mismatch"));
                }
                let mut salt = [0u8; 8];
                for (i, b) in salt.iter_mut().enumerate() {
                    *b = new_nonce[i] ^ server_nonce[i];
                }
                return Ok(AuthKeyResult {
                    key: AuthKey::from_bytes(*auth_key),
                    time_offset,
                    server_salt: i64::from_le_bytes(salt),
                });
            }
            enums::SetClientDHParamsAnswer::DhGenRetry(retry) => {
                if retry.nonce != nonce || retry.server_nonce != server_nonce {
                    return Err(AuthKeyError::Protocol("dh_gen_retry nonce mismatch"));
                }
                if retry.new_nonce_hash2 != hash_for(2) {
                    return Err(AuthKeyError::Protocol("new_nonce_hash2 mismatch"));
                }
                retry_id = i64::from_le_bytes(aux_hash);
            }
            enums::SetClientDHParamsAnswer::DhGenFail(fail) => {
                if fail.nonce != nonce || fail.server_nonce != server_nonce {
                    return Err(AuthKeyError::Protocol("dh_gen_fail nonce mismatch"));
                }
                if fail.new_nonce_hash3 != hash_for(3) {
                    return Err(AuthKeyError::Protocol("new_nonce_hash3 mismatch"));
                }
                return Err(AuthKeyError::Protocol("server failed to generate the key"));
            }
        }
    }
    Err(AuthKeyError::Protocol("too many DH retries"))
}
