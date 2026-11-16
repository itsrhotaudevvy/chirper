//! Core events to the webview. A dedicated thread drains the core channel, batches events for up
//! to 16 ms or 256 items, coalesces chat updates per room and emits one `core-batch` event. It
//! also posts OS notifications for incoming messages while the window is not focused.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use chirper_core::model::{CoreEvent, event_kind};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

pub const EVENT_CORE_BATCH: &str = "core-batch";
pub const BATCH_WINDOW: Duration = Duration::from_millis(16);
pub const BATCH_MAX_ITEMS: usize = 256;

const NOTIFY_TITLE: &str = "Chirper";
const NOTIFY_HIDDEN_BODY: &str = "New message";
const NOTIFY_MAX_CHARS: usize = 200;

/// Starts the thread that turns core events into `core-batch` events. It ends when the core is
/// closed and the channel disconnects.
pub fn spawn_pump(app: AppHandle, rx: Receiver<CoreEvent>) {
    let spawned = std::thread::Builder::new()
        .name("core-events".into())
        .spawn(move || pump(&app, &rx));
    if let Err(e) = spawned {
        log::error!("cannot start the event thread: {e}");
    }
}

fn pump(app: &AppHandle, rx: &Receiver<CoreEvent>) {
    let mut batches: u64 = 0;
    while let Ok(first) = rx.recv() {
        let batch = collect_batch(first, rx, BATCH_MAX_ITEMS, BATCH_WINDOW);
        for event in &batch {
            notify_if_needed(app, event);
        }
        let batch = coalesce_chats(batch);
        let payload = match encode_batch(&batch) {
            Ok(payload) => payload,
            Err(e) => {
                log::warn!("cannot encode a core batch: {e}");
                continue;
            }
        };
        if let Err(e) = app.emit(EVENT_CORE_BATCH, payload) {
            log::warn!("cannot emit a core batch: {e}");
        }
        batches += 1;
        log::debug!("core-batch {batches} emitted with {} events", batch.len());
    }
}

/// Collects events that arrive within `window` of `first`, up to `max` items in total.
pub fn collect_batch(
    first: CoreEvent,
    rx: &Receiver<CoreEvent>,
    max: usize,
    window: Duration,
) -> Vec<CoreEvent> {
    let deadline = Instant::now() + window;
    let mut batch = vec![first];
    while batch.len() < max {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining) {
            Ok(event) => batch.push(event),
            Err(_) => break,
        }
    }
    batch
}

#[derive(Deserialize)]
struct RoomRef {
    #[serde(rename = "roomId")]
    room_id: String,
}

fn room_of(event: &CoreEvent) -> Option<String> {
    serde_json::from_str::<RoomRef>(event.payload.get())
        .ok()
        .map(|r| r.room_id)
}

/// Keeps only the last `chat` event of each room; other events keep their order.
pub fn coalesce_chats(events: Vec<CoreEvent>) -> Vec<CoreEvent> {
    let mut last: HashMap<String, usize> = HashMap::new();
    let rooms: Vec<Option<String>> = events
        .iter()
        .enumerate()
        .map(|(i, event)| {
            if event.kind != event_kind::CHAT {
                return None;
            }
            let room = room_of(event);
            if let Some(room) = &room {
                last.insert(room.clone(), i);
            }
            room
        })
        .collect();
    events
        .into_iter()
        .enumerate()
        .filter(|(i, event)| {
            if event.kind != event_kind::CHAT {
                return true;
            }
            match &rooms[*i] {
                Some(room) => last.get(room) == Some(i),
                None => true,
            }
        })
        .map(|(_, event)| event)
        .collect()
}

#[derive(Serialize)]
struct BatchItem<'a> {
    kind: &'a str,
    payload: &'a RawValue,
}

