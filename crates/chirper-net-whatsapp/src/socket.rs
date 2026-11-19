//! The WhatsApp WebSocket: framing, the connection header, the Noise exchange and the encrypted
//! frame stream.
//!
//! Every frame is a 3-byte big-endian length followed by the payload. A WebSocket message may hold
//! several frames, and a frame may span messages, so reads go through [`FrameDecoder`]. Before the
//! handshake the payloads are plaintext, and the first frame carries the connection header in
//! front of its length. After the finish, each payload is one AES-GCM frame.

use std::time::Duration;

use chirper_bridge::{Error, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async_with_config};

use crate::binary::{self, Node};
use crate::keys::KeyPair;
use crate::noise::{Handshake, TransportCipher, VerifiedCertChain, WA_CONN_HEADER};

/// The WebSocket endpoint of the multi-device web protocol.
pub const WS_URL: &str = "wss://web.whatsapp.com/ws/chat";

/// The `Origin` the endpoint expects on the upgrade request.
pub const WS_ORIGIN: &str = "https://web.whatsapp.com";

/// The largest frame, and the largest WebSocket message, the client accepts: 16 MiB.
pub const MAX_MESSAGE: usize = 16 << 20;

/// How long each step of the connect may take before the attempt is abandoned.
const STEP_TIMEOUT: Duration = Duration::from_secs(20);

/// The 3-byte length prefix, as an integer, must stay below this. It is also the most a 3-byte
/// prefix can name, so the check bounds allocation for any frame the peer announces.
const LENGTH_LIMIT: usize = (1 << 24) - 1;

/// Encode one frame: the optional `header` once in front, then the 3-byte length, then the payload.
pub fn encode_frame(payload: &[u8], header: Option<&[u8]>) -> Result<Vec<u8>> {
    if payload.len() > LENGTH_LIMIT {
        return Err(Error::Internal(
            "an outgoing frame is larger than the frame format allows".to_owned(),
        ));
    }
    let header_len = header.map_or(0, <[u8]>::len);
    let mut out = Vec::with_capacity(header_len + 3 + payload.len());
    if let Some(header) = header {
        out.extend_from_slice(header);
    }
    let length = payload.len() as u32;
    out.extend_from_slice(&length.to_be_bytes()[1..]);
    out.extend_from_slice(payload);
    Ok(out)
}

/// Reassembles frames from the bytes of WebSocket messages.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
    /// Where the first unread byte of `buffer` is.
    start: usize,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append the bytes of one WebSocket message.
    pub fn feed(&mut self, data: &[u8]) {
        // Drop the consumed prefix once it dominates the buffer, so reads stay linear.
        if self.start > 0 && self.start * 2 >= self.buffer.len() {
            self.buffer.drain(..self.start);
            self.start = 0;
        }
        self.buffer.extend_from_slice(data);
    }

    /// The next complete frame, or `None` until more bytes arrive.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>> {
        let pending = self.buffer.get(self.start..).unwrap_or_default();
        let [a, b, c, rest @ ..] = pending else {
            return Ok(None);
        };
        let length = (usize::from(*a) << 16) | (usize::from(*b) << 8) | usize::from(*c);
        if length > LENGTH_LIMIT {
            return Err(Error::Protocol(
                "a frame announces an impossible length".to_owned(),
            ));
        }
        let Some(payload) = rest.get(..length) else {
            return Ok(None);
        };
        let frame = payload.to_vec();
        self.start += 3 + length;
        if self.start == self.buffer.len() {
            self.buffer.clear();
            self.start = 0;
        }
        Ok(Some(frame))
    }
}

/// The WebSocket and its frame decoder: the raw, unencrypted byte pipe the handshake runs on.
struct Wire {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    frames: FrameDecoder,
    open: bool,
}

impl Wire {
    async fn send_frame(&mut self, bytes: Vec<u8>) -> Result<()> {
        timeout(STEP_TIMEOUT, self.socket.send(Message::binary(bytes)))
            .await
            .map_err(|_| Error::Network("sending timed out".to_owned()))?
            .map_err(|err| Error::Network(format!("sending failed: {err}")))
    }

