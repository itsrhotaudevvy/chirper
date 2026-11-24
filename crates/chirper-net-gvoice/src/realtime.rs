//! The realtime channel: Google's WebChannel long-poll that tells the client when a thread changed.
//!
//! Ported from `pkg/libgv/channel.go` of mautrix-gvoice (AGPL-3.0). The loop subscribes, polls
//! with an acknowledgement id, reads the framed entries of each poll as they arrive, and
//! resubscribes on a reconnect instruction, after an hour, or after Google forgets the session.
//! Errors that end the loop are returned; the session turns them into a transient disconnect and
//! the runtime reconnects.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use url::form_urlencoded::Serializer;

use chirper_bridge::{Error, Result};

use crate::api::{Api, UNKNOWN_SID, request_id};
use crate::chunks::ChunkReader;
use crate::schema::{WEBCHANNEL_EVENT, WEBCHANNEL_NOOP};
use crate::wire::{self, Msg};

/// The channel is subscribed again after this long, whatever it reports.
const FORCE_RESUBSCRIBE: Duration = Duration::from_secs(3600);
/// Unknown-session answers tolerated before the channel gives up.
const MAX_UNKNOWN_SESSIONS: u32 = 10;
/// The seven subscriptions the web client opens, as `req0___data__` .. `req6___data__`.
const SUBSCRIPTIONS: [&str; 7] = [
    r#"[[["1",[null,null,null,[7,5],null,[null,[null,1],[[["2"]]]],null,1,2],null,3]]]"#,
    r#"[[["2",[null,null,null,[7,5],null,[null,[null,1],[[["3"]]]],null,1,2],null,3]]]"#,
    r#"[[["3",[null,null,null,[7,5],null,[null,[null,1],[[["3"]]]],null,1,2],null,3]]]"#,
    r#"[[["4",[null,null,null,[7,5],null,[null,[null,1],[[["1"]]]],null,1,2],null,3]]]"#,
    r#"[[["5",[null,null,null,[7,5],null,[null,[null,1],[[["1"]]]],null,1,2],null,3]]]"#,
    r#"[[["6",[null,null,null,[7,5],null,[null,[null,1],[[["1"]]]],null,1,2],null,3]]]"#,
    r#"[[["9",[null,null,null,[7,5],null,[null,[null,1],[[["1"]]]],null,1,2],null,3]]]"#,
];

/// What the channel tells the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// The channel is up (a fresh subscription or a poll after trouble).
    Connected,
    /// Something changed in the account's threads: list them again.
    NewMessages,
}

/// A subscribed channel: the server session, the channel id and the last acknowledged array id.
struct Channel {
    server: String,
    session_id: String,
    ack: u64,
}

/// What a poll decided about the channel.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    Poll,
    Resubscribe,
}

/// Run the channel until it fails for good. Returns only errors.
pub async fn run(api: &Api, emit: &mut (dyn FnMut(Signal) + Send)) -> Result<()> {
    let mut channel = subscribe(api).await?;
    let mut subscribed_at = Instant::now();
    let mut failures = 0u32;
    loop {
        if subscribed_at.elapsed() > FORCE_RESUBSCRIBE {
            channel = subscribe(api).await?;
            subscribed_at = Instant::now();
        }
        match poll(api, &mut channel, &mut failures, emit).await {
            Ok(Next::Poll) => {}
            Ok(Next::Resubscribe) => {
                channel = subscribe(api).await?;
                subscribed_at = Instant::now();
            }
            Err(Error::Protocol(message)) if message == UNKNOWN_SID => {
                failures += 1;
                if failures > MAX_UNKNOWN_SESSIONS {
                    return Err(Error::Network(
                        "Google Voice keeps forgetting the realtime session".to_owned(),
                    ));
                }
                tokio::time::sleep(Duration::from_secs(u64::from(failures - 1) * 2)).await;
                channel = subscribe(api).await?;
                subscribed_at = Instant::now();
            }
            Err(err) => return Err(err),
        }
    }
}

async fn subscribe(api: &Api) -> Result<Channel> {
    let server = api.choose_server().await?;
    let body = subscription_body();
    let reply = api.create_channel(&server, request_id(), body).await?;
    let session_id = reply.message(1).message(2).string(2);
    if session_id.is_empty() {
        return Err(wire::protocol("the realtime channel has no session id"));
    }
    Ok(Channel {
        server,
        session_id,
        ack: 0,
    })
}

/// The form body of a channel subscription: the seven subscriptions and their count.
fn subscription_body() -> String {
    let mut form = Serializer::new(String::new());
    form.append_pair("count", "7").append_pair("ofs", "0");
    for (index, data) in SUBSCRIPTIONS.iter().enumerate() {
        form.append_pair(&format!("req{index}___data__"), data);
    }
    form.finish()
}

/// One long-poll: read its entries until it ends, then say whether to poll again.
async fn poll(
    api: &Api,
    channel: &mut Channel,
    failures: &mut u32,
    emit: &mut (dyn FnMut(Signal) + Send),
) -> Result<Next> {
    let mut response = api
        .poll_channel(&channel.server, &channel.session_id, channel.ack)
        .await?;
    if *failures != 0 || channel.ack == 0 {
        emit(Signal::Connected);
    }
    let mut reader = ChunkReader::new();
    loop {
        let bytes = match response.chunk().await {
            Ok(Some(bytes)) => bytes,
            // The poll ended, or the server held it for its full time: poll again.
            Ok(None) => return Ok(Next::Poll),
            Err(err) if err.is_timeout() => return Ok(Next::Poll),
            Err(_) => return Err(Error::Network("the realtime stream broke".to_owned())),
        };
        reader.push(&bytes);
        while let Some(frame) = reader.next_chunk()? {
            // A frame that is not a JSON array ends this poll; the next one starts afresh.
            let Ok(Value::Array(entries)) = serde_json::from_slice::<Value>(&frame) else {
                return Ok(Next::Poll);
            };
            for entry in &entries {
                if handle_entry(entry, channel, failures, emit)? == Next::Resubscribe {
                    return Ok(Next::Resubscribe);
                }
            }
        }
    }
}

