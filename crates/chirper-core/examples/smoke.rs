//! End-to-end smoke test of the real Go core through the Rust wrapper.
//!
//! Build the Go library first (`scripts/build-go.sh <triple> [--release]`), then:
//!   cargo run -p chirper-core --example smoke --features static-link   (macOS / iOS)
//!   cargo run -p chirper-core --example smoke --features dynamic-link  (others)
//!
//! Uses a temporary data dir and an in-memory key, so it never touches the OS keychain. It logs
//! into the built-in demo network, reads chats and messages, sends one message, waits for its
//! events, closes, and reopens with the same key to prove the encrypted database reopens.

use std::{sync::Arc, time::Duration};

use chirper_core::{Core, ffi::Backend, model::ChatFilter};
use serde_json::json;
use zeroize::Zeroizing;

fn backend() -> Arc<dyn Backend> {
    #[cfg(feature = "static-link")]
    return Arc::new(chirper_core::ffi::StaticBackend::new());
    #[cfg(all(feature = "dynamic-link", not(feature = "static-link")))]
    return Arc::new(
        chirper_core::ffi::DynamicBackend::load_default().expect("load libchirpercore"),
    );
    #[cfg(not(any(feature = "static-link", feature = "dynamic-link")))]
    panic!("enable static-link or dynamic-link");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let dir = std::env::temp_dir().join(format!("chirper-smoke-{}", std::process::id()));
    let key = Zeroizing::new([7u8; 32]);
    let backend = backend();

    let (core, events) = Core::open(backend.clone(), &dir, Some(&key))?;
    println!("version: {}", core.version());
    rt.block_on(async {
        core.call_raw("setSettings", json!({"settings": {"demoNetwork": true}}))
            .await?;
        core.start().await?;
        let login = core.start_login("demo", "demo").await?;
        let values = [("name".to_owned(), "smoke".to_owned())]
            .into_iter()
            .collect();
        core.submit_login_input(&login.process_id, &values).await?;
        std::thread::sleep(Duration::from_secs(2));
        let accounts = core.accounts().await?;
        let chats = core.list_chats(&ChatFilter::default()).await?;
        if accounts.is_empty() || chats.is_empty() {
            return Err("smoke: demo network produced no accounts or chats".into());
        }
        println!("smoke: accounts and chats present");
        let chat = chats.first().ok_or("demo network produced no chats")?;
        let messages = core.list_messages(&chat.room_id, 0, 20).await?;
        if messages.is_empty() {
            return Err("smoke: first chat has no messages".into());
        }
        println!("smoke: messages present");
        let event_id = core
            .send_text(&chat.room_id, "smoke test", None, "smoke-txn-1")
            .await?;
        if event_id.is_empty() {
            return Err("smoke: send returned no event id".into());
        }
        println!("smoke: send ok");
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    std::thread::sleep(Duration::from_millis(500));
    let kinds: Vec<String> = events.try_iter().map(|e| e.kind).collect();
    if kinds.is_empty() {
        return Err("smoke: no events received".into());
    }
    println!("smoke: events received");
    core.close();
    drop(core);

    let (again, _events) = Core::open(backend, &dir, Some(&key))?;
    let chats = rt.block_on(again.list_chats(&ChatFilter::default()))?;
    if chats.is_empty() {
        return Err("smoke: no chats after reopening with the key".into());
    }
    println!("smoke: reopened with key");
    again.close();
    let header = std::fs::read(dir.join("chirper.db")).map(|b| b.starts_with(b"SQLite format 3"));
    if header.is_ok_and(|plaintext| plaintext) {
        return Err("smoke: database file is readable without the key".into());
    }
    println!("smoke: database file is not plaintext");
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
