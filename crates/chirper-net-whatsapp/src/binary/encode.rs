//! The encoder: nodes, strings, bytes and JIDs in the binary format.
//!
//! String classification follows the order WhatsApp's own writer uses: a token first, then a
//! packed nibble or hex string, then a JID-shaped string, then raw bytes. Strings longer than 127
//! bytes are always raw, because no token or packed form can hold them.

use chirper_bridge::{Error, Result};

use super::jid::{Jid, Server};
use super::tokens::{
    AD_JID, BINARY_8, BINARY_20, BINARY_32, DICTIONARY_0, HEX_8, INTEROP_JID, JID_PAIR, LIST_8,
    LIST_16, LIST_EMPTY, NIBBLE_8, PACKED_MAX, Token, lookup,
};
use super::{Node, NodeContent, NodeValue};

/// Append one node: its list header, tag, attributes and content.
pub fn write_node(out: &mut Vec<u8>, node: &Node) -> Result<()> {
    let content_slots = usize::from(node.content.is_some());
    write_list_start(out, 1 + node.attrs.len() * 2 + content_slots)?;
    write_string(out, &node.tag)?;
    for (key, value) in &node.attrs {
        write_string(out, key)?;
        match value {
            NodeValue::String(text) => write_string(out, text)?,
            NodeValue::Jid(jid) => write_jid(out, jid)?,
        }
    }
    match &node.content {
        None => {}
        Some(NodeContent::Bytes(bytes)) => write_bytes(out, bytes)?,
        Some(NodeContent::Nodes(children)) => {
            write_list_start(out, children.len())?;
            for child in children {
                write_node(out, child)?;
            }
        }
    }
    Ok(())
}

/// A list header: `LIST_EMPTY`, `LIST_8` or `LIST_16`.
pub fn write_list_start(out: &mut Vec<u8>, len: usize) -> Result<()> {
    if len == 0 {
        out.push(LIST_EMPTY);
    } else if len < 256 {
        out.push(LIST_8);
        out.push(len as u8);
    } else if len <= usize::from(u16::MAX) {
        out.push(LIST_16);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        return Err(Error::Internal(
            "a node list is too long to encode".to_owned(),
        ));
    }
    Ok(())
}

/// A byte string with its length: `BINARY_8`, `BINARY_20` or `BINARY_32`.
pub fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    let len = bytes.len();
    if len < 256 {
        out.push(BINARY_8);
        out.push(len as u8);
    } else if len < 1 << 20 {
        out.push(BINARY_20);
        out.push(((len >> 16) & 0x0F) as u8);
        out.push((len >> 8) as u8);
        out.push(len as u8);
    } else if u32::try_from(len).is_ok() {
        out.push(BINARY_32);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    } else {
        return Err(Error::Internal(
            "a byte string is too long to encode".to_owned(),
        ));
    }
    out.extend_from_slice(bytes);
    Ok(())
}

/// Append a string in whichever form is shortest and valid for it.
pub fn write_string(out: &mut Vec<u8>, text: &str) -> Result<()> {
    if text.len() > PACKED_MAX {
        return write_bytes(out, text.as_bytes());
    }
    if text.is_empty() {
        return write_bytes(out, &[]);
    }
    match lookup(text) {
        Some(Token::Single(index)) => {
            out.push(index);
            return Ok(());
        }
        Some(Token::Double(dictionary, index)) => {
            out.push(DICTIONARY_0 + dictionary);
            out.push(index);
            return Ok(());
        }
        None => {}
    }
    if is_nibble(text) {
        write_packed(out, text, NIBBLE_8, pack_nibble);
        return Ok(());
    }
    if is_hex(text) {
        write_packed(out, text, HEX_8, pack_hex);
        return Ok(());
    }
    if write_jid_string(out, text)? {
        return Ok(());
    }
    write_bytes(out, text.as_bytes())
}

/// A JID typed in an attribute. Interop JIDs with an integrator keep it; AD form carries the
/// device; everything else is a user and server pair.
pub fn write_jid(out: &mut Vec<u8>, jid: &Jid) -> Result<()> {
    if jid.server == Server::Interop && jid.integrator != 0 {
        out.push(INTEROP_JID);
        write_string(out, &jid.user)?;
        out.extend_from_slice(&jid.device.to_be_bytes());
        out.extend_from_slice(&jid.integrator.to_be_bytes());
        return Ok(());
    }
    if jid.device > 0 && jid.server.supports_ad() {
        let device = u8::try_from(jid.device)
            .map_err(|_| Error::Internal("a device id does not fit the AD form".to_owned()))?;
        out.push(AD_JID);
        out.push(jid.server.domain_type());
        out.push(device);
        return write_string(out, &jid.user);
    }
    out.push(JID_PAIR);
    if jid.user.is_empty() {
        out.push(LIST_EMPTY);
    } else {
        write_string(out, &jid.user)?;
    }
    write_string(out, jid.server.as_str())
}

/// Write a JID-shaped string as a JID. Returns `false` when the string is not one. The device is
/// kept only in AD form (phone and LID servers, a device that fits a byte); anything else stays
/// in the user part, so the text reads back unchanged.
fn write_jid_string(out: &mut Vec<u8>, text: &str) -> Result<bool> {
    let Some((user_part, server_name)) = text.split_once('@') else {
        return Ok(false);
    };
    let Some(server) = Server::parse_known(server_name) else {
        return Ok(false);
    };
    if user_part.is_empty() {
        return Ok(false);
    }
    let ad_device = user_part
        .split_once(':')
        .filter(|_| server.supports_ad())
        .and_then(|(user, device)| device.parse::<u8>().ok().map(|device| (user, device)));
    match ad_device {
        Some((user, device)) => {
            out.push(AD_JID);
            out.push(server.domain_type());
            out.push(device);
            write_string(out, user)?;
        }
        None => {
            out.push(JID_PAIR);
            write_string(out, user_part)?;
            write_string(out, server_name)?;
        }
    }
    Ok(true)
}

fn write_packed(out: &mut Vec<u8>, text: &str, tag: u8, pack: fn(u8) -> u8) {
    let bytes = text.as_bytes();
    let mut length = bytes.len().div_ceil(2) as u8;
    if bytes.len() % 2 == 1 {
        length |= 0x80;
    }
    out.push(tag);
    out.push(length);
    for pair in bytes.chunks(2) {
        let high = pack(pair[0]);
        let low = pair.get(1).map_or(15, |&byte| pack(byte));
        out.push((high << 4) | low);
    }
}

fn is_nibble(text: &str) -> bool {
    text.bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'-' || byte == b'.')
}

fn is_hex(text: &str) -> bool {
    text.bytes()
        .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte))
}

fn pack_nibble(byte: u8) -> u8 {
    match byte {
        b'-' => 10,
        b'.' => 11,
        digit => digit - b'0',
    }
}

fn pack_hex(byte: u8) -> u8 {
    match byte {
        b'A'..=b'F' => 10 + (byte - b'A'),
        digit => digit - b'0',
    }
}
