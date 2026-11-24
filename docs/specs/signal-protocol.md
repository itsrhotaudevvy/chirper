# Task `signal-protocol`: finish `crates/chirper-signal-protocol`

**Owned:** `crates/chirper-signal-protocol/**` (and nothing else).
**Consumers (later tasks):** WhatsApp (protocol v3, no post-quantum), Signal (v4, PQXDH with
Kyber1024), Messenger E2EE (same as WhatsApp).

## Starting point

Already written and compiling: `curve.rs` (Curve25519 keys with the 0x05 type byte, XEdDSA
signatures, agreement), `kem.rs` (Kyber768/1024 + ML-KEM via `libcrux-ml-kem`), `error.rs`,
`ProtocolAddress` in `lib.rs`. `protocol.rs` is an empty placeholder. Keep their public APIs; you may
extend them.

## References

- Primary (exact wire behaviour WhatsApp uses): `~/go/pkg/mod/go.mau.fi/libsignal@v0.2.3-0.20261103023201-5ce3f7a54370/`
  - `serialize/WhisperTextProtocol.proto`, `serialize/LocalStorageProtocol.proto` (field numbers)
  - `ratchet/`, `session/`, `state/record/`, `protocol/`, `groups/`, `kdf/`, `keys/`, `cipher/`
  - `tests/*_test.go` (port each scenario as a Rust test)
