//! The Google Voice HTTP client: the URLs, the cookie and `SAPISIDHASH` headers, the request and
//! reply encodings, and the mapping of every HTTP answer onto the bridge's errors.
//!
//! Ported from `pkg/libgv/{request,client,constants,channel,upload}.go` of mautrix-gvoice
//! (AGPL-3.0). Requests are pblite, as the reference sends them. Replies are read in whichever
//! encoding Google marks them with (`text/plain` chunks around base64 protobuf, raw binary
//! protobuf, or pblite), so the same code serves every endpoint. Retries on 5xx and transport
//! errors are left to the runtime, which reconnects with its own back-off.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Response, StatusCode};
use serde_json::Value;
use sha1::{Digest, Sha1};
use url::Url;

use chirper_bridge::{Error, Result};

use crate::chunks::ChunkReader;
use crate::schema::{
    REQ_GET_ACCOUNT, REQ_GET_THREAD, REQ_LIST_THREADS, REQ_SEND_SMS, REQ_UPDATE_ATTRIBUTES,
    RESP_CHOOSE_SERVER, RESP_CREATE_CHANNEL, RESP_GET_ACCOUNT, RESP_GET_THREAD, RESP_LIST_THREADS,
    RESP_SEND_SMS,
};
use crate::wire::{self, Msg, Schema, Val};

/// The browser user agent Google expects from the web client.
pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";
/// The origin the cookies belong to, also the audience of the `SAPISIDHASH`.
pub const ORIGIN: &str = "https://voice.google.com";
/// The public web client key every API request carries.
const API_KEY: &str = "AIzaSyDTYc1N4xiODyrQYK0Kl6g_y279LjYkrBg";
const CH_UA: &str = r#""Chromium";v="128", "Not;A=Brand";v="24", "Google Chrome";v="128""#;
const CH_PLATFORM: &str = r#""Linux""#;
const CLIENT_VERSION: &str = "665865172";
const JS_USER_AGENT: &str = "google-api-javascript-client/1.1.0";
/// The Google account index every request names.
const AUTH_USER: &str = "0";
/// The largest reply body read into memory.
const MAX_REPLY: usize = 32 << 20;
/// The largest attachment the driver downloads; larger files stay as a notice.
pub const MAX_ATTACHMENT: usize = 100 << 20;
/// Bytes of an error body read to recognise a realtime session that expired.
const MAX_ERROR_BODY: usize = 4096;
/// The message the realtime poll reports when Google forgot its session.
pub const UNKNOWN_SID: &str = "the realtime session is unknown to Google";
/// The request body Google expects when choosing a realtime server.
const CHOOSE_SERVER_BODY: &str = r#"[[null,null,null,[7,5],null,[null,[null,1],[[["3"]]]]]]"#;

/// Attachment kinds the download endpoint serves, by the `MediaType` of the attachment.
pub const MEDIA_AUDIO: i64 = 1;
pub const MEDIA_VIDEO: i64 = 2;

/// The base URLs. Production uses Google's hosts; tests point all of them at one local server.
#[derive(Clone, Debug)]
pub struct Hosts {
    /// The `voiceclient` RPC base (`/account/get`, `/api2thread/list`, ...).
    pub api: String,
    /// The realtime base (`/punctual/...`).
    pub realtime: String,
    /// The web origin, which serves attachments.
    pub voice: String,
}

impl Hosts {
    /// Google's production hosts.
    pub fn production() -> Self {
        Self {
            api: "https://clients6.google.com/voice/v1/voiceclient".to_owned(),
            realtime: "https://signaler-pa.clients6.google.com".to_owned(),
            voice: ORIGIN.to_owned(),
        }
    }

    /// Every endpoint under one root, with the production paths.
    pub fn single(root: &str) -> Self {
        let root = root.trim_end_matches('/');
        Self {
            api: format!("{root}/voice/v1/voiceclient"),
            realtime: root.to_owned(),
            voice: root.to_owned(),
        }
    }
}

/// Which host a request goes to. The host decides the query and headers Google expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    /// The RPC API: the key, `alt=proto` and the web client headers.
    Api,
    /// The realtime service: the key only.
    Realtime,
    /// The web origin: no key.
    Voice,
}

