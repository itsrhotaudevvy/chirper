//! App state: the synced collections (contacts, settings, chat metadata), decrypted and verified.
//!
//! The wire format and the checks follow whatsmeow's `appstate/` package (Mozilla Public License
//! 2.0): key expansion from the sync key, the LTHash over value MACs, the snapshot and patch MACs,
//! each mutation's value MAC (HMAC-SHA512, truncated) and index MAC (HMAC-SHA256), and AES-256-CBC
//! for the action itself. A collection whose key we do not have yet is skipped, not an error: the
//! phone sends the key later and the collection is synced again.
//!
//! The bridge model has no place for mute, pin or archive state, so those actions are verified and
//! dropped. Contact names and the account's own push name are kept.

use chirper_bridge::model::{ChatMeta, RemoteUser};
use chirper_bridge::{Error, Event, Result};
use prost::Message as _;

use crate::binary::{Jid, Node};
use crate::crypto::{constant_time_eq, hkdf, hmac256, hmac512};
use crate::ids;
use crate::proto::e2e::AppStateSyncKeyShare;
use crate::proto::sync::{SyncActionData, SyncdMutation, SyncdPatch, SyncdSnapshot, operation};
use crate::session::{Client, SERVER_JID};
use crate::transport::Wire;

/// The collections, in the order whatsmeow syncs them.
pub const COLLECTIONS: [&str; 5] = [
    "critical_block",
    "critical_unblock_low",
    "regular_high",
    "regular",
    "regular_low",
];

/// Patches fetched per collection per sync round, a bound on a misbehaving server.
const MAX_ROUNDS: usize = 16;

/// The five keys an app-state key expands into.
#[derive(Clone)]
pub struct ExpandedKeys {
    pub index: [u8; 32],
    pub value_encryption: [u8; 32],
    pub value_mac: [u8; 32],
    pub snapshot_mac: [u8; 32],
    pub patch_mac: [u8; 32],
}

/// Expands a sync key: 160 bytes of HKDF-SHA256 with the `WhatsApp Mutation Keys` info.
pub fn expand_key(key_data: &[u8]) -> Result<ExpandedKeys> {
    let material = hkdf(key_data, b"WhatsApp Mutation Keys", 160)?;
    let take = |start: usize| -> Result<[u8; 32]> {
        material
            .get(start..start + 32)
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .ok_or_else(|| Error::Internal("app state key expansion".to_owned()))
    };
    Ok(ExpandedKeys {
        index: take(0)?,
        value_encryption: take(32)?,
        value_mac: take(64)?,
        snapshot_mac: take(96)?,
        patch_mac: take(128)?,
    })
}

/// The LTHash of a collection: 128 bytes that summarise its value MACs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HashState {
    pub version: u64,
    pub hash: [u8; 128],
}

impl Default for HashState {
    fn default() -> Self {
        Self {
            version: 0,
            hash: [0; 128],
        }
    }
}

/// Adds and subtracts items in the LTHash: each item is expanded to 128 bytes and added or
/// subtracted pointwise as little-endian 16-bit words, with wrap-around.
pub fn lt_hash(hash: &mut [u8; 128], subtract: &[Vec<u8>], add: &[Vec<u8>]) -> Result<()> {
    for (items, negative) in [(subtract, true), (add, false)] {
        for item in items {
            let expanded = hkdf(item, b"WhatsApp Patch Integrity", 128)?;
            let (words, _) = hash.as_chunks_mut::<2>();
            let (deltas, _) = expanded.as_chunks::<2>();
            for (word, delta) in words.iter_mut().zip(deltas) {
                let x = u16::from_le_bytes(*word);
                let y = u16::from_le_bytes(*delta);
                let result = if negative {
                    x.wrapping_sub(y)
                } else {
                    x.wrapping_add(y)
                };
                *word = result.to_le_bytes();
            }
        }
    }
    Ok(())
}

/// The snapshot MAC: HMAC-SHA256 of the hash, the version and the collection name.
pub fn snapshot_mac(keys: &ExpandedKeys, state: &HashState, name: &str) -> Result<[u8; 32]> {
    hmac256(
        &keys.snapshot_mac,
        &[&state.hash, &state.version.to_be_bytes(), name.as_bytes()],
    )
}

/// The patch MAC: HMAC-SHA256 of the snapshot MAC, each value MAC, the version and the name.
pub fn patch_mac(
    keys: &ExpandedKeys,
    patch: &SyncdPatch,
    version: u64,
    name: &str,
) -> Result<[u8; 32]> {
    let snapshot = patch.snapshot_mac.clone().unwrap_or_default();
    let value_macs: Vec<Vec<u8>> = patch.mutations.iter().map(value_mac_of).collect();
    let mut parts: Vec<&[u8]> = vec![&snapshot];
    parts.extend(value_macs.iter().map(Vec::as_slice));
    let version_bytes = version.to_be_bytes();
    parts.push(&version_bytes);
    parts.push(name.as_bytes());
    hmac256(&keys.patch_mac, &parts)
}

