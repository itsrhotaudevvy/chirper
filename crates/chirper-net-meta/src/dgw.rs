//! The DGW frame codec: the binary framing Facebook's realtime gateway speaks over a websocket.
//!
//! Ported from `pkg/messagix/dgw/frames.go` (AGPL-3.0, mautrix-meta). Every frame starts with a
//! type byte; stream-bound frames carry a little-endian `u16` stream id and a 24-bit little-endian
//! length. Data frames carry a 15-bit ack id whose top bit asks for an acknowledgement; with the
//! extended-data variant a content-type byte precedes the payload.

use chirper_bridge::Error;

/// The frame type bytes of the wire format.
pub mod kind {
    pub const DRAIN: u8 = 3;
    pub const DEAUTH: u8 = 4;
    pub const PING: u8 = 9;
    pub const PONG: u8 = 10;
    pub const ACK: u8 = 12;
    pub const DATA: u8 = 13;
    pub const END_OF_DATA: u8 = 14;
    pub const ESTABLISH_STREAM: u8 = 15;
    pub const END_OF_DATA_WITH_REASON: u8 = 16;
    pub const EXTENDED_DATA: u8 = 17;
}

/// The largest stream id the wire format allows.
pub const MAX_STREAM_ID: u16 = u16::MAX;
/// The largest ack id a data frame may carry (15 bits).
pub const MAX_ACK_ID: u16 = 0x7fff;

/// One decoded or encoded frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    Drain { reason: u8 },
    Deauth,
    Ping,
    Pong,
    Ack { stream: u16, ack_id: u16 },
    Data {
        stream: u16,
        ack_id: u16,
        requires_ack: bool,
        /// The content type byte of an extended data frame.
        content_type: Option<u8>,
        payload: Vec<u8>,
    },
    EndOfData { stream: u16 },
    EndOfDataWithReason { stream: u16, reason: u8 },
    /// The stream's JSON parameters, answered with a `code` by the server.
    EstablishStream { stream: u16, parameters: Vec<u8> },
}

fn protocol(message: &str) -> Error {
    Error::Protocol(format!("dgw: {message}"))
}

fn uint24_le(bytes: &[u8]) -> Option<usize> {
    match bytes {
        [a, b, c, ..] => Some(usize::from(*a) | usize::from(*b) << 8 | usize::from(*c) << 16),
        _ => None,
    }
}

fn push_uint24_le(out: &mut Vec<u8>, value: usize) {
    // The length of any frame we produce is far below 2^24; the mask keeps the encoding total.
    let value = value & 0x00ff_ffff;
    out.push((value & 0xff) as u8);
    out.push(((value >> 8) & 0xff) as u8);
    out.push(((value >> 16) & 0xff) as u8);
}

