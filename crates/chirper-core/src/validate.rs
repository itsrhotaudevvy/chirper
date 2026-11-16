//! Input validation run before any value crosses the FFI boundary. Error messages name the field
//! only, never the value, so that message bodies and ids do not leak into logs.

use std::path::{Path, PathBuf};

use crate::{CoreError, Result};

pub const MAX_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_REACTION_KEY_BYTES: usize = 64;
pub const MAX_PAGE_LIMIT: u32 = 1000;
pub const MAX_QUERY_BYTES: usize = 1024;
const MAX_ID_LEN: usize = 200;
const MAX_MXC_ID_LEN: usize = 128;
const MAX_NETWORK_ID_LEN: usize = 32;
const MAX_FILE_NAME_BYTES: usize = 255;
const MAX_MIME_BYTES: usize = 255;
const MEDIA_KINDS: [&str; 6] = ["image", "video", "audio", "file", "sticker", "voice"];

fn invalid<T>(field: &str) -> Result<T> {
    Err(CoreError::InvalidInput(field.to_owned()))
}

fn is_id_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'=' | b'-')
}

fn is_id_body(s: &str, max_len: usize) -> bool {
    (1..=max_len).contains(&s.len()) && s.bytes().all(is_id_byte)
}

/// `^![A-Za-z0-9._=-]{1,200}:chirper\.local$`
pub fn room_id(s: &str) -> Result<()> {
    match s
        .strip_prefix('!')
        .and_then(|rest| rest.strip_suffix(":chirper.local"))
    {
        Some(body) if is_id_body(body, MAX_ID_LEN) => Ok(()),
        _ => invalid("room id"),
    }
}

/// `^\$[A-Za-z0-9._=-]{1,200}$`. Local ids such as `$local.<uuid>` match this pattern too.
pub fn event_id(s: &str) -> Result<()> {
    match s.strip_prefix('$') {
        Some(body) if is_id_body(body, MAX_ID_LEN) => Ok(()),
        _ => invalid("event id"),
    }
}

/// Optional event id: an empty string means "none".
pub fn optional_event_id(s: &str) -> Result<()> {
    if s.is_empty() { Ok(()) } else { event_id(s) }
}

/// `^[a-z0-9]{1,32}$`
pub fn network_id(s: &str) -> Result<()> {
    let ok = (1..=MAX_NETWORK_ID_LEN).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    if ok { Ok(()) } else { invalid("network id") }
}

/// A network id where an empty string means "all networks".
pub fn optional_network_id(s: &str) -> Result<()> {
    if s.is_empty() { Ok(()) } else { network_id(s) }
}

/// `^mxc://chirper\.local/[A-Za-z0-9_-]{1,128}$`
pub fn mxc_url(s: &str) -> Result<()> {
    let body = s.strip_prefix("mxc://chirper.local/").unwrap_or("");
    let ok = (1..=MAX_MXC_ID_LEN).contains(&body.len())
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if ok { Ok(()) } else { invalid("mxc url") }
}

/// Message text and captions: at most 64 KiB of UTF-8.
pub fn text(s: &str) -> Result<()> {
    if s.len() <= MAX_TEXT_BYTES {
        Ok(())
    } else {
        invalid("text")
    }
}

/// Settings and login values share the text limit.
pub fn bounded_value(s: &str) -> Result<()> {
    text(s).map_err(|_| CoreError::InvalidInput("value".to_owned()))
}

/// A reaction key (usually an emoji): 1 to 64 bytes, no control characters.
pub fn reaction_key(s: &str) -> Result<()> {
    let ok = (1..=MAX_REACTION_KEY_BYTES).contains(&s.len()) && !s.chars().any(char::is_control);
    if ok { Ok(()) } else { invalid("reaction key") }
}

/// Opaque ids minted by the shell or core (process, flow, login, transaction): 1 to 200 printable
/// ASCII characters with no spaces.
pub fn token(s: &str) -> Result<()> {
    let ok = (1..=MAX_ID_LEN).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_graphic());
    if ok { Ok(()) } else { invalid("token") }
}

pub fn media_kind(s: &str) -> Result<()> {
    if MEDIA_KINDS.contains(&s) {
        Ok(())
    } else {
        invalid("media kind")
    }
}

pub fn mime_type(s: &str) -> Result<()> {
    let ok = (1..=MAX_MIME_BYTES).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_graphic());
    if ok { Ok(()) } else { invalid("mime type") }
}

/// A display file name: no path separators and no control characters.
pub fn file_name(s: &str) -> Result<()> {
    let ok = s.len() <= MAX_FILE_NAME_BYTES
        && !s.contains(['/', '\\'])
        && !s.chars().any(char::is_control);
    if ok { Ok(()) } else { invalid("file name") }
}

pub fn query(s: &str) -> Result<()> {
    if s.len() <= MAX_QUERY_BYTES {
        Ok(())
    } else {
        invalid("query")
    }
}

pub fn page_limit(limit: u32) -> Result<u32> {
    if (1..=MAX_PAGE_LIMIT).contains(&limit) {
        Ok(limit)
    } else {
        invalid("limit")
    }
}