/// The last 32 bytes of a mutation's value blob: its value MAC.
fn value_mac_of(mutation: &SyncdMutation) -> Vec<u8> {
    let blob = mutation
        .record
        .as_ref()
        .and_then(|record| record.value.as_ref())
        .and_then(|value| value.blob.clone())
        .unwrap_or_default();
    blob.get(blob.len().saturating_sub(32)..)
        .unwrap_or_default()
        .to_vec()
}

/// One decrypted and verified mutation.
#[derive(Debug)]
pub struct Decoded {
    pub operation: i32,
    pub index: Vec<String>,
    pub index_mac: Vec<u8>,
    pub value_mac: Vec<u8>,
    pub action: SyncActionData,
}

/// Decrypts and verifies one mutation with `keys`.
pub fn decode_mutation(
    keys: &ExpandedKeys,
    key_id: &[u8],
    mutation: &SyncdMutation,
) -> Result<Decoded> {
    let op = mutation.operation.unwrap_or(operation::SET);
    let record = mutation
        .record
        .as_ref()
        .ok_or_else(|| Error::Protocol("app state: a mutation has no record".to_owned()))?;
    let blob = record
        .value
        .as_ref()
        .and_then(|value| value.blob.as_deref())
        .ok_or_else(|| Error::Protocol("app state: a mutation has no value".to_owned()))?;
    let split = blob
        .len()
        .checked_sub(32)
        .filter(|len| *len > 16)
        .ok_or_else(|| Error::Protocol("app state: a mutation value is too short".to_owned()))?;
    let (content, value_mac) = blob.split_at(split);
    let operation_byte = [u8::try_from(op + 1).unwrap_or(1)];
    let key_len = u64::try_from(key_id.len() + 1).unwrap_or(1).to_be_bytes();
    let expected = hmac512(
        &keys.value_mac,
        &[&operation_byte, key_id, content, &key_len],
    )?;
    if !constant_time_eq(&expected[..32], value_mac) {
        return Err(Error::Protocol(
            "app state: a value MAC does not verify".to_owned(),
        ));
    }
    let (iv, ciphertext) = content.split_at(16);
    let iv: [u8; 16] = iv
        .try_into()
        .map_err(|_| Error::Protocol("app state: a value IV is malformed".to_owned()))?;
    let plaintext =
        chirper_signal_protocol::cipher::aes_cbc_decrypt(&keys.value_encryption, &iv, ciphertext)
            .map_err(|_| Error::Protocol("app state: a value does not decrypt".to_owned()))?;
    let action = SyncActionData::decode(plaintext.as_slice())
        .map_err(|_| Error::Protocol("app state: an action is malformed".to_owned()))?;
    let index_bytes = action.index.clone().unwrap_or_default();
    let index_mac = record
        .index
        .as_ref()
        .and_then(|index| index.blob.clone())
        .unwrap_or_default();
    let expected_index = hmac256(&keys.index, &[&index_bytes])?;
    if !constant_time_eq(&expected_index, &index_mac) {
        return Err(Error::Protocol(
            "app state: an index MAC does not verify".to_owned(),
        ));
    }
    let index: Vec<String> = serde_json::from_slice(&index_bytes)
        .map_err(|_| Error::Protocol("app state: an index is not a JSON array".to_owned()))?;
    Ok(Decoded {
        operation: op,
        index,
        index_mac,
        value_mac: value_mac.to_vec(),
        action,
    })
}

fn state_secret(name: &str) -> String {
    format!("appstate/{name}")
}

fn key_secret(key_id: &[u8]) -> String {
    format!("appstate-key/{}", hex::encode(key_id))
}

fn mac_secret(name: &str, index_mac: &[u8]) -> String {
    format!("appstate-mac/{name}/{}", hex::encode(index_mac))
}

impl<W: Wire> Client<W> {
    /// Stores the sync keys the phone shared.
    pub(crate) fn store_app_state_keys(&mut self, share: &AppStateSyncKeyShare) -> Result<()> {
        for key in &share.keys {
            let (Some(id), Some(data)) = (
                key.key_id.as_ref().and_then(|id| id.key_id.as_deref()),
                key.key_data.as_ref(),
            ) else {
                continue;
            };
            let Some(material) = data.key_data.as_deref() else {
                continue;
            };
            let mut record = data.timestamp.unwrap_or(0).to_be_bytes().to_vec();
            record.extend_from_slice(material);
            self.ctx.set_secret(&key_secret(id), &record)?;
        }
        Ok(())
    }

