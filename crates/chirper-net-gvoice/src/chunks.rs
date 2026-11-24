//! The length-prefixed chunk framing Google uses for `text/plain` answers and for the realtime
//! long-poll stream.
//!
//! A chunk is a decimal length, a newline, then UTF-8 text whose length counts UTF-16 code units
//! (a character outside the BMP counts twice). Ported from `pkg/libgv/utf16chunk/chunkreader.go`
//! of mautrix-gvoice (AGPL-3.0). The reader is fed bytes as they arrive, so it works on a stream.

use chirper_bridge::{Error, Result};

/// Longest decimal length prefix accepted; a longer run is not a length.
const MAX_PREFIX: usize = 12;

/// Incremental reader of length-prefixed chunks.
#[derive(Debug, Default)]
pub struct ChunkReader {
    buf: Vec<u8>,
}

impl ChunkReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add bytes as they arrive.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete chunk, or `None` when more bytes are needed.
    pub fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        let Some(newline) = self.buf.iter().take(MAX_PREFIX + 1).position(|&b| b == b'\n') else {
            if self.buf.len() > MAX_PREFIX {
                return Err(bad_frame("no chunk length"));
            }
            return Ok(None);
        };
        let expected = parse_length(&self.buf[..newline])?;
        let start = newline + 1;
        let Some(end) = units_end(&self.buf[start..], expected)? else {
            return Ok(None);
        };
        let chunk = self.buf[start..start + end].to_vec();
        self.buf.drain(..start + end);
        Ok(Some(chunk))
    }
}

fn parse_length(digits: &[u8]) -> Result<usize> {
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err(bad_frame("the chunk length is not a number"));
    }
    std::str::from_utf8(digits)
        .ok()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| bad_frame("the chunk length is not a number"))
}

/// The byte length of the prefix of `text` that holds `units` UTF-16 code units, or `None` when
/// `text` ends before that many units.
fn units_end(text: &[u8], units: usize) -> Result<Option<usize>> {
    let mut counted = 0usize;
    let mut pos = 0usize;
    while counted < units {
        let Some(&lead) = text.get(pos) else {
            return Ok(None);
        };
        let (len, weight) = match lead {
            0x00..=0x7f => (1, 1),
            0xc0..=0xdf => (2, 1),
            0xe0..=0xef => (3, 1),
            0xf0..=0xf7 => (4, 2),
            _ => return Err(bad_frame("the chunk is not UTF-8")),
        };
        if pos + len > text.len() {
            return Ok(None);
        }
        if text[pos + 1..pos + len].iter().any(|&b| b & 0xc0 != 0x80) {
            return Err(bad_frame("the chunk is not UTF-8"));
        }
        counted += weight;
        pos += len;
    }
    Ok(Some(pos))
}

fn bad_frame(detail: &str) -> Error {
    Error::Protocol(format!("Google Voice sent a malformed chunk: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_chunks(reader: &mut ChunkReader) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Some(chunk) = reader.next_chunk().expect("well formed") {
            out.push(chunk);
        }
        out
    }

    #[test]
    fn reads_consecutive_chunks_from_one_buffer() {
        let mut reader = ChunkReader::new();
        reader.push(b"5\n[1,2]4\n[3]");
        assert_eq!(all_chunks(&mut reader), vec![b"[1,2]".to_vec()]);
        reader.push(b"]");
        assert_eq!(all_chunks(&mut reader), vec![b"[3]]".to_vec()]);
    }

    #[test]
    fn waits_for_the_rest_of_a_chunk() {
        let mut reader = ChunkReader::new();
        reader.push(b"10\nabc");
        assert_eq!(reader.next_chunk().expect("ok"), None);
        reader.push(b"defghij");
        assert_eq!(reader.next_chunk().expect("ok"), Some(b"abcdefghij".to_vec()));
    }

    #[test]
    fn waits_for_the_length_line() {
        let mut reader = ChunkReader::new();
        reader.push(b"1");
        assert_eq!(reader.next_chunk().expect("ok"), None);
        reader.push(b"2\n");
        assert_eq!(reader.next_chunk().expect("ok"), None);
    }

    #[test]
    fn counts_a_non_bmp_character_as_two_units() {
        // "😀" is one code point, four UTF-8 bytes and two UTF-16 units.
        let mut reader = ChunkReader::new();
        reader.push("2\n😀".as_bytes());
        assert_eq!(all_chunks(&mut reader), vec!["😀".as_bytes().to_vec()]);
        // A two-unit prefix does not stop after half of a pair.
        let mut reader = ChunkReader::new();
        reader.push("3\n😀a".as_bytes());
        assert_eq!(all_chunks(&mut reader), vec!["😀a".as_bytes().to_vec()]);
    }

    #[test]
    fn splits_multibyte_characters_across_reads() {
        let text = "1\nü".as_bytes();
        let mut reader = ChunkReader::new();
        reader.push(&text[..3]);
        assert_eq!(reader.next_chunk().expect("ok"), None);
        reader.push(&text[3..]);
        assert_eq!(reader.next_chunk().expect("ok"), Some("ü".as_bytes().to_vec()));
    }

    #[test]
    fn a_zero_length_chunk_is_empty() {
        let mut reader = ChunkReader::new();
        reader.push(b"0\nxy");
        assert_eq!(reader.next_chunk().expect("ok"), Some(Vec::new()));
    }

    #[test]
    fn refuses_malformed_framing() {
        let cases: [&[u8]; 4] = [b"x\nabc", b"\nabc", b"12345678901234567890", b"1\n\xff"];
        for case in cases {
            let mut reader = ChunkReader::new();
            reader.push(case);
            assert!(
                matches!(reader.next_chunk(), Err(Error::Protocol(_))),
                "{case:?} must be refused"
            );
        }
    }
}
