//! The WhatsApp binary XML: the node tree the protocol exchanges, and its compact wire format.
//!
//! A payload is one flags byte followed by one encoded node. Bit 1 (`& 2`) of the flags marks a
//! zlib-compressed body; the encoder always writes flags `0`. The format is WhatsApp's own; this
//! module is written from its structure, with its token tables in [`tokens`].

pub mod decode;
pub mod encode;
pub mod jid;
pub mod tokens;

use std::collections::BTreeMap;

use chirper_bridge::{Error, Result};

pub use jid::{Jid, Server};

/// The most a payload may decompress to: 16 MiB.
pub const MAX_DECOMPRESSED: usize = 16 << 20;

/// The deepest a node may nest. Real stanzas nest well under twenty levels.
pub const MAX_DEPTH: usize = 64;

/// One element of the protocol: a tag, its attributes, and its content.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub tag: String,
    pub attrs: BTreeMap<String, NodeValue>,
    pub content: Option<NodeContent>,
}

/// An attribute value: text, or a JID.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeValue {
    String(String),
    Jid(Jid),
}

/// What a node holds after its attributes: child nodes, or bytes. Text is carried as bytes.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeContent {
    Nodes(Vec<Node>),
    Bytes(Vec<u8>),
}

impl Node {
    /// A node with no attributes and no content.
    pub fn new(tag: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            attrs: BTreeMap::new(),
            content: None,
        }
    }

    /// Set an attribute to text.
    pub fn attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attrs
            .insert(key.into(), NodeValue::String(value.into()));
        self
    }

    /// Set an attribute to a JID.
    pub fn jid_attr(mut self, key: impl Into<String>, jid: Jid) -> Self {
        self.attrs.insert(key.into(), NodeValue::Jid(jid));
        self
    }

    /// Set the content to child nodes.
    pub fn children(mut self, children: Vec<Node>) -> Self {
        self.content = Some(NodeContent::Nodes(children));
        self
    }

    /// Set the content to bytes.
    pub fn bytes(mut self, bytes: Vec<u8>) -> Self {
        self.content = Some(NodeContent::Bytes(bytes));
        self
    }

    /// The text of an attribute. A JID attribute reads as its text form.
    pub fn get_attr(&self, key: &str) -> Option<String> {
        match self.attrs.get(key)? {
            NodeValue::String(text) => Some(text.clone()),
            NodeValue::Jid(jid) => Some(jid.to_string()),
        }
    }

    /// The JID of an attribute, if it holds one.
    pub fn get_jid(&self, key: &str) -> Option<&Jid> {
        match self.attrs.get(key)? {
            NodeValue::Jid(jid) => Some(jid),
            NodeValue::String(_) => None,
        }
    }

    /// The child nodes, or an empty slice when the content is not a list.
    pub fn child_nodes(&self) -> &[Node] {
        match &self.content {
            Some(NodeContent::Nodes(nodes)) => nodes,
            _ => &[],
        }
    }

    /// The first child with `tag`.
    pub fn child(&self, tag: &str) -> Option<&Node> {
        self.child_nodes().iter().find(|node| node.tag == tag)
    }

    /// The bytes of the content, if it holds bytes.
    pub fn content_bytes(&self) -> Option<&[u8]> {
        match &self.content {
            Some(NodeContent::Bytes(bytes)) => Some(bytes),
            _ => None,
        }
    }
}

/// Encode a node as an uncompressed payload: the flags byte `0`, then the node.
pub fn marshal(node: &Node) -> Result<Vec<u8>> {
    let mut out = vec![0u8];
    encode::write_node(&mut out, node)?;
    Ok(out)
}

/// Decode a payload: check the flags, decompress if needed, and decode exactly one node.
pub fn unmarshal(payload: &[u8]) -> Result<Node> {
    let (flags, body) = payload
        .split_first()
        .ok_or_else(|| Error::Protocol("an empty payload".to_owned()))?;
    let decompressed;
    let body = if flags & 2 != 0 {
        decompressed = decompress(body)?;
        decompressed.as_slice()
    } else {
        body
    };
    decode::decode_all(body)
}