    fn load_key(&self, key_id: &[u8]) -> Result<Option<ExpandedKeys>> {
        match self.ctx.get_secret(&key_secret(key_id))? {
            None => Ok(None),
            Some(record) => {
                let material = record.get(8..).unwrap_or_default();
                expand_key(material).map(Some)
            }
        }
    }

    fn load_state(&self, name: &str) -> Result<HashState> {
        match self.ctx.get_secret(&state_secret(name))? {
            None => Ok(HashState::default()),
            Some(bytes) if bytes.len() == 136 => {
                let mut version = [0u8; 8];
                version.copy_from_slice(&bytes[..8]);
                let mut hash = [0u8; 128];
                hash.copy_from_slice(&bytes[8..]);
                Ok(HashState {
                    version: u64::from_be_bytes(version),
                    hash,
                })
            }
            Some(_) => Ok(HashState::default()),
        }
    }

    fn save_state(&self, name: &str, state: &HashState) -> Result<()> {
        let mut bytes = state.version.to_be_bytes().to_vec();
        bytes.extend_from_slice(&state.hash);
        self.ctx.set_secret(&state_secret(name), &bytes)
    }

    /// Syncs one collection: its snapshot if it has none yet, then its patches.
    pub(crate) async fn sync_collection(&mut self, name: &str) -> Result<()> {
        for _ in 0..MAX_ROUNDS {
            let state = self.load_state(name)?;
            let id = self.next_id();
            let collection = Node::new("collection")
                .attr("name", name)
                .attr("version", state.version.to_string())
                .attr(
                    "return_snapshot",
                    if state.version == 0 { "true" } else { "false" },
                );
            let request = Node::new("iq")
                .attr("to", SERVER_JID)
                .attr("type", "set")
                .attr("xmlns", "w:sync:app:state")
                .attr("id", id)
                .children(vec![Node::new("sync").children(vec![collection])]);
            let reply = self.request(request).await?;
            let collection = reply
                .child("sync")
                .and_then(|sync| sync.child("collection"))
                .ok_or_else(|| {
                    Error::Protocol("app state: no collection in the answer".to_owned())
                })?;
            if let Some(snapshot) = collection.child("snapshot").and_then(Node::content_bytes) {
                let snapshot = SyncdSnapshot::decode(snapshot).map_err(|_| {
                    Error::Protocol("app state: the snapshot is malformed".to_owned())
                })?;
                if !self.apply_snapshot(name, &snapshot)? {
                    return Ok(());
                }
            }
            let patches: Vec<Vec<u8>> = collection
                .child("patches")
                .map(|patches| {
                    patches
                        .child_nodes()
                        .iter()
                        .filter(|node| node.tag == "patch")
                        .filter_map(|node| node.content_bytes().map(<[u8]>::to_vec))
                        .collect()
                })
                .unwrap_or_default();
            for bytes in &patches {
                let patch = SyncdPatch::decode(bytes.as_slice())
                    .map_err(|_| Error::Protocol("app state: a patch is malformed".to_owned()))?;
                if patch.external_mutations.is_some() {
                    log::debug!("app state {name}: external mutations are not downloaded");
                    return Ok(());
                }
                if !self.apply_patch(name, &patch)? {
                    return Ok(());
                }
            }
            let more = collection.get_attr("has_more_patches").as_deref() == Some("true");
            if !more {
                return Ok(());
            }
        }
        Ok(())
    }

    /// Applies a snapshot. `Ok(false)` when its key is missing.
    fn apply_snapshot(&mut self, name: &str, snapshot: &SyncdSnapshot) -> Result<bool> {
        let key_id = snapshot
            .key_id
            .as_ref()
            .and_then(|id| id.id.clone())
            .unwrap_or_default();
        let Some(keys) = self.load_key(&key_id)? else {
            return Ok(false);
        };
        let version = snapshot
            .version
            .as_ref()
            .and_then(|v| v.version)
            .unwrap_or(0);
        let mut state = HashState {
            version,
            hash: [0; 128],
        };
        let mut decoded = Vec::with_capacity(snapshot.records.len());
        for record in &snapshot.records {
            let mutation = SyncdMutation {
                operation: Some(operation::SET),
                record: Some(record.clone()),
            };
            let item = decode_mutation(&keys, &key_id, &mutation)?;
            decoded.push((mutation, item));
        }
        let added: Vec<Vec<u8>> = decoded.iter().map(|(m, _)| value_mac_of(m)).collect();
        lt_hash(&mut state.hash, &[], &added)?;
        let expected = snapshot.mac.clone().unwrap_or_default();
        if !constant_time_eq(&snapshot_mac(&keys, &state, name)?, &expected) {
            return Err(Error::Protocol(
                "app state: the snapshot MAC does not verify".to_owned(),
            ));
        }
        for (_, item) in &decoded {
            self.ctx
                .set_secret(&mac_secret(name, &item.index_mac), &item.value_mac)?;
            self.note_action(item);
        }
        self.save_state(name, &state)?;
        Ok(true)
    }