/// Canonicalises a file chosen by the user for upload. The path must name an existing regular file
/// and be valid UTF-8 (it travels as JSON).
pub fn existing_file(path: &Path) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)?;
    if !canonical.is_file() {
        return invalid("file path");
    }
    if canonical.to_str().is_none() {
        return invalid("file path");
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepts(r: Result<()>) -> bool {
        r.is_ok()
    }

    #[test]
    fn room_id_table() {
        let long = format!("!{}:chirper.local", "a".repeat(200));
        let too_long = format!("!{}:chirper.local", "a".repeat(201));
        let cases: &[(&str, bool)] = &[
            ("!abc123:chirper.local", true),
            ("!a.b_c=d-e:chirper.local", true),
            (long.as_str(), true),
            (too_long.as_str(), false),
            ("!:chirper.local", false),
            ("abc:chirper.local", false),
            ("!abc:example.org", false),
            ("!abc:chirper.local:chirper.local", false),
            ("!ab c:chirper.local", false),
            ("!ab/c:chirper.local", false),
            ("", false),
        ];
        for (input, ok) in cases {
            assert_eq!(accepts(room_id(input)), *ok, "room id {input:?}");
        }
    }

    #[test]
    fn event_id_table() {
        let cases: &[(&str, bool)] = &[
            ("$abc.def_1=2-3", true),
            ("$local.3f2a9c0e-1b7d-4e8f-9a6b-0c1d2e3f4a5b", true),
            ("$", false),
            ("abc", false),
            ("$a b", false),
            ("$a/b", false),
            ("$é", false),
        ];
        for (input, ok) in cases {
            assert_eq!(accepts(event_id(input)), *ok, "event id {input:?}");
        }
        assert!(optional_event_id("").is_ok());
        assert!(optional_event_id("x").is_err());
    }

    #[test]
    fn network_id_table() {
        assert!(network_id("discord").is_ok());
        assert!(network_id("tg2").is_ok());
        assert!(network_id(&"a".repeat(32)).is_ok());
        assert!(network_id(&"a".repeat(33)).is_err());
        assert!(network_id("").is_err());
        assert!(network_id("Discord").is_err());
        assert!(network_id("sig-nal").is_err());
        assert!(optional_network_id("").is_ok());
    }

    #[test]
    fn mxc_table() {
        assert!(mxc_url("mxc://chirper.local/abc_DEF-123").is_ok());
        assert!(mxc_url(&format!("mxc://chirper.local/{}", "a".repeat(128))).is_ok());
        assert!(mxc_url(&format!("mxc://chirper.local/{}", "a".repeat(129))).is_err());
        assert!(mxc_url("mxc://matrix.org/abc").is_err());
        assert!(mxc_url("mxc://chirper.local/").is_err());
        assert!(mxc_url("mxc://chirper.local/../etc").is_err());
        assert!(mxc_url("https://chirper.local/abc").is_err());
    }

    #[test]
    fn text_and_key_limits() {
        assert!(text("").is_ok());
        assert!(text(&"x".repeat(MAX_TEXT_BYTES)).is_ok());
        assert!(text(&"x".repeat(MAX_TEXT_BYTES + 1)).is_err());
        assert!(reaction_key("👍").is_ok());
        assert!(reaction_key(&"k".repeat(64)).is_ok());
        assert!(reaction_key(&"k".repeat(65)).is_err());
        assert!(reaction_key("").is_err());
        assert!(reaction_key("a\nb").is_err());
    }

    #[test]
    fn token_and_media_tables() {
        assert!(token("txn-1700000000.abc").is_ok());
        assert!(token("").is_err());
        assert!(token("has space").is_err());
        assert!(token("ctl\u{7}").is_err());
        assert!(media_kind("voice").is_ok());
        assert!(media_kind("document").is_err());
        assert!(mime_type("image/png").is_ok());
        assert!(mime_type("image png").is_err());
        assert!(file_name("photo.jpg").is_ok());
        assert!(file_name("../photo.jpg").is_err());
        assert!(file_name("a\\b").is_err());
        assert!(file_name(&"f".repeat(256)).is_err());
    }

    #[test]
    fn page_limit_and_query_bounds() {
        assert_eq!(page_limit(1).unwrap(), 1);
        assert!(page_limit(0).is_err());
        assert!(page_limit(MAX_PAGE_LIMIT + 1).is_err());
        assert!(query(&"q".repeat(MAX_QUERY_BYTES)).is_ok());
        assert!(query(&"q".repeat(MAX_QUERY_BYTES + 1)).is_err());
    }

    #[test]
    fn existing_file_canonicalises_and_rejects_directories() {
        let dir = std::env::temp_dir();
        assert!(existing_file(&dir).is_err());
        assert!(existing_file(&dir.join("chirper-core-no-such-file")).is_err());
        let file = dir.join(format!("chirper-core-validate-{}", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let canonical = existing_file(&file).unwrap();
        assert!(canonical.is_absolute());
        std::fs::remove_file(&file).unwrap();
    }
}
