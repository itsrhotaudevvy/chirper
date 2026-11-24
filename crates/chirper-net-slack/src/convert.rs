//! Slack messages and conversations as the bridge's domain types, and Chirper's outgoing text and
//! reaction keys as Slack's.
//!
//! The rules follow `third_party/slack/pkg/msgconv/from-slack.go` and `connector/chatinfo.go`
//! (mautrix-slack, AGPL-3.0): one message becomes a text part (part id empty) and one part per
//! file (`file-INDEX-ID`); a single file takes the text as its caption; a thread reply points at its
//! thread root; an edit reuses the id of the message it changes.

use std::sync::Arc;

use log::debug;
use serde_json::Value;

use chirper_bridge::model::{
    ChatMeta, Content, MediaInfo, PortalRef, Reaction, RemoteMessage, RemoteUser,
};

use crate::api::{Api, MAX_FILE_BYTES, channel_label};
use crate::blocks;
use crate::directory::{self, Directory};
use crate::emoji;
use crate::html;
use crate::ids;
use crate::mrkdwn;
use crate::wire::{Channel, File, Message};

/// Who a conversion is for: the workspace, the local user and the login.
#[derive(Clone, Copy)]
pub struct Scope<'a> {
    /// The workspace id.
    pub team: &'a str,
    /// The local user's Slack id.
    pub me: &'a str,
    /// The login id of the local account.
    pub login_id: &'a str,
    /// The names this login knows.
    pub directory: &'a Directory,
}

impl Scope<'_> {
    /// The portal of a channel, or of the workspace when `channel` is empty.
    pub fn portal(&self, channel: &str) -> PortalRef {
        if channel.is_empty() {
            return PortalRef {
                id: ids::team_portal_id(self.team),
                receiver: String::new(),
            };
        }
        PortalRef {
            id: ids::portal_id(self.team, channel),
            receiver: ids::portal_receiver(self.directory.is_dm(channel), self.login_id),
        }
    }

    /// The identity of a sender, from the cache or the raw id.
    fn sender(&self, slack_id: &str) -> RemoteUser {
        self.directory
            .user(slack_id)
            .unwrap_or_else(|| directory::placeholder(self.team, slack_id))
    }
}

/// The chat of the workspace itself: a space with the team's name.
pub fn team_meta(team: &str, team_name: &str) -> ChatMeta {
    ChatMeta {
        portal: PortalRef {
            id: ids::team_portal_id(team),
            receiver: String::new(),
        },
        name: team_name.to_owned(),
        is_space: true,
        ..ChatMeta::default()
    }
}

/// A channel, group or DM. `partner` is the other person of a DM, resolved by the caller.
pub fn chat_meta(scope: &Scope<'_>, team_name: &str, conversation: &Channel, partner: Option<RemoteUser>) -> ChatMeta {
    let is_dm = conversation.is_im || conversation.is_mpim;
    let name = if conversation.is_im {
        let base = partner
            .as_ref()
            .map(|user| user.display_name.clone())
            .unwrap_or_else(|| conversation.user.clone());
        if conversation.user == scope.me {
            format!("{base} (you)")
        } else {
            base
        }
    } else {
        channel_label(conversation)
    };
    let topic = if conversation.topic.value.is_empty() && !is_dm {
        conversation.purpose.value.clone()
    } else {
        conversation.topic.value.clone()
    };
    let member_count = if conversation.is_im {
        2
    } else {
        conversation.num_members
    };
    ChatMeta {
        portal: scope.portal(&conversation.id),
        name,
        topic,
        avatar_url: String::new(),
        is_dm,
        is_space: false,
        member_count,
        parent: (!is_dm).then(|| scope.portal("")),
        parent_name: team_name.to_owned(),
        dm_user: if conversation.is_im { partner } else { None },
    }
}

