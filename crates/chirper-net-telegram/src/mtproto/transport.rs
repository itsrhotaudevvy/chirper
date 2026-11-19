//! The TCP transport: the "intermediate" framing, optionally wrapped in the obfuscated2 layer that
//! most networks cannot tell apart from random bytes. Obfuscation is the default.
//!
//! Frames are a 4-byte little-endian length followed by the payload. A negative 4-byte value sent
//! by the server, with no payload, is a transport error code such as -404.

use std::net::SocketAddr;
use std::time::Duration;

use aes::Aes256;
use ctr::cipher::{KeyIvInit, StreamCipher};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use zeroize::Zeroizing;

use super::crypto::random_array;

/// Tag that selects the intermediate framing, sent first on every connection.
const INTERMEDIATE_TAG: u32 = 0xeeee_eeee;
/// Largest frame accepted from the network.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;
/// How long a TCP connection may take to open.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// First four bytes an obfuscated init payload must not start with: other protocols' openings
/// and the tags of the other transports.
const FORBIDDEN_STARTS: [[u8; 4]; 6] = [
    *b"HEAD",
    *b"POST",
    *b"GET ",
    *b"OPTI",
    [0xdd, 0xdd, 0xdd, 0xdd],
    [0xee, 0xee, 0xee, 0xee],
];

/// One of Telegram's data centres.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dc {
    pub id: i32,
    pub addr: SocketAddr,
}

/// The production data centres (IPv4, port 443), as published by Telegram.
pub fn production_dc(id: i32) -> Option<Dc> {
    let ip = match id {
        1 => [149, 154, 175, 53],
        2 => [149, 154, 167, 51],
        3 => [149, 154, 175, 100],
        4 => [149, 154, 167, 91],
        5 => [91, 108, 56, 130],
        _ => return None,
    };
    Some(Dc {
        id,
        addr: SocketAddr::from((ip, 443)),
    })
}

/// Test data centre 2.
pub fn test_dc2() -> Dc {
    Dc {
        id: 2,
        addr: SocketAddr::from(([149, 154, 167, 40], 443)),
    }
}

/// Why the transport failed. Messages never carry bytes from the connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// The connection could not be opened in time or was refused.
    Connect(String),
    /// Reading or writing failed, or the peer closed the connection.
    Io(String),
    /// The server sent a transport error code, for example -404.
    Code(i32),
    /// A frame that the framing does not allow.
    Frame(&'static str),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(m) => write!(f, "connect failed: {m}"),
            Self::Io(m) => write!(f, "transport I/O failed: {m}"),
            Self::Code(code) => write!(f, "transport error {code}"),
            Self::Frame(m) => write!(f, "bad frame: {m}"),
        }
    }
}

impl std::error::Error for TransportError {}

type Stream = ctr::Ctr128BE<Aes256>;

/// The sending half: one stream cipher per direction, kept for the life of the connection.
pub struct TransportWriter {
    half: OwnedWriteHalf,
    encrypt: Option<Stream>,
}

/// The receiving half. Owns the decrypting cipher.
pub struct TransportReader {
    half: OwnedReadHalf,
    decrypt: Option<Stream>,
}

/// A connected transport, before or after its handshake.
pub struct Transport {
    reader: TransportReader,
    writer: TransportWriter,
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transport")
            .field("obfuscated", &self.writer.encrypt.is_some())
            .finish_non_exhaustive()
    }
}

/// Open a connection to `dc` with the obfuscated2 layer (the default).
pub async fn connect(dc: Dc) -> Result<Transport, TransportError> {
    connect_with(dc, true).await
}

/// Open a connection to `dc`. `obfuscated` selects the obfuscated2 wrapper.
pub async fn connect_with(dc: Dc, obfuscated: bool) -> Result<Transport, TransportError> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(dc.addr))
        .await
        .map_err(|_| TransportError::Connect("timed out".to_owned()))?
        .map_err(|e| TransportError::Connect(e.kind().to_string()))?;
    stream
        .set_nodelay(true)
        .map_err(|e| TransportError::Io(e.kind().to_string()))?;
    let (read, write) = stream.into_split();
    let mut transport = Transport {
        reader: TransportReader {
            half: read,
            decrypt: None,
        },
        writer: TransportWriter {
            half: write,
            encrypt: None,
        },
    };
    if obfuscated {
        let random: [u8; 64] =
            random_array().map_err(|_| TransportError::Io("random".to_owned()))?;
        // `obfuscated_init` has already advanced the encryptor over the whole init payload, so the
        // data that follows continues the same keystream the server expects.
        let (wire, encrypt, decrypt) = obfuscated_init(random)?;
        transport.writer.write_raw(&wire).await?;
        transport.writer.encrypt = Some(encrypt);
        transport.reader.decrypt = Some(decrypt);
    } else {
        transport
            .writer
            .write_raw(&INTERMEDIATE_TAG.to_le_bytes())
            .await?;
    }
    Ok(transport)
}

/// Build the 64-byte obfuscated2 init payload. Returns the bytes to send and the two ciphers:
/// the encryptor (client to server, advanced past the init) and the decryptor (server to client).
fn obfuscated_init(random: [u8; 64]) -> Result<([u8; 64], Stream, Stream), TransportError> {
    let mut init = Zeroizing::new(random);
    let start: [u8; 4] = [init[0], init[1], init[2], init[3]];
    if init[0] == 0xef || FORBIDDEN_STARTS.contains(&start) || init[4..8] == [0, 0, 0, 0] {
        return Err(TransportError::Frame("random init payload rejected"));
    }
    init[56..60].copy_from_slice(&INTERMEDIATE_TAG.to_le_bytes());

    let mut reversed = Zeroizing::new([0u8; 64]);
    for (i, byte) in init.iter().rev().enumerate() {
        reversed[i] = *byte;
    }
    let mut encrypt = Stream::new_from_slices(&init[8..40], &init[40..56])
        .map_err(|_| TransportError::Frame("obfuscation key"))?;
    let decrypt = Stream::new_from_slices(&reversed[8..40], &reversed[40..56])
        .map_err(|_| TransportError::Frame("obfuscation key"))?;

    // The whole init goes through the encryptor so its keystream advances; only bytes 56..64 of
    // the ciphertext are sent, the first 56 stay as generated.
    let mut encrypted = *init;
    encrypt.apply_keystream(&mut encrypted);
    let mut wire = *init;
    wire[56..64].copy_from_slice(&encrypted[56..64]);
    Ok((wire, encrypt, decrypt))
}