    /// Applies one patch. `Ok(false)` when its key is missing.
    fn apply_patch(&mut self, name: &str, patch: &SyncdPatch) -> Result<bool> {
        let key_id = patch
            .key_id
            .as_ref()
            .and_then(|id| id.id.clone())
            .unwrap_or_default();
        let Some(keys) = self.load_key(&key_id)? else {
            return Ok(false);
        };
        let current = self.load_state(name)?;
        let version = patch.version.as_ref().and_then(|v| v.version).unwrap_or(0);
        let mut next = HashState {
            version,
            hash: current.hash,
        };

        // The previous value MAC of each index: from this patch first, then from storage.
        let mut removed: Vec<Vec<u8>> = Vec::new();
        let mut added: Vec<Vec<u8>> = Vec::new();
        for (i, mutation) in patch.mutations.iter().enumerate() {
            let index_mac = mutation
                .record
                .as_ref()
                .and_then(|record| record.index.as_ref())
                .and_then(|index| index.blob.clone())
                .unwrap_or_default();
            if mutation.operation.unwrap_or(operation::SET) == operation::SET {
                added.push(value_mac_of(mutation));
            }
            if let Some(previous) = self.previous_value_mac(name, patch, i, &index_mac)? {
                removed.push(previous);
            }
        }
        lt_hash(&mut next.hash, &removed, &added)?;

        let expected_snapshot = patch.snapshot_mac.clone().unwrap_or_default();
        if !constant_time_eq(&snapshot_mac(&keys, &next, name)?, &expected_snapshot) {
            return Err(Error::Protocol(
                "app state: the snapshot MAC of a patch does not verify".to_owned(),
            ));
        }
        let expected_patch = patch.patch_mac.clone().unwrap_or_default();
        if !constant_time_eq(&patch_mac(&keys, patch, version, name)?, &expected_patch) {
            return Err(Error::Protocol(
                "app state: the patch MAC does not verify".to_owned(),
            ));
        }

        for mutation in &patch.mutations {
            let item = decode_mutation(&keys, &key_id, mutation)?;
            if item.operation == operation::SET {
                self.ctx
                    .set_secret(&mac_secret(name, &item.index_mac), &item.value_mac)?;
                self.note_action(&item);
            } else {
                self.ctx.delete_secret(&mac_secret(name, &item.index_mac))?;
            }
        }
        self.save_state(name, &next)?;
        Ok(true)
    }

    /// The value MAC an index held before mutation `i` of `patch`, if it held one.
    fn previous_value_mac(
        &self,
        name: &str,
        patch: &SyncdPatch,
        i: usize,
        index_mac: &[u8],
    ) -> Result<Option<Vec<u8>>> {
        for earlier in patch.mutations.iter().take(i).rev() {
            let earlier_index = earlier
                .record
                .as_ref()
                .and_then(|record| record.index.as_ref())
                .and_then(|index| index.blob.as_deref())
                .unwrap_or_default();
            if earlier_index == index_mac {
                return Ok(match earlier.operation.unwrap_or(operation::SET) {
                    operation::SET => Some(value_mac_of(earlier)),
                    _ => None,
                });
            }
        }
        self.ctx
            .get_secret(&mac_secret(name, index_mac))
            .map(|value| value.map(|bytes| bytes.to_vec()))
    }

    /// Reports what an action means for the bridge: contact names and the account's push name.
    fn note_action(&mut self, item: &Decoded) {
        if item.operation != operation::SET {
            return;
        }
        let Some(value) = item.action.value.as_ref() else {
            return;
        };
        match item.index.first().map(String::as_str) {
            Some("contact") => {
                let Some(jid_text) = item.index.get(1) else {
                    return;
                };
                let Ok(jid) = jid_text.parse::<Jid>() else {
                    return;
                };
                let Some(contact) = value.contact_action.as_ref() else {
                    return;
                };
                let name = contact
                    .full_name
                    .clone()
                    .or_else(|| contact.first_name.clone())
                    .unwrap_or_default();
                if name.is_empty() {
                    return;
                }
                let user_id = ids::user_id(&jid);
                self.names.insert(user_id.clone(), name.clone());
                self.events.push_back(Event::Chat(ChatMeta {
                    portal: self.portal(&jid),
                    name: name.clone(),
                    is_dm: true,
                    dm_user: Some(RemoteUser {
                        id: user_id,
                        display_name: name,
                        phone: jid.user.clone(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }));
            }
            Some("setting_pushName") => {
                if let Some(name) = value
                    .push_name_setting
                    .as_ref()
                    .and_then(|s| s.name.clone())
                {
                    self.push_name = name;
                }
            }
            _ => {}
        }
    }
}