fn handle_entry(
    entry: &Value,
    channel: &mut Channel,
    failures: &mut u32,
    emit: &mut (dyn FnMut(Signal) + Send),
) -> Result<Next> {
    if is_noop(entry) {
        channel.ack = wire::decode_pblite(&WEBCHANNEL_NOOP, entry)?.int(1).unsigned_abs();
    } else {
        let event: Msg = wire::decode_pblite(&WEBCHANNEL_EVENT, entry)?;
        if is_reconnect(&event) {
            return Ok(Next::Resubscribe);
        }
        channel.ack = event.int(1).unsigned_abs();
        if is_new_messages(&event) {
            emit(Signal::NewMessages);
        }
    }
    *failures = 0;
    Ok(Next::Poll)
}

/// The keep-alive entry: its last element is `["noop"]`.
fn is_noop(entry: &Value) -> bool {
    entry
        .as_array()
        .and_then(|items| items.last())
        .is_some_and(|last| *last == json!(["noop"]))
}

/// The server asked for a fresh subscription: one data wrapper that says so.
fn is_reconnect(event: &Msg) -> bool {
    let mut wrappers = event.messages(2);
    match (wrappers.next(), wrappers.next()) {
        (Some(wrapper), None) => wrapper.message(2).flag(1),
        _ => false,
    }
}

/// A thread changed: the first entry of the first wrapper carries a JSON array of changes.
fn is_new_messages(event: &Msg) -> bool {
    let first = event
        .messages(2)
        .next()
        .and_then(|wrapper| wrapper.messages(1).next());
    let Some(data) = first else {
        return false;
    };
    data.message(2)
        .message(2)
        .messages(1)
        .next()
        .is_some_and(|item| item.bytes(2).first() == Some(&b'['))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An event entry as Google frames it: `[arrayId, [wrapper]]`. The wrapper holds one data item
    /// whose event carries `payload` as the unknown bytes of its sub2 record, base64 in pblite.
    fn event_entry(array_id: i64, payload: Option<&[u8]>, reconnect: bool) -> Value {
        let body = match payload {
            Some(payload) => json!([null, [[[null, base64(payload)]]]]),
            None => json!([]),
        };
        let data = json!(["1", body]);
        let alt = if reconnect { json!([true]) } else { Value::Null };
        json!([array_id, [[[data], alt]]])
    }

    fn base64(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn noop_entries_are_recognised_by_their_last_element() {
        assert!(is_noop(&json!([7, ["noop"]])));
        assert!(!is_noop(&json!([7, [["x"]]])));
        assert!(!is_noop(&json!([])));
    }

    #[test]
    fn a_noop_carries_the_acknowledgement_id() {
        let mut channel = Channel {
            server: String::new(),
            session_id: String::new(),
            ack: 0,
        };
        let mut failures = 3;
        let mut signals = Vec::new();
        let next = handle_entry(&json!([42, ["noop"]]), &mut channel, &mut failures, &mut |s| {
            signals.push(s);
        })
        .expect("handled");
        assert_eq!(next, Next::Poll);
        assert_eq!(channel.ack, 42);
        assert_eq!(failures, 0);
        assert!(signals.is_empty());
    }

    #[test]
    fn a_new_messages_event_is_signalled() {
        let entry = event_entry(5, Some(b"[{\"thread\":1}]"), false);
        let event = wire::decode_pblite(&WEBCHANNEL_EVENT, &entry).expect("decodes");
        assert_eq!(event.int(1), 5);
        assert!(is_new_messages(&event));

        let mut channel = Channel {
            server: String::new(),
            session_id: String::new(),
            ack: 0,
        };
        let mut failures = 0;
        let mut signals = Vec::new();
        handle_entry(&entry, &mut channel, &mut failures, &mut |s| signals.push(s)).expect("handled");
        assert_eq!(channel.ack, 5);
        assert_eq!(signals, vec![Signal::NewMessages]);
    }

    #[test]
    fn other_payloads_are_not_new_messages() {
        let entry = event_entry(6, Some(b"presence"), false);
        let event = wire::decode_pblite(&WEBCHANNEL_EVENT, &entry).expect("decodes");
        assert!(!is_new_messages(&event));
        let bare = event_entry(7, None, false);
        let bare = wire::decode_pblite(&WEBCHANNEL_EVENT, &bare).expect("decodes");
        assert!(!is_new_messages(&bare));
    }

    #[test]
    fn a_single_reconnect_wrapper_asks_for_a_new_subscription() {
        let entry = event_entry(8, None, true);
        let event = wire::decode_pblite(&WEBCHANNEL_EVENT, &entry).expect("decodes");
        assert!(is_reconnect(&event));
        let mut channel = Channel {
            server: String::new(),
            session_id: String::new(),
            ack: 0,
        };
        let mut failures = 0;
        let next = handle_entry(&entry, &mut channel, &mut failures, &mut |_| {}).expect("handled");
        assert_eq!(next, Next::Resubscribe);
    }

    #[test]
    fn the_subscription_body_names_all_seven_channels() {
        let body = subscription_body();
        assert!(body.starts_with("count=7&ofs=0&req0___data__="));
        assert!(body.contains("req6___data__="));
        assert_eq!(body.matches("___data__=").count(), 7);
    }
}