/// One POST to an endpoint.
struct Post<'a> {
    url: String,
    host: Host,
    query: Vec<(&'static str, String)>,
    content_type: &'static str,
    body: String,
    extra: &'a [(&'static str, &'static str)],
}

/// A reply the driver does not read: any body is accepted and nothing is kept from it.
static IGNORE: Schema = Schema { fields: &[] };

/// A REST client for one set of cookies.
#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    hosts: Hosts,
    sapisid: String,
    cookie_header: String,
}

impl fmt::Debug for Api {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Api")
            .field("hosts", &self.hosts)
            .finish_non_exhaustive()
    }
}

impl Api {
    /// A client for `cookies`, which must hold `SAPISID` and the other session cookies.
    pub fn new(
        http: reqwest::Client,
        hosts: Hosts,
        cookies: &BTreeMap<String, String>,
    ) -> Result<Self> {
        let sapisid = cookies
            .get("SAPISID")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::Auth("the SAPISID cookie is missing".to_owned()))?
            .clone();
        let cookie_header = cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        HeaderValue::from_str(&cookie_header)
            .map_err(|_| Error::Auth("a stored cookie is not valid text".to_owned()))?;
        Ok(Self {
            http,
            hosts,
            sapisid,
            cookie_header,
        })
    }

    /// The account record: its primary number and the Google Voice numbers it holds.
    pub async fn get_account(&self) -> Result<Msg> {
        let request = Msg::new().add(2, Val::Int(1));
        let reply = self
            .rpc(
                "/account/get",
                &REQ_GET_ACCOUNT,
                &request,
                &RESP_GET_ACCOUNT,
            )
            .await?;
        Ok(reply.message(1).clone())
    }

    /// One page of threads, newest first, and the token of the next page.
    pub async fn list_threads(&self, page_token: &str, version_token: &str) -> Result<Msg> {
        let mut request = Msg::new()
            .add(1, Val::Int(1))
            .add(2, Val::Int(if version_token.is_empty() { 20 } else { 10 }))
            .add(3, Val::Int(15));
        if !page_token.is_empty() {
            request = request.add(4, Val::Str(page_token.to_owned()));
        }
        if !version_token.is_empty() {
            request = request.add(5, Val::Str(version_token.to_owned()));
        }
        let request = request.add(6, Val::Msg(unknown_wrapper()));
        self.rpc(
            "/api2thread/list",
            &REQ_LIST_THREADS,
            &request,
            &RESP_LIST_THREADS,
        )
        .await
    }

    /// One page of a thread's messages, newest first, and the token of the next older page.
    pub async fn get_thread(&self, thread_id: &str, count: i64, page_token: &str) -> Result<Msg> {
        let mut request = Msg::new()
            .add(1, Val::Str(thread_id.to_owned()))
            .add(2, Val::Int(count));
        if !page_token.is_empty() {
            request = request.add(3, Val::Str(page_token.to_owned()));
        }
        let request = request.add(4, Val::Msg(unknown_wrapper()));
        let reply = self
            .rpc(
                "/api2thread/get",
                &REQ_GET_THREAD,
                &request,
                &RESP_GET_THREAD,
            )
            .await?;
        Ok(reply.message(1).clone())
    }

    /// Send a text message to a thread. The reply carries the new message's id and time.
    pub async fn send_text(&self, thread_id: &str, text: &str) -> Result<Msg> {
        let request = send_request(thread_id).add(5, Val::Str(text.to_owned()));
        self.send(request).await
    }

    /// Send an image to a thread, with an optional caption. `media_type` is the reference's
    /// `ReqSendSMS.Media.Type` value.
    pub async fn send_image(
        &self,
        thread_id: &str,
        media_type: i64,
        data: &[u8],
        caption: &str,
    ) -> Result<Msg> {
        let media = Msg::new()
            .add(1, Val::Int(media_type))
            .add(2, Val::Bytes(data.to_vec()));
        let mut request = send_request(thread_id).add(10, Val::Msg(media));
        if !caption.is_empty() {
            request = request.add(5, Val::Str(caption.to_owned()));
        }
        self.send(request).await
    }

    /// Set the read state of a thread.
    pub async fn mark_read(&self, thread_id: &str) -> Result<()> {
        let attributes = Msg::new()
            .add(1, Val::Str(thread_id.to_owned()))
            .add(4, Val::Int(1));
        let other = Msg::new().add(4, Val::Int(1));
        let request = Msg::new()
            .add(1, Val::Msg(attributes))
            .add(2, Val::Msg(other))
            .add(3, Val::Int(1));
        self.rpc(
            "/thread/updateattributes",
            &REQ_UPDATE_ATTRIBUTES,
            &request,
            &IGNORE,
        )
        .await
        .map(drop)
    }

    /// The realtime server that serves this session.
    pub async fn choose_server(&self) -> Result<String> {
        let reply = self
            .post(
                Post {
                    url: format!("{}/punctual/v1/chooseServer", self.hosts.realtime),
                    host: Host::Realtime,
                    query: Vec::new(),
                    content_type: "application/json+protobuf",
                    body: CHOOSE_SERVER_BODY.to_owned(),
                    extra: &[],
                },
                &RESP_CHOOSE_SERVER,
            )
            .await?;
        Ok(reply.string(1))
    }

    /// Open a realtime channel on a server session. The reply carries the channel's session id.
    pub async fn create_channel(
        &self,
        server_session: &str,
        rid: u32,
        body: String,
    ) -> Result<Msg> {
        self.post(
            Post {
                url: format!("{}/punctual/multi-watch/channel", self.hosts.realtime),
                host: Host::Realtime,
                query: vec![
                    ("VER", "8".to_owned()),
                    ("gsessionid", server_session.to_owned()),
                    ("RID", rid.to_string()),
                    ("CVER", "22".to_owned()),
                    ("t", "1".to_owned()),
                ],
                content_type: "application/x-www-form-urlencoded",
                body,
                extra: &[("x-webchannel-content-type", "application/json+protobuf")],
            },
            &RESP_CREATE_CHANNEL,
        )
        .await
    }

    /// One long-poll of the realtime channel. The body is read by the caller as it arrives.
    pub async fn poll_channel(
        &self,
        server_session: &str,
        session_id: &str,
        ack: u64,
    ) -> Result<Response> {
        let query = [
            ("VER", "8".to_owned()),
            ("gsessionid", server_session.to_owned()),
            ("RID", "rpc".to_owned()),
            ("SID", session_id.to_owned()),
            ("AID", ack.to_string()),
            ("CI", "0".to_owned()),
            ("TYPE", "xmlhttp".to_owned()),
            ("t", "1".to_owned()),
            ("key", API_KEY.to_owned()),
        ];
        let url = format!("{}/punctual/multi-watch/channel", self.hosts.realtime);
        let response = self
            .http
            .get(url)
            .query(&query)
            .headers(self.headers(Host::Realtime)?)
            .send()
            .await
            .map_err(network_error)?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let mut response = response;
        if status == StatusCode::BAD_REQUEST {
            let body = read_capped(&mut response, MAX_ERROR_BODY)
                .await?
                .unwrap_or_default();
            if String::from_utf8_lossy(&body).contains("Unknown SID") {
                return Err(Error::Protocol(UNKNOWN_SID.to_owned()));
            }
        }
        Err(status_error(status, response.headers()))
    }

    /// The bytes of an attachment, and their type as Google Voice reports it. `None` when the
    /// file is larger than [`MAX_ATTACHMENT`].
    pub async fn download(
        &self,
        attachment_id: &str,
        media_type: i64,
    ) -> Result<Option<(Vec<u8>, String)>> {
        let mut url = Url::parse(&self.hosts.voice)
            .map_err(|_| Error::Internal("the Google Voice origin is not a URL".to_owned()))?;
        {
            let mut segments = url.path_segments_mut().map_err(|()| {
                Error::Internal("the Google Voice origin cannot hold a path".to_owned())
            })?;
            let kind = match media_type {
                MEDIA_VIDEO => "vi",
                MEDIA_AUDIO => "au",
                _ => "i",
            };
            segments.extend(["u", AUTH_USER, "a", kind, attachment_id]);
        }
        if media_type == MEDIA_VIDEO {
            url.query_pairs_mut().append_pair("m", "content");
        } else if media_type != MEDIA_AUDIO {
            url.query_pairs_mut().append_pair("s", "1");
        }
        let response = self
            .http
            .get(url)
            .headers(self.headers(Host::Voice)?)
            .send()
            .await
            .map_err(network_error)?;
        let response = check(response).await?;
        let mime = mime_of(response.headers().get(CONTENT_TYPE));
        let mut response = response;
        Ok(read_capped(&mut response, MAX_ATTACHMENT)
            .await?
            .map(|bytes| (bytes, mime)))
    }

    async fn send(&self, request: Msg) -> Result<Msg> {
        self.rpc(
            "/api2thread/sendsms",
            &REQ_SEND_SMS,
            &request,
            &RESP_SEND_SMS,
        )
        .await
    }

    /// An RPC call to the API host, with the reference's transaction and tracking defaults.
    async fn rpc(
        &self,
        path: &str,
        request_schema: &Schema,
        request: &Msg,
        reply: &Schema,
    ) -> Result<Msg> {
        let body = serde_json::to_string(&wire::encode_pblite(request_schema, request))
            .map_err(|err| Error::Internal(err.to_string()))?;
        self.post(
            Post {
                url: format!("{}{path}", self.hosts.api),
                host: Host::Api,
                query: Vec::new(),
                content_type: "application/json+protobuf",
                body,
                extra: &[],
            },
            reply,
        )
        .await
    }

    async fn post(&self, post: Post<'_>, reply: &Schema) -> Result<Msg> {
        let mut query = post.query;
        match post.host {
            Host::Api => {
                query.push(("key", API_KEY.to_owned()));
                query.push(("alt", "proto".to_owned()));
            }
            Host::Realtime => query.push(("key", API_KEY.to_owned())),
            Host::Voice => {}
        }
        let mut headers = self.headers(post.host)?;
        insert(&mut headers, "content-type", post.content_type)?;
        for (name, value) in post.extra {
            insert(&mut headers, name, value)?;
        }
        let response = self
            .http
            .post(post.url)
            .query(&query)
            .headers(headers)
            .body(post.body)
            .send()
            .await
            .map_err(network_error)?;
        let response = check(response).await?;
        read_reply(response, reply).await
    }

    fn headers(&self, host: Host) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        let auth = sapisid_hash(ORIGIN, &self.sapisid, now_secs());
        insert(&mut headers, "authorization", &auth)?;
        insert(&mut headers, "cookie", &self.cookie_header)?;
        insert(&mut headers, "user-agent", USER_AGENT)?;
        insert(&mut headers, "x-goog-authuser", AUTH_USER)?;
        insert(&mut headers, "accept", "*/*")?;
        insert(&mut headers, "accept-language", "en-US,en;q=0.5")?;
        insert(&mut headers, "sec-fetch-dest", "empty")?;
        insert(&mut headers, "sec-fetch-mode", "cors")?;
        insert(&mut headers, "sec-fetch-site", "same-origin")?;
        insert(&mut headers, "origin", ORIGIN)?;
        insert(&mut headers, "referer", &format!("{ORIGIN}/"))?;
        if host == Host::Api {
            insert(&mut headers, "sec-ch-ua", CH_UA)?;
            insert(&mut headers, "sec-ch-ua-platform", CH_PLATFORM)?;
            insert(&mut headers, "sec-ch-ua-mobile", "?0")?;
            insert(&mut headers, "x-client-version", CLIENT_VERSION)?;
            insert(&mut headers, "x-clientdetails", &client_details())?;
            insert(&mut headers, "x-javascript-user-agent", JS_USER_AGENT)?;
            insert(&mut headers, "x-requested-with", "XMLHttpRequest")?;
            insert(
                &mut headers,
                "x-goog-encode-response-if-executable",
                "base64",
            )?;
        }
        Ok(headers)
    }
}

