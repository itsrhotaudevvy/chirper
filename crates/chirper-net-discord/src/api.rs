//! The Discord REST client: one place for the API version, the auth header, rate limits, and the
//! mapping of every HTTP status onto the bridge's errors.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use log::{debug, warn};
use reqwest::header::{AUTHORIZATION, HeaderMap, USER_AGENT};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use chirper_bridge::{Error, Result};

use crate::map::{self, is_snowflake};

/// The API version every request uses.
pub const API: &str = "https://discord.com/api/v10";
/// The CDN base of avatars and attachments.
pub const CDN: &str = "https://cdn.discordapp.com";
/// Discord's limit on one message's `content`, in characters.
pub const MAX_CONTENT: usize = 2000;

/// Bot tokens need the `DiscordBot` agent prefix that Discord asks of bot applications.
const BOT_AGENT: &str = concat!(
    "DiscordBot (https://chirper.app, ",
    env!("CARGO_PKG_VERSION"),
    ")"
);
/// User tokens keep the agent they have always used.
const USER_AGENT_VALUE: &str = "Chirper (https://chirper.app, 1.0)";
/// Times one request is repeated after a 429 whose wait is short enough to honour.
const MAX_RATE_RETRIES: u32 = 3;
/// The longest 429 wait the client sleeps through; longer ones go back to the caller.
const MAX_RETRY_WAIT_SECS: f64 = 60.0;
/// The longest pre-emptive wait for a bucket that is known to be empty.
const MAX_PREEMPTIVE_WAIT: Duration = Duration::from_secs(60);
/// Bucket resets are clamped to this, so a hostile header cannot overflow an `Instant`.
const MAX_RESET_SECS: f64 = 3600.0;
/// Characters of an error body that go into an error message.
const MAX_QUOTED_CHARS: usize = 300;
/// Bytes of an error body read from a CDN answer.
const MAX_ERROR_BYTES: usize = 4096;

/// A REST client for one token.
#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    token: String,
    bot: bool,
    base: String,
    agent: &'static str,
    /// Bucket key (bucket id and major parameter) to the instant the bucket refills.
    buckets: Arc<Mutex<HashMap<String, Instant>>>,
    /// Route template to the bucket id Discord reported for it.
    routes: Arc<Mutex<HashMap<String, String>>>,
}

impl std::fmt::Debug for Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Api")
            .field("bot", &self.bot)
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

/// Whether a token belongs to a bot application (`Bot …`) rather than a user.
pub fn is_bot_token(token: &str) -> bool {
    token
        .trim_start()
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bot "))
}

impl Api {
    pub fn new(http: reqwest::Client, token: &str, base: &str) -> Self {
        let token = token.trim().to_owned();
        let bot = is_bot_token(&token);
        let token = token
            .strip_prefix("Bot ")
            .or_else(|| token.strip_prefix("bot "))
            .unwrap_or(&token)
            .trim()
            .to_owned();
        Self {
            http,
            token,
            bot,
            base: base.trim_end_matches('/').to_owned(),
            agent: if bot { BOT_AGENT } else { USER_AGENT_VALUE },
            buckets: Arc::default(),
            routes: Arc::default(),
        }
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{path}", self.base);
        let auth = if self.bot {
            format!("Bot {}", self.token)
        } else {
            self.token.clone()
        };
        self.http
            .request(method, url)
            .header(AUTHORIZATION, auth)
            .header(USER_AGENT, self.agent)
    }

