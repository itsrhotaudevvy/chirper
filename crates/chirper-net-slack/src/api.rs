//! The Slack Web API client: one place for the request shape, the auth headers, the status and
//! `ok` mapping onto the bridge's errors, file transfers, and the allowlist of hosts a token may be
//! sent to.

use std::fmt;

use log::debug;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, HeaderMap, RETRY_AFTER};
use reqwest::{StatusCode, Url};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use chirper_bridge::{Error, Result};

use crate::wire::{
    AuthTest, Bot, BotInfo, Channel, CompletedUpload, ConversationsList, History, Posted,
    ReactionsGet, RtmConnect, UploadUrl, User, UserInfo, UsersList,
};

/// The Web API root of the public Slack cloud.
pub const SLACK_API: &str = "https://slack.com/api";
/// Largest file a message may carry into the local store.
pub const MAX_FILE_BYTES: u64 = 100 << 20;
/// The page size of the list methods.
const PAGE: &str = "200";
/// The retry hint used when Slack asks to wait without saying for how long.
const DEFAULT_RETRY_MS: u64 = 1_000;

/// A client for one token, and the cookie that goes with a browser token.
#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    base: String,
    base_host: Option<String>,
    token: String,
    cookie: Option<String>,
}

impl fmt::Debug for Api {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Api")
            .field("base", &self.base)
            .field("token", &"[redacted]")
            .field("cookie", &self.cookie.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

impl Api {
    /// A client for `base` (the Web API root, ending in `/api`) with the given token and cookie.
    pub fn new(http: reqwest::Client, base: &str, token: &str, cookie: Option<String>) -> Self {
        let base = base.trim_end_matches('/').to_owned();
        let base_host = Url::parse(&base)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned));
        Self {
            http,
            base,
            base_host,
            token: token.trim().to_owned(),
            cookie,
        }
    }

    /// Whether the token is a bot token. Bots cannot mark conversations as read.
    pub fn is_bot(&self) -> bool {
        self.token.starts_with("xoxb-")
    }

    /// `auth.test`: validates the token and names its team and user.
    pub async fn auth_test(&self) -> Result<AuthTest> {
        self.call("auth.test", &[]).await
    }

    /// `rtm.connect`: the websocket URL for the real-time events.
    pub async fn rtm_connect(&self) -> Result<RtmConnect> {
        let answer: RtmConnect = self.call("rtm.connect", &[]).await?;
        let url = Url::parse(&answer.url)
            .map_err(|_| Error::Protocol("Slack sent an invalid socket URL".to_owned()))?;
        if !self.is_allowed(&url, &["ws", "wss"]) {
            return Err(Error::Protocol(
                "Slack sent a socket URL outside Slack".to_owned(),
            ));
        }
        Ok(answer)
    }

    /// One page of the conversations the user can see: channels, private channels, group DMs and
    /// DMs.
    pub async fn conversations_list(&self, cursor: &str) -> Result<ConversationsList> {
        let mut params = vec![
            ("types", "public_channel,private_channel,mpim,im"),
            ("limit", PAGE),
            ("exclude_archived", "true"),
        ];
        if !cursor.is_empty() {
            params.push(("cursor", cursor));
        }
        self.call("conversations.list", &params).await
    }

    /// One page of the workspace's users.
    pub async fn users_list(&self, cursor: &str) -> Result<UsersList> {
        let mut params = vec![("limit", PAGE)];
        if !cursor.is_empty() {
            params.push(("cursor", cursor));
        }
        self.call("users.list", &params).await
    }

    /// One user by id.
    pub async fn user_info(&self, id: &str) -> Result<User> {
        let answer: UserInfo = self.call("users.info", &[("user", id)]).await?;
        Ok(answer.user)
    }

    /// One bot by id.
    pub async fn bot_info(&self, id: &str) -> Result<Bot> {
        let answer: BotInfo = self.call("bots.info", &[("bot", id)]).await?;
        Ok(answer.bot)
    }

    /// One conversation, the newest message first. `latest` is exclusive: the page holds messages
    /// older than it.
    pub async fn history(
        &self,
        channel: &str,
        latest: Option<&str>,
        limit: usize,
    ) -> Result<History> {
        let limit = limit.clamp(1, 200).to_string();
        let mut params = vec![
            ("channel", channel),
            ("limit", limit.as_str()),
            ("inclusive", "false"),
        ];
        if let Some(latest) = latest {
            params.push(("latest", latest));
        }
        self.call("conversations.history", &params).await
    }

