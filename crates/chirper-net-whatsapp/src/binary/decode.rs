//! The decoder: one node from the binary format, with every length checked against the bytes that
//! are actually there before anything is allocated for it.

use std::collections::BTreeMap;

use chirper_bridge::{Error, Result};

use super::jid::{Jid, Server};
use super::tokens::{
    AD_JID, BINARY_8, BINARY_20, BINARY_32, DICTIONARY_0, DICTIONARY_3, FB_JID, HEX_8, INTEROP_JID,
    JID_PAIR, LIST_8, LIST_16, LIST_EMPTY, NIBBLE_8,
};
use super::{MAX_DEPTH, Node, NodeContent, NodeValue, tokens};

/// Decode a body that must hold exactly one node and nothing after it.
pub fn decode_all(body: &[u8]) -> Result<Node> {
    let mut decoder = Decoder { data: body, pos: 0 };
    let node = decoder.read_node(0)?;
    if decoder.pos != body.len() {
        return Err(Error::Protocol(
            "a payload has bytes after its node".to_owned(),
        ));
    }
    Ok(node)
}

fn truncated() -> Error {
    Error::Protocol("a payload ends in the middle of a value".to_owned())
}

fn invalid(what: &str) -> Error {
    Error::Protocol(format!("a payload holds an invalid {what}"))
}

