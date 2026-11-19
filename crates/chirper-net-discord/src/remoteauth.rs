//! Discord's QR-code login ("remote auth"). The user scans a code with the Discord mobile app and
//! approves the login there; this side then receives an RSA-encrypted user token.
//!
//! A background task runs the handshake and hands each step to [`LoginProcess::next`] through a
//! channel. The key is generated per login and never leaves this process.

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use futures_util::{SinkExt, StreamExt};
use log::debug;
use rsa::pkcs8::EncodePublicKey;
use rsa::{Oaep, RsaPrivateKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use zeroize::Zeroizing;

use chirper_bridge::model::LoginStep;
use chirper_bridge::network::LoginInput;
use chirper_bridge::{Error, LoginCtx, LoginProcess, Result};

use crate::api::{self, Api};
use crate::map::is_snowflake;

const GATEWAY_URL: &str = "wss://remote-auth-gateway.discord.gg/?v=2";
const ORIGIN: &str = "https://discord.com";
const QR_PREFIX: &str = "https://discord.com/ra/";
const TICKET_URL: &str = "https://discord.com/api/v9/users/@me/remote-auth/login";
const RSA_BITS: usize = 2048;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_HEARTBEAT_MS: u64 = 41_250;
/// Heartbeats that may go unanswered before the gateway is given up on.
const MAX_MISSED_ACKS: u32 = 2;

/// The QR login process. Steps arrive from the background handshake.
pub struct QrLogin {
    steps: mpsc::Receiver<Result<LoginStep>>,
    task: JoinHandle<()>,
    done: bool,
}

/// Start the QR handshake for a new login.
pub fn start(ctx: LoginCtx) -> QrLogin {
    let (tx, rx) = mpsc::channel(4);
    let task = tokio::spawn(async move {
        if let Err(err) = handshake(&ctx, &tx).await {
            let _ = tx.send(Err(err)).await;
        }
    });
    QrLogin {
        steps: rx,
        task,
        done: false,
    }
}

impl Drop for QrLogin {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[async_trait]
impl LoginProcess for QrLogin {
    async fn next(&mut self, _input: LoginInput) -> Result<LoginStep> {
        if self.done {
            return Err(Error::Protocol("the login already finished".to_owned()));
        }
        match self.steps.recv().await {
            Some(Ok(step)) => {
                self.done = step.is_complete();
                Ok(step)
            }
            Some(Err(err)) => {
                self.done = true;
                Err(err)
            }
            None => {
                self.done = true;
                Err(Error::Protocol(
                    "the QR login ended before it finished".to_owned(),
                ))
            }
        }
    }

    async fn cancel(&mut self) {
        self.done = true;
        self.task.abort();
    }
}

/// Run the handshake to completion. Steps are sent as they happen; the function returns once the
/// login is complete, the process is cancelled, or the handshake fails.
async fn handshake(ctx: &LoginCtx, steps: &mpsc::Sender<Result<LoginStep>>) -> Result<()> {
    let key = generate_key().await?;
    let mut request = GATEWAY_URL
        .into_client_request()
        .map_err(|err| Error::Internal(format!("remote auth request: {err}")))?;
    request
        .headers_mut()
        .insert("origin", HeaderValue::from_static(ORIGIN));
    let (mut socket, _response) =
        tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(request))
            .await
            .map_err(|_| Error::Network("timed out connecting to remote auth".to_owned()))?
            .map_err(|err| Error::Network(format!("remote auth gateway: {err}")))?;

    let mut beats: Option<tokio::time::Interval> = None;
    let mut missed: u32 = 0;
    loop {
        tokio::select! {
            message = socket.next() => {
                let text = match message {
                    Some(Ok(WsMessage::Text(text))) => text,
                    Some(Ok(WsMessage::Ping(payload))) => {
                        let _ = socket.send(WsMessage::Pong(payload)).await;
                        continue;
                    }
                    Some(Ok(WsMessage::Close(_))) | None => {
                        return Err(Error::Network("the remote auth connection closed".to_owned()));
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(err)) => {
                        return Err(Error::Network(format!("remote auth connection: {err}")));
                    }
                };
                let packet: Value = serde_json::from_str(&text)
                    .map_err(|_| Error::Protocol("remote auth sent a non-JSON message".to_owned()))?;
                match packet.get("op").and_then(Value::as_str).unwrap_or_default() {
                    "hello" => {
                        let ms = packet
                            .get("heartbeat_interval")
                            .and_then(Value::as_u64)
                            .filter(|ms| *ms > 0)
                            .unwrap_or(DEFAULT_HEARTBEAT_MS);
                        let period = Duration::from_millis(ms);
                        beats = Some(tokio::time::interval_at(Instant::now() + period, period));
                        let public_key = STANDARD.encode(public_key_der(&key)?);
                        send(&mut socket, json!({ "op": "init", "encoded_public_key": public_key })).await?;
                    }
                    "nonce_proof" => {
                        let nonce = decrypt(&key, str_field(&packet, "encrypted_nonce")?)?;
                        let proof = nonce_proof(&nonce);
                        send(&mut socket, json!({ "op": "nonce_proof", "proof": proof })).await?;
                    }
                    "pending_remote_init" => {
                        let fingerprint = str_field(&packet, "fingerprint")?;
                        let step = qr_step(format!("{QR_PREFIX}{fingerprint}"));
                        if steps.send(Ok(step)).await.is_err() {
                            // The process was cancelled while the code was being shown.
                            return Ok(());
                        }
                    }
                    "pending_ticket" => {
                        // The phone has scanned the code. The payload names the user; it is only
                        // informational, the token that follows is what counts.
                        let payload = packet.get("encrypted_user_payload").and_then(Value::as_str);
                        if let Some(payload) = payload.and_then(|p| decrypt(&key, p).ok())
                            && let Some(user) = parse_user_payload(&String::from_utf8_lossy(&payload))
                        {
                            debug!("discord: a phone scanned the QR login for user {}", user.id);
                        }
                    }
                    "pending_login" => {
                        let ticket = str_field(&packet, "ticket")?;
                        let token = exchange_ticket(ctx, &key, ticket).await?;
                        let text = std::str::from_utf8(&token)
                            .map_err(|_| Error::Protocol("the login token is not text".to_owned()))?;
                        // The token is checked before it is kept.
                        let me = Api::new(ctx.http.clone(), text, api::API).me().await?;
                        ctx.set_secret("token", &token)?;
                        debug!("discord: QR login completed for user {}", me.id);
                        let _ = steps
                            .send(Ok(LoginStep::Complete {
                                login_id: format!("discord_{}", me.id),
                            }))
                            .await;
                        return Ok(());
                    }
                    "heartbeat_ack" => missed = 0,
                    "cancel" => {
                        return Err(Error::Auth("the login was cancelled on the phone".to_owned()));
                    }
                    _ => {
                        return Err(Error::Protocol("remote auth sent an unexpected message".to_owned()));
                    }
                }
            }
            () = tick(&mut beats) => {
                missed += 1;
                if missed > MAX_MISSED_ACKS {
                    return Err(Error::Network("the remote auth gateway stopped answering".to_owned()));
                }
                send(&mut socket, json!({ "op": "heartbeat" })).await?;
            }
        }
    }
}

/// Wait for the next heartbeat; never resolves before the hello gave an interval.
async fn tick(beats: &mut Option<tokio::time::Interval>) {
    match beats {
        Some(interval) => {
            interval.tick().await;
        }
        None => std::future::pending().await,
    }
}

async fn send<S>(socket: &mut S, value: Value) -> Result<()>
where
    S: SinkExt<WsMessage> + Unpin,
{
    socket
        .send(WsMessage::Text(value.to_string().into()))
        .await
        .map_err(|_| Error::Network("the remote auth connection closed".to_owned()))
}

/// The 2048-bit RSA key of this login. Generated on a blocking thread: it takes a while.
async fn generate_key() -> Result<RsaPrivateKey> {
    tokio::task::spawn_blocking(|| {
        let mut rng = rand::rngs::OsRng;
        RsaPrivateKey::new(&mut rng, RSA_BITS)
    })
    .await
    .map_err(|_| Error::Internal("the login key task stopped".to_owned()))?
    .map_err(|_| Error::Internal("cannot generate the login key".to_owned()))
}

/// The public key as DER-encoded SubjectPublicKeyInfo.
fn public_key_der(key: &RsaPrivateKey) -> Result<Vec<u8>> {
    let document = key
        .to_public_key()
        .to_public_key_der()
        .map_err(|_| Error::Internal("cannot encode the login key".to_owned()))?;
    Ok(document.as_bytes().to_vec())
}

/// Decrypt one base64 payload sent by Discord (RSA-OAEP with SHA-256).
fn decrypt(key: &RsaPrivateKey, payload: &str) -> Result<Zeroizing<Vec<u8>>> {
    let raw = STANDARD
        .decode(payload)
        .map_err(|_| Error::Protocol("remote auth sent a bad payload".to_owned()))?;
    key.decrypt(Oaep::new::<Sha256>(), &raw)
        .map(Zeroizing::new)
        .map_err(|_| Error::Protocol("cannot decrypt the remote auth payload".to_owned()))
}

/// Trade the approved ticket for the encrypted token, and decrypt the token.
async fn exchange_ticket(
    ctx: &LoginCtx,
    key: &RsaPrivateKey,
    ticket: &str,
) -> Result<Zeroizing<Vec<u8>>> {
    let response = ctx
        .http
        .post(TICKET_URL)
        .json(&json!({ "ticket": ticket }))
        .send()
        .await
        .map_err(|err| Error::Network(format!("remote auth login: {err}")))?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .bytes()
        .await
        .map_err(|err| Error::Network(format!("remote auth login: {err}")))?;
    api::check(status, &headers, &body)?;
    let value: Value = serde_json::from_slice(&body)
        .map_err(|_| Error::Protocol("Discord sent bad JSON for the login".to_owned()))?;
    let encrypted = value
        .get("encrypted_token")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Protocol("Discord sent no login token".to_owned()))?;
    decrypt(key, encrypted)
}

