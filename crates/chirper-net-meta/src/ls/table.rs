//! Typed rows of the lightspeed tables the port reads.
//!
//! Field positions are the `index` tags of the reference's `pkg/messagix/table/*.go` structs
//! (AGPL-3.0, mautrix-meta); a procedure's arguments are read by position and anything missing or of
//! the wrong type becomes its zero value, which is how the reference treats undefined arguments.
//! Only the procedures the driver uses are typed; every other call is ignored.

use super::decode::{Call, Value};

/// The dependency name each typed procedure is sent under, from `table.go`'s `SPTable`.
pub mod proc {
    pub const DELETE_THEN_INSERT_THREAD: &str = "deleteThenInsertThread";
    pub const UPSERT_INBOX_THREADS_RANGE: &str = "upsertInboxThreadsRange";
    pub const UPSERT_SYNC_GROUP_THREADS_RANGE: &str = "upsertSyncGroupThreadsRange";
    pub const THREADS_RANGES_QUERY: &str = "threadsRangesQuery";
    pub const INSERT_MESSAGE: &str = "insertMessage";
    pub const UPSERT_MESSAGE: &str = "upsertMessage";
    pub const DELETE_THEN_INSERT_MESSAGE: &str = "deleteThenInsertMessage";
    pub const INSERT_NEW_MESSAGE_RANGE: &str = "insertNewMessageRange";
    pub const UPDATE_EXISTING_MESSAGE_RANGE: &str = "updateExistingMessageRange";
    pub const DELETE_THEN_INSERT_CONTACT: &str = "deleteThenInsertContact";
    pub const UPDATE_READ_RECEIPT: &str = "updateReadReceipt";
    pub const UPDATE_TYPING_INDICATOR: &str = "updateTypingIndicator";
    pub const DELETE_MESSAGE: &str = "deleteMessage";
    pub const EDIT_MESSAGE: &str = "editMessage";
    pub const UPSERT_REACTION: &str = "upsertReaction";
    pub const DELETE_REACTION: &str = "deleteReaction";
    pub const UPDATE_OR_INSERT_REACTION_V2: &str = "updateOrInsertReactionV2";
    pub const DELETE_REACTION_V2: &str = "deleteReactionV2";
    pub const DELETE_THREAD: &str = "deleteThread";
    pub const MARK_THREAD_READ: &str = "markThreadRead";
    pub const MARK_THREAD_READ_V2: &str = "markThreadReadV2";
    pub const EXECUTE_FIRST_BLOCK_FOR_SYNC_TRANSACTION_V4: &str = "executeFirstBlockForSyncTransactionV4";
    pub const EXECUTE_FINALLY_BLOCK_FOR_SYNC_TRANSACTION: &str = "executeFinallyBlockForSyncTransaction";
    pub const HANDLE_SYNC_FAILURE: &str = "handleSyncFailure";
}