struct Decoder<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    /// The next `len` bytes, or an error when fewer remain.
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(len).ok_or_else(truncated)?;
        let bytes = self.data.get(self.pos..end).ok_or_else(truncated)?;
        self.pos = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8> {
        self.take(1)?.first().copied().ok_or_else(truncated)
    }

    fn u16(&mut self) -> Result<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes(
            <[u8; 2]>::try_from(bytes).map_err(|_| truncated())?,
        ))
    }

    fn u20(&mut self) -> Result<usize> {
        let [high, mid, low] = <[u8; 3]>::try_from(self.take(3)?).map_err(|_| truncated())?;
        Ok((usize::from(high & 0x0F) << 16) | (usize::from(mid) << 8) | usize::from(low))
    }

    fn u32(&mut self) -> Result<usize> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes(<[u8; 4]>::try_from(bytes).map_err(|_| truncated())?) as usize)
    }

    /// A list header's size. Only the three list tags are valid here.
    fn list_size(&mut self, tag: u8) -> Result<usize> {
        match tag {
            LIST_EMPTY => Ok(0),
            LIST_8 => Ok(usize::from(self.u8()?)),
            LIST_16 => Ok(usize::from(self.u16()?)),
            _ => Err(invalid("list tag")),
        }
    }

    /// A node: list header, tag, attributes, then content if the list size says there is one.
    fn read_node(&mut self, depth: usize) -> Result<Node> {
        if depth >= MAX_DEPTH {
            return Err(Error::Protocol("nodes nest deeper than allowed".to_owned()));
        }
        let header = self.u8()?;
        let size = self.list_size(header)?;
        if size == 0 {
            return Err(invalid("node: its list is empty"));
        }
        let tag_token = self.u8()?;
        let tag = self
            .string_from_tag(tag_token)?
            .ok_or_else(|| invalid("node tag"))?;

        let attr_count = (size - 1) / 2;
        let has_content = size % 2 == 0;
        // Every attribute takes at least two bytes, so a count the input cannot hold is refused
        // before the loop starts.
        if attr_count.saturating_mul(2) > self.remaining() {
            return Err(truncated());
        }
        let mut attrs = BTreeMap::new();
        for _ in 0..attr_count {
            let key_token = self.u8()?;
            let key = self
                .string_from_tag(key_token)?
                .ok_or_else(|| invalid("attribute name"))?;
            let value_token = self.u8()?;
            let value = self
                .value_from_tag(value_token)?
                .unwrap_or_else(|| NodeValue::String(String::new()));
            attrs.insert(key, value);
        }

        let content = if has_content {
            self.read_content(depth)?
        } else {
            None
        };
        Ok(Node {
            tag,
            attrs,
            content,
        })
    }

    fn read_content(&mut self, depth: usize) -> Result<Option<NodeContent>> {
        let tag = self.u8()?;
        match tag {
            LIST_EMPTY => Ok(None),
            LIST_8 | LIST_16 => {
                let count = self.list_size(tag)?;
                // Each child takes at least three bytes; the capacity is bounded by the input.
                if count > self.remaining() {
                    return Err(truncated());
                }
                let mut children = Vec::with_capacity(count);
                for _ in 0..count {
                    children.push(self.read_node(depth + 1)?);
                }
                Ok(Some(NodeContent::Nodes(children)))
            }
            BINARY_8 | BINARY_20 | BINARY_32 => {
                let bytes = self.binary(tag)?;
                Ok(Some(NodeContent::Bytes(bytes.to_vec())))
            }
            _ => Ok(self
                .string_from_tag(tag)?
                .map(|text| NodeContent::Bytes(text.into_bytes()))),
        }
    }

    /// The bytes of a `BINARY_*` value whose tag was already read.
    fn binary(&mut self, tag: u8) -> Result<&'a [u8]> {
        let len = match tag {
            BINARY_8 => usize::from(self.u8()?),
            BINARY_20 => self.u20()?,
            BINARY_32 => self.u32()?,
            _ => return Err(invalid("binary tag")),
        };
        self.take(len)
    }

    /// A value that may be text or a JID, for an attribute.
    fn value_from_tag(&mut self, tag: u8) -> Result<Option<NodeValue>> {
        Ok(match tag {
            LIST_EMPTY => None,
            JID_PAIR => Some(NodeValue::Jid(self.jid_pair()?)),
            AD_JID => Some(NodeValue::Jid(self.ad_jid()?)),
            INTEROP_JID => Some(NodeValue::Jid(self.interop_jid()?)),
            FB_JID => Some(NodeValue::Jid(self.fb_jid()?)),
            _ => self.string_from_tag(tag)?.map(NodeValue::String),
        })
    }

    /// Text for any value tag. JIDs read as their text form, so a string position never fails on
    /// a JID.
    fn string_from_tag(&mut self, tag: u8) -> Result<Option<String>> {
        Ok(match tag {
            LIST_EMPTY => None,
            BINARY_8 | BINARY_20 | BINARY_32 => {
                let bytes = self.binary(tag)?;
                Some(
                    std::str::from_utf8(bytes)
                        .map_err(|_| invalid("text: not UTF-8"))?
                        .to_owned(),
                )
            }
            JID_PAIR => Some(self.jid_pair()?.to_string()),
            AD_JID => Some(self.ad_jid()?.to_string()),
            INTEROP_JID => Some(self.interop_jid()?.to_string()),
            FB_JID => Some(self.fb_jid()?.to_string()),
            NIBBLE_8 | HEX_8 => Some(self.packed(tag)?),
            DICTIONARY_0..=DICTIONARY_3 => {
                let index = self.u8()?;
                let text = tokens::double(tag - DICTIONARY_0, index)
                    .ok_or_else(|| invalid("double-byte token"))?;
                Some(text.to_owned())
            }
            _ => {
                let text = tokens::single(tag).ok_or_else(|| invalid("token"))?;
                Some(text.to_owned())
            }
        })
    }

    fn string_or_error(&mut self) -> Result<String> {
        let tag = self.u8()?;
        self.string_from_tag(tag)?
            .ok_or_else(|| invalid("JID part"))
    }

    /// `JID_PAIR`: a user (possibly empty) and a known server.
    fn jid_pair(&mut self) -> Result<Jid> {
        let user_tag = self.u8()?;
        let user = self.string_from_tag(user_tag)?.unwrap_or_default();
        let server_name = self.string_or_error()?;
        let server = Server::parse_known(&server_name).ok_or_else(|| invalid("JID server"))?;
        Ok(Jid::new(user, server))
    }

    /// `AD_JID`: a domain byte, a device byte and the user.
    fn ad_jid(&mut self) -> Result<Jid> {
        let domain = self.u8()?;
        let device = self.u8()?;
        let server = Server::from_domain_type(domain).ok_or_else(|| invalid("JID domain"))?;
        let user = self.string_or_error()?;
        Ok(Jid::new(user, server).with_device(u16::from(device)))
    }

    /// `FB_JID`: the user, a `u16` device and the `msgr` server.
    fn fb_jid(&mut self) -> Result<Jid> {
        let user = self.string_or_error()?;
        let device = self.u16()?;
        let server = self.string_or_error()?;
        if server != Server::Messenger.as_str() {
            return Err(invalid("Messenger JID"));
        }
        Ok(Jid::new(user, Server::Messenger).with_device(device))
    }

    /// `INTEROP_JID`: the user, a `u16` device, a `u16` integrator and the `interop` server.
    fn interop_jid(&mut self) -> Result<Jid> {
        let user = self.string_or_error()?;
        let device = self.u16()?;
        let integrator = self.u16()?;
        let server = self.string_or_error()?;
        if server != Server::Interop.as_str() {
            return Err(invalid("interop JID"));
        }
        let mut jid = Jid::new(user, Server::Interop).with_device(device);
        jid.integrator = integrator;
        Ok(jid)
    }

    /// `NIBBLE_8` (digits, `-`, `.`) and `HEX_8` (uppercase hex): two characters per byte, with the
    /// high bit of the length byte marking a padded final character.
    fn packed(&mut self, tag: u8) -> Result<String> {
        let length_byte = self.u8()?;
        let padded = length_byte & 0x80 != 0;
        let len = usize::from(length_byte & 0x7F);
        let data = self.take(len)?;

        let mut chars: Vec<u8> = Vec::with_capacity(len * 2);
        for &byte in data {
            for nibble in [byte >> 4, byte & 0x0F] {
                chars.push(if tag == HEX_8 {
                    b"0123456789ABCDEF"[usize::from(nibble)]
                } else {
                    match nibble {
                        0..=9 => b'0' + nibble,
                        10 => b'-',
                        11 => b'.',
                        15 => 0,
                        _ => return Err(invalid("packed nibble")),
                    }
                });
            }
        }
        if padded {
            chars.pop();
        }
        if chars.contains(&0) {
            return Err(invalid("packed string: padding in the middle"));
        }
        String::from_utf8(chars).map_err(|_| invalid("packed string"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `<iq id="…"/>` with the id as a packed nibble string: list of 3 (tag, key, value), no
    /// content.
    fn node_with_packed_nibble(data: &[u8], padded: bool) -> Vec<u8> {
        let mut body = vec![
            LIST_8,
            3,
            25,
            8,
            NIBBLE_8,
            data.len() as u8 | if padded { 0x80 } else { 0 },
        ];
        body.extend_from_slice(data);
        body
    }

    /// A chain of `levels` nodes, each holding the next as its only child.
    fn nested(levels: usize) -> Vec<u8> {
        let mut body = vec![LIST_8, 1, 25];
        for _ in 1..levels {
            let mut outer = vec![LIST_8, 2, 25, LIST_8, 1];
            outer.extend(body);
            body = outer;
        }
        body
    }

    #[test]
    fn decodes_a_packed_nibble_value_and_drops_padding() {
        // "123" packed as 0x12, 0x3F (padded), with the odd-length flag.
        let body = node_with_packed_nibble(&[0x12, 0x3F], true);
        let node = decode_all(&body).expect("decodes");
        assert_eq!(
            node.attrs.get("id"),
            Some(&NodeValue::String("123".to_owned()))
        );
    }

    #[test]
    fn rejects_an_invalid_nibble() {
        let body = node_with_packed_nibble(&[0xC0], false);
        assert!(decode_all(&body).is_err());
    }

    #[test]
    fn rejects_trailing_bytes_and_empty_input() {
        assert!(decode_all(&[]).is_err());
        assert!(decode_all(&[LIST_8, 1, 25, 0]).is_err());
    }

    #[test]
    fn rejects_a_list_larger_than_the_input() {
        // A LIST_16 claiming 60000 children in a 5-byte body.
        let body = [LIST_16, 0xEA, 0x60, 25, 0];
        assert!(decode_all(&body).is_err());
    }

    #[test]
    fn rejects_a_binary_length_past_the_end() {
        let body = [LIST_8, 2, 25, BINARY_8, 5, 1, 2];
        assert!(decode_all(&body).is_err());
        let body = [LIST_8, 2, 25, BINARY_32, 0x7F, 0xFF, 0xFF, 0xFF, 1];
        assert!(decode_all(&body).is_err());
    }

    #[test]
    fn nesting_up_to_the_limit_decodes_and_past_it_fails() {
        assert!(decode_all(&nested(MAX_DEPTH - 4)).is_ok());
        assert!(decode_all(&nested(MAX_DEPTH + 6)).is_err());
    }

    #[test]
    fn unknown_token_is_rejected() {
        // Tag 240 is not assigned in any table.
        assert!(decode_all(&[LIST_8, 1, 240]).is_err());
    }

    #[test]
    fn a_jid_pair_with_an_unknown_server_is_rejected() {
        let body = [LIST_8, 2, 25, JID_PAIR, BINARY_8, 1, b'x'];
        // The content slot is missing, so the server read runs out of bytes.
        assert!(decode_all(&body).is_err());
        let body = [
            LIST_8, 3, 25, JID_PAIR, BINARY_8, 1, b'x', BINARY_8, 3, b'n', b'o', b'!',
        ];
        assert!(decode_all(&body).is_err());
    }
}
