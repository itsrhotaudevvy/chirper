//! End-to-end behaviour of the engine against the in-process demo driver.

use std::time::Duration;

use chirper_core::model::{ChatFilter, CoreEvent, MessageStatus};
use chirper_core::{Core, EventStream, Options};
use serde_json::Value;

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "chirper-engine-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn open(dir: &std::path::Path) -> (Core, EventStream) {
    let mut options = Options::new(dir);
    options.demo_network = true;
    Core::open(options).unwrap()
}

/// Collect events until `done` returns true, or fail after `secs`.
async fn drain_until(
    stream: &mut EventStream,
    secs: u64,
    mut done: impl FnMut(&[(String, Value)]) -> bool,
) -> Vec<(String, Value)> {
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        if done(&seen) {
            return seen;
        }
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let event: CoreEvent = tokio::time::timeout(left, stream.recv())
            .await
            .expect("timed out waiting for core events")
            .expect("event stream closed");
        if std::env::var_os("CHIRPER_TRACE").is_some() {
            eprintln!("event: {} {}", event.kind, event.payload.get());
        }
        seen.push((
            event.kind.clone(),
            serde_json::from_str(event.payload.get()).unwrap(),
        ));
    }
}

fn kind_count(events: &[(String, Value)], kind: &str) -> usize {
    events.iter().filter(|(k, _)| k == kind).count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demo_login_stores_chats_and_sends_a_message() {
    let dir = temp_dir("demo");
    let (core, mut stream) = open(&dir).await;

    let networks = core.networks().await.unwrap();
    let testnet = networks.iter().find(|n| n.id == "testnet").unwrap();
    assert!(testnet.available);
    assert!(!testnet.login_flows.is_empty());
    // Networks that are not ported are still listed, and explain themselves.
    let telegram = networks.iter().find(|n| n.id == "telegram").unwrap();
    assert!(!telegram.available);
    assert!(!telegram.reason.is_empty());

    let state = core.start_login("testnet", "demo").await.unwrap();
    assert_eq!(state.step.complete.as_ref().unwrap().login_id, "demo");

    let events = drain_until(&mut stream, 5, |seen| kind_count(seen, "chat") >= 2).await;
    assert!(kind_count(&events, "account") >= 1);

    let chats = core.list_chats(&ChatFilter::default()).await.unwrap();
    assert_eq!(chats.len(), 2);
    let dm = chats.iter().find(|c| c.is_dm).unwrap().clone();
    assert_eq!(dm.name, "Ada Lovelace");
    assert_eq!(dm.network, "testnet");
    assert!(!dm.room_id.is_empty());

    // History is fetched on demand and fills the empty chat.
    assert!(
        core.list_messages(&dm.room_id, 0, 50)
            .await
            .unwrap()
            .is_empty()
    );
    let more = core.load_older(&dm.room_id).await.unwrap();
    assert!(!more);
    let history = core.list_messages(&dm.room_id, 0, 50).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].body, "Welcome to the demo network.");
    assert_eq!(history[0].sender_name, "Ada Lovelace");

    // Sending stores the message at once and updates it when the network confirms.
    let sent = core
        .send_text(&dm.room_id, "hello from the test", None, "txn.1")
        .await
        .unwrap();
    assert!(sent.starts_with('$'));
    let events = drain_until(&mut stream, 5, |seen| {
        seen.iter().any(|(kind, payload)| {
            kind == "message"
                && payload["body"] == "hello from the test"
                && payload["status"] == "delivered"
        })
    })
    .await;
    assert!(kind_count(&events, "message") >= 1);

    let messages = core.list_messages(&dm.room_id, 0, 50).await.unwrap();
    let mine = messages
        .iter()
        .find(|m| m.event_id == sent)
        .expect("the optimistic message keeps its id");
    assert_eq!(mine.body, "hello from the test");
    assert_eq!(mine.status, MessageStatus::Delivered);
    assert!(mine.from_me);

    // Unread counting: the incoming history did not become unread, and marking read clears it.
    core.mark_unread(&dm.room_id, true).await.unwrap();
    let chat = core.get_chat(&dm.room_id).await.unwrap().unwrap();
    assert!(chat.unread_count >= 1);
    let last = messages.last().unwrap().event_id.clone();
    core.mark_read(&dm.room_id, &last).await.unwrap();
    let chat = core.get_chat(&dm.room_id).await.unwrap().unwrap();
    assert_eq!(chat.unread_count, 0);

    core.shutdown();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_and_settings_survive_a_restart() {
    let dir = temp_dir("restart");
    let (core, mut stream) = open(&dir).await;
    core.start_login("testnet", "demo").await.unwrap();
    drain_until(&mut stream, 5, |seen| kind_count(seen, "chat") >= 2).await;
    let chats = core.list_chats(&ChatFilter::default()).await.unwrap();
    let room = chats[0].room_id.clone();
    core.load_older(&room).await.unwrap();
    let mut settings = core.settings().await.unwrap();
    settings.demo_network = true;
    settings
        .send_typing
        .as_object_mut()
        .unwrap()
        .insert("testnet".to_owned(), Value::Bool(false));
    core.set_settings(&settings).await.unwrap();
    let before = core.list_messages(&room, 0, 50).await.unwrap().len();
    core.shutdown();
    drop(core);
    drop(stream);

    let (core, _stream) = open(&dir).await;
    let chats = core.list_chats(&ChatFilter::default()).await.unwrap();
    assert_eq!(chats.len(), 2);
    assert_eq!(
        core.settings().await.unwrap().send_typing["testnet"],
        Value::Bool(false)
    );
    assert_eq!(
        core.list_messages(&room, 0, 50).await.unwrap().len(),
        before
    );
    core.shutdown();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_input_is_rejected_before_it_reaches_the_store() {
    let dir = temp_dir("invalid");
    let (core, _stream) = open(&dir).await;
    assert!(core.get_chat("not-a-room").await.is_err());
    assert!(
        core.send_text("!x:chirper.local", "  ", None, "t")
            .await
            .is_err()
    );
    assert!(core.react("!x:chirper.local", "$e", "").await.is_err());
    assert!(core.media_file("https://example.invalid/x").await.is_err());
    assert!(
        core.list_messages("!x:chirper.local", -1, 10)
            .await
            .is_err()
    );
    assert!(core.search("ok", 0).await.is_err());
    assert!(core.start_login("nope", "demo").await.is_err());
    core.shutdown();
    std::fs::remove_dir_all(&dir).unwrap();
}
