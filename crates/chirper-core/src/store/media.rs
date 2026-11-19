//! Media rows and the files they point at.

use std::path::{Path, PathBuf};

use rusqlite::types::Value as SqlValue;
use rusqlite::{OptionalExtension, params, params_from_iter};

use super::{Error, Result, Store, now_ms};

/// One stored media file.
#[derive(Clone, Debug, Default)]
pub struct Media {
    pub media_id: String,
    pub mime: String,
    pub size: i64,
    pub file_name: String,
    /// Path relative to the media directory.
    pub path: String,
    /// The JSON key of an encrypted file, when media encryption is on.
    pub encrypted_key_json: Option<String>,
    pub created_ts: i64,
}

impl Store {
    /// Record a media file. The bytes must already be at `m.path`.
    pub fn put_media(&self, m: &Media) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO media (media_id, mime, size, file_name, path, encrypted_key_json, created_ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                m.media_id,
                m.mime,
                m.size,
                m.file_name,
                m.path,
                m.encrypted_key_json,
                if m.created_ts == 0 { now_ms() } else { m.created_ts },
            ],
        )?;
        Ok(())
    }

    pub fn media(&self, media_id: &str) -> Result<Media> {
        let conn = self.conn();
        conn.query_row(
            "SELECT media_id, mime, size, file_name, path, encrypted_key_json, created_ts
             FROM media WHERE media_id = ?1",
            params![media_id],
            |row| {
                Ok(Media {
                    media_id: row.get(0)?,
                    mime: row.get(1)?,
                    size: row.get(2)?,
                    file_name: row.get(3)?,
                    path: row.get(4)?,
                    encrypted_key_json: row.get(5)?,
                    created_ts: row.get(6)?,
                })
            },
        )
        .optional()?
        .ok_or(Error::NotFound)
    }
}

/// Record that an event refers to the local media named in its content.
pub(super) fn link_media_tx(
    tx: &rusqlite::Transaction<'_>,
    event_id: &str,
    room_id: &str,
    content_json: &str,
) -> Result<()> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content_json) else {
        return Ok(());
    };
    // Both the new content shape (`media.url`/`media.thumbnailUrl`) and the Go one
    // (`url`/`info.thumbnail_url`) name media that may live on this device.
    let mut uris: Vec<String> = Vec::new();
    let mut push = |u: Option<&str>| {
        if let Some(u) = u
            && !u.is_empty()
        {
            uris.push(u.to_owned());
        }
    };
    push(value.pointer("/media/url").and_then(|v| v.as_str()));
    push(
        value
            .pointer("/media/thumbnailUrl")
            .and_then(|v| v.as_str()),
    );
    push(value.get("url").and_then(|v| v.as_str()));
    push(
        value
            .pointer("/info/thumbnail_url")
            .and_then(|v| v.as_str()),
    );
    for uri in uris {
        let Some(media_id) = chirper_bridge::ids::media_id_of(&uri) else {
            continue;
        };
        tx.execute(
            "INSERT INTO event_media (event_id, room_id, media_id) VALUES (?1, ?2, ?3)
             ON CONFLICT DO NOTHING",
            params![event_id, room_id, media_id],
        )?;
    }
    Ok(())
}

/// Remove the media links selected by `where`, delete rows left without links, and return the file
/// paths to delete after the transaction commits. Must run inside a transaction.
pub(super) fn unlink_media_tx(
    tx: &rusqlite::Transaction<'_>,
    where_clause: &str,
    args: &[SqlValue],
) -> Result<Vec<String>> {
    let media_ids: Vec<String> = {
        let mut stmt = tx.prepare(&format!(
            "SELECT DISTINCT media_id FROM event_media WHERE {where_clause}"
        ))?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    tx.execute(
        &format!("DELETE FROM event_media WHERE {where_clause}"),
        params_from_iter(args.iter()),
    )?;
    let mut paths = Vec::new();
    for media_id in media_ids {
        let still_linked: bool = tx
            .query_row(
                "SELECT 1 FROM event_media WHERE media_id = ?1 LIMIT 1",
                params![media_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if still_linked {
            continue;
        }
        let path: Option<String> = tx
            .query_row(
                "SELECT path FROM media WHERE media_id = ?1",
                params![media_id],
                |row| row.get(0),
            )
            .optional()?;
        tx.execute("DELETE FROM media WHERE media_id = ?1", params![media_id])?;
        if let Some(path) = path {
            paths.push(path);
        }
    }
    Ok(paths)
}

/// Delete media files after their rows are committed. Failures are logged, not fatal: the row is
/// already gone, so the file would otherwise leak forever.
pub(super) fn remove_media_files(paths: &[String]) {
    for path in paths {
        if let Err(err) = std::fs::remove_file(Path::new(path))
            && err.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("cannot delete media file: {err}");
        }
    }
}

/// The media directory of a data directory.
pub fn media_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("media")
}