fn send_request(thread_id: &str) -> Msg {
    let transaction = Msg::new().add(1, Val::Int(transaction_id()));
    let tracking = Msg::new().add(1, Val::Str("!".to_owned()));
    Msg::new()
        .add(6, Val::Str(thread_id.to_owned()))
        .add(9, Val::Msg(transaction))
        .add(11, Val::Msg(tracking))
}

fn unknown_wrapper() -> Msg {
    Msg::new().add(2, Val::Int(1)).add(3, Val::Int(1))
}

/// The value of the `X-ClientDetails` header: the reference's encoded `url.Values`.
fn client_details() -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .append_pair("appVersion", "5.0 (X11)")
        .append_pair("platform", "Linux x86_64")
        .append_pair("userAgent", USER_AGENT)
        .finish()
}

/// The `Authorization` value Google accepts for a `SAPISID` cookie:
/// `SAPISIDHASH <timestamp>_<sha1("<timestamp> <SAPISID> <origin>")>`.
pub fn sapisid_hash(origin: &str, sapisid: &str, timestamp: u64) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("{timestamp} {sapisid} {origin}").as_bytes());
    format!("SAPISIDHASH {timestamp}_{}", hex::encode(hasher.finalize()))
}

/// A random transaction id, as the reference chooses them: below 10^14.
pub fn transaction_id() -> i64 {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).ok();
    i64::from_le_bytes(bytes).rem_euclid(100_000_000_000_000)
}