/// The Slack ids a message needs names for: its sender and everyone it mentions.
pub fn user_ids(message: &Message) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let sender = sender_id(message);
    if !sender.is_empty() {
        ids.push(sender.to_owned());
    }
    for id in mrkdwn::mentioned_users(&message.text)
        .into_iter()
        .chain(block_user_ids(&message.blocks))
    {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// The Slack id of who sent a message: the user, or the bot for a bot message.
pub fn sender_id(message: &Message) -> &str {
    if message.user.is_empty() {
        &message.bot_id
    } else {
        &message.user
    }
}

/// Every user id a rich text block mentions.
fn block_user_ids(blocks: &[Value]) -> Vec<String> {
    let mut found = Vec::new();
    for block in blocks {
        collect_block_users(block, &mut found);
    }
    found
}

fn collect_block_users(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("user")
                && let Some(id) = map.get("user_id").and_then(Value::as_str)
            {
                found.push(id.to_owned());
            }
            for child in map.values() {
                collect_block_users(child, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_block_users(item, found);
            }
        }
        _ => {}
    }
}

/// The text of a message as content: rich text blocks when there are any, else its mrkdwn.
fn text_content(directory: &Directory, message: &Message) -> Option<Content> {
    let formatted = blocks::to_html(&message.blocks, directory)
        .unwrap_or_else(|| mrkdwn::to_html(&message.text, directory));
    if formatted.is_empty() {
        return None;
    }
    let body = html::to_text(&formatted);
    if message.subtype == "me_message" {
        return Some(Content::Emote { body });
    }
    let formatted = formatted.contains('<').then_some(formatted);
    Some(Content::Text { body, formatted })
}

/// The parts of a live or history message. Returns nothing for a message with neither text nor a
/// file this driver can show.
pub async fn messages(
    api: &Api,
    scope: &Scope<'_>,
    channel: &str,
    message: &Message,
    live: bool,
) -> Vec<RemoteMessage> {
    resolve_all(api, scope, message).await;
    let text = text_content(scope.directory, message);
    let files: Vec<(usize, &File)> = message
        .files
        .iter()
        .enumerate()
        .filter(|(_, file)| file.mode != "tombstone" && file.filetype != "quip")
        .collect();
    let mut parts: Vec<(String, Content)> = Vec::new();
    let caption = match (&text, files.len()) {
        (Some(Content::Text { body, .. } | Content::Emote { body }), 1) => Some(body.clone()),
        _ => None,
    };
    if caption.is_none()
        && let Some(text) = text
    {
        parts.push((String::new(), text));
    }
    for (index, file) in files {
        let mut content = file_content(api, file).await;
        if let (Some(caption), Content::Media { caption: slot, .. }) = (&caption, &mut content) {
            *slot = caption.clone();
        }
        parts.push((ids::part_id("file", index, &file.id), content));
    }
    if parts.is_empty() {
        return Vec::new();
    }
    let id = ids::message_id(scope.team, channel, &message.ts);
    let reply_to = thread_root(scope.team, channel, message);
    let sender = scope.sender(sender_id(message));
    let portal = scope.portal(channel);
    let timestamp_ms = ids::timestamp_ms(&message.ts);
    let from_me = !message.user.is_empty() && message.user == scope.me;
    parts
        .into_iter()
        .map(|(part, content)| RemoteMessage {
            portal: portal.clone(),
            id: id.clone(),
            part,
            sender: sender.clone(),
            timestamp_ms,
            content,
            reply_to: reply_to.clone(),
            edit_of: None,
            live,
            from_me,
        })
        .collect()
}

/// The text of an edited message, as an edit of the message it replaces.
pub async fn edit(
    api: &Api,
    scope: &Scope<'_>,
    channel: &str,
    message: &Message,
    event_ts: &str,
) -> Option<RemoteMessage> {
    resolve_all(api, scope, message).await;
    let content = text_content(scope.directory, message)?;
    let id = ids::message_id(scope.team, channel, &message.ts);
    let when = if event_ts.is_empty() {
        &message.ts
    } else {
        event_ts
    };
    Some(RemoteMessage {
        portal: scope.portal(channel),
        id: id.clone(),
        part: String::new(),
        sender: scope.sender(sender_id(message)),
        timestamp_ms: ids::timestamp_ms(when),
        content,
        reply_to: None,
        edit_of: Some(id),
        live: true,
        from_me: !message.user.is_empty() && message.user == scope.me,
    })
}

