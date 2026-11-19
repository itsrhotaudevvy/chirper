//! Unencrypted MTProto messages, used only while the auth key is being created: an 8-byte zero
//! `auth_key_id`, the `msg_id`, the body length and the body.

/// A decoded plain message.
#[derive(Debug, PartialEq, Eq)]
pub struct PlainMessage {
    pub msg_id: i64,
    pub body: Vec<u8>,
}

/// Encode a plain message.
pub fn encode(msg_id: i64, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(20 + body.len());
    out.extend_from_slice(&0i64.to_le_bytes());
    out.extend_from_slice(&msg_id.to_le_bytes());
    let len = i32::try_from(body.len()).unwrap_or(i32::MAX);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// Decode a plain message. Every length is checked against the frame before it is used.
pub fn decode(frame: &[u8]) -> Result<PlainMessage, &'static str> {
    let mut cursor = crate::tl::Cursor::new(frame);
    let auth_key_id = cursor.take(8).map_err(|_| "plain frame too short")?;
    if auth_key_id != [0u8; 8] {
        return Err("plain frame has an auth key id");
    }
    let msg_id = cursor.take(8).map_err(|_| "plain frame too short")?;
    let len = cursor.read_i32().map_err(|_| "plain frame too short")?;
    let len = usize::try_from(len).map_err(|_| "negative plain length")?;
    let body = cursor.take(len).map_err(|_| "plain length exceeds frame")?;
    if cursor.remaining() != 0 {
        return Err("plain frame has trailing bytes");
    }
    let mut id = [0u8; 8];
    id.copy_from_slice(msg_id);
    Ok(PlainMessage {
        msg_id: i64::from_le_bytes(id),
        body: body.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let frame = encode(0x5f00_0000_0000_0004, &[1, 2, 3, 4]);
        assert_eq!(frame.len(), 20 + 4);
        let decoded = decode(&frame).unwrap();
        assert_eq!(decoded.msg_id, 0x5f00_0000_0000_0004);
        assert_eq!(decoded.body, vec![1, 2, 3, 4]);
    }

    #[test]
    fn rejects_bad_frames() {
        let mut frame = encode(4, &[1, 2, 3, 4]);
        frame[0] = 1;
        assert!(decode(&frame).is_err());
        let frame = encode(4, &[1, 2, 3, 4]);
        assert!(decode(&frame[..10]).is_err());
        let mut long = encode(4, &[1, 2, 3, 4]);
        long[16] = 99;
        assert!(decode(&long).is_err());
        let mut trailing = encode(4, &[1, 2, 3, 4]);
        trailing.push(0);
        assert!(decode(&trailing).is_err());
    }
}