impl TransportWriter {
    async fn write_raw(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        self.half
            .write_all(bytes)
            .await
            .map_err(|e| TransportError::Io(e.kind().to_string()))
    }

    /// Send one payload as an intermediate frame. The payload length must be a multiple of 4.
    pub async fn send(&mut self, payload: &[u8]) -> Result<(), TransportError> {
        if !payload.len().is_multiple_of(4) || payload.len() > MAX_FRAME {
            return Err(TransportError::Frame("outgoing frame size"));
        }
        let len = u32::try_from(payload.len())
            .map_err(|_| TransportError::Frame("outgoing frame size"))?;
        let mut frame = Vec::with_capacity(payload.len() + 4);
        frame.extend_from_slice(&len.to_le_bytes());
        frame.extend_from_slice(payload);
        if let Some(encrypt) = self.encrypt.as_mut() {
            encrypt.apply_keystream(&mut frame);
        }
        self.write_raw(&frame).await
    }
}

impl TransportReader {
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), TransportError> {
        self.half
            .read_exact(buf)
            .await
            .map_err(|e| TransportError::Io(e.kind().to_string()))?;
        if let Some(decrypt) = self.decrypt.as_mut() {
            decrypt.apply_keystream(buf);
        }
        Ok(())
    }

    /// Receive one intermediate frame. A negative 4-byte value is a transport error code.
    pub async fn recv(&mut self) -> Result<Vec<u8>, TransportError> {
        let mut head = [0u8; 4];
        self.read_exact(&mut head).await?;
        let word = i32::from_le_bytes(head);
        if word < 0 {
            return Err(TransportError::Code(word));
        }
        let len = usize::try_from(word).map_err(|_| TransportError::Frame("length"))?;
        if len == 0 || !len.is_multiple_of(4) || len > MAX_FRAME {
            return Err(TransportError::Frame("incoming frame size"));
        }
        let mut payload = vec![0u8; len];
        self.read_exact(&mut payload).await?;
        Ok(payload)
    }
}

impl Transport {
    /// Split into independent halves, one per task.
    pub fn split(self) -> (TransportReader, TransportWriter) {
        (self.reader, self.writer)
    }

    pub async fn send(&mut self, payload: &[u8]) -> Result<(), TransportError> {
        self.writer.send(payload).await
    }

    pub async fn recv(&mut self) -> Result<Vec<u8>, TransportError> {
        self.reader.recv().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_addresses_match_the_published_list() {
        let expected = [
            (1, [149, 154, 175, 53]),
            (2, [149, 154, 167, 51]),
            (3, [149, 154, 175, 100]),
            (4, [149, 154, 167, 91]),
            (5, [91, 108, 56, 130]),
        ];
        for (id, ip) in expected {
            let dc = production_dc(id).unwrap();
            assert_eq!(dc.addr, SocketAddr::from((ip, 443)));
        }
        assert!(production_dc(6).is_none());
        assert_eq!(
            test_dc2().addr,
            SocketAddr::from(([149, 154, 167, 40], 443))
        );
    }

    #[test]
    fn init_payload_carries_the_tag_and_the_checked_fields() {
        let mut random = [7u8; 64];
        random[0] = 0x12;
        random[4] = 1;
        let (wire, _, _) = obfuscated_init(random).unwrap();
        // The first 56 bytes are sent as generated, apart from nothing else.
        assert_eq!(&wire[..56], &random[..56]);
    }

    #[test]
    fn forbidden_starts_are_rejected() {
        for start in [*b"POST", *b"GET ", [0xef, 1, 2, 3], [0xdd; 4], [0xee; 4]] {
            let mut random = [9u8; 64];
            random[..4].copy_from_slice(&start);
            random[4] = 1;
            assert!(obfuscated_init(random).is_err(), "start {start:?} accepted");
        }
        let mut zero = [9u8; 64];
        zero[4..8].copy_from_slice(&[0, 0, 0, 0]);
        assert!(obfuscated_init(zero).is_err());
    }

    #[test]
    fn server_side_stream_recovers_the_tag_and_the_data() {
        // The server builds its decryptor from the init payload's key and IV, advances it over the
        // 64-byte init, and then reads the tag and the data that follow.
        let mut random = [0u8; 64];
        for (i, b) in random.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(37).wrapping_add(11);
        }
        random[0] = 0x10;
        random[4] = 2;
        let (wire, mut encrypt, _) = obfuscated_init(random).unwrap();
        let mut server = Stream::new_from_slices(&random[8..40], &random[40..56]).unwrap();
        let mut init_seen = wire;
        server.apply_keystream(&mut init_seen);
        assert_eq!(&init_seen[56..60], &INTERMEDIATE_TAG.to_le_bytes());

        let mut data = b"0123456789abcdef".to_vec();
        let original = data.clone();
        encrypt.apply_keystream(&mut data);
        assert_ne!(data, original);
        server.apply_keystream(&mut data);
        assert_eq!(data, original);
    }
}