    /// Send one request, honouring the known rate-limit buckets and repeating short 429 waits. The
    /// answer comes back unmapped; [`check`] decides what it means.
    async fn execute(
        &self,
        method: &Method,
        path: &str,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<Reply> {
        let route = route_key(method, path);
        let major = major_param(path);
        let mut retries = 0;
        loop {
            if let Some(wait) = self.bucket_wait(&route, &major) {
                tokio::time::sleep(wait).await;
            }
            let response = build().send().await.map_err(network_error)?;
            let status = response.status();
            let headers = response.headers().clone();
            let body = response.bytes().await.map_err(network_error)?.to_vec();
            self.remember(&route, &major, &headers);
            if status == StatusCode::TOO_MANY_REQUESTS && retries < MAX_RATE_RETRIES {
                let wait = RetryAfter::read(&headers, &body);
                if wait.secs <= MAX_RETRY_WAIT_SECS {
                    debug!(
                        "discord: {} rate limit on {route}, retrying in {:.2}s",
                        if wait.global { "global" } else { "route" },
                        wait.secs
                    );
                    retries += 1;
                    tokio::time::sleep(Duration::from_secs_f64(wait.secs)).await;
                    continue;
                }
            }
            return Ok(Reply {
                status,
                headers,
                body,
            });
        }
    }

    /// One request with an optional JSON body.
    async fn call(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Reply> {
        self.execute(&method, path, || {
            let request = self.request(method.clone(), path);
            match body {
                Some(body) => request.json(body),
                None => request,
            }
        })
        .await
    }

    /// A request whose answer is a JSON document.
    async fn send_json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        let reply = self.call(method, path, body.as_ref()).await?;
        check(reply.status, &reply.headers, &reply.body)?;
        serde_json::from_slice(&reply.body)
            .map_err(|e| Error::Protocol(format!("Discord JSON: {e}")))
    }

    /// A request whose answer is not used. A DELETE that finds nothing has already done its job.
    async fn send_empty(&self, method: Method, path: &str) -> Result<()> {
        let reply = self.call(method.clone(), path, None).await?;
        if reply.status == StatusCode::NOT_FOUND && method == Method::DELETE {
            return Ok(());
        }
        check(reply.status, &reply.headers, &reply.body)
    }

    /// The logged-in user, which also validates the token.
    pub async fn me(&self) -> Result<map::User> {
        self.send_json(Method::GET, "/users/@me", None).await
    }

    /// The gateway URL to connect to.
    pub async fn gateway_url(&self) -> Result<String> {
        let value: Value = self.send_json(Method::GET, "/gateway", None).await?;
        value
            .get("url")
            .and_then(|u| u.as_str())
            .map(str::to_owned)
            .ok_or_else(|| Error::Protocol("Discord sent no gateway url".to_owned()))
    }

    pub async fn guild_channels(&self, guild: &str) -> Result<Vec<map::Channel>> {
        self.send_json(Method::GET, &format!("/guilds/{guild}/channels"), None)
            .await
    }

    /// One page of messages, newest first, at most `limit` (1 to 100).
    pub async fn history(
        &self,
        channel: &str,
        before: Option<&str>,
        limit: usize,
    ) -> Result<Vec<map::Message>> {
        let mut path = format!("/channels/{channel}/messages?limit={}", limit.clamp(1, 100));
        if let Some(before) = before {
            path.push_str("&before=");
            path.push_str(before);
        }
        self.send_json(Method::GET, &path, None).await
    }

    pub async fn message(&self, channel: &str, message: &str) -> Result<map::Message> {
        self.send_json(
            Method::GET,
            &format!("/channels/{channel}/messages/{message}"),
            None,
        )
        .await
    }

    /// Send text, split into as many messages as Discord's limit needs. The first part carries
    /// the reply; the first message is returned, since the engine tracks that one.
    pub async fn send_text(
        &self,
        channel: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<map::Message> {
        let mut first: Option<map::Message> = None;
        for (index, part) in split_text(text, MAX_CONTENT).iter().enumerate() {
            let reply = if index == 0 { reply_to } else { None };
            match self.post_message(channel, part, reply).await {
                Ok(message) => {
                    first.get_or_insert(message);
                }
                Err(err) if first.is_some() => {
                    warn!("discord: a later part of a long message was not sent: {err}");
                    break;
                }
                Err(err) => return Err(err),
            }
        }
        first.ok_or_else(|| Error::Protocol("Discord sent no message".to_owned()))
    }

    async fn post_message(
        &self,
        channel: &str,
        content: &str,
        reply_to: Option<&str>,
    ) -> Result<map::Message> {
        let mut body = json!({ "content": content, "allowed_mentions": {"parse": []} });
        if let Some(reply) = reply_to {
            body["message_reference"] = message_reference(reply);
        }
        self.send_json(
            Method::POST,
            &format!("/channels/{channel}/messages"),
            Some(body),
        )
        .await
    }

    /// Upload one attachment with the first part of its caption; the rest of a long caption
    /// follows as text. Returns the attachment's message.
    pub async fn send_media(
        &self,
        channel: &str,
        data: Vec<u8>,
        file_name: &str,
        mime: &str,
        caption: &str,
        reply_to: Option<&str>,
    ) -> Result<map::Message> {
        let mut parts = split_text(caption, MAX_CONTENT).into_iter();
        let first = parts.next().unwrap_or_default();
        let message = self
            .upload(channel, &data, file_name, mime, &first, reply_to)
            .await?;
        for part in parts {
            if let Err(err) = self.post_message(channel, &part, None).await {
                warn!("discord: a later part of a long caption was not sent: {err}");
                break;
            }
        }
        Ok(message)
    }

    async fn upload(
        &self,
        channel: &str,
        data: &[u8],
        file_name: &str,
        mime: &str,
        caption: &str,
        reply_to: Option<&str>,
    ) -> Result<map::Message> {
        let mut payload = json!({ "content": caption, "allowed_mentions": {"parse": []} });
        if let Some(reply) = reply_to {
            payload["message_reference"] = message_reference(reply);
        }
        let payload = payload.to_string();
        let path = format!("/channels/{channel}/messages");
        // The part is rebuilt for every attempt, because a multipart body cannot be resent.
        let make_part =
            || reqwest::multipart::Part::bytes(data.to_vec()).file_name(file_name.to_owned());
        make_part()
            .mime_str(mime)
            .map_err(|e| Error::Protocol(format!("attachment type: {e}")))?;
        let build = || {
            let part = make_part().mime_str(mime).unwrap_or_else(|_| make_part());
            let form = reqwest::multipart::Form::new()
                .text("payload_json", payload.clone())
                .part("files[0]", part);
            self.request(Method::POST, &path).multipart(form)
        };
        let reply = self.execute(&Method::POST, &path, build).await?;
        check(reply.status, &reply.headers, &reply.body)?;
        serde_json::from_slice(&reply.body)
            .map_err(|e| Error::Protocol(format!("Discord JSON: {e}")))
    }

    pub async fn edit(&self, channel: &str, message: &str, text: &str) -> Result<()> {
        let _: Value = self
            .send_json(
                Method::PATCH,
                &format!("/channels/{channel}/messages/{message}"),
                Some(json!({ "content": text })),
            )
            .await?;
        Ok(())
    }

    pub async fn delete(&self, channel: &str, message: &str) -> Result<()> {
        self.send_empty(
            Method::DELETE,
            &format!("/channels/{channel}/messages/{message}"),
        )
        .await
    }

    /// Add or remove the user's own reaction.
    pub async fn react(&self, channel: &str, message: &str, key: &str, add: bool) -> Result<()> {
        let emoji = encode_reaction(key);
        let method = if add { Method::PUT } else { Method::DELETE };
        self.send_empty(
            method,
            &format!("/channels/{channel}/messages/{message}/reactions/{emoji}/@me"),
        )
        .await
    }

    pub async fn typing(&self, channel: &str) -> Result<()> {
        self.send_empty(Method::POST, &format!("/channels/{channel}/typing"))
            .await
    }

    /// Download a file from one of Discord's media hosts, refusing anything above `limit` bytes.
    /// The request carries no Authorization header.
    pub async fn download(&self, url: &str, limit: usize) -> Result<Vec<u8>> {
        let target = reqwest::Url::parse(url)
            .ok()
            .filter(is_discord_media)
            .ok_or_else(refused_media)?;
        let mut response = self.http.get(target).send().await.map_err(network_error)?;
        // Redirects are followed by the client, so the host that answered is checked too.
        if !is_discord_media(response.url()) {
            return Err(refused_media());
        }
        let status = response.status();
        let headers = response.headers().clone();
        if !status.is_success() {
            let body = read_capped(&mut response, MAX_ERROR_BYTES).await?;
            check(status, &headers, &body)?;
        }
        if response
            .content_length()
            .is_some_and(|len| len > limit as u64)
        {
            return Err(too_large());
        }
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(network_error)? {
            if data.len() + chunk.len() > limit {
                return Err(too_large());
            }
            data.extend_from_slice(&chunk);
        }
        Ok(data)
    }

    /// The pre-emptive wait for a request whose bucket is known to be empty, if any.
    fn bucket_wait(&self, route: &str, major: &str) -> Option<Duration> {
        let bucket = self.routes.lock().ok()?.get(route)?.clone();
        let reset = *self.buckets.lock().ok()?.get(&bucket_key(&bucket, major))?;
        let now = Instant::now();
        (reset > now).then(|| (reset - now).min(MAX_PREEMPTIVE_WAIT))
    }

    /// Learn the bucket of a route from one answer, and when the bucket is empty, when it refills.
    fn remember(&self, route: &str, major: &str, headers: &HeaderMap) {
        let Some(bucket) = header_str(headers, "x-ratelimit-bucket") else {
            return;
        };
        if let Ok(mut routes) = self.routes.lock() {
            routes.insert(route.to_owned(), bucket.to_owned());
        }
        if header_str(headers, "x-ratelimit-remaining") != Some("0") {
            return;
        }
        let Some(secs) = header_str(headers, "x-ratelimit-reset-after")
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|s| s.is_finite() && *s >= 0.0)
        else {
            return;
        };
        let reset = Instant::now() + Duration::from_secs_f64(secs.min(MAX_RESET_SECS));
        if let Ok(mut buckets) = self.buckets.lock() {
            buckets.insert(bucket_key(bucket, major), reset);
        }
    }
}

/// One answered request, before its status is mapped.
struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

/// The single status handler: every request of this client is mapped through it.
pub fn check(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> Result<()> {
    if status.is_success() {
        return Ok(());
    }
    match status.as_u16() {
        401 => Err(Error::Auth("Discord refused the token".to_owned())),
        // A missing permission is about one chat, so it must not log the whole account out.
        403 => Err(Error::Protocol(
            "Discord did not allow this action (missing permission)".to_owned(),
        )),
        429 => Err(Error::RateLimited {
            retry_after_ms: RetryAfter::read(headers, body).millis(),
        }),
        500..=599 => Err(Error::Network(format!("Discord answered {status}"))),
        _ => Err(Error::Protocol(format!(
            "Discord answered {status}: {}",
            quote(body)
        ))),
    }
}

/// How long a 429 asks to wait, and whether the limit is global.
struct RetryAfter {
    secs: f64,
    global: bool,
}

impl RetryAfter {
    fn read(headers: &HeaderMap, body: &[u8]) -> Self {
        let json = serde_json::from_slice::<Value>(body).ok();
        let secs = json
            .as_ref()
            .and_then(|v| v.get("retry_after"))
            .and_then(Value::as_f64)
            .or_else(|| header_str(headers, "retry-after").and_then(|s| s.trim().parse().ok()))
            .filter(|s: &f64| s.is_finite() && *s >= 0.0)
            .unwrap_or(1.0);
        let global = json
            .as_ref()
            .and_then(|v| v.get("global"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || headers.contains_key("x-ratelimit-global");
        Self { secs, global }
    }

    fn millis(&self) -> u64 {
        (self.secs * 1000.0).ceil() as u64
    }
}

/// Whether a URL is HTTPS on one of Discord's media hosts.
pub fn is_discord_media(url: &reqwest::Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    match url.host_str() {
        Some(host) => {
            host == "cdn.discordapp.com"
                || host == "media.discordapp.net"
                || host.ends_with(".discordapp.net")
                || host.ends_with(".discordapp.com")
        }
        None => false,
    }
}

fn refused_media() -> Error {
    Error::Protocol("refusing a non-Discord media URL".to_owned())
}

fn too_large() -> Error {
    Error::Protocol("attachment is too large".to_owned())
}

/// Read at most `cap` bytes of a response body.
async fn read_capped(response: &mut reqwest::Response, cap: usize) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    while data.len() < cap {
        let Some(chunk) = response.chunk().await.map_err(network_error)? else {
            break;
        };
        let room = cap - data.len();
        data.extend_from_slice(&chunk[..chunk.len().min(room)]);
    }
    Ok(data)
}

/// The first characters of an error body, cut on a character boundary.
fn quote(body: &[u8]) -> String {
    String::from_utf8_lossy(body)
        .chars()
        .take(MAX_QUOTED_CHARS)
        .collect()
}

/// Split text into pieces of at most `max` characters. A piece ends at the last line break in its
/// window, else at the last space, else inside a word at a character boundary. No piece is empty
/// unless the whole text is.
pub fn split_text(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut pieces = Vec::new();
    let mut rest = text;
    loop {
        // Byte offset just past the `max`th character, or the end of the text.
        let window_end = rest
            .char_indices()
            .nth(max)
            .map_or(rest.len(), |(index, _)| index);
        if window_end == rest.len() {
            pieces.push(rest.to_owned());
            break;
        }
        let window = &rest[..window_end];
        let (cut, skip) = match window.rfind('\n').filter(|&index| index > 0) {
            Some(index) => (index, '\n'.len_utf8()),
            None => match window
                .char_indices()
                .rev()
                .find(|(index, c)| *index > 0 && c.is_whitespace())
            {
                Some((index, c)) => (index, c.len_utf8()),
                None => (window_end, 0),
            },
        };
        pieces.push(rest[..cut].to_owned());
        rest = &rest[cut + skip..];
    }
    pieces
}

fn message_reference(message: &str) -> Value {
    json!({ "message_id": message, "fail_if_not_exists": false })
}

fn bucket_key(bucket: &str, major: &str) -> String {
    format!("{bucket}:{major}")
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

/// The rate-limit route of a request: its method and path with every snowflake replaced by `:id`.
fn route_key(method: &Method, path: &str) -> String {
    let path = path.split('?').next().unwrap_or_default();
    let template: Vec<&str> = path
        .split('/')
        .map(|segment| {
            if is_snowflake(segment) {
                ":id"
            } else {
                segment
            }
        })
        .collect();
    format!("{method} {}", template.join("/"))
}

/// The major parameter of a route: the channel or guild a request is scoped to.
fn major_param(path: &str) -> String {
    let path = path.split('?').next().unwrap_or_default();
    let mut segments = path.split('/');
    while let Some(segment) = segments.next() {
        if segment == "channels" || segment == "guilds" {
            return segments.next().unwrap_or_default().to_owned();
        }
    }
    String::new()
}

/// Percent-encode an emoji key for a reaction path.
pub fn encode_reaction(key: &str) -> String {
    // A custom emoji is `name:id`; the API wants `name:id` with the colon encoded.
    key.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn network_error(err: reqwest::Error) -> Error {
    if err.is_timeout() || err.is_connect() || err.is_request() {
        Error::Network(err.to_string())
    } else {
        Error::Protocol(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderValue, RETRY_AFTER};

    fn answer(status: u16, headers: &HeaderMap, body: &str) -> Result<()> {
        check(
            StatusCode::from_u16(status).expect("valid status"),
            headers,
            body.as_bytes(),
        )
    }

    #[test]
    fn status_mapping_follows_the_bridge_rules() {
        let none = HeaderMap::new();
        assert!(answer(200, &none, "{}").is_ok());
        assert!(matches!(
            answer(401, &none, ""),
            Err(Error::Auth(message)) if message == "Discord refused the token"
        ));
        // 403 is per chat, never Auth.
        assert!(matches!(
            answer(403, &none, "nope"),
            Err(Error::Protocol(message)) if message.contains("missing permission")
        ));
        assert!(matches!(answer(502, &none, ""), Err(Error::Network(_))));
        assert!(matches!(
            answer(404, &none, r#"{"message":"Unknown Message"}"#),
            Err(Error::Protocol(message)) if message.starts_with("Discord answered 404 Not Found")
        ));
    }

    #[test]
    fn rate_limits_read_the_body_then_the_header() {
        let none = HeaderMap::new();
        assert!(matches!(
            answer(429, &none, r#"{"retry_after": 1.5, "global": false}"#),
            Err(Error::RateLimited {
                retry_after_ms: 1500
            })
        ));
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("2"));
        assert!(matches!(
            answer(429, &headers, ""),
            Err(Error::RateLimited {
                retry_after_ms: 2000
            })
        ));
        assert!(matches!(
            answer(429, &none, ""),
            Err(Error::RateLimited {
                retry_after_ms: 1000
            })
        ));
    }

    #[test]
    fn error_bodies_are_cut_on_a_character_boundary() {
        let body = "é".repeat(500);
        match answer(400, &HeaderMap::new(), &body) {
            Err(Error::Protocol(message)) => {
                let quoted = message.trim_start_matches("Discord answered 400 Bad Request: ");
                assert_eq!(quoted.chars().count(), MAX_QUOTED_CHARS);
            }
            other => panic!("expected a protocol error, got {other:?}"),
        }
    }

    #[test]
    fn media_hosts_are_an_allowlist() {
        let allowed = [
            "https://cdn.discordapp.com/attachments/1/2/a.png",
            "https://media.discordapp.net/stickers/1.png?size=320",
            "https://images-ext-1.discordapp.net/external/x",
        ];
        for url in allowed {
            let parsed = reqwest::Url::parse(url).expect("valid url");
            assert!(is_discord_media(&parsed), "{url} should be allowed");
        }
        let refused = [
            "http://cdn.discordapp.com/a.png",
            "https://evil.example/cdn.discordapp.com/a.png",
            "https://cdn.discordapp.com.evil.example/a.png",
            "https://discordapp.net.evil.example/a.png",
            "https://cdn.discordapp.com@evil.example/a.png",
        ];
        for url in refused {
            let parsed = reqwest::Url::parse(url).expect("valid url");
            assert!(!is_discord_media(&parsed), "{url} should be refused");
        }
    }

    #[test]
    fn short_text_is_one_piece() {
        assert_eq!(split_text("hello", MAX_CONTENT), vec!["hello".to_owned()]);
        assert_eq!(split_text("", MAX_CONTENT), vec![String::new()]);
    }

    #[test]
    fn long_text_prefers_a_line_break_then_a_space() {
        assert_eq!(split_text("aaa\nbbb", 5), vec!["aaa", "bbb"]);
        assert_eq!(split_text("aaa bbbb", 5), vec!["aaa", "bbbb"]);
        assert_eq!(split_text("abcdefgh", 3), vec!["abc", "def", "gh"]);
    }

    #[test]
    fn splitting_respects_characters_not_bytes() {
        assert_eq!(split_text("éééé", 2), vec!["éé", "éé"]);
    }

    #[test]
    fn split_pieces_fit_and_are_never_empty() {
        let text: String = (0..4500)
            .map(|i| if i % 97 == 0 { '\n' } else { 'a' })
            .collect();
        let pieces = split_text(&text, MAX_CONTENT);
        assert!(pieces.len() >= 3);
        for piece in &pieces {
            assert!(piece.chars().count() <= MAX_CONTENT);
            assert!(!piece.is_empty());
        }
    }

    #[test]
    fn routes_drop_snowflakes_and_keep_the_major_parameter() {
        let path = "/channels/123/messages/456?limit=50";
        assert_eq!(
            route_key(&Method::GET, path),
            "GET /channels/:id/messages/:id"
        );
        assert_eq!(major_param(path), "123");
        assert_eq!(major_param("/users/@me"), "");
    }

    #[test]
    fn reaction_keys_are_percent_encoded() {
        assert_eq!(encode_reaction("wave:123"), "wave%3A123");
        assert_eq!(encode_reaction("👍"), "%F0%9F%91%8D");
    }
}