/// The names of every procedure above, for the decoder's filter.
pub fn typed_procedures() -> std::collections::HashSet<String> {
    [
        proc::DELETE_THEN_INSERT_THREAD,
        proc::UPSERT_INBOX_THREADS_RANGE,
        proc::UPSERT_SYNC_GROUP_THREADS_RANGE,
        proc::THREADS_RANGES_QUERY,
        proc::INSERT_MESSAGE,
        proc::UPSERT_MESSAGE,
        proc::DELETE_THEN_INSERT_MESSAGE,
        proc::INSERT_NEW_MESSAGE_RANGE,
        proc::UPDATE_EXISTING_MESSAGE_RANGE,
        proc::DELETE_THEN_INSERT_CONTACT,
        proc::UPDATE_READ_RECEIPT,
        proc::UPDATE_TYPING_INDICATOR,
        proc::DELETE_MESSAGE,
        proc::EDIT_MESSAGE,
        proc::UPSERT_REACTION,
        proc::DELETE_REACTION,
        proc::UPDATE_OR_INSERT_REACTION_V2,
        proc::DELETE_REACTION_V2,
        proc::DELETE_THREAD,
        proc::MARK_THREAD_READ,
        proc::MARK_THREAD_READ_V2,
        proc::EXECUTE_FIRST_BLOCK_FOR_SYNC_TRANSACTION_V4,
        proc::EXECUTE_FINALLY_BLOCK_FOR_SYNC_TRANSACTION,
        proc::HANDLE_SYNC_FAILURE,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

static NULL: Value = Value::Null;

fn arg(args: &[Value], index: usize) -> &Value {
    args.get(index).unwrap_or(&NULL)
}

fn int(args: &[Value], index: usize) -> i64 {
    arg(args, index).int_or_zero()
}

fn text(args: &[Value], index: usize) -> String {
    arg(args, index).str_or_empty().to_owned()
}

fn flag(args: &[Value], index: usize) -> bool {
    arg(args, index).as_bool().unwrap_or(false)
}

/// A thread row: `deleteThenInsertThread`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreadRow {
    pub thread_key: i64,
    pub last_activity_ms: i64,
    pub last_read_watermark_ms: i64,
    pub snippet: String,
    pub thread_name: String,
    pub thread_picture_url: String,
    pub authority_level: i64,
    pub mailbox_type: i64,
    pub thread_type: i64,
    pub folder_name: String,
    pub remove_watermark_ms: i64,
    pub mute_expire_ms: i64,
    pub snippet_sender_contact_id: i64,
}

/// A message row: `insertMessage`, `upsertMessage` and `deleteThenInsertMessage` share positions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MessageRow {
    pub text: String,
    pub thread_key: i64,
    pub timestamp_ms: i64,
    pub primary_sort_key: i64,
    pub secondary_sort_key: i64,
    pub message_id: String,
    pub offline_threading_id: String,
    pub sender_id: i64,
    pub sticker_id: i64,
    pub is_admin_message: bool,
    pub send_status: i64,
    pub is_unsent: bool,
    pub unsent_timestamp_ms: i64,
    pub mention_offsets: String,
    pub mention_lengths: String,
    pub mention_ids: String,
    pub reply_source_id: String,
    pub reply_source_type: i64,
    pub reply_snippet: String,
    pub reply_message_text: String,
    pub is_forwarded: bool,
    pub edit_count: i64,
    pub reply_source_timestamp_ms: i64,
}

/// `deleteThenInsertContact`: a user the thread list refers to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContactRow {
    pub id: i64,
    pub profile_picture_url: String,
    pub name: String,
    pub first_name: String,
    pub username: String,
    pub is_messenger_user: bool,
}

/// `updateOrInsertReactionV2`: a reaction count and the viewer's own state for one emoji.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReactionV2Row {
    pub thread_key: i64,
    pub message_id: String,
    pub reaction_fbid: i64,
    pub reaction_literal: String,
    pub count: i64,
    pub viewer_is_reactor: bool,
    pub last_updated_ms: i64,
}

/// `upsertReaction` and `deleteReaction`: the legacy per-reactor reaction rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReactionRow {
    pub thread_key: i64,
    pub timestamp_ms: i64,
    pub message_id: String,
    pub actor_id: i64,
    pub reaction: String,
    /// True for `upsertReaction`, false for `deleteReaction`.
    pub add: bool,
}

/// `updateReadReceipt`: another participant read up to a watermark.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadReceiptRow {
    pub thread_key: i64,
    pub contact_id: i64,
    pub watermark_ms: i64,
    pub read_action_ms: i64,
}

/// `updateTypingIndicator`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TypingRow {
    pub thread_key: i64,
    pub sender_id: i64,
    pub is_typing: bool,
}

/// `editMessage`: the new text of an edited message.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditRow {
    pub message_id: String,
    pub text: String,
    pub edit_count: i64,
}

/// `insertNewMessageRange`: the range of loaded messages of a thread and whether more exist.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MessageRangeRow {
    pub thread_key: i64,
    pub min_timestamp_ms: i64,
    pub max_timestamp_ms: i64,
    pub min_message_id: String,
    pub has_more_before: bool,
    pub has_more_after: bool,
}

/// `updateExistingMessageRange`: the history request of `timestamp_ms` is finished.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExistingRangeRow {
    pub thread_key: i64,
    pub timestamp_ms: i64,
}