    /// Post a message, optionally into a thread. The answer carries its timestamp.
    pub async fn post_message(
        &self,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<Posted> {
        let mut params = vec![
            ("channel", channel),
            ("text", text),
            ("unfurl_links", "false"),
            ("unfurl_media", "false"),
        ];
        if let Some(thread) = thread_ts {
            params.push(("thread_ts", thread));
        }
        self.call("chat.postMessage", &params).await
    }

    /// Replace the text of one of the user's messages.
    pub async fn update_message(&self, channel: &str, ts: &str, text: &str) -> Result<()> {
        self.call_unit(
            "chat.update",
            &[("channel", channel), ("ts", ts), ("text", text)],
        )
        .await
    }

    /// Delete one of the user's messages.
    pub async fn delete_message(&self, channel: &str, ts: &str) -> Result<()> {
        self.call_unit("chat.delete", &[("channel", channel), ("ts", ts)])
            .await
    }

    /// Add or remove the user's reaction. `name` is an emoji name without colons.
    pub async fn reaction(&self, add: bool, name: &str, channel: &str, ts: &str) -> Result<()> {
        let method = if add {
            "reactions.add"
        } else {
            "reactions.remove"
        };
        self.call_unit(
            method,
            &[("name", name), ("channel", channel), ("timestamp", ts)],
        )
        .await
    }

    /// The reactions of one message, with the users who gave them.
    pub async fn reactions_get(&self, channel: &str, ts: &str) -> Result<ReactionsGet> {
        self.call("reactions.get", &[("channel", channel), ("timestamp", ts)])
            .await
    }

    /// Mark a conversation as read up to a message.
    pub async fn mark(&self, channel: &str, ts: &str) -> Result<()> {
        self.call_unit("conversations.mark", &[("channel", channel), ("ts", ts)])
            .await
    }

    /// Step one of a file upload: where to send the bytes, and the id the file will have.
    pub async fn upload_url(&self, filename: &str, length: usize) -> Result<UploadUrl> {
        let length = length.to_string();
        self.call(
            "files.getUploadURLExternal",
            &[("filename", filename), ("length", length.as_str())],
        )
        .await
    }

    /// Step two: send the file's bytes to the URL from [`Api::upload_url`].
    pub async fn put_upload(&self, url: &str, mime: &str, data: &[u8]) -> Result<()> {
        let target = Url::parse(url)
            .ok()
            .filter(|target| self.is_allowed(target, &["https", "http"]))
            .ok_or_else(|| Error::Protocol("Slack sent an upload URL outside Slack".to_owned()))?;
        let response = self
            .http
            .post(target)
            .header(CONTENT_TYPE, mime)
            .body(data.to_vec())
            .send()
            .await
            .map_err(network_error)?;
        check_status(response.status(), response.headers())
    }

    /// Step three: share the uploaded file in a conversation, optionally into a thread.
    pub async fn complete_upload(
        &self,
        file_id: &str,
        title: &str,
        channel: &str,
        thread_ts: Option<&str>,
        comment: &str,
    ) -> Result<CompletedUpload> {
        let files = json!([{ "id": file_id, "title": title }]).to_string();
        let mut params = vec![("files", files.as_str()), ("channel_id", channel)];
        if let Some(thread) = thread_ts {
            params.push(("thread_ts", thread));
        }
        if !comment.is_empty() {
            params.push(("initial_comment", comment));
        }
        self.call("files.completeUploadExternal", &params).await
    }

    /// Download a file from Slack, refusing anything above `limit` bytes. The token and cookie are
    /// sent only to Slack's own hosts.
    pub async fn download(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
        let target = Url::parse(url)
            .ok()
            .filter(|target| self.is_allowed(target, &["https", "http"]))
            .ok_or_else(|| Error::Protocol("refusing a non-Slack file URL".to_owned()))?;
        let mut request = self
            .http
            .get(target)
            .header(AUTHORIZATION, format!("Bearer {}", self.token));
        if let Some(cookie) = &self.cookie {
            request = request.header(COOKIE, format!("d={cookie}"));
        }
        let mut response = request.send().await.map_err(network_error)?;
        check_status(response.status(), response.headers())?;
        if response.content_length().is_some_and(|len| len > limit) {
            return Err(too_large());
        }
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(network_error)? {
            if data.len() as u64 + chunk.len() as u64 > limit {
                return Err(too_large());
            }
            data.extend_from_slice(&chunk);
        }
        if data.starts_with(b"<!DOCTYPE html>") || data.starts_with(b"<!doctype html>") {
            return Err(Error::Protocol(
                "Slack sent a web page instead of the file".to_owned(),
            ));
        }
        Ok(data)
    }

    /// Whether a URL may receive this client's credentials: the configured API host, or a Slack
    /// host over TLS. `schemes` lists the schemes accepted for Slack hosts.
    fn is_allowed(&self, url: &Url, schemes: &[&str]) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        if self.base_host.as_deref() == Some(host) {
            return true;
        }
        let secure = matches!(url.scheme(), "https" | "wss");
        let slack_host = host == "slack.com"
            || host.ends_with(".slack.com")
            || host.ends_with(".slack-edge.com");
        schemes.contains(&url.scheme()) && secure && slack_host
    }

