//! End-to-end smoke test of the Rust core, against the in-process demo network.
//!
//! ```text
//! cargo run -p chirper-core --example smoke
//! ```
//!
//! Uses a temporary data directory and an in-memory key, so it never touches the OS keychain. It
//! logs into the demo network, reads chats, fetches history, sends a message, waits for the events,
//! then closes and reopens the encrypted database with the same key.

use std::time::Duration;

use chirper_core::model::ChatFilter;
use chirper_core::{Core, Options};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let dir = std::env::temp_dir().join(format!("chirper-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let key = [7u8; 32];
    // The default feature set is plain SQLite for fast tests; encryption needs `sqlcipher`.
    let encrypted = cfg!(feature = "sqlcipher");

    rt.block_on(async {
        let mut options = Options::new(&dir);
        options.db_key = encrypted.then_some(key);
        options.demo_network = true;
        let (core, mut events) = Core::open(options)?;

        core.start_login("testnet", "demo").await?;
        let mut chats = Vec::new();
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            chats = core.list_chats(&ChatFilter::default()).await?;
            if chats.len() >= 2 {
                break;
            }
        }
        if chats.is_empty() {
            return Err("smoke: the demo network produced no chats".into());
        }
        println!("smoke: {} chats", chats.len());

        let chat = &chats[0];
        core.load_older(&chat.room_id).await?;
        let messages = core.list_messages(&chat.room_id, 0, 20).await?;
        println!("smoke: {} messages in the first chat", messages.len());

        let event_id = core
            .send_text(&chat.room_id, "smoke test", None, "smoke-txn-1")
            .await?;
        if event_id.is_empty() {
            return Err("smoke: send returned no event id".into());
        }
        println!("smoke: send ok ({event_id})");

        let mut kinds = Vec::new();
        while let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_millis(200), events.recv()).await
        {
            kinds.push(event.kind);
        }
        if kinds.is_empty() {
            return Err("smoke: no events received".into());
        }
        println!("smoke: {} events received", kinds.len());

        core.shutdown();
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;

    // The database must really be encrypted, not merely named SQLCipher.
    if encrypted {
        let header = std::fs::read(dir.join("chirper.db")).map(|b| b.starts_with(b"SQLite format 3"));
        if header.is_ok_and(|plaintext| plaintext) {
            return Err("smoke: the database file is plaintext".into());
        }
        println!("smoke: the database file is not plaintext");
    } else {
        println!("smoke: built without sqlcipher, skipping the encryption check");
    }

    rt.block_on(async {
        let mut options = Options::new(&dir);
        options.db_key = encrypted.then_some(key);
        options.demo_network = true;
        let (core, _events) = Core::open(options)?;
        let chats = core.list_chats(&ChatFilter::default()).await?;
        if chats.is_empty() {
            return Err("smoke: no chats after reopening with the key".into());
        }
        println!("smoke: reopened with the key");
        core.shutdown();
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;

    std::fs::remove_dir_all(&dir)?;
    println!("smoke: ok");
    Ok(())
}