fn decompress(body: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;

    let mut out = Vec::new();
    let limit = (MAX_DECOMPRESSED as u64) + 1;
    flate2::read::ZlibDecoder::new(body)
        .take(limit)
        .read_to_end(&mut out)
        .map_err(|_| Error::Protocol("a compressed payload does not decompress".to_owned()))?;
    if out.len() > MAX_DECOMPRESSED {
        return Err(Error::Protocol(
            "a compressed payload expands beyond the limit".to_owned(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::tokens::{BINARY_8, FB_JID, INTEROP_JID, LIST_8, LIST_16};
    use super::*;

    fn round_trip_bytes(node: &Node) -> Vec<u8> {
        let bytes = marshal(node).expect("encodes");
        let decoded = unmarshal(&bytes).expect("decodes");
        let again = marshal(&decoded).expect("re-encodes");
        assert_eq!(again, bytes, "decode then encode must reproduce the bytes");
        bytes
    }

    fn round_trip(node: &Node) {
        let bytes = round_trip_bytes(node);
        assert_eq!(&unmarshal(&bytes).expect("decodes"), node);
    }

    #[test]
    fn iq_ping_matches_the_hand_computed_encoding() {
        // Tokens: iq=25, id=8, type=4, xmlns=22, "1"=85, get=41, w:p=87, ping=86. Lists: 8 entries
        // for the iq (tag, three attributes, one content slot), 1 entry for the ping.
        let node = Node::new("iq")
            .attr("id", "1")
            .attr("type", "get")
            .attr("xmlns", "w:p")
            .children(vec![Node::new("ping")]);
        let expected: Vec<u8> = vec![
            0x00, // flags: uncompressed
            248, 8,  // LIST_8, 8 entries
            25, // "iq"
            8, 85, // id = "1"
            4, 41, // type = "get"
            22, 87, // xmlns = "w:p"
            248, 1, // LIST_8, 1 child
            248, 1, 86, // <ping/>
        ];
        assert_eq!(marshal(&node).expect("encodes"), expected);
        assert_eq!(unmarshal(&expected).expect("decodes"), node);
    }

    #[test]
    fn a_jid_string_writes_as_a_pair_with_packed_user() {
        let mut out = Vec::new();
        encode::write_string(&mut out, "15551234567@s.whatsapp.net").expect("encodes");
        assert_eq!(
            out,
            vec![250, 255, 0x86, 0x15, 0x55, 0x12, 0x34, 0x56, 0x7F, 0x03]
        );
    }

    #[test]
    fn every_token_class_round_trips() {
        let dictionary_zero = tokens::double(0, 0).expect("entry");
        let dictionary_three = tokens::double(3, 200).expect("entry");
        let node = Node::new("message")
            .attr("single", "text")
            .attr("double", dictionary_zero)
            .attr("double3", dictionary_three)
            .attr("nibble", "1234-5.6")
            .attr("odd", "12345")
            .attr("hex", "ABCDEF0123")
            .attr("empty", "")
            .attr("long", "x".repeat(200))
            .attr("plain", "not-a-token-or-packed")
            .jid_attr("to", Jid::new("15551234567", Server::Pn))
            .jid_attr("device", Jid::new("15551234567", Server::Pn).with_device(3))
            .jid_attr("lid", Jid::new("abc.def", Server::Lid).with_device(2))
            .jid_attr("group", Jid::new("120363000000", Server::Group))
            .children(vec![
                Node::new("body").bytes(b"hello".to_vec()),
                Node::new("empty").bytes(Vec::new()),
                Node::new("bare"),
            ]);
        round_trip(&node);
    }

    #[test]
    fn a_jid_text_attribute_decodes_as_a_jid_with_the_same_bytes() {
        let node = Node::new("x").attr("from", "15551234567@s.whatsapp.net");
        let bytes = marshal(&node).expect("encodes");
        let decoded = unmarshal(&bytes).expect("decodes");
        assert_eq!(
            decoded.get_jid("from"),
            Some(&Jid::new("15551234567", Server::Pn))
        );
        assert_eq!(marshal(&decoded).expect("re-encodes"), bytes);
    }

    #[test]
    fn large_content_uses_the_wide_binary_forms() {
        let twenty = Node::new("a").bytes(vec![7; 300]);
        round_trip(&twenty);
        assert!(round_trip_bytes(&twenty).contains(&tokens::BINARY_20));
        let thirty_two = Node::new("a").bytes(vec![9; 1 << 20]);
        round_trip(&thirty_two);
    }

    #[test]
    fn long_child_lists_use_list_16() {
        let children: Vec<Node> = (0..300)
            .map(|i| Node::new("c").attr("i", i.to_string()))
            .collect();
        let node = Node::new("root").children(children);
        round_trip(&node);
        let bytes = marshal(&node).expect("encodes");
        assert!(bytes.contains(&LIST_16));
    }

    #[test]
    fn interop_and_messenger_jids_decode() {
        // iq with id = INTEROP_JID(user "u", device 5, integrator 7, server "interop").
        let interop = [
            LIST_8,
            3,
            25,
            8,
            INTEROP_JID,
            BINARY_8,
            1,
            b'u',
            0,
            5,
            0,
            7,
            BINARY_8,
            7,
            b'i',
            b'n',
            b't',
            b'e',
            b'r',
            b'o',
            b'p',
        ];
        let node = unmarshal(&[&[0u8][..], &interop[..]].concat()).expect("decodes");
        let jid = node.get_jid("id").expect("a JID");
        assert_eq!(
            (jid.server, jid.device, jid.integrator),
            (Server::Interop, 5, 7)
        );

        // FB_JID(user "m", device 2, server "msgr").
        let messenger = [
            LIST_8, 3, 25, 8, FB_JID, BINARY_8, 1, b'm', 0, 2, BINARY_8, 4, b'm', b's', b'g', b'r',
        ];
        let node = unmarshal(&[&[0u8][..], &messenger[..]].concat()).expect("decodes");
        let jid = node.get_jid("id").expect("a JID");
        assert_eq!((jid.server, jid.device), (Server::Messenger, 2));
    }

    #[test]
    fn compressed_payloads_decode_and_bombs_are_refused() {
        let node = Node::new("iq").attr("id", "42").bytes(vec![1; 500]);
        let plain = marshal(&node).expect("encodes");
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&plain[1..]).expect("compresses");
        let mut compressed = vec![2u8];
        compressed.extend(encoder.finish().expect("finishes"));
        assert_eq!(unmarshal(&compressed).expect("decodes"), node);

        let mut bomb_encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        bomb_encoder
            .write_all(&vec![0u8; MAX_DECOMPRESSED + 1024])
            .expect("compresses");
        let mut bomb = vec![2u8];
        bomb.extend(bomb_encoder.finish().expect("finishes"));
        assert!(unmarshal(&bomb).is_err());
    }

    #[test]
    fn empty_and_flagless_garbage_is_rejected() {
        assert!(unmarshal(&[]).is_err());
        assert!(unmarshal(&[0, 0xF9, 0xFF]).is_err());
    }
}