    /// A method whose answer is a JSON document.
    async fn call<T: DeserializeOwned>(&self, method: &str, params: &[(&str, &str)]) -> Result<T> {
        let value = self.raw(method, params).await?;
        serde_json::from_value(value)
            .map_err(|err| Error::Protocol(format!("Slack {method} answer: {err}")))
    }

    /// A method whose answer is only `ok`.
    async fn call_unit(&self, method: &str, params: &[(&str, &str)]) -> Result<()> {
        self.raw(method, params).await.map(|_| ())
    }

    async fn raw(&self, method: &str, params: &[(&str, &str)]) -> Result<Value> {
        let mut pairs = Vec::with_capacity(params.len() + 1);
        pairs.push(("token", self.token.as_str()));
        pairs.extend_from_slice(params);
        let body = form_body(&pairs);
        let mut request = self
            .http
            .post(format!("{}/{method}", self.base))
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body);
        if let Some(cookie) = &self.cookie {
            request = request.header(COOKIE, format!("d={cookie}"));
        }
        let response = request.send().await.map_err(network_error)?;
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.bytes().await.map_err(network_error)?;
        check_status(status, &headers)?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| Error::Protocol(format!("Slack {method} sent no JSON")))?;
        if value.get("ok").and_then(Value::as_bool) == Some(true) {
            debug!("slack: {method} ok");
            return Ok(value);
        }
        let code = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        Err(slack_error(code))
    }
}

/// The form body of a request: every key and value percent-encoded.
pub fn form_body(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                utf8_percent_encode(key, NON_ALPHANUMERIC),
                utf8_percent_encode(value, NON_ALPHANUMERIC)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// The bridge error of a Slack `ok: false` answer. Error codes are Slack's own words, never
/// credentials, so they may appear in the message.
pub fn slack_error(code: &str) -> Error {
    match code {
        "invalid_auth"
        | "not_authed"
        | "token_revoked"
        | "token_expired"
        | "account_inactive"
        | "team_disabled"
        | "user_removed_from_team" => Error::Auth(format!("Slack refused the token ({code})")),
        "ratelimited" | "rate_limited" => Error::RateLimited {
            retry_after_ms: DEFAULT_RETRY_MS,
        },
        "internal_error" | "fatal_error" | "service_unavailable" | "request_timeout" => {
            Error::Network(format!("Slack answered {code}"))
        }
        other => Error::Protocol(format!("Slack refused the request: {other}")),
    }
}

/// The bridge error of an HTTP status. Any 2xx is success.
fn check_status(status: StatusCode, headers: &HeaderMap) -> Result<()> {
    if status.is_success() {
        return Ok(());
    }
    match status.as_u16() {
        401 | 403 => Err(Error::Auth("Slack refused the token".to_owned())),
        429 => Err(Error::RateLimited {
            retry_after_ms: retry_after_ms(headers),
        }),
        500..=599 => Err(Error::Network(format!("Slack answered {status}"))),
        _ => Err(Error::Protocol(format!("Slack answered {status}"))),
    }
}

fn retry_after_ms(headers: &HeaderMap) -> u64 {
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.trim().parse::<f64>().ok())
        .filter(|secs| secs.is_finite() && *secs >= 0.0)
        .map_or(DEFAULT_RETRY_MS, |secs| (secs * 1000.0).ceil() as u64)
}

fn network_error(err: reqwest::Error) -> Error {
    if err.is_timeout() || err.is_connect() || err.is_request() {
        Error::Network(err.to_string())
    } else {
        Error::Protocol(err.to_string())
    }
}

fn too_large() -> Error {
    Error::Protocol("attachment is larger than 100 MiB".to_owned())
}

/// The channel name of a conversation as the directory stores it.
pub fn channel_label(channel: &Channel) -> String {
    if channel.is_im {
        String::new()
    } else if channel.is_mpim {
        mpim_label(&channel.name)
    } else if channel.is_channel && !channel.is_private && !channel.is_group {
        format!("#{}", channel.name)
    } else {
        channel.name.clone()
    }
}

