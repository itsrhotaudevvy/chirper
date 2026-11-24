//! The payloads the driver sends over lightspeed: task batches (`type 3`) and database sync queries
//! (`type 1` and `2`).
//!
//! Field names and queue names follow `pkg/messagix/socket/*.go` and `pkg/messagix/taskManager.go`
//! (AGPL-3.0, mautrix-meta); the JSON produced here is what the server expects.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// Task labels, from `socket/task.go`'s `TaskLabels`.
pub mod label {
    pub const THREAD_MARK_READ: &str = "21";
    pub const DELETE_MESSAGE: &str = "33";
    pub const SEND_MESSAGE: &str = "46";
    pub const FETCH_THREADS: &str = "145";
    pub const FETCH_MESSAGES: &str = "228";
    pub const EDIT_MESSAGE: &str = "742";
    pub const SEND_REACTION_V2: &str = "604";
}

/// The generator of epoch ids: `(milliseconds << 22) | (counter << 12) | 42`, as the reference's
/// `methods.GenerateEpochID` builds them. Also used as the offline threading id of a send.
#[derive(Debug, Default)]
pub struct EpochClock {
    state: Mutex<(i64, i64)>,
}

impl EpochClock {
    pub fn next(&self) -> i64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as i64);
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if now == state.0 {
            state.1 += 1;
        } else {
            *state = (now, 0);
        }
        (now << 22) | (state.1 << 12) | 42
    }
}

/// One task of a batch, as the server receives it.
#[derive(Clone, Debug, Serialize)]
pub struct TaskData {
    pub failure_count: Option<i64>,
    pub label: String,
    pub payload: String,
    pub queue_name: String,
    pub task_id: i64,
}

/// A batch of tasks: the outer payload of a `type 3` lightspeed request.
#[derive(Clone, Debug, Serialize)]
pub struct TaskBatch {
    pub epoch_id: i64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<TaskData>,
    pub version_id: String,
}

/// Collects tasks with their ids, as the reference's task manager does.
#[derive(Debug)]
pub struct TaskQueue {
    next_task_id: i64,
    tasks: Vec<TaskData>,
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self {
            next_task_id: -1,
            tasks: Vec::new(),
        }
    }
}

impl TaskQueue {
    /// Queue one task. `payload` is serialised as the task's JSON body.
    pub fn push<T: Serialize>(&mut self, label: &str, queue_name: String, payload: &T) -> Result<(), serde_json::Error> {
        self.next_task_id += 1;
        self.tasks.push(TaskData {
            failure_count: None,
            label: label.to_owned(),
            payload: serde_json::to_string(payload)?,
            queue_name,
            task_id: self.next_task_id,
        });
        Ok(())
    }

    /// The batch payload for every queued task, emptying the queue.
    pub fn finish(&mut self, epoch: &EpochClock, version_id: i64) -> TaskBatch {
        TaskBatch {
            epoch_id: epoch.next(),
            tasks: std::mem::take(&mut self.tasks),
            version_id: version_id.to_string(),
        }
    }
}

/// The `FetchThreadsTask` (label 145, queue `trq`).
#[derive(Clone, Debug, Serialize)]
pub struct FetchThreads {
    pub is_after: i64,
    pub parent_thread_key: i64,
    pub reference_thread_key: i64,
    pub reference_activity_timestamp: i64,
    pub additional_pages_to_fetch: i64,
    pub cursor: Option<String>,
    pub messaging_tag: Option<String>,
    pub sync_group: i64,
}

impl FetchThreads {
    /// The first page of the newest threads of one sync group, as the reference asks at connect.
    pub fn first_page(sync_group: i64, parent_thread_key: i64) -> Self {
        Self {
            is_after: 0,
            parent_thread_key,
            reference_thread_key: 0,
            reference_activity_timestamp: 9_999_999_999_999,
            additional_pages_to_fetch: 0,
            cursor: None,
            messaging_tag: None,
            sync_group,
        }
    }
}

/// The `FetchMessagesTask` (label 228, queue `mrq.<thread>`): one page of a thread's history.
#[derive(Clone, Debug, Serialize)]
pub struct FetchMessages {
    pub thread_key: i64,
    pub direction: i64,
    pub reference_timestamp_ms: i64,
    pub reference_message_id: String,
    pub sync_group: i64,
    pub cursor: String,
}

/// The `SendMessageTask` (label 46, queue = thread id).
#[derive(Clone, Debug, Serialize)]
pub struct SendMessage {
    pub thread_id: i64,
    #[serde(serialize_with = "as_string")]
    pub otid: i64,
    pub source: i64,
    pub send_type: i64,
    pub sync_group: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_metadata: Option<ReplyMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initiating_source: Option<i64>,
    pub skip_url_preview_gen: i64,
    pub text_has_links: i64,
    pub multitab_env: i64,
}

/// Reply metadata of a send, from `socket/threads.go`'s `ReplyMetaData`.
#[derive(Clone, Debug, Serialize)]
pub struct ReplyMetadata {
    pub reply_source_id: String,
    pub reply_source_type: i64,
    pub reply_type: i64,
}

fn as_string<S: serde::Serializer>(value: &i64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_string())
}

/// The `ThreadMarkReadTask` (label 21, queue = thread id).
#[derive(Clone, Debug, Serialize)]
pub struct ThreadMarkRead {
    pub thread_id: i64,
    pub last_read_watermark_ts: i64,
    pub sync_group: i64,
}