/// The thread root of a thread reply, as the message it replies to. Slack threads are not portals
/// in the Go bridge: a reply stays in its channel and names its root.
fn thread_root(team: &str, channel: &str, message: &Message) -> Option<String> {
    (!message.thread_ts.is_empty() && message.thread_ts != message.ts)
        .then(|| ids::message_id(team, channel, &message.thread_ts))
}

async fn resolve_all(api: &Api, scope: &Scope<'_>, message: &Message) {
    for id in user_ids(message) {
        directory::resolve(api, scope.directory, scope.team, &id).await;
    }
}

/// A file of a message as content. Files that cannot be downloaded or shown become a notice that
/// says so, so one bad file does not hide the rest of the message.
async fn file_content(api: &Api, file: &File) -> Content {
    let name = file_name(file);
    if file.is_external {
        return Content::Unsupported {
            body: format!("External file: {name}"),
        };
    }
    let url = if file.url_private_download.is_empty() {
        &file.url_private
    } else {
        &file.url_private_download
    };
    if url.is_empty() {
        return Content::Unsupported {
            body: format!("File without a download link: {name}"),
        };
    }
    if file.size > MAX_FILE_BYTES {
        return Content::Unsupported {
            body: format!("File too large to download: {name}"),
        };
    }
    match api.download(url, MAX_FILE_BYTES).await {
        Ok(data) => Content::Media {
            kind: media_kind(file).to_owned(),
            caption: String::new(),
            info: MediaInfo {
                mime: file.mimetype.clone(),
                file_name: name,
                size: data.len(),
                width: file.original_w,
                height: file.original_h,
                duration_ms: file.duration_ms,
                blurhash: String::new(),
            },
            data: Arc::new(data),
            thumbnail: None,
        },
        Err(err) => {
            debug!("slack: a file could not be downloaded: {err}");
            Content::Unsupported {
                body: format!("File could not be downloaded: {name}"),
            }
        }
    }
}

fn file_name(file: &File) -> String {
    [&file.name, &file.title]
        .into_iter()
        .find(|name| !name.is_empty())
        .cloned()
        .unwrap_or_else(|| "file".to_owned())
}

/// The media kind the engine uses for a file.
fn media_kind(file: &File) -> &'static str {
    if file.subtype == "slack_audio" {
        "voice"
    } else if file.mimetype.starts_with("image/") {
        "image"
    } else if file.mimetype.starts_with("video/") {
        "video"
    } else if file.mimetype.starts_with("audio/") {
        "audio"
    } else {
        "file"
    }
}

/// The reactions of a message as the bridge reports them. Custom emoji keep their `:name:` form.
pub fn reactions(reactions: &[crate::wire::Reaction], me: &str) -> Vec<Reaction> {
    reactions
        .iter()
        .map(|reaction| Reaction {
            key: reaction_key(&reaction.name),
            count: reaction.count,
            from_me: reaction.users.iter().any(|user| user == me),
            senders: Vec::new(),
        })
        .collect()
}

/// The bridge's key of a Slack emoji name: the character for a standard emoji, else `:name:`.
pub fn reaction_key(name: &str) -> String {
    match emoji::unicode_for(name) {
        Some(character) => character.to_owned(),
        None => format!(":{name}:"),
    }
}

/// The Slack emoji name of a reaction key, without colons. `None` when Slack cannot name it.
pub fn reaction_name(key: &str) -> Option<String> {
    if let Some(inner) = key.strip_prefix(':').and_then(|rest| rest.strip_suffix(':')) {
        return (!inner.is_empty()).then(|| inner.to_owned());
    }
    if !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+'))
    {
        return Some(key.to_owned());
    }
    emoji::name_for(key).map(str::to_owned)
}