/// `[{"kind":..,"payload":..}, ..]` built without re-parsing the event payloads.
pub fn encode_batch(events: &[CoreEvent]) -> serde_json::Result<Box<RawValue>> {
    let items: Vec<BatchItem<'_>> = events
        .iter()
        .map(|event| BatchItem {
            kind: &event.kind,
            payload: &event.payload,
        })
        .collect();
    RawValue::from_string(serde_json::to_string(&items)?)
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct MessageNotice {
    live: bool,
    from_me: bool,
    sender_name: String,
    body: String,
}

/// The notification text for a message, or `None` when no notification should be posted.
pub fn notice_text(payload: &str, hide_content: bool) -> Option<(String, String)> {
    let message: MessageNotice = serde_json::from_str(payload).ok()?;
    if !message.live || message.from_me {
        return None;
    }
    if hide_content {
        return Some((NOTIFY_TITLE.to_owned(), NOTIFY_HIDDEN_BODY.to_owned()));
    }
    let title = if message.sender_name.trim().is_empty() {
        NOTIFY_TITLE.to_owned()
    } else {
        message.sender_name
    };
    let body: String = message.body.chars().take(NOTIFY_MAX_CHARS).collect();
    let body = if body.trim().is_empty() {
        NOTIFY_HIDDEN_BODY.to_owned()
    } else {
        body
    };
    Some((title, body))
}

fn notify_if_needed(app: &AppHandle, event: &CoreEvent) {
    if event.kind != event_kind::MESSAGE {
        return;
    }
    let state = app.state::<AppState>();
    let prefs = state.prefs();
    if !prefs.notifications || state.window_focused() {
        return;
    }
    let Some((title, body)) = notice_text(event.payload.get(), prefs.hide_notification_content)
    else {
        return;
    };
    let Some(room_id) = room_of(event) else {
        return;
    };
    if let Err(e) = crate::platform::show_message(
        app,
        &room_id,
        &title,
        &body,
        prefs.hide_notification_content,
    ) {
        log::warn!("cannot show a notification: {}", e.message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    fn event(kind: &str, payload: &str) -> CoreEvent {
        CoreEvent {
            kind: kind.to_owned(),
            payload: RawValue::from_string(payload.to_owned()).unwrap(),
        }
    }

    fn chat(room: &str, unread: u32) -> CoreEvent {
        event(
            "chat",
            &format!(r#"{{"roomId":"{room}","unreadCount":{unread}}}"#),
        )
    }

    #[test]
    fn coalesce_keeps_last_chat_per_room_in_place() {
        let out = coalesce_chats(vec![
            chat("!a:chirper.local", 1),
            event("typing", r#"{"roomId":"!a:chirper.local"}"#),
            chat("!b:chirper.local", 1),
            chat("!a:chirper.local", 2),
        ]);
        let kinds: Vec<_> = out.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, ["typing", "chat", "chat"]);
        assert!(
            out[1]
                .payload
                .get()
                .contains(r#""roomId":"!b:chirper.local""#)
        );
        assert!(out[2].payload.get().contains(r#""unreadCount":2"#));
    }

    #[test]
    fn coalesce_keeps_chats_without_room_id() {
        let out = coalesce_chats(vec![event("chat", "{}"), event("chat", "{}")]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn encode_batch_embeds_payloads_verbatim() {
        let raw = encode_batch(&[event("account", r#"{"name":"x"}"#)]).unwrap();
        assert_eq!(raw.get(), r#"[{"kind":"account","payload":{"name":"x"}}]"#);
    }

    #[test]
    fn collect_batch_stops_at_the_item_limit() {
        let (tx, rx) = sync_channel(16);
        for _ in 0..5 {
            tx.send(event("typing", "{}")).unwrap();
        }
        let batch = collect_batch(event("typing", "{}"), &rx, 3, Duration::from_secs(5));
        assert_eq!(batch.len(), 3);
        assert_eq!(rx.try_iter().count(), 3);
    }

    #[test]
    fn collect_batch_stops_at_the_time_window() {
        let (tx, rx) = sync_channel(16);
        let batch = collect_batch(event("typing", "{}"), &rx, 256, Duration::from_millis(5));
        assert_eq!(batch.len(), 1);
        drop(tx);
    }

    #[test]
    fn notice_for_live_incoming_message() {
        let payload = r#"{"live":true,"fromMe":false,"senderName":"Bob","body":"hi"}"#;
        assert_eq!(
            notice_text(payload, false),
            Some(("Bob".to_owned(), "hi".to_owned()))
        );
        assert_eq!(
            notice_text(payload, true),
            Some(("Chirper".to_owned(), "New message".to_owned()))
        );
    }

    #[test]
    fn no_notice_for_history_or_own_messages() {
        assert_eq!(notice_text(r#"{"live":false,"fromMe":false}"#, false), None);
        assert_eq!(notice_text(r#"{"live":true,"fromMe":true}"#, false), None);
        assert_eq!(notice_text("not json", false), None);
    }

    #[test]
    fn notice_body_is_truncated_on_characters() {
        let long = "é".repeat(500);
        let payload = format!(r#"{{"live":true,"fromMe":false,"body":"{long}"}}"#);
        let (_, body) = notice_text(&payload, false).unwrap();
        assert_eq!(body.chars().count(), NOTIFY_MAX_CHARS);
    }
}