/// The `DeleteMessageTask` (label 33, queue `unsend_message`).
#[derive(Clone, Debug, Serialize)]
pub struct DeleteMessage {
    pub message_id: String,
}

/// The `EditMessageTask` (label 742, queue `edit_message`).
#[derive(Clone, Debug, Serialize)]
pub struct EditMessage {
    pub message_id: String,
    pub text: String,
}

/// The `SendReactionV2Task` (label 604, queue `["reaction_v2", message id]`).
#[derive(Clone, Debug, Serialize)]
pub struct SendReactionV2 {
    pub thread_id: i64,
    pub message_id: String,
    pub message_timestamp: i64,
    pub actor_id: i64,
    pub reaction_fbid: i64,
    pub reaction_style: i64,
    pub current_count: i64,
    pub viewer_is_reactor: i64,
    pub operation: i64,
    pub reaction_literal: String,
    pub entry_point: Option<String>,
    pub sync_group: i64,
}

/// The `DatabaseQuery` of a sync step: the outer payload of a `type 1` (send params) or `type 2`
/// (cursor) lightspeed request.
#[derive(Clone, Debug, Serialize)]
pub struct DatabaseQuery {
    pub database: i64,
    pub last_applied_cursor: Option<String>,
    pub sync_params: Option<String>,
    pub epoch_id: i64,
    pub version: i64,
    pub failure_count: Option<i64>,
}

/// The outer envelope of every lightspeed request: `{app_id, payload, request_id, type}`.
#[derive(Clone, Debug, Serialize)]
pub struct LsRequest {
    pub app_id: String,
    pub payload: String,
    pub request_id: i64,
    #[serde(rename = "type")]
    pub kind: i64,
}

/// Wrap a serialised payload into its lightspeed envelope.
pub fn ls_request<T: Serialize>(
    app_id: &str,
    payload: &T,
    request_id: i64,
    kind: i64,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&LsRequest {
        app_id: app_id.to_owned(),
        payload: serde_json::to_string(payload)?,
        request_id,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn epoch_ids_follow_the_reference_layout() {
        let clock = EpochClock::default();
        let first = clock.next();
        let second = clock.next();
        assert_eq!(first & 0xfff, 42, "the low bits are the fixed marker");
        assert_ne!(first, second, "two ids in a row never collide");
        assert!(first >> 22 > 1_600_000_000_000);
    }

    #[test]
    fn a_task_batch_numbers_tasks_from_zero_and_empties_the_queue() {
        let mut queue = TaskQueue::default();
        queue
            .push(label::FETCH_THREADS, "trq".to_owned(), &FetchThreads::first_page(1, -1))
            .expect("serialises");
        queue
            .push(label::FETCH_THREADS, "trq".to_owned(), &FetchThreads::first_page(95, -1))
            .expect("serialises");
        let clock = EpochClock::default();
        let batch = queue.finish(&clock, 1_000);
        let json: Value = serde_json::to_value(&batch).expect("json");
        assert_eq!(json["version_id"], "1000");
        assert_eq!(json["tasks"][0]["task_id"], 0);
        assert_eq!(json["tasks"][1]["task_id"], 1);
        assert_eq!(json["tasks"][0]["label"], "145");
        assert_eq!(json["tasks"][0]["queue_name"], "trq");
        assert_eq!(json["tasks"][0]["failure_count"], Value::Null);
        let inner: Value =
            serde_json::from_str(json["tasks"][1]["payload"].as_str().expect("string")).expect("json");
        assert_eq!(inner["sync_group"], 95);
        assert_eq!(inner["reference_activity_timestamp"], 9_999_999_999_999_i64);
        assert!(queue.tasks.is_empty());
    }

    #[test]
    fn a_send_carries_its_otid_as_a_string() {
        let send = SendMessage {
            thread_id: 146,
            otid: 1234,
            source: 65537,
            send_type: 1,
            sync_group: 1,
            reply_metadata: None,
            text: Some("hi".to_owned()),
            initiating_source: Some(1),
            skip_url_preview_gen: 0,
            text_has_links: 0,
            multitab_env: 0,
        };
        let json: Value = serde_json::to_value(&send).expect("json");
        assert_eq!(json["otid"], "1234");
        assert_eq!(json["thread_id"], 146);
        assert_eq!(json["text"], "hi");
        assert!(json.get("reply_metadata").is_none(), "absent replies are omitted");
    }

    #[test]
    fn a_database_query_sends_null_cursors_explicitly() {
        let query = DatabaseQuery {
            database: 1,
            last_applied_cursor: None,
            sync_params: Some("{}".to_owned()),
            epoch_id: 7,
            version: 9,
            failure_count: None,
        };
        let json: Value = serde_json::to_value(&query).expect("json");
        assert_eq!(json["last_applied_cursor"], Value::Null);
        assert_eq!(json["sync_params"], "{}");
        assert_eq!(json["failure_count"], Value::Null);
    }

    #[test]
    fn the_envelope_nests_the_payload_as_text() {
        let envelope = ls_request("2220391788200892", &ThreadMarkRead {
            thread_id: 5,
            last_read_watermark_ts: 6,
            sync_group: 1,
        }, 3, 3)
        .expect("serialises");
        let json: Value = serde_json::from_str(&envelope).expect("json");
        assert_eq!(json["app_id"], "2220391788200892");
        assert_eq!(json["request_id"], 3);
        assert_eq!(json["type"], 3);
        let inner: Value = serde_json::from_str(json["payload"].as_str().expect("text")).expect("json");
        assert_eq!(inner["thread_id"], 5);
    }
}