/// A random realtime request id, below 100000.
pub fn request_id() -> u32 {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).ok();
    u32::from_le_bytes(bytes) % 100_000
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

fn insert(headers: &mut HeaderMap, name: &'static str, value: &str) -> Result<()> {
    let value = HeaderValue::from_str(value)
        .map_err(|_| Error::Internal(format!("the {name} header is not valid text")))?;
    headers.insert(name, value);
    Ok(())
}

fn network_error(err: reqwest::Error) -> Error {
    if err.is_timeout() || err.is_connect() || err.is_request() {
        Error::Network("Google Voice could not be reached".to_owned())
    } else {
        Error::Protocol("Google Voice closed the connection early".to_owned())
    }
}

/// Map a failed status onto the bridge's errors. Bodies are never quoted: they may echo cookies.
pub fn status_error(status: StatusCode, headers: &HeaderMap) -> Error {
    match status.as_u16() {
        401 | 403 => Error::Auth("Google Voice refused the saved cookies".to_owned()),
        429 => Error::RateLimited {
            retry_after_ms: retry_after_ms(headers),
        },
        500..=599 => Error::Network(format!("Google Voice answered {status}")),
        _ => Error::Protocol(format!("Google Voice answered {status}")),
    }
}

fn retry_after_ms(headers: &HeaderMap) -> u64 {
    headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.trim().parse::<f64>().ok())
        .filter(|secs| secs.is_finite() && *secs >= 0.0)
        .map_or(60_000, |secs| (secs * 1000.0) as u64)
}

