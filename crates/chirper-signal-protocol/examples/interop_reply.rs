//! Writes Rust-encrypted replies for the Go `verify` mode of `tests/interop/generate`.
//!
//! Bob's session is rebuilt from the Go vectors, the follow-up message (index 2) is decrypted,
//! and two replies are encrypted under the ratchet key that the follow-up produced. Go decrypts
//! each reply starting from Alice's session as recorded at message 3.
//!
//! ```text
//! cargo run -p chirper-signal-protocol --example interop_reply
//! cd crates/chirper-signal-protocol/tests/interop/generate
//! go run . verify -vectors ../go_vectors.json -replies ../rust_replies.json
//! ```

#[path = "../tests/interop/vectors.rs"]
mod vectors;

use std::error::Error;

use chirper_signal_protocol::cipher::{decrypt_signal_message, encrypt};
use chirper_signal_protocol::message::{CiphertextMessage, SignalMessage};
use rand::SeedableRng;
use rand::rngs::StdRng;
use vectors::{
    Replies, Reply, alice_address, bob_store, bytes, load, load_receiver_session, message,
};

/// Plaintexts of the Rust replies. Go checks each one after decryption.
const REPLIES: [&str; 2] = [
    "Rust reply one: Bob's ratchet has turned, so this uses a fresh key.",
    "Rust reply two: a second message on the same new sending chain.",
];

/// Index of the message whose recorded Alice session the replies are decrypted against.
const REPLY_CONTEXT: usize = 3;

/// Index of the follow-up that Bob's Rust store decrypts before replying.
const FOLLOW_UP: usize = 2;

fn main() -> Result<(), Box<dyn Error>> {
    let out = match std::env::args().nth(1) {
        Some(path) => path.into(),
        None => std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/interop/rust_replies.json"
        )),
    };
    let vectors = load()?;
    let alice = alice_address(&vectors);
    let mut store = bob_store(&vectors)?;

    let follow_up = message(&vectors, FOLLOW_UP)?;
    load_receiver_session(&mut store, &alice, follow_up)?;
    let wire = SignalMessage::deserialize(&bytes(&follow_up.ciphertext)?)?;
    let plain = decrypt_signal_message(&mut store, &alice, &wire, &mut StdRng::seed_from_u64(11))?;
    if plain != follow_up.plaintext.as_bytes() {
        return Err("Rust did not decrypt the Go follow-up to its plaintext".into());
    }

    let mut replies = Vec::new();
    for text in REPLIES {
        let ciphertext = encrypt(&mut store, &alice, text.as_bytes())?;
        if !matches!(ciphertext, CiphertextMessage::Signal(_)) {
            return Err("Rust reply is not a signal message".into());
        }
        replies.push(Reply {
            decrypt_with: REPLY_CONTEXT,
            plaintext: text.to_string(),
            ciphertext: hex::encode(ciphertext.serialized()),
        });
    }

    let mut json = serde_json::to_string_pretty(&Replies { replies })?;
    json.push('\n');
    std::fs::write(&out, json)?;
    println!("wrote {} replies to {}", REPLIES.len(), out.display());
    Ok(())
}
