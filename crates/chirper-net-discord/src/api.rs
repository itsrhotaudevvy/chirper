//! The Discord REST client: one place for the API version, the auth header and rate limits.

use serde::de::DeserializeOwned;
use serde_json::Value;

use chirper_bridge::{Error, Result};

/// The API version every request uses.
pub const API: &str = "https://discord.com/api/v10";
/// The CDN base of avatars and attachments.
pub const CDN: &str = "https://cdn.discordapp.com";

/// A REST client for one token.
#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    token: String,
    bot: bool,
    base: String,
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
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{path}", self.base);
        let auth = if self.bot {
            format!("Bot {}", self.token)
        } else {
            self.token.clone()
        };
        self.http
            .request(method, url)
            .header(reqwest::header::AUTHORIZATION, auth)
            .header(reqwest::header::USER_AGENT, "Chirper (https://chirper.app, 1.0)")
    }

    async fn send<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        let mut request = self.request(method, path);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(network_error)?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(network_error)?;
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(Error::Auth(match status.as_u16() {
                401 => "Discord refused the token".to_owned(),
                _ => "Discord did not allow this action".to_owned(),
            }));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(rate_limit(&bytes));
        }
        if !status.is_success() {
            return Err(Error::Protocol(format!(
                "Discord answered {status}: {}",
                String::from_utf8_lossy(&bytes)
            )));
        }
        serde_json::from_slice(&bytes).map_err(|e| Error::Protocol(format!("Discord JSON: {e}")))
    }

    /// The logged-in user, which also validates the token.
    pub async fn me(&self) -> Result<super::map::User> {
        self.send(reqwest::Method::GET, "/users/@me", None).await
    }

    /// The gateway URL to connect to.
    pub async fn gateway_url(&self) -> Result<String> {
        let value: Value = self.send(reqwest::Method::GET, "/gateway", None).await?;
        value
            .get("url")
            .and_then(|u| u.as_str())
            .map(str::to_owned)
            .ok_or_else(|| Error::Protocol("Discord sent no gateway url".to_owned()))
    }

    pub async fn guild_channels(&self, guild: &str) -> Result<Vec<super::map::Channel>> {
        self.send(
            reqwest::Method::GET,
            &format!("/guilds/{guild}/channels"),
            None,
        )
        .await
    }

    /// One page of messages, newest first.
    pub async fn history(&self, channel: &str, before: Option<&str>) -> Result<Vec<super::map::Message>> {
        let mut path = format!("/channels/{channel}/messages?limit=50");
        if let Some(before) = before {
            path.push_str("&before=");
            path.push_str(before);
        }
        self.send(reqwest::Method::GET, &path, None).await
    }

    pub async fn message(&self, channel: &str, message: &str) -> Result<super::map::Message> {
        self.send(
            reqwest::Method::GET,
            &format!("/channels/{channel}/messages/{message}"),
            None,
        )
        .await
    }

    pub async fn send_text(
        &self,
        channel: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<super::map::Message> {
        let mut body = serde_json::json!({ "content": text, "allowed_mentions": {"parse": []} });
        if let Some(reply) = reply_to {
            body["message_reference"] = serde_json::json!({ "message_id": reply });
        }
        self.send(
            reqwest::Method::POST,
            &format!("/channels/{channel}/messages"),
            Some(body),
        )
        .await
    }

    /// Upload one attachment with its caption.
    pub async fn send_media(
        &self,
        channel: &str,
        data: Vec<u8>,
        file_name: &str,
        mime: &str,
        caption: &str,
        reply_to: Option<&str>,
    ) -> Result<super::map::Message> {
        let mut payload = serde_json::json!({
            "content": caption,
            "allowed_mentions": {"parse": []},
        });
        if let Some(reply) = reply_to {
            payload["message_reference"] = serde_json::json!({ "message_id": reply });
        }
        let part = reqwest::multipart::Part::bytes(data)
            .file_name(file_name.to_owned())
            .mime_str(mime)
            .map_err(|e| Error::Protocol(format!("attachment type: {e}")))?;
        let form = reqwest::multipart::Form::new()
            .text("payload_json", payload.to_string())
            .part("files[0]", part);
        let response = self
            .request(
                reqwest::Method::POST,
                &format!("/channels/{channel}/messages"),
            )
            .multipart(form)
            .send()
            .await
            .map_err(network_error)?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(network_error)?;
        if !status.is_success() {
            return Err(if status == reqwest::StatusCode::UNAUTHORIZED {
                Error::Auth("Discord refused the token".to_owned())
            } else {
                Error::Protocol(format!("Discord answered {status} on upload"))
            });
        }
        serde_json::from_slice(&bytes).map_err(|e| Error::Protocol(format!("Discord JSON: {e}")))
    }

    pub async fn edit(&self, channel: &str, message: &str, text: &str) -> Result<()> {
        let _: Value = self
            .send(
                reqwest::Method::PATCH,
                &format!("/channels/{channel}/messages/{message}"),
                Some(serde_json::json!({ "content": text })),
            )
            .await?;
        Ok(())
    }

    pub async fn delete(&self, channel: &str, message: &str) -> Result<()> {
        let response = self
            .request(
                reqwest::Method::DELETE,
                &format!("/channels/{channel}/messages/{message}"),
            )
            .send()
            .await
            .map_err(network_error)?;
        if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(Error::Protocol(format!(
            "Discord answered {} on delete",
            response.status()
        )))
    }

    /// Add or remove the user's own reaction.
    pub async fn react(
        &self,
        channel: &str,
        message: &str,
        key: &str,
        add: bool,
    ) -> Result<()> {
        let emoji = encode_reaction(key);
        let method = if add {
            reqwest::Method::PUT
        } else {
            reqwest::Method::DELETE
        };
        let response = self
            .request(
                method,
                &format!("/channels/{channel}/messages/{message}/reactions/{emoji}/@me"),
            )
            .send()
            .await
            .map_err(network_error)?;
        if response.status().is_success() {
            return Ok(());
        }
        Err(Error::Protocol(format!(
            "Discord answered {} on a reaction",
            response.status()
        )))
    }

    pub async fn typing(&self, channel: &str) -> Result<()> {
        let response = self
            .request(reqwest::Method::POST, &format!("/channels/{channel}/typing"))
            .send()
            .await
            .map_err(network_error)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(Error::Protocol(format!(
                "Discord answered {} on typing",
                response.status()
            )))
        }
    }

    /// Download an attachment, refusing anything above `limit` bytes.
    pub async fn download(&self, url: &str, limit: usize) -> Result<Vec<u8>> {
        let response = self.http.get(url).send().await.map_err(network_error)?;
        if !response.status().is_success() {
            return Err(Error::Protocol(format!(
                "Discord CDN answered {}",
                response.status()
            )));
        }
        if response.content_length().is_some_and(|len| len > limit as u64) {
            return Err(Error::Protocol("attachment is too large".to_owned()));
        }
        let bytes = response.bytes().await.map_err(network_error)?;
        if bytes.len() > limit {
            return Err(Error::Protocol("attachment is too large".to_owned()));
        }
        Ok(bytes.to_vec())
    }
}

fn encode_reaction(key: &str) -> String {
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

fn rate_limit(body: &[u8]) -> Error {
    let retry_after_ms = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("retry_after").and_then(|r| r.as_f64()))
        .map(|secs| (secs * 1000.0) as u64)
        .unwrap_or(1_000);
    Error::RateLimited { retry_after_ms }
}