async fn check(response: Response) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        Ok(response)
    } else {
        Err(status_error(status, response.headers()))
    }
}

/// The media type of a `Content-Type` value, without parameters, in lower case.
fn mime_of(value: Option<&HeaderValue>) -> String {
    value
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.split(';').next())
        .map(|mime| mime.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Read a reply in the encoding Google marks it with, mirroring `ReadProtoResponse` of the
/// reference: `X-Goog-Safety-Content-Type` wins over `Content-Type`, and only a `text/plain`
/// reply is unframed first.
async fn read_reply(mut response: Response, schema: &Schema) -> Result<Msg> {
    let plain = mime_of(response.headers().get(CONTENT_TYPE));
    let safety = mime_of(response.headers().get("x-goog-safety-content-type"));
    let real = if safety.is_empty() {
        plain.clone()
    } else {
        safety
    };
    let body = read_capped(&mut response, MAX_REPLY)
        .await?
        .ok_or_else(|| wire::protocol("the reply is larger than the driver accepts"))?;
    match real.as_str() {
        "text/plain" => {
            let chunk = first_chunk(&body)?;
            pblite_reply(schema, &chunk)
        }
        "application/json+protobuf" => pblite_reply(schema, &body),
        "application/x-protobuf" => {
            let bytes = if plain == "text/plain" {
                decode_base64(&body)?
            } else {
                body
            };
            wire::decode_binary(schema, &bytes)
        }
        other => Err(wire::protocol(format!("an unknown reply type {other:?}"))),
    }
}

fn first_chunk(body: &[u8]) -> Result<Vec<u8>> {
    let mut reader = ChunkReader::new();
    reader.push(body);
    reader
        .next_chunk()?
        .ok_or_else(|| wire::protocol("the reply has no complete chunk"))
}

fn pblite_reply(schema: &Schema, body: &[u8]) -> Result<Msg> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| wire::protocol("the pblite reply is not JSON"))?;
    wire::decode_pblite(schema, &value)
}

