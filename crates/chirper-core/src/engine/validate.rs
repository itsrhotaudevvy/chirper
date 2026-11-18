//! Input checks shared by the facade. The shell validates too; a wrong argument must never reach
//! the database or a network driver.

use super::{Error, Result};

/// Largest text a message may carry.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// Largest page a caller may ask for.
pub const MAX_PAGE: u32 = 500;

pub fn room_id(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 255 || !value.starts_with('!') || !value.contains(':') {
        return Err(Error::InvalidInput("room id".to_owned()));
    }
    if value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(Error::InvalidInput("room id".to_owned()));
    }
    Ok(())
}

pub fn event_id(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 255 || !value.starts_with('$') {
        return Err(Error::InvalidInput("event id".to_owned()));
    }
    if value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(Error::InvalidInput("event id".to_owned()));
    }
    Ok(())
}

/// A short identifier: a login id, a process id, a transaction id or a flow id.
pub fn token(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 200 {
        return Err(Error::InvalidInput("identifier".to_owned()));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '@' | '!'))
    {
        return Err(Error::InvalidInput("identifier".to_owned()));
    }
    Ok(())
}

pub fn network_id(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 40 || !value.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
        return Err(Error::InvalidInput("network".to_owned()));
    }
    Ok(())
}

pub fn text(value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::InvalidInput("text is empty".to_owned()));
    }
    if value.len() > MAX_TEXT_BYTES {
        return Err(Error::InvalidInput("text is too long".to_owned()));
    }
    Ok(())
}

pub fn caption(value: &str) -> Result<()> {
    if value.len() > MAX_TEXT_BYTES {
        return Err(Error::InvalidInput("caption is too long".to_owned()));
    }
    Ok(())
}

pub fn page_limit(limit: u32) -> Result<usize> {
    if limit == 0 || limit > MAX_PAGE {
        return Err(Error::InvalidInput("page limit".to_owned()));
    }
    Ok(limit as usize)
}

pub fn reaction_key(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 64 {
        return Err(Error::InvalidInput("reaction".to_owned()));
    }
    Ok(())
}

pub fn media_kind(value: &str) -> Result<()> {
    match value {
        "image" | "video" | "audio" | "voice" | "file" | "sticker" => Ok(()),
        _ => Err(Error::InvalidInput("media kind".to_owned())),
    }
}

pub fn mime_type(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 200 || !value.contains('/') {
        return Err(Error::InvalidInput("mime type".to_owned()));
    }
    Ok(())
}

pub fn file_name(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 255
        || value.contains('/')
        || value.contains('\\')
        || value.contains("..")
        || value.chars().any(char::is_control)
    {
        return Err(Error::InvalidInput("file name".to_owned()));
    }
    Ok(())
}

pub fn existing_file(path: &std::path::Path) -> Result<std::path::PathBuf> {
    let path = path
        .canonicalize()
        .map_err(|_| Error::InvalidInput("file".to_owned()))?;
    let meta = std::fs::metadata(&path).map_err(|_| Error::InvalidInput("file".to_owned()))?;
    if !meta.is_file() {
        return Err(Error::InvalidInput("file".to_owned()));
    }
    if meta.len() > 512 * 1024 * 1024 {
        return Err(Error::InvalidInput("file is too large".to_owned()));
    }
    Ok(path)
}

/// A local media URI: `mxc://chirper.local/<id>` from this device only.
pub fn mxc_url(value: &str) -> Result<()> {
    if crate::ids::media_id_of(value).is_none() {
        return Err(Error::InvalidInput("media url".to_owned()));
    }
    Ok(())
}

pub fn query(value: &str) -> Result<()> {
    if value.len() > 500 {
        return Err(Error::InvalidInput("query".to_owned()));
    }
    Ok(())
}