/// A sync cursor range from `upsertInboxThreadsRange` / `upsertSyncGroupThreadsRange`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreadRangeRow {
    pub sync_group: i64,
    pub parent_thread_key: i64,
    pub min_last_activity_ms: i64,
    pub has_more_before: bool,
    pub min_thread_key: i64,
}

/// `executeFirstBlockForSyncTransactionV4`: the cursors of one database sync step.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncBlockRow {
    pub database_id: i64,
    pub current_cursor: String,
    pub next_cursor: String,
    pub send_sync_params: bool,
    pub sync_channel: i64,
}

/// Everything the typed procedures of one response said, in the order they were said.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LsTable {
    pub threads: Vec<ThreadRow>,
    pub thread_ranges: Vec<ThreadRangeRow>,
    pub messages: Vec<MessageRow>,
    pub message_ranges: Vec<MessageRangeRow>,
    pub existing_ranges: Vec<ExistingRangeRow>,
    pub contacts: Vec<ContactRow>,
    pub reactions_v2: Vec<ReactionV2Row>,
    pub reactions: Vec<ReactionRow>,
    pub read_receipts: Vec<ReadReceiptRow>,
    pub typing: Vec<TypingRow>,
    pub deleted_messages: Vec<(i64, String)>,
    pub edits: Vec<EditRow>,
    pub deleted_threads: Vec<i64>,
    pub marked_read: Vec<(i64, i64)>,
    pub sync_blocks: Vec<SyncBlockRow>,
    pub sync_failures: Vec<(i64, String)>,
}

impl LsTable {
    /// Build the table from the calls of one response.
    pub fn from_calls(calls: &[Call]) -> Self {
        let mut table = Self::default();
        for call in calls {
            table.add(call);
        }
        table
    }