/// `mpdm-ada--grace--linus-1` becomes `ada, grace, linus`: the names of the group's members.
fn mpim_label(name: &str) -> String {
    name.strip_prefix("mpdm-")
        .unwrap_or(name)
        .strip_suffix("-1")
        .unwrap_or(name)
        .split("--")
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn client(base: &str) -> Api {
        Api::new(reqwest::Client::new(), base, "xoxb-test", None)
    }

    #[test]
    fn form_bodies_encode_every_value() {
        assert_eq!(
            form_body(&[("text", "a b&c=d"), ("ts", "1.2")]),
            "text=a%20b%26c%3Dd&ts=1%2E2"
        );
    }

    #[test]
    fn slack_error_codes_map_onto_the_bridge_errors() {
        assert!(matches!(slack_error("invalid_auth"), Error::Auth(_)));
        assert!(matches!(slack_error("token_revoked"), Error::Auth(_)));
        assert!(matches!(
            slack_error("ratelimited"),
            Error::RateLimited {
                retry_after_ms: 1000
            }
        ));
        assert!(matches!(slack_error("internal_error"), Error::Network(_)));
        assert!(matches!(
            slack_error("channel_not_found"),
            Error::Protocol(message) if message.contains("channel_not_found")
        ));
    }

    #[test]
    fn http_statuses_map_onto_the_bridge_errors() {
        let none = HeaderMap::new();
        assert!(check_status(StatusCode::OK, &none).is_ok());
        assert!(matches!(
            check_status(StatusCode::UNAUTHORIZED, &none),
            Err(Error::Auth(_))
        ));
        assert!(matches!(
            check_status(StatusCode::FORBIDDEN, &none),
            Err(Error::Auth(_))
        ));
        assert!(matches!(
            check_status(StatusCode::BAD_GATEWAY, &none),
            Err(Error::Network(_))
        ));
        assert!(matches!(
            check_status(StatusCode::NOT_FOUND, &none),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn retry_after_is_read_in_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("2.5"));
        assert_eq!(retry_after_ms(&headers), 2500);
        assert_eq!(retry_after_ms(&HeaderMap::new()), 1000);
        let err = check_status(StatusCode::TOO_MANY_REQUESTS, &headers).unwrap_err();
        assert!(matches!(
            err,
            Error::RateLimited {
                retry_after_ms: 2500
            }
        ));
    }

    #[test]
    fn credentials_only_go_to_slack_hosts() {
        let api = client(SLACK_API);
        let allowed = |url: &str, schemes: &[&str]| {
            Url::parse(url).is_ok_and(|url| api.is_allowed(&url, schemes))
        };
        assert!(allowed(
            "https://files.slack.com/files-pri/T1/x",
            &["https"]
        ));
        assert!(allowed("wss://wss-primary.slack.com/x", &["ws", "wss"]));
        assert!(!allowed("http://files.slack.com/x", &["https"]));
        assert!(!allowed("https://slack.com.evil.example/x", &["https"]));
        assert!(!allowed("https://evil.example/slack.com", &["https"]));
        assert!(!allowed("wss://wss-primary.slack.com/x", &["https"]));
    }

    #[test]
    fn the_configured_host_is_trusted_for_tests() {
        let api = client("http://127.0.0.1:9/api");
        assert!(
            Url::parse("http://127.0.0.1:9/files/1")
                .is_ok_and(|url| api.is_allowed(&url, &["http"]))
        );
        assert!(
            !Url::parse("http://127.0.0.2:9/files/1")
                .is_ok_and(|url| api.is_allowed(&url, &["http"]))
        );
    }

    #[test]
    fn bot_tokens_are_recognised() {
        assert!(client(SLACK_API).is_bot());
        let user = Api::new(reqwest::Client::new(), SLACK_API, "xoxc-1", None);
        assert!(!user.is_bot());
    }

    #[test]
    fn debug_output_hides_the_token_and_cookie() {
        let api = Api::new(
            reqwest::Client::new(),
            SLACK_API,
            "xoxc-secret",
            Some("xoxd-secret".into()),
        );
        let text = format!("{api:?}");
        assert!(!text.contains("secret"));
        assert!(text.contains("[redacted]"));
    }

    #[test]
    fn mpim_names_become_member_lists() {
        assert_eq!(mpim_label("mpdm-ada--grace--linus-1"), "ada, grace, linus");
    }
}