/// Base64 as the reference decodes it: standard alphabet, line breaks ignored.
fn decode_base64(body: &[u8]) -> Result<Vec<u8>> {
    let text: Vec<u8> = body
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    STANDARD
        .decode(text)
        .map_err(|_| wire::protocol("the base64 reply does not decode"))
}

/// Read a body up to `cap` bytes; `None` when it is longer.
async fn read_capped(response: &mut Response, cap: usize) -> Result<Option<Vec<u8>>> {
    let mut out = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if out.len() + chunk.len() > cap {
            return Ok(None);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sapisid_hash_matches_the_reference_formula() {
        // sha1("1700000000 abc123 https://voice.google.com") computed once with shasum.
        assert_eq!(
            sapisid_hash(ORIGIN, "abc123", 1_700_000_000),
            "SAPISIDHASH 1700000000_0af60f2c8319974dffd04c264464cd61b24a1ffd"
        );
    }

    #[test]
    fn a_missing_sapisid_is_an_auth_error() {
        let cookies = BTreeMap::from([("SID".to_owned(), "x".to_owned())]);
        assert!(matches!(
            Api::new(
                reqwest::Client::new(),
                Hosts::single("http://127.0.0.1:1"),
                &cookies
            ),
            Err(Error::Auth(_))
        ));
    }

    #[test]
    fn statuses_map_onto_the_bridge_errors() {
        let none = HeaderMap::new();
        assert!(matches!(
            status_error(StatusCode::UNAUTHORIZED, &none),
            Error::Auth(_)
        ));
        assert!(matches!(
            status_error(StatusCode::FORBIDDEN, &none),
            Error::Auth(_)
        ));
        assert!(matches!(
            status_error(StatusCode::TOO_MANY_REQUESTS, &none),
            Error::RateLimited {
                retry_after_ms: 60_000
            }
        ));
        assert!(matches!(
            status_error(StatusCode::BAD_GATEWAY, &none),
            Error::Network(_)
        ));
        assert!(matches!(
            status_error(StatusCode::NOT_FOUND, &none),
            Error::Protocol(_)
        ));
    }

    #[test]
    fn retry_after_is_read_in_milliseconds() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("2.5"));
        assert_eq!(retry_after_ms(&headers), 2500);
    }

    #[test]
    fn replies_are_read_in_each_marked_encoding() {
        use crate::wire::{Kind, one};
        static REPLY: Schema = Schema {
            fields: &[one(1, Kind::Str)],
        };
        // Binary protobuf: field 1 "hi".
        let binary = wire::decode_binary(&REPLY, &[0x0a, 0x02, b'h', b'i']).expect("binary");
        assert_eq!(binary.string(1), "hi");
        // Base64 of the same bytes, with a line break the reference ignores.
        assert_eq!(
            decode_base64(b"CgJoa\nQ==").expect("base64"),
            vec![0x0a, 0x02, b'h', b'i']
        );
        // A pblite reply and its first chunk.
        assert_eq!(
            pblite_reply(&REPLY, br#"["hi"]"#)
                .expect("pblite")
                .string(1),
            "hi"
        );
        assert_eq!(
            first_chunk(b"6\n[\"hi\"]").expect("chunk"),
            br#"["hi"]"#.to_vec()
        );
        assert!(decode_base64(b"!!").is_err());
    }

    #[test]
    fn the_send_request_carries_transaction_and_tracking_defaults() {
        let request = send_request("t.+15550001").add(5, Val::Str("hello".to_owned()));
        let encoded = wire::encode_pblite(&REQ_SEND_SMS, &request);
        let items = encoded.as_array().expect("array");
        assert_eq!(items[4], Value::String("hello".to_owned()));
        assert_eq!(items[5], Value::String("t.+15550001".to_owned()));
        assert_eq!(
            items[8][0].as_i64().map(|id| id < 100_000_000_000_000),
            Some(true)
        );
        assert_eq!(items[10], serde_json::json!(["!"]));
    }

    #[test]
    fn client_details_are_form_encoded_like_the_reference() {
        assert!(
            client_details()
                .starts_with("appVersion=5.0+%28X11%29&platform=Linux+x86_64&userAgent=")
        );
    }
}