    fn add(&mut self, call: &Call) {
        let args = call.args.as_slice();
        match call.name.as_str() {
            proc::DELETE_THEN_INSERT_THREAD => self.threads.push(thread_row(args)),
            proc::UPSERT_INBOX_THREADS_RANGE => self.thread_ranges.push(ThreadRangeRow {
                sync_group: 1,
                parent_thread_key: -1,
                min_last_activity_ms: int(args, 1),
                has_more_before: flag(args, 2),
                min_thread_key: int(args, 4),
            }),
            proc::UPSERT_SYNC_GROUP_THREADS_RANGE => self.thread_ranges.push(ThreadRangeRow {
                sync_group: int(args, 0),
                parent_thread_key: int(args, 1),
                min_last_activity_ms: int(args, 2),
                has_more_before: flag(args, 3),
                min_thread_key: int(args, 5),
            }),
            proc::INSERT_MESSAGE | proc::UPSERT_MESSAGE | proc::DELETE_THEN_INSERT_MESSAGE => {
                self.messages.push(message_row(args));
            }
            proc::INSERT_NEW_MESSAGE_RANGE => self.message_ranges.push(MessageRangeRow {
                thread_key: int(args, 0),
                min_timestamp_ms: int(args, 5),
                max_timestamp_ms: int(args, 6),
                min_message_id: text(args, 3),
                has_more_before: flag(args, 7),
                has_more_after: flag(args, 8),
            }),
            proc::UPDATE_EXISTING_MESSAGE_RANGE => self.existing_ranges.push(ExistingRangeRow {
                thread_key: int(args, 0),
                timestamp_ms: int(args, 1),
            }),
            proc::DELETE_THEN_INSERT_CONTACT => self.contacts.push(ContactRow {
                id: int(args, 0),
                profile_picture_url: text(args, 2),
                name: text(args, 9),
                first_name: text(args, 18),
                username: text(args, 43),
                is_messenger_user: flag(args, 11),
            }),
            proc::UPDATE_READ_RECEIPT => self.read_receipts.push(ReadReceiptRow {
                watermark_ms: int(args, 0),
                thread_key: int(args, 1),
                contact_id: int(args, 2),
                read_action_ms: int(args, 3),
            }),
            proc::UPDATE_TYPING_INDICATOR => self.typing.push(TypingRow {
                thread_key: int(args, 0),
                sender_id: int(args, 1),
                is_typing: flag(args, 2),
            }),
            proc::DELETE_MESSAGE => self
                .deleted_messages
                .push((int(args, 0), text(args, 1))),
            proc::EDIT_MESSAGE => self.edits.push(EditRow {
                message_id: text(args, 0),
                text: text(args, 2),
                edit_count: int(args, 3),
            }),
            proc::UPSERT_REACTION => self.reactions.push(ReactionRow {
                thread_key: int(args, 0),
                timestamp_ms: int(args, 1),
                message_id: text(args, 2),
                actor_id: int(args, 3),
                reaction: text(args, 4),
                add: true,
            }),
            proc::DELETE_REACTION => self.reactions.push(ReactionRow {
                thread_key: int(args, 0),
                message_id: text(args, 1),
                actor_id: int(args, 2),
                add: false,
                ..ReactionRow::default()
            }),
            proc::UPDATE_OR_INSERT_REACTION_V2 => self.reactions_v2.push(ReactionV2Row {
                thread_key: int(args, 0),
                message_id: text(args, 1),
                reaction_fbid: int(args, 2),
                reaction_literal: text(args, 3),
                count: int(args, 4),
                viewer_is_reactor: flag(args, 6),
                last_updated_ms: int(args, 8),
            }),
            proc::DELETE_REACTION_V2 => self.reactions_v2.push(ReactionV2Row {
                thread_key: int(args, 0),
                message_id: text(args, 1),
                reaction_fbid: int(args, 2),
                reaction_literal: String::new(),
                count: -1,
                viewer_is_reactor: false,
                last_updated_ms: int(args, 3),
            }),
            proc::DELETE_THREAD => self.deleted_threads.push(int(args, 0)),
            proc::MARK_THREAD_READ => self.marked_read.push((int(args, 1), int(args, 0))),
            proc::MARK_THREAD_READ_V2 => self.marked_read.push((int(args, 0), int(args, 1))),
            proc::EXECUTE_FIRST_BLOCK_FOR_SYNC_TRANSACTION_V4 => self.sync_blocks.push(SyncBlockRow {
                database_id: int(args, 0),
                current_cursor: text(args, 2),
                next_cursor: text(args, 3),
                send_sync_params: flag(args, 6),
                sync_channel: int(args, 9),
            }),
            proc::HANDLE_SYNC_FAILURE => self.sync_failures.push((int(args, 0), text(args, 1))),
            _ => {}
        }
    }
}

fn thread_row(args: &[Value]) -> ThreadRow {
    ThreadRow {
        last_activity_ms: int(args, 0),
        last_read_watermark_ms: int(args, 1),
        snippet: text(args, 2),
        thread_name: text(args, 3),
        thread_picture_url: text(args, 4),
        authority_level: int(args, 6),
        thread_key: int(args, 7),
        mailbox_type: int(args, 8),
        thread_type: int(args, 9),
        folder_name: text(args, 10),
        remove_watermark_ms: int(args, 13),
        mute_expire_ms: int(args, 14),
        snippet_sender_contact_id: int(args, 18),
    }
}

fn message_row(args: &[Value]) -> MessageRow {
    MessageRow {
        text: text(args, 0),
        thread_key: int(args, 3),
        timestamp_ms: int(args, 5),
        primary_sort_key: int(args, 6),
        secondary_sort_key: int(args, 7),
        message_id: text(args, 8),
        offline_threading_id: text(args, 9),
        sender_id: int(args, 10),
        sticker_id: int(args, 11),
        is_admin_message: flag(args, 12),
        send_status: int(args, 15),
        is_unsent: flag(args, 17),
        unsent_timestamp_ms: int(args, 18),
        mention_offsets: text(args, 19),
        mention_lengths: text(args, 20),
        mention_ids: text(args, 21),
        reply_source_id: text(args, 23),
        reply_source_type: int(args, 24),
        reply_snippet: text(args, 27),
        reply_message_text: text(args, 28),
        is_forwarded: flag(args, 43),
        edit_count: int(args, 68),
        reply_source_timestamp_ms: int(args, 59),
    }
}