fn push_u16_le(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

impl Frame {
    /// The stream a frame belongs to, for frames that have one.
    pub fn stream(&self) -> Option<u16> {
        match self {
            Self::Ack { stream, .. }
            | Self::Data { stream, .. }
            | Self::EndOfData { stream }
            | Self::EndOfDataWithReason { stream, .. }
            | Self::EstablishStream { stream, .. } => Some(*stream),
            Self::Drain { .. } | Self::Deauth | Self::Ping | Self::Pong => None,
        }
    }

    /// Append the wire form of this frame. `extended` selects the extended data variant for data
    /// frames that have no content type yet, as a socket that negotiated extended data does.
    pub fn encode_into(&self, out: &mut Vec<u8>, extended: bool) {
        match self {
            Self::Drain { reason } => {
                out.push(kind::DRAIN);
                push_uint24_le(out, 1);
                out.push(*reason);
            }
            Self::Deauth => out.push(kind::DEAUTH),
            Self::Ping => out.push(kind::PING),
            Self::Pong => out.push(kind::PONG),
            Self::Ack { stream, ack_id } => {
                out.push(kind::ACK);
                push_u16_le(out, *stream);
                push_uint24_le(out, 2);
                push_u16_le(out, *ack_id);
            }
            Self::Data {
                stream,
                ack_id,
                requires_ack,
                content_type,
                payload,
            } => {
                let content_type = match content_type {
                    Some(value) => Some(*value),
                    None if extended => Some(0),
                    None => None,
                };
                out.push(if content_type.is_some() {
                    kind::EXTENDED_DATA
                } else {
                    kind::DATA
                });
                push_u16_le(out, *stream);
                let header_extra = usize::from(content_type.is_some());
                push_uint24_le(out, 2 + header_extra + payload.len());
                push_u16_le(out, *ack_id & MAX_ACK_ID);
                if *requires_ack {
                    if let Some(last) = out.last_mut() {
                        *last |= 0x80;
                    }
                }
                if let Some(value) = content_type {
                    out.push(value);
                }
                out.extend_from_slice(payload);
            }
            Self::EndOfData { stream } => {
                out.push(kind::END_OF_DATA);
                push_u16_le(out, *stream);
            }
            Self::EndOfDataWithReason { stream, reason } => {
                out.push(kind::END_OF_DATA_WITH_REASON);
                push_u16_le(out, *stream);
                push_uint24_le(out, 1);
                out.push(*reason);
            }
            Self::EstablishStream { stream, parameters } => {
                out.push(kind::ESTABLISH_STREAM);
                push_u16_le(out, *stream);
                push_uint24_le(out, parameters.len());
                out.extend_from_slice(parameters);
            }
        }
    }

    /// Encode a list of frames into one websocket message.
    pub fn encode_all(frames: &[Frame], extended: bool) -> Vec<u8> {
        let mut out = Vec::new();
        for frame in frames {
            frame.encode_into(&mut out, extended);
        }
        out
    }

    /// Decode the frame at the start of `input`, returning it with the bytes that follow.
    ///
    /// Every length is checked before it is used, so hostile input yields `Error::Protocol`.
    pub fn decode(input: &[u8]) -> Result<(Frame, &[u8]), Error> {
        let (&frame_type, _) = input.split_first().ok_or_else(|| protocol("empty frame"))?;
        match frame_type {
            kind::DEAUTH => Ok((Self::Deauth, rest(input, 1)?)),
            kind::PING => Ok((Self::Ping, rest(input, 1)?)),
            kind::PONG => Ok((Self::Pong, rest(input, 1)?)),
            kind::DRAIN => {
                // A drain frame has no stream id, so its 24-bit length sits right after the type.
                let length = uint24_le(slice(input, 1, 4)?)
                    .ok_or_else(|| protocol("drain frame is truncated"))?;
                if length != 1 {
                    return Err(protocol("unnatural drain frame"));
                }
                let reason = byte_at(input, 4)?;
                Ok((Self::Drain { reason }, rest(input, 5)?))
            }
            kind::ACK => {
                let stream = stream_id(input)?;
                if frame_length(input)? != 2 {
                    return Err(protocol("unnatural ack frame"));
                }
                let ack_id = u16_at(input, 6)?;
                Ok((Self::Ack { stream, ack_id }, rest(input, 8)?))
            }
            kind::END_OF_DATA => {
                let stream = stream_id(input)?;
                Ok((Self::EndOfData { stream }, rest(input, 3)?))
            }
            kind::END_OF_DATA_WITH_REASON => {
                let stream = stream_id(input)?;
                if frame_length(input)? < 1 {
                    return Err(protocol("unnatural end-of-data frame"));
                }
                let reason = byte_at(input, 6)?;
                let total = 6 + frame_length(input)?;
                Ok((Self::EndOfDataWithReason { stream, reason }, rest(input, total)?))
            }
            kind::ESTABLISH_STREAM => {
                let stream = stream_id(input)?;
                let length = frame_length(input)?;
                let parameters = slice(input, 6, 6 + length)?.to_vec();
                Ok((
                    Self::EstablishStream { stream, parameters },
                    rest(input, 6 + length)?,
                ))
            }
            kind::DATA | kind::EXTENDED_DATA => decode_data(input, frame_type),
            other => Err(protocol(&format!("unsupported frame type {other}"))),
        }
    }

    /// Decode every frame of one websocket message.
    pub fn decode_all(mut input: &[u8]) -> Result<Vec<Frame>, Error> {
        let mut frames = Vec::new();
        while !input.is_empty() {
            let (frame, rest) = Frame::decode(input)?;
            frames.push(frame);
            input = rest;
        }
        Ok(frames)
    }
}

fn decode_data(input: &[u8], frame_type: u8) -> Result<(Frame, &[u8]), Error> {
    let header = if frame_type == kind::EXTENDED_DATA { 9 } else { 8 };
    let stream = stream_id(input)?;
    let length = frame_length(input)?;
    if length < header - 6 || input.len() < 6 + length {
        return Err(protocol("data frame is shorter than its header"));
    }
    let ack_bytes = slice(input, 6, 8)?;
    let ack_id = u16::from_le_bytes([ack_bytes[0], ack_bytes[1]]) & MAX_ACK_ID;
    let requires_ack = byte_at(input, 7)? & 0x80 != 0;
    let content_type = if header == 9 {
        Some(byte_at(input, 8)?)
    } else {
        None
    };
    let payload = slice(input, header, 6 + length)?.to_vec();
    Ok((
        Frame::Data {
            stream,
            ack_id,
            requires_ack,
            content_type,
            payload,
        },
        rest(input, 6 + length)?,
    ))
}

fn stream_id(input: &[u8]) -> Result<u16, Error> {
    u16_at(input, 1)
}

fn frame_length(input: &[u8]) -> Result<usize, Error> {
    input
        .get(3..)
        .and_then(uint24_le)
        .ok_or_else(|| protocol("frame header is truncated"))
}

fn u16_at(input: &[u8], offset: usize) -> Result<u16, Error> {
    let bytes = slice(input, offset, offset + 2)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn byte_at(input: &[u8], offset: usize) -> Result<u8, Error> {
    input
        .get(offset)
        .copied()
        .ok_or_else(|| protocol("frame is truncated"))
}

fn slice(input: &[u8], start: usize, end: usize) -> Result<&[u8], Error> {
    input
        .get(start..end)
        .ok_or_else(|| protocol("frame is truncated"))
}

fn rest(input: &[u8], consumed: usize) -> Result<&[u8], Error> {
    input
        .get(consumed..)
        .ok_or_else(|| protocol("frame is truncated"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_data_frame_has_the_documented_layout() {
        let frame = Frame::Data {
            stream: 0x0102,
            ack_id: 5,
            requires_ack: true,
            content_type: None,
            payload: b"{}".to_vec(),
        };
        let mut out = Vec::new();
        frame.encode_into(&mut out, false);
        // type, stream LE, length LE24 (ack 2 + payload 2), ack LE with the top bit set.
        assert_eq!(out, [13, 0x02, 0x01, 4, 0, 0, 5, 0x80, b'{', b'}']);
    }

    #[test]
    fn extended_data_adds_a_zero_content_type_byte() {
        let frame = Frame::Data {
            stream: 7,
            ack_id: 0,
            requires_ack: false,
            content_type: None,
            payload: vec![1, 2],
        };
        let mut out = Vec::new();
        frame.encode_into(&mut out, true);
        // Length counts the ack id, the content type and the payload: 2 + 1 + 2.
        assert_eq!(out, [17, 7, 0, 5, 0, 0, 0, 0, 0, 1, 2]);
        let (decoded, rest) = Frame::decode(&out).expect("decodes");
        assert!(rest.is_empty());
        assert_eq!(
            decoded,
            Frame::Data {
                stream: 7,
                ack_id: 0,
                requires_ack: false,
                content_type: Some(0),
                payload: vec![1, 2],
            }
        );
    }

    #[test]
    fn every_frame_kind_round_trips() {
        let frames = vec![
            Frame::Ping,
            Frame::Pong,
            Frame::Deauth,
            Frame::Drain { reason: 3 },
            Frame::Ack {
                stream: 9,
                ack_id: 0x7fff,
            },
            Frame::Data {
                stream: 1,
                ack_id: 42,
                requires_ack: true,
                content_type: Some(0),
                payload: br#"{"request_id":1}"#.to_vec(),
            },
            Frame::EndOfData { stream: 2 },
            Frame::EndOfDataWithReason { stream: 3, reason: 4 },
            Frame::EstablishStream {
                stream: 4,
                parameters: b"{}".to_vec(),
            },
        ];
        let bytes = Frame::encode_all(&frames, true);
        let decoded = Frame::decode_all(&bytes).expect("decodes every frame");
        // Data frames without a content type come back with the extended one they were sent as.
        assert_eq!(decoded.len(), frames.len());
        assert_eq!(decoded[0], Frame::Ping);
        assert_eq!(decoded[4], frames[4]);
        assert_eq!(decoded[5], frames[5]);
        assert_eq!(decoded[8], frames[8]);
    }

    #[test]
    fn a_truncated_frame_is_a_protocol_error_not_a_panic() {
        assert!(matches!(
            Frame::decode(&[13, 1, 0, 9, 0, 0, 0, 0]),
            Err(Error::Protocol(_))
        ));
        assert!(matches!(Frame::decode(&[]), Err(Error::Protocol(_))));
        assert!(matches!(Frame::decode(&[99, 0, 0]), Err(Error::Protocol(_))));
        assert!(matches!(
            Frame::decode(&[3, 0, 0, 2, 0, 0, 1]),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn an_ack_must_carry_exactly_two_bytes() {
        assert!(matches!(
            Frame::decode(&[12, 1, 0, 3, 0, 0, 0, 0, 0]),
            Err(Error::Protocol(_))
        ));
        let (frame, rest) = Frame::decode(&[12, 1, 0, 2, 0, 0, 7, 0]).expect("decodes");
        assert!(rest.is_empty());
        assert_eq!(frame, Frame::Ack { stream: 1, ack_id: 7 });
    }

    #[test]
    fn a_data_frame_with_a_short_length_field_is_rejected() {
        // Length 1 cannot hold the two-byte ack id of a non-extended data frame.
        assert!(matches!(
            Frame::decode(&[13, 1, 0, 1, 0, 0, 0, 0]),
            Err(Error::Protocol(_))
        ));
    }
}