- Signal's v4 / PQXDH (Kyber prekeys, `kyber_pre_key_id`/`kyber_ciphertext` fields 7/8 of
  `PreKeySignalMessage`, the extra KEM shared secret appended to the X3DH secret, HKDF info
  `"WhisperText_X25519_SHA-256_CRYSTALS-KYBER-1024"`): see how
  `/Volumes/SSD/Code/Chirper/third_party/signal/pkg/libsignalgo/` drives it and the PQXDH paper
  (https://signal.org/docs/specifications/pqxdh/ — read from memory; no network needed).
  Message version 4 is used exactly when the session was set up with a Kyber prekey.
- Out of scope: SPQR / the triple ratchet, `fingerprint`, `provision`.

## Modules to add

| module | contents |
| --- | --- |
| `proto` | `prost` derive structs (hand-written, no build script) for `SignalMessage`, `PreKeySignalMessage`, `SenderKeyMessage`, `SenderKeyDistributionMessage`, and storage `SessionStructure` (+ `Chain`, `ChainKey`, `MessageKey`, `PendingPreKey`, `PendingKyberPreKey`), `RecordStructure`, `PreKeyRecordStructure`, `SignedPreKeyRecordStructure`, `KyberPreKeyRecordStructure`, `SenderKeyStateStructure`, `SenderKeyRecordStructure`. Use libsignal's field numbers so records interoperate. |
| `identity` | `IdentityKey` (wraps `PublicKey`), `IdentityKeyPair` (serialize/deserialize as `IdentityKeyPairStructure`). |
| `kdf` | `HKDF-SHA256` helpers; `RootKey`, `ChainKey` (`HMAC(key, 0x01)` message seed, `HMAC(key, 0x02)` next), `MessageKeys` (HKDF info `"WhisperMessageKeys"` → 32 cipher, 32 MAC, 16 IV). |
| `prekey` | `PreKeyRecord`, `SignedPreKeyRecord` (with XEdDSA signature + timestamp), `KyberPreKeyRecord` (signed), `PreKeyBundle` (registration id, device id, optional one-time prekey, signed prekey + signature, identity key, optional Kyber prekey + signature). Generation helpers. |
| `ratchet` | X3DH/PQXDH for both roles (`initialize_alice_session`, `initialize_bob_session`), with the `0xFF × 32` discontinuity prefix, info `"WhisperText"` (v3) or the PQXDH info (v4), root/chain derivation `"WhisperRatchet"`. |
| `session` | `SessionState` / `SessionRecord`: current session + up to 40 archived previous states; up to 5 receiver chains; up to 2000 stored skipped message keys per chain; `MAX_FORWARD_JUMPS = 25_000`; pending prekey bookkeeping; serialize to/from `RecordStructure`. |
| `message` | `SignalMessage` (version byte `(v << 4) | CURRENT`, protobuf body, 8-byte truncated `HMAC-SHA256(mac_key, sender_identity ‖ receiver_identity ‖ version ‖ body)`), `PreKeySignalMessage`, `CiphertextMessage` enum, `SenderKeyMessage` (signed with the sender signing key), `SenderKeyDistributionMessage`. Strict parsing: unknown versions, short buffers, bad MACs are `Error`s. |
| `store` | Synchronous traits: `IdentityKeyStore` (own pair, own registration id, `save_identity`, `is_trusted_identity`, `get_identity`), `PreKeyStore`, `SignedPreKeyStore`, `KyberPreKeyStore` (incl. `mark_used`), `SessionStore`, `SenderKeyStore`. All return `Result<_, StoreError>`. Plus `InMemory*` implementations behind `pub mod memory` (used by tests and handy for drivers). |
| `session_builder` | `process_prekey_bundle` (Alice; verifies signed-prekey and Kyber-prekey signatures; untrusted identity → error) and the Bob side inside decryption. |
| `cipher` | `encrypt(store…, address, plaintext) -> CiphertextMessage`, `decrypt_prekey_message`, `decrypt_signal_message`. AES-256-CBC with PKCS#7 (`cbc` + `aes`). Try the current state, then archived states, without mutating on failure (clone, then commit). Duplicate message → `Error::DuplicatedMessage`. Consumed one-time prekeys are removed. |
| `group` | Sender keys: `SenderKeyRecord` (up to 5 states), chain key iteration (`HMAC(key,0x01)`/`0x02`), message keys via HKDF info `"WhisperGroup"`, `create_sender_key_distribution_message`, `process_sender_key_distribution_message`, `group_encrypt`, `group_decrypt` (skipped-iteration keys up to 2000, forward jump cap 25_000). |
| `sealed_sender` | Signal only: `UnidentifiedSenderMessage` v1 decrypt (ephemeral → static → message layers as in libsignal), `ServerCertificate`/`SenderCertificate` parsing and validation against a trust root, and v1 encrypt. Do this **last**; if time runs out, leave the module out entirely and say so (never a stub). |

Use `getrandom` (already in the workspace lockfile at `=0.4.3`) or `rand` `=0.8.8` with `OsRng`;
all functions that need randomness take `&mut (impl CryptoRng + RngCore)` so tests are
deterministic with a seeded RNG (`rand_chacha` is not in the lockfile: use `rand::rngs::StdRng`
via `rand::SeedableRng`).

Secrets: every private key, root/chain/message key buffer is `Zeroizing` or derives `ZeroizeOnDrop`;
`Debug` prints `[redacted]`. Never log names or plaintext.

## Tests (required)

1. Every scenario in the Go `tests/` directory that is in scope: basic session (Alice↔Bob, both
   directions, 100 messages), out-of-order and dropped messages, message key limits, prekey
   reuse/duplicate detection, simultaneous initiation (both sides send a prekey message), session
   archiving after a new prekey message, untrusted identity rejection, bad MAC rejection, group
   sender keys with 3 members and out-of-order delivery.
2. v4: an Alice/Bob session through a Kyber1024 prekey bundle; the message version is 4. A bundle without Kyber yields version 3.
3. Serialization round trips for every record type; a `SessionRecord` survives
   serialize → deserialize → continue the conversation.
4. Known-answer vectors: derive root/chain/message keys from a fixed shared secret and compare to
   values you compute with the Go implementation's algorithm by hand in the test comments (or a
   vector from the Go tests). At least one fixed-RNG end-to-end ciphertext is pinned so accidental
   wire changes are caught.
5. Fuzz-ish: feeding truncated and bit-flipped messages to every parser returns `Err`, never
   panics (loop over every prefix length and every single-bit flip of one message).

## Done means

`cargo test -p chirper-signal-protocol` and `cargo clippy -p chirper-signal-protocol --all-targets
-- -D warnings` pass with no unused dependencies (remove any you do not use). Public API documented.
Update the crate doc comment in `lib.rs` to list exactly what is implemented.
