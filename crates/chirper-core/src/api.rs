//! Typed methods of the core (CONTRACT.md section B). Every argument is validated before the call.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::model::{
    Account, Chat, ChatFilter, ChatFlags, CoreSettings, LoginState, Message, Network,
};
use crate::{Core, CoreError, Result, validate};

/// A local file to send as a media message.
#[derive(Debug, Clone, Copy)]
pub struct MediaUpload<'a> {
    pub path: &'a Path,
    pub mime_type: &'a str,
    pub file_name: &'a str,
    pub caption: &'a str,
    /// One of `image`, `video`, `audio`, `file`, `sticker`, `voice`.
    pub kind: &'a str,
    pub reply_to: Option<&'a str>,
    pub txn_id: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventRef {
    event_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HasMore {
    has_more: bool,
}

#[derive(Deserialize)]
struct PathRef {
    path: String,
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

fn optional_event(reply_to: Option<&str>) -> Result<&str> {
    match reply_to {
        None => Ok(""),
        Some(id) => {
            validate::optional_event_id(id)?;
            Ok(id)
        }
    }
}

fn decode<T: DeserializeOwned>(raw: &serde_json::value::RawValue) -> Result<T> {
    Ok(serde_json::from_str(raw.get())?)
}

impl Core {
    async fn request<T: DeserializeOwned>(&self, method: &str, args: Value) -> Result<T> {
        let raw = self.call_raw(method, args).await?;
        decode(&raw)
    }

    /// `start`: starts the bridges that have logins.
    pub async fn start(&self) -> Result<()> {
        self.request("start", json!({})).await
    }

    /// `networks`: the networks that can be used now.
    pub async fn networks(&self) -> Result<Vec<Network>> {
        self.request("networks", json!({})).await
    }

    /// `accounts`: every login.
    pub async fn accounts(&self) -> Result<Vec<Account>> {
        self.request("accounts", json!({})).await
    }

    /// `startLogin`: begins a login flow of `network`.
    pub async fn start_login(&self, network: &str, flow_id: &str) -> Result<LoginState> {
        validate::network_id(network)?;
        validate::token(flow_id)?;
        self.request(
            "startLogin",
            json!({ "network": network, "flowId": flow_id }),
        )
        .await
    }

    /// `submitLoginInput`: answers a user input step.
    pub async fn submit_login_input(
        &self,
        process_id: &str,
        values: &BTreeMap<String, String>,
    ) -> Result<LoginState> {
        validate_login_map(process_id, values)?;
        self.request(
            "submitLoginInput",
            json!({ "processId": process_id, "values": values }),
        )
        .await
    }

    /// `submitLoginCookies`: answers a cookies step.
    pub async fn submit_login_cookies(
        &self,
        process_id: &str,
        cookies: &BTreeMap<String, String>,
    ) -> Result<LoginState> {
        validate_login_map(process_id, cookies)?;
        self.request(
            "submitLoginCookies",
            json!({ "processId": process_id, "cookies": cookies }),
        )
        .await
    }

    /// `waitLoginStep`: blocks until a display-and-wait step completes.
    pub async fn wait_login_step(&self, process_id: &str) -> Result<LoginState> {
        validate::token(process_id)?;
        self.request("waitLoginStep", json!({ "processId": process_id }))
            .await
    }

    /// `cancelLogin`: abandons a login process.
    pub async fn cancel_login(&self, process_id: &str) -> Result<()> {
        validate::token(process_id)?;
        self.request("cancelLogin", json!({ "processId": process_id }))
            .await
    }

    /// `logout`: logs a login out and removes it.
    pub async fn logout(&self, network: &str, login_id: &str) -> Result<()> {
        validate::network_id(network)?;
        validate::token(login_id)?;
        self.request("logout", json!({ "network": network, "loginId": login_id }))
            .await
    }

    /// `listChats`: chats matching the filter. An empty network lists every network.
    pub async fn list_chats(&self, filter: &ChatFilter) -> Result<Vec<Chat>> {
        validate::optional_network_id(&filter.network)?;
        validate::query(&filter.query)?;
        self.request("listChats", json!({ "filter": to_value(filter)? }))
            .await
    }

    /// `getChat`: one chat, or `None` when there is no such room.
    pub async fn get_chat(&self, room_id: &str) -> Result<Option<Chat>> {
        validate::room_id(room_id)?;
        self.request("getChat", json!({ "roomId": room_id })).await
    }

    /// `listMessages`: a page of messages, newest last. `before_order` 0 means the latest page.
    pub async fn list_messages(
        &self,
        room_id: &str,
        before_order: i64,
        limit: u32,
    ) -> Result<Vec<Message>> {
        validate::room_id(room_id)?;
        if before_order < 0 {
            return Err(CoreError::InvalidInput("before order".to_owned()));
        }
        let limit = validate::page_limit(limit)?;
        self.request(
            "listMessages",
            json!({ "roomId": room_id, "beforeOrder": before_order, "limit": limit }),
        )
        .await
    }

    /// `loadOlder`: fetches older history from the network. Returns `false` at the beginning.
    pub async fn load_older(&self, room_id: &str) -> Result<bool> {
        validate::room_id(room_id)?;
        let result: HasMore = self
            .request("loadOlder", json!({ "roomId": room_id }))
            .await?;
        Ok(result.has_more)
    }

    /// `syncHistory`: pulls past history for `network`, or for all networks when empty.
    pub async fn sync_history(&self, network: &str) -> Result<()> {
        validate::optional_network_id(network)?;
        self.request("syncHistory", json!({ "network": network }))
            .await
    }

    /// `markUnread`: marks a chat unread, or read when `unread` is false.
    pub async fn mark_unread(&self, room_id: &str, unread: bool) -> Result<()> {
        validate::room_id(room_id)?;
        self.request("markUnread", json!({ "roomId": room_id, "unread": unread }))
            .await
    }

    /// `sendText`: sends a text message and returns its local event id.
    pub async fn send_text(
        &self,
        room_id: &str,
        text: &str,
        reply_to: Option<&str>,
        txn_id: &str,
    ) -> Result<String> {
        validate::room_id(room_id)?;
        validate::text(text)?;
        let reply_to = optional_event(reply_to)?;
        validate::token(txn_id)?;
        let sent: EventRef = self
            .request(
                "sendText",
                json!({ "roomId": room_id, "text": text, "replyTo": reply_to, "txnId": txn_id }),
            )
            .await?;
        Ok(sent.event_id)
    }

    /// `sendMedia`: sends a local file and returns its local event id. The file is canonicalised
    /// and must exist.
    pub async fn send_media(&self, room_id: &str, media: MediaUpload<'_>) -> Result<String> {
        validate::room_id(room_id)?;
        let path = validate::existing_file(media.path)?;
        let path = path
            .to_str()
            .ok_or_else(|| CoreError::InvalidInput("file path".to_owned()))?
            .to_owned();
        validate::mime_type(media.mime_type)?;
        validate::file_name(media.file_name)?;
        validate::text(media.caption)?;
        validate::media_kind(media.kind)?;
        let reply_to = optional_event(media.reply_to)?;
        validate::token(media.txn_id)?;
        let sent: EventRef = self
            .request(
                "sendMedia",
                json!({
                    "roomId": room_id,
                    "path": path,
                    "mime": media.mime_type,
                    "fileName": media.file_name,
                    "caption": media.caption,
                    "kind": media.kind,
                    "replyTo": reply_to,
                    "txnId": media.txn_id,
                }),
            )
            .await?;
        Ok(sent.event_id)
    }

    /// `edit`: replaces the text of a message.
    pub async fn edit(&self, room_id: &str, event_id: &str, text: &str) -> Result<()> {
        validate::room_id(room_id)?;
        validate::event_id(event_id)?;
        validate::text(text)?;
        self.request(
            "edit",
            json!({ "roomId": room_id, "eventId": event_id, "text": text }),
        )
        .await
    }

    /// `redact`: removes a message.
    pub async fn redact(&self, room_id: &str, event_id: &str) -> Result<()> {
        validate::room_id(room_id)?;
        validate::event_id(event_id)?;
        self.request("redact", json!({ "roomId": room_id, "eventId": event_id }))
            .await
    }

    /// `react`: adds a reaction to a message.
    pub async fn react(&self, room_id: &str, event_id: &str, key: &str) -> Result<()> {
        validate_reaction(room_id, event_id, key)?;
        self.request(
            "react",
            json!({ "roomId": room_id, "eventId": event_id, "key": key }),
        )
        .await
    }

    /// `unreact`: removes the user's reaction from a message.
    pub async fn unreact(&self, room_id: &str, event_id: &str, key: &str) -> Result<()> {
        validate_reaction(room_id, event_id, key)?;
        self.request(
            "unreact",
            json!({ "roomId": room_id, "eventId": event_id, "key": key }),
        )
        .await
    }

    /// `markRead`: moves the read position of a chat to `event_id`.
    pub async fn mark_read(&self, room_id: &str, event_id: &str) -> Result<()> {
        validate::room_id(room_id)?;
        validate::event_id(event_id)?;
        self.request(
            "markRead",
            json!({ "roomId": room_id, "eventId": event_id }),
        )
        .await
    }

    /// `setTyping`: reports whether the user is typing in a chat.
    pub async fn set_typing(&self, room_id: &str, typing: bool) -> Result<()> {
        validate::room_id(room_id)?;
        self.request("setTyping", json!({ "roomId": room_id, "typing": typing }))
            .await
    }

    /// `setChatFlags`: sets local flags. `None` fields are left unchanged.
    pub async fn set_chat_flags(&self, room_id: &str, flags: &ChatFlags) -> Result<()> {
        validate::room_id(room_id)?;
        self.request(
            "setChatFlags",
            json!({ "roomId": room_id, "flags": to_value(flags)? }),
        )
        .await
    }

    /// `mediaFile`: the local path of an `mxc://chirper.local/...` media file.
    pub async fn media_file(&self, mxc_url: &str) -> Result<PathBuf> {
        validate::mxc_url(mxc_url)?;
        let found: PathRef = self.request("mediaFile", json!({ "url": mxc_url })).await?;
        Ok(PathBuf::from(found.path))
    }

    /// `search`: messages whose text contains `query`, newest first.
    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<Message>> {
        validate::query(query)?;
        let limit = validate::page_limit(limit)?;
        self.request("search", json!({ "query": query, "limit": limit }))
            .await
    }

    /// `settings`: the current user settings.
    pub async fn settings(&self) -> Result<CoreSettings> {
        self.request("settings", json!({})).await
    }

    /// `setSettings`: stores the user settings.
    pub async fn set_settings(&self, settings: &CoreSettings) -> Result<()> {
        self.request("setSettings", json!({ "settings": to_value(settings)? }))
            .await
    }
}

fn validate_login_map(process_id: &str, values: &BTreeMap<String, String>) -> Result<()> {
    validate::token(process_id)?;
    for (field, value) in values {
        validate::token(field)?;
        validate::bounded_value(value)?;
    }
    Ok(())
}

fn validate_reaction(room_id: &str, event_id: &str, key: &str) -> Result<()> {
    validate::room_id(room_id)?;
    validate::event_id(event_id)?;
    validate::reaction_key(key)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::ffi::MockBackend;
    use crate::model::ChatFlags;

    fn core(name: &str) -> (Arc<MockBackend>, Core, std::path::PathBuf) {
        let mock = Arc::new(MockBackend::new());
        let dir =
            std::env::temp_dir().join(format!("chirper-core-api-{name}-{}", std::process::id()));
        let (core, _rx) = Core::open(mock.clone(), &dir, None).unwrap();
        (mock, core, dir)
    }

    #[tokio::test]
    async fn typed_helpers_decode_fixtures() {
        let (_mock, core, dir) = core("decode");
        let networks = core.networks().await.unwrap();
        assert_eq!(networks[0].id, "discord");
        assert!(networks[0].capabilities.read_receipts);
        let chats = core.list_chats(&ChatFilter::default()).await.unwrap();
        assert_eq!(chats[0].room_id, "!mock1:chirper.local");
        assert!(
            core.get_chat("!mock1:chirper.local")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            core.list_messages("!mock1:chirper.local", 0, 50)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(core.settings().await.unwrap().send_typing["*"] == Value::Bool(true));
        core.set_chat_flags(
            "!mock1:chirper.local",
            &ChatFlags {
                pinned: Some(true),
                ..ChatFlags::default()
            },
        )
        .await
        .unwrap();
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn invalid_arguments_never_reach_the_core() {
        let (mock, core, dir) = core("invalid");
        let calls_before = mock.call_count();
        assert!(matches!(
            core.get_chat("not-a-room").await,
            Err(CoreError::InvalidInput(_))
        ));
        assert!(matches!(
            core.list_chats(&ChatFilter {
                network: "Bad Net".into(),
                ..ChatFilter::default()
            })
            .await,
            Err(CoreError::InvalidInput(_))
        ));
        assert!(matches!(
            core.send_text("!mock1:chirper.local", "hi", Some("bad"), "t")
                .await,
            Err(CoreError::InvalidInput(_))
        ));
        assert!(matches!(
            core.react("!mock1:chirper.local", "$e", "").await,
            Err(CoreError::InvalidInput(_))
        ));
        assert!(matches!(
            core.media_file("https://example.invalid/x").await,
            Err(CoreError::InvalidInput(_))
        ));
        assert!(matches!(
            core.list_messages("!mock1:chirper.local", -1, 10).await,
            Err(CoreError::InvalidInput(_))
        ));
        assert_eq!(mock.call_count(), calls_before);
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn send_text_returns_event_id_and_media_checks_file() {
        let (_mock, core, dir) = core("send");
        let id = core
            .send_text("!mock1:chirper.local", "hello", None, "txn.1")
            .await
            .unwrap();
        assert_eq!(id, crate::ffi::MOCK_EVENT_ID);

        let missing = MediaUpload {
            path: &dir.join("missing.bin"),
            mime_type: "application/octet-stream",
            file_name: "missing.bin",
            caption: "",
            kind: "file",
            reply_to: None,
            txn_id: "txn.2",
        };
        assert!(matches!(
            core.send_media("!mock1:chirper.local", missing).await,
            Err(CoreError::Io(_))
        ));
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