/// Outgoing text as Slack sends it: escaped so no mention or link is created by accident.
pub fn outgoing_text(text: &str) -> String {
    mrkdwn::escape_outbound(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(value: Value) -> Message {
        serde_json::from_value(value).expect("fixture parses")
    }

    #[test]
    fn reaction_keys_round_trip_for_standard_and_custom_emoji() {
        assert_eq!(reaction_key("+1"), "👍\u{fe0f}");
        assert_eq!(reaction_name("👍").as_deref(), Some("+1"));
        assert_eq!(reaction_key("partyparrot"), ":partyparrot:");
        assert_eq!(reaction_name(":partyparrot:").as_deref(), Some("partyparrot"));
        assert_eq!(reaction_name("plain").as_deref(), Some("plain"));
        assert_eq!(reaction_name("🦀").as_deref(), emoji::name_for("🦀"));
    }

    #[test]
    fn reactions_report_their_own_use() {
        let raw = vec![crate::wire::Reaction {
            name: "wave".to_owned(),
            count: 2,
            users: vec!["U1".to_owned(), "U9".to_owned()],
        }];
        let mapped = reactions(&raw, "U1");
        assert_eq!(mapped[0].key, "👋");
        assert_eq!(mapped[0].count, 2);
        assert!(mapped[0].from_me);
        assert!(!reactions(&raw, "U7")[0].from_me);
    }

    #[test]
    fn thread_replies_name_their_root_and_roots_do_not() {
        let reply = message(serde_json::json!({"ts": "2.0", "thread_ts": "1.0"}));
        assert_eq!(
            thread_root("T1", "C1", &reply).as_deref(),
            Some("T1-C1-1.0")
        );
        let root = message(serde_json::json!({"ts": "1.0", "thread_ts": "1.0"}));
        assert_eq!(thread_root("T1", "C1", &root), None);
    }

    #[test]
    fn user_ids_cover_the_sender_mentions_and_block_mentions() {
        let msg = message(serde_json::json!({
            "user": "U1",
            "text": "hi <@U2>",
            "blocks": [{"type": "rich_text", "elements": [{"type": "rich_text_section",
                "elements": [{"type": "user", "user_id": "U3"}, {"type": "user", "user_id": "U2"}]}]}]
        }));
        assert_eq!(user_ids(&msg), vec!["U1", "U2", "U3"]);
    }

    #[test]
    fn bot_messages_are_sent_by_their_bot() {
        let msg = message(serde_json::json!({"bot_id": "B5", "text": "build passed"}));
        assert_eq!(sender_id(&msg), "B5");
    }

    #[test]
    fn media_kinds_follow_the_mime_type() {
        let image = File {
            mimetype: "image/png".to_owned(),
            ..File::default()
        };
        assert_eq!(media_kind(&image), "image");
        let voice = File {
            subtype: "slack_audio".to_owned(),
            mimetype: "audio/webm".to_owned(),
            ..File::default()
        };
        assert_eq!(media_kind(&voice), "voice");
        assert_eq!(media_kind(&File::default()), "file");
    }

    #[test]
    fn outgoing_text_is_escaped() {
        assert_eq!(outgoing_text("<!channel> & more"), "&lt;!channel&gt; &amp; more");
    }

    #[test]
    fn text_content_prefers_blocks_and_marks_formatting() {
        let directory = Directory::default();
        let plain = message(serde_json::json!({"text": "hello"}));
        match text_content(&directory, &plain) {
            Some(Content::Text { body, formatted }) => {
                assert_eq!(body, "hello");
                assert_eq!(formatted, None);
            }
            other => panic!("expected text, got {other:?}"),
        }
        let styled = message(serde_json::json!({"text": "*hi*"}));
        match text_content(&directory, &styled) {
            Some(Content::Text { body, formatted }) => {
                assert_eq!(body, "hi");
                assert_eq!(formatted.as_deref(), Some("<b>hi</b>"));
            }
            other => panic!("expected text, got {other:?}"),
        }
        let emote = message(serde_json::json!({"text": "waves", "subtype": "me_message"}));
        assert!(matches!(
            text_content(&directory, &emote),
            Some(Content::Emote { body }) if body == "waves"
        ));
    }
}
