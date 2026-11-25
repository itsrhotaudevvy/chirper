//! The wire contract, pinned against the Go core.
//!
//! `tests/golden/*.json` were captured from the Go implementation (`go/core/facade`) driving its
//! demo network: settings, the network list, login steps, accounts, chats, messages, search and the
//! first event of every kind. The Rust core must produce envelopes of the same shape (the same keys
//! with the same JSON types, recursively), so the UI cannot tell the two cores apart. Values differ
//! where the demo networks differ; the values both builds must agree on are asserted exactly.

use std::collections::BTreeMap;
use std::time::Duration;

use chirper_core::model::{ChatFilter, CoreEvent};
use chirper_core::{Core, EventStream, Options};
use serde_json::Value;

fn golden(name: &str) -> Value {
    let path = format!("{}/tests/golden/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap()
}

/// Every difference in shape between `go` (the golden) and `rust`, as `path: reason` lines.
/// A `null` in the golden is an optional value: any type is accepted there. Fields the contract
/// declares nullable (`NULLABLE`, from `ui/src/lib/core/types.ts` and the Go pointer fields) may be
/// `null` on the Rust side too. Arrays are compared by their first element, when both have one.
fn shape_diff(path: &str, go: &Value, rust: &Value, out: &mut Vec<String>) {
    let field = path.rsplit('.').next().unwrap_or_default();
    match (go, rust) {
        (Value::Null, _) => {}
        (_, Value::Null) if NULLABLE.contains(&field) => {}
        (Value::Object(g), Value::Object(r)) => {
            for (key, gv) in g {
                match r.get(key) {
                    Some(rv) => shape_diff(&format!("{path}.{key}"), gv, rv, out),
                    None => out.push(format!("{path}.{key}: missing in Rust")),
                }
            }
            for key in r.keys().filter(|key| !g.contains_key(*key)) {
                out.push(format!("{path}.{key}: not in the Go contract"));
            }
        }
        (Value::Array(g), Value::Array(r)) => {
            if let (Some(gv), Some(rv)) = (g.first(), r.first()) {
                shape_diff(&format!("{path}[0]"), gv, rv, out);
            }
        }
        (Value::Bool(_), Value::Bool(_))
        | (Value::Number(_), Value::Number(_))
        | (Value::String(_), Value::String(_)) => {}
        (g, r) => out.push(format!("{path}: Go has {}, Rust has {}", kind(g), kind(r))),
    }
}

/// Fields of the wire contract that may be `null`.
const NULLABLE: &[&str] = &["preview", "media", "replyTo", "profile"];

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn assert_same_shape(name: &str, rust: &Value) {
    let mut diffs = Vec::new();
    shape_diff(name, &golden(name), rust, &mut diffs);
    assert!(
        diffs.is_empty(),
        "{name} differs from the Go contract:\n{}",
        diffs.join("\n")
    );
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("chirper-golden-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn next_events(
    stream: &mut EventStream,
    until: impl Fn(&BTreeMap<String, Value>) -> bool,
) -> BTreeMap<String, Value> {
    let mut first = BTreeMap::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !until(&first) {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let event: CoreEvent = tokio::time::timeout(left, stream.recv())
            .await
            .expect("timed out waiting for core events")
            .expect("event stream closed");
        first
            .entry(event.kind.clone())
            .or_insert_with(|| serde_json::from_str(event.payload.get()).unwrap());
    }
    first
}

#[tokio::test]
async fn default_settings_match_the_go_core() {
    let dir = temp_dir("settings");
    let (core, _stream) = Core::open(Options::new(&dir)).unwrap();
    let settings = serde_json::to_value(core.settings().await.unwrap()).unwrap();
    assert_same_shape("settings_default", &settings);
    assert_eq!(settings, golden("settings_default"));
    core.shutdown();
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Whether the driver behind a Go network id is compiled into this build (see the `net-*` features
/// in `Cargo.toml`). The Go networks of the other ids ship in every build.
fn compiled_in(id: &str) -> bool {
    let gated = [
        ("signal", cfg!(feature = "net-signal")),
        ("messenger", cfg!(feature = "net-meta")),
        ("instagram", cfg!(feature = "net-meta")),
        ("twitter", cfg!(feature = "net-twitter")),
        ("linkedin", cfg!(feature = "net-linkedin")),
        ("gmessages", cfg!(feature = "net-gmessages")),
        ("gvoice", cfg!(feature = "net-gvoice")),
        ("slack", cfg!(feature = "net-slack")),
        ("bluesky", cfg!(feature = "net-bluesky")),
    ];
    gated
        .iter()
        .find(|(name, _)| *name == id)
        .is_none_or(|(_, on)| *on)
}

#[tokio::test]
async fn network_rows_match_the_go_core() {
    let dir = temp_dir("networks");
    let mut options = Options::new(&dir);
    options.demo_network = true;
    let (core, _stream) = Core::open(options).unwrap();
    let rust = core.networks().await.unwrap();
    let rust_json = serde_json::to_value(&rust).unwrap();
    assert_same_shape("networks", &rust_json);

    // Every network of the Go build is listed, under the same id, name and banner colour, and a
    // usable Rust driver offers at least the login flows the Go build offered.
    let mut problems = Vec::new();
    let rows = rust_json.as_array().unwrap();

    // The default build ships exactly these networks; `more-networks` adds the rest.
    let mut listed: Vec<&str> = rows.iter().map(|row| row["id"].as_str().unwrap()).collect();
    listed.sort_unstable();
    let mut expected = vec![
        "discord", "fluxer", "sms", "telegram", "testnet", "whatsapp",
    ];
    if cfg!(feature = "more-networks") {
        expected.extend([
            "bluesky",
            "gmessages",
            "gvoice",
            "imessage",
            "instagram",
            "linkedin",
            "messenger",
            "signal",
            "slack",
            "twitter",
        ]);
    }
    expected.sort_unstable();
    assert_eq!(
        listed, expected,
        "the build lists other networks than its feature set says"
    );

    for network in golden("networks").as_array().unwrap() {
        let id = network["id"].as_str().unwrap();
        if id == "demo" {
            continue; // the Rust demo network is `testnet`
        }
        if !compiled_in(id) {
            continue; // its driver is behind a feature this build does not enable
        }
        let Some(row) = rows.iter().find(|row| row["id"] == id) else {
            problems.push(format!("{id}: not listed"));
            continue;
        };
        for field in ["name", "color"] {
            if row[field] != network[field] {
                problems.push(format!(
                    "{id}: {field} {} != Go {}",
                    row[field], network[field]
                ));
            }
        }
        if row["available"] != true {
            continue;
        }
        for flow in network["loginFlows"].as_array().unwrap() {
            let kept = row["loginFlows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|own| own["id"] == flow["id"]);
            if !kept {
                problems.push(format!("{id}: lost the {} login flow", flow["id"]));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "network rows differ from the Go build:\n{}",
        problems.join("\n")
    );
    core.shutdown();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn envelopes_of_a_demo_session_match_the_go_core() {
    let dir = temp_dir("session");
    let mut options = Options::new(&dir);
    options.demo_network = true;
    let (core, mut stream) = Core::open(options).unwrap();

    let state = core.start_login("testnet", "demo").await.unwrap();
    assert_same_shape(
        "login_step_complete",
        &serde_json::to_value(&state).unwrap(),
    );

    let events = next_events(&mut stream, |seen| {
        seen.contains_key("chat") && seen.contains_key("account")
    })
    .await;
    assert_same_shape("event_account", &events["account"]);
    assert_same_shape("event_chat", &events["chat"]);
    assert_same_shape(
        "accounts",
        &serde_json::to_value(core.accounts().await.unwrap()).unwrap(),
    );

    let chats = core.list_chats(&ChatFilter::default()).await.unwrap();
    assert_same_shape("chats", &serde_json::to_value(&chats).unwrap());
    let dm = chats
        .iter()
        .find(|chat| chat.is_dm)
        .unwrap()
        .room_id
        .clone();
    let chat = core.get_chat(&dm).await.unwrap().unwrap();
    assert_same_shape("chat", &serde_json::to_value(&chat).unwrap());

    core.load_older(&dm).await.unwrap();
    let sent = core
        .send_text(&dm, "golden hello", None, "txn.golden")
        .await
        .unwrap();
    assert!(sent.starts_with('$'));
    let events = next_events(&mut stream, |seen| seen.contains_key("message")).await;
    assert_same_shape("event_message", &events["message"]);
    let messages = core.list_messages(&dm, 0, 50).await.unwrap();
    assert_same_shape("messages", &serde_json::to_value(&messages).unwrap());
    let found = core.search("golden", 10).await.unwrap();
    assert!(!found.is_empty());
    assert_same_shape("search", &serde_json::to_value(&found).unwrap());

    core.shutdown();
    std::fs::remove_dir_all(&dir).unwrap();
}