    /// The next complete frame's payload, whatever the message boundaries were. Waits without a
    /// deadline: callers that need one wrap the call, and dropping it loses no bytes.
    async fn next_frame(&mut self) -> Result<Vec<u8>> {
        loop {
            if let Some(frame) = self.frames.next_frame()? {
                return Ok(frame);
            }
            let message = self
                .socket
                .next()
                .await
                .ok_or_else(|| Error::Network("the connection closed".to_owned()))?
                .map_err(|err| Error::Network(format!("the connection failed: {err}")))?;
            match message {
                Message::Binary(data) => self.frames.feed(&data),
                Message::Close(_) => {
                    self.open = false;
                    return Err(Error::Network(
                        "the server closed the connection".to_owned(),
                    ));
                }
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
                Message::Text(_) => {
                    return Err(Error::Protocol(
                        "the server sent a text message on the binary socket".to_owned(),
                    ));
                }
            }
        }
    }

    async fn close(&mut self) {
        if self.open {
            self.open = false;
            let _ = self.socket.close(None).await;
        }
    }
}

/// One open, handshaken connection with its transport ciphers.
pub struct Connection {
    wire: Wire,
    write: TransportCipher,
    read: TransportCipher,
    cert_chain: VerifiedCertChain,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("open", &self.wire.open)
            .finish_non_exhaustive()
    }
}