/// The proof of a nonce: unpadded URL-safe base64 of its SHA-256.
pub fn nonce_proof(nonce: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(nonce))
}

/// The user a login is approved for, from the payload `id:discriminator:avatar:username`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserPayload {
    pub id: String,
    pub discriminator: String,
    pub avatar: String,
    pub username: String,
}

/// Parse the user payload. The username may itself contain colons.
pub fn parse_user_payload(text: &str) -> Option<UserPayload> {
    let mut parts = text.splitn(4, ':');
    let id = parts.next()?;
    let discriminator = parts.next()?;
    let avatar = parts.next()?;
    let username = parts.next()?;
    is_snowflake(id).then(|| UserPayload {
        id: id.to_owned(),
        discriminator: discriminator.to_owned(),
        avatar: avatar.to_owned(),
        username: username.to_owned(),
    })
}

fn qr_step(url: String) -> LoginStep {
    LoginStep::DisplayAndWait {
        step_id: "qr".to_owned(),
        instructions:
            "Open Discord on your phone, go to Settings → Scan QR Code, and scan this code."
                .to_owned(),
        kind: "qr".to_owned(),
        data: url,
        image_url: String::new(),
    }
}

fn str_field<'a>(packet: &'a Value, key: &str) -> Result<&'a str> {
    packet
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Protocol(format!("remote auth message has no `{key}`")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_proof_is_unpadded_url_safe_sha256() {
        // Reference values from an independent SHA-256 and base64url implementation.
        assert_eq!(
            nonce_proof(b""),
            "47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU"
        );
        assert_eq!(
            nonce_proof(b"nonce-1"),
            "nj8VYyTULw6ktvT86B1W-9ZKIUOj_dYKEw2ckOW01og"
        );
        assert!(!nonce_proof(b"x").contains('='));
        assert!(!nonce_proof(b"x").contains('+'));
        assert!(!nonce_proof(b"x").contains('/'));
    }

    #[test]
    fn user_payloads_parse_with_colons_in_the_name() {
        let parsed = parse_user_payload("1234:0:abc:ada").expect("a valid payload");
        assert_eq!(parsed.id, "1234");
        assert_eq!(parsed.discriminator, "0");
        assert_eq!(parsed.avatar, "abc");
        assert_eq!(parsed.username, "ada");

        let colon = parse_user_payload("1234:0:abc:ada:lovelace").expect("a valid payload");
        assert_eq!(colon.username, "ada:lovelace");
    }

    #[test]
    fn malformed_user_payloads_are_rejected() {
        assert!(
            parse_user_payload("x:0:abc:ada").is_none(),
            "the id must be a snowflake"
        );
        assert!(
            parse_user_payload("1234:0:abc").is_none(),
            "a field is missing"
        );
        assert!(parse_user_payload("").is_none());
    }
}
