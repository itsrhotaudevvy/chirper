//! One-time prekey maintenance: count on the server, top up the store, upload the batch.
//!
//! Ported from signalmeow's `keys.go` (AGPL-3.0): the server keeps up to 100 one-time EC prekeys
//! and 100 one-time Kyber prekeys per account. When the server count falls below 50, the store is
//! topped up to 100 and the most recent 100 of each are uploaded with the identity key.

use rand::rngs::OsRng;
use serde::Deserialize;
use serde_json::{Value, json};

use chirper_bridge::{Error, Result};
use chirper_signal_protocol::identity::IdentityKeyPair;
use chirper_signal_protocol::prekey::{generate_kyber_pre_key, generate_pre_keys};
use chirper_signal_protocol::store::{KyberPreKeyStore, PreKeyStore};

use crate::keystore::KeyStore;
use crate::provision::{kyber_prekey_json, proto};
use crate::session::Shared;

/// The number of one-time prekeys of each kind the store aims to hold.
pub const PREKEY_BATCH: usize = 100;
/// Below this server count, a new batch is uploaded.
pub const PREKEY_LOW_WATER: u64 = 50;

/// The `GET /v2/keys` answer.
#[derive(Debug, Deserialize)]
struct KeyCounts {
    count: u64,
    #[serde(rename = "pqCount")]
    pq_count: u64,
}

/// Whether a batch must be uploaded given the server's count.
pub fn needs_upload(server_count: u64) -> bool {
    server_count < PREKEY_LOW_WATER
}

/// Tops up the store to one full batch of each one-time key kind.
pub fn top_up(store: &mut KeyStore, identity: &IdentityKeyPair, now_ms: u64) -> Result<()> {
    let mut rng = OsRng;
    let have = store.pre_key_count();
    if have < PREKEY_BATCH {
        let start = store.max_pre_key_id() + 1;
        let count = u32::try_from(PREKEY_BATCH - have)
            .map_err(|_| Error::Internal("a prekey batch is too large".to_owned()))?;
        for record in generate_pre_keys(start, count, &mut rng).map_err(proto)? {
            store.store_pre_key(&record).map_err(|_| store_error())?;
        }
    }
    let have_kyber = store.kyber_pre_key_count();
    if have_kyber < PREKEY_BATCH {
        let start = store.max_kyber_pre_key_id() + 1;
        for offset in 0..(PREKEY_BATCH - have_kyber) {
            let id = start
                .checked_add(u32::try_from(offset).unwrap_or(u32::MAX))
                .ok_or_else(|| Error::Internal("Kyber prekey ids are exhausted".to_owned()))?;
            let record = generate_kyber_pre_key(identity, id, now_ms, &mut rng).map_err(proto)?;
            store.store_kyber_pre_key(&record).map_err(|_| store_error())?;
        }
    }
    Ok(())
}

fn store_error() -> Error {
    Error::Internal("the key store could not be written".to_owned())
}

/// The JSON body of `PUT /v2/keys`: the most recent batch of each kind, plus the identity key.
pub fn upload_body(store: &KeyStore, identity: &IdentityKeyPair) -> Value {
    use base64::Engine;
    let pre_keys: Vec<Value> = {
        let mut records = store.pre_key_records();
        records.sort_by_key(|r| r.id());
        let skip = records.len().saturating_sub(PREKEY_BATCH);
        records
            .iter()
            .skip(skip)
            .map(|r| {
                json!({
                    "keyId": r.id(),
                    "publicKey": base64::engine::general_purpose::STANDARD.encode(r.key_pair().public_key.serialize()),
                })
            })
            .collect()
    };
    let kyber: Vec<Value> = {
        let mut records = store.kyber_records();
        records.sort_by_key(|r| r.id());
        let skip = records.len().saturating_sub(PREKEY_BATCH);
        records.iter().skip(skip).map(kyber_prekey_json).collect()
    };
    json!({
        "preKeys": pre_keys,
        "pqPreKeys": kyber,
        "identityKey": base64::engine::general_purpose::STANDARD.encode(identity.identity_key().serialize()),
    })
}

/// Checks the server's count, tops the store up when it is low and uploads the batch.
/// Returns whether anything was uploaded.
pub async fn refresh(shared: &Shared, now_ms: u64) -> Result<bool> {
    let ws = &shared.ws;
    let identity = &shared.identity;
    let response = ws.request("GET", "/v2/keys?identity=aci", None, Vec::new()).await?;
    ensure_ok(response.status)?;
    let counts: KeyCounts = serde_json::from_slice(response.body.as_deref().unwrap_or_default())
        .map_err(|_| Error::Protocol("the key count is not JSON".to_owned()))?;
    if !needs_upload(counts.count) && !needs_upload(counts.pq_count) {
        return Ok(false);
    }
    let body = shared.with_store(|store| {
        top_up(store, identity, now_ms)?;
        Ok(upload_body(store, identity))
    })?;
    let payload = serde_json::to_vec(&body).map_err(|_| Error::Internal("encoding prekeys".to_owned()))?;
    let response = ws
        .request(
            "PUT",
            "/v2/keys?identity=aci",
            Some(payload),
            vec!["content-type:application/json".to_owned()],
        )
        .await?;
    ensure_ok(response.status)?;
    Ok(true)
}

/// Maps a non-2xx status from the chat server to a driver error.
pub fn ensure_ok(status: Option<u32>) -> Result<()> {
    match status.unwrap_or(0) {
        200..=299 => Ok(()),
        401 | 403 => Err(Error::Auth("Signal refused these credentials".to_owned())),
        404 => Err(Error::Protocol("Signal does not know this resource".to_owned())),
        429 => Err(Error::RateLimited { retry_after_ms: 60_000 }),
        500..=599 => Err(Error::Network("Signal is unavailable".to_owned())),
        code => Err(Error::Protocol(format!("Signal answered {code}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    #[test]
    fn a_batch_is_uploaded_only_when_the_server_runs_low() {
        assert!(needs_upload(0));
        assert!(needs_upload(49));
        assert!(!needs_upload(50));
        assert!(!needs_upload(100));
    }

    #[test]
    fn top_up_fills_both_kinds_to_a_full_batch_and_is_idempotent() {
        let mut rng = StdRng::seed_from_u64(21);
        let identity = IdentityKeyPair::generate(&mut rng);
        let mut store = KeyStore::new(&identity, 5);
        top_up(&mut store, &identity, 1).expect("top up");
        assert_eq!(store.pre_key_count(), PREKEY_BATCH);
        assert_eq!(store.kyber_pre_key_count(), PREKEY_BATCH);
        assert_eq!(store.max_pre_key_id(), PREKEY_BATCH as u32);
        top_up(&mut store, &identity, 1).expect("top up again");
        assert_eq!(store.pre_key_count(), PREKEY_BATCH);
    }

    #[test]
    fn the_upload_body_has_the_reference_field_names() {
        let mut rng = StdRng::seed_from_u64(22);
        let identity = IdentityKeyPair::generate(&mut rng);
        let mut store = KeyStore::new(&identity, 5);
        top_up(&mut store, &identity, 1).expect("top up");
        let body = upload_body(&store, &identity);
        let pre = body["preKeys"].as_array().expect("array");
        assert_eq!(pre.len(), PREKEY_BATCH);
        assert!(pre[0].get("keyId").is_some() && pre[0].get("publicKey").is_some());
        let pq = body["pqPreKeys"].as_array().expect("array");
        assert!(pq[0].get("signature").is_some());
        assert!(body["identityKey"].is_string());
    }
}