impl Connection {
    /// Connect to `url`, run the Noise XX handshake with `static_key` and `payload` (the encoded
    /// `ClientPayload`), and return the encrypted connection.
    pub async fn connect(url: &str, static_key: KeyPair, payload: Vec<u8>) -> Result<Self> {
        let mut request = url
            .into_client_request()
            .map_err(|err| Error::Protocol(format!("the WebSocket URL is invalid: {err}")))?;
        request
            .headers_mut()
            .insert("Origin", HeaderValue::from_static(WS_ORIGIN));
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE));
        let (socket, _response) = timeout(
            STEP_TIMEOUT,
            connect_async_with_config(request, Some(config), false),
        )
        .await
        .map_err(|_| Error::Network("the WebSocket connect timed out".to_owned()))?
        .map_err(|err| Error::Network(format!("the WebSocket connect failed: {err}")))?;
        let mut wire = Wire {
            socket,
            frames: FrameDecoder::new(),
            open: true,
        };

        let handshake = Handshake::new(static_key, payload)?;
        let first = encode_frame(&handshake.client_hello(), Some(&WA_CONN_HEADER))?;
        wire.send_frame(first).await?;
        let server_hello = timeout(STEP_TIMEOUT, wire.next_frame())
            .await
            .map_err(|_| {
                Error::Network("the server did not answer the handshake in time".to_owned())
            })??;
        let (finish, established) = match handshake.finish(&server_hello) {
            Ok(done) => done,
            Err(err) => {
                wire.close().await;
                return Err(err);
            }
        };
        wire.send_frame(encode_frame(&finish, None)?).await?;

        Ok(Self {
            wire,
            write: established.write,
            read: established.read,
            cert_chain: established.cert_chain,
        })
    }

    /// The server certificate chain this connection verified.
    pub fn cert_chain(&self) -> &VerifiedCertChain {
        &self.cert_chain
    }

    /// Encrypt `plaintext` as one frame and send it.
    pub async fn send_payload(&mut self, plaintext: &[u8]) -> Result<()> {
        let ciphertext = self.write.encrypt(plaintext)?;
        self.wire.send_frame(encode_frame(&ciphertext, None)?).await
    }

    /// The next decrypted payload from the server.
    pub async fn next_payload(&mut self) -> Result<Vec<u8>> {
        let ciphertext = self.wire.next_frame().await?;
        self.read.decrypt(&ciphertext)
    }

    /// Encode a node and send it.
    pub async fn send_node(&mut self, node: &Node) -> Result<()> {
        let payload = binary::marshal(node)?;
        self.send_payload(&payload).await
    }

    /// The next node from the server. Cancel-safe: dropping the future loses no bytes.
    pub async fn next_node(&mut self) -> Result<Node> {
        let payload = self.next_payload().await?;
        binary::unmarshal(&payload)
    }

    /// Close the WebSocket. Idempotent.
    pub async fn close(&mut self) {
        self.wire.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_writes_the_length_big_endian_after_the_header() {
        let frame = encode_frame(&[1, 2, 3], Some(&WA_CONN_HEADER)).expect("encodes");
        assert_eq!(&frame[..4], &WA_CONN_HEADER);
        assert_eq!(&frame[4..7], &[0, 0, 3]);
        assert_eq!(&frame[7..], &[1, 2, 3]);

        let plain = encode_frame(&vec![0; 0x0102], None).expect("encodes");
        assert_eq!(&plain[..3], &[0, 0x01, 0x02]);
    }

    #[test]
    fn one_frame_in_one_message() {
        let mut decoder = FrameDecoder::new();
        decoder.feed(&encode_frame(b"hello", None).expect("encodes"));
        assert_eq!(
            decoder.next_frame().expect("ok").as_deref(),
            Some(&b"hello"[..])
        );
        assert_eq!(decoder.next_frame().expect("ok"), None);
    }

    #[test]
    fn several_frames_merged_into_one_message() {
        let mut bytes = encode_frame(b"one", None).expect("encodes");
        bytes.extend(encode_frame(b"", None).expect("encodes"));
        bytes.extend(encode_frame(b"three", None).expect("encodes"));
        let mut decoder = FrameDecoder::new();
        decoder.feed(&bytes);
        assert_eq!(
            decoder.next_frame().expect("ok").as_deref(),
            Some(&b"one"[..])
        );
        assert_eq!(decoder.next_frame().expect("ok").as_deref(), Some(&b""[..]));
        assert_eq!(
            decoder.next_frame().expect("ok").as_deref(),
            Some(&b"three"[..])
        );
        assert_eq!(decoder.next_frame().expect("ok"), None);
    }

    #[test]
    fn one_frame_split_across_messages_byte_by_byte() {
        let bytes = encode_frame(b"split across many messages", None).expect("encodes");
        let mut decoder = FrameDecoder::new();
        let mut frames = Vec::new();
        for byte in bytes {
            decoder.feed(&[byte]);
            while let Some(frame) = decoder.next_frame().expect("ok") {
                frames.push(frame);
            }
        }
        assert_eq!(frames, vec![b"split across many messages".to_vec()]);
    }

    #[test]
    fn a_frame_and_the_head_of_the_next_split_across_two_messages() {
        let mut first = encode_frame(b"alpha", None).expect("encodes");
        let second = encode_frame(b"beta", None).expect("encodes");
        first.extend_from_slice(&second[..2]);
        let mut decoder = FrameDecoder::new();
        decoder.feed(&first);
        assert_eq!(
            decoder.next_frame().expect("ok").as_deref(),
            Some(&b"alpha"[..])
        );
        assert_eq!(decoder.next_frame().expect("ok"), None);
        decoder.feed(&second[2..]);
        assert_eq!(
            decoder.next_frame().expect("ok").as_deref(),
            Some(&b"beta"[..])
        );
    }

    #[test]
    fn a_huge_announced_length_waits_for_bytes_and_allocates_nothing_yet() {
        let mut decoder = FrameDecoder::new();
        decoder.feed(&[0xFF, 0xFF, 0xFF, 1, 2, 3]);
        assert_eq!(decoder.next_frame().expect("ok"), None);
        assert!(decoder.buffer.capacity() < 1024);
    }

    #[test]
    fn decoder_compacts_consumed_bytes() {
        let mut decoder = FrameDecoder::new();
        for _ in 0..1000 {
            decoder.feed(&encode_frame(&[7; 100], None).expect("encodes"));
            assert!(decoder.next_frame().expect("ok").is_some());
        }
        assert!(decoder.buffer.len() < 1000);
    }
}
