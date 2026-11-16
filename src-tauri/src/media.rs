//! The `chirper-media` custom protocol. `chirper-media://localhost/<urlencoded mxc>` is resolved
//! by the core to a file inside the data directory and served with the right MIME type, byte
//! ranges (206) for audio and video, and no CORS headers. Every failure is a 404.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use percent_encoding::percent_decode_str;
use tauri::http::{self, Request, Response, StatusCode, header};
use tauri::{AppHandle, Manager, UriSchemeContext, UriSchemeResponder, Wry};

use crate::state::AppState;

pub const SCHEME: &str = "chirper-media";
const CACHE_CONTROL: &str = "private, max-age=31536000, immutable";
const SNIFF_BYTES: usize = 64;

/// The protocol handler. It answers asynchronously because the core resolves the file.
pub fn handle(
    ctx: UriSchemeContext<'_, Wry>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let Some(mxc) = mxc_from_uri(request.uri()) else {
        responder.respond(not_found());
        return;
    };
    tauri::async_runtime::spawn(async move {
        let response = serve(&app, &mxc, range.as_deref())
            .await
            .unwrap_or_else(|_| not_found());
        responder.respond(response);
    });
}

/// The mxc URI of a request, or `None` when the path is not a valid mxc URI.
pub fn mxc_from_uri(uri: &http::Uri) -> Option<String> {
    let encoded = uri.path().strip_prefix('/')?;
    let mxc = percent_decode_str(encoded).decode_utf8().ok()?.into_owned();
    chirper_core::validate::mxc_url(&mxc).ok()?;
    Some(mxc)
}

async fn serve(
    app: &AppHandle,
    mxc: &str,
    range: Option<&str>,
) -> Result<Response<Vec<u8>>, ServeError> {
    let state = app.state::<AppState>();
    let core = state.ready_core().map_err(|_| ServeError)?;
    let path = core.media_file(mxc).await.map_err(|_| ServeError)?;
    let root = state.core_dir().to_path_buf();
    let range = range.map(str::to_owned);
    tauri::async_runtime::spawn_blocking(move || read_inside(&root, &path, range.as_deref()))
        .await
        .map_err(|_| ServeError)?
}

#[derive(Debug)]
pub struct ServeError;

/// Reads `path` only when it resolves inside `root`, then builds the response.
fn read_inside(
    root: &Path,
    path: &Path,
    range: Option<&str>,
) -> Result<Response<Vec<u8>>, ServeError> {
    let canonical = std::fs::canonicalize(path).map_err(|_| ServeError)?;
    if !canonical.starts_with(root) || !canonical.is_file() {
        return Err(ServeError);
    }
    read_range(&canonical, range)
}

fn read_range(path: &Path, range: Option<&str>) -> Result<Response<Vec<u8>>, ServeError> {
    let mut file = File::open(path).map_err(|_| ServeError)?;
    let size = file.metadata().map_err(|_| ServeError)?.len();
    let mut head = [0u8; SNIFF_BYTES];
    let read = file.read(&mut head).map_err(|_| ServeError)?;
    let mime = sniff_mime(&head[..read]);
    file.seek(SeekFrom::Start(0)).map_err(|_| ServeError)?;

    let (status, start, end) = match range {
        None => (StatusCode::OK, 0, size.saturating_sub(1)),
        Some(value) => {
            let (start, end) = parse_range(value, size).ok_or(ServeError)?;
            (StatusCode::PARTIAL_CONTENT, start, end)
        }
    };
    let length = if size == 0 { 0 } else { end - start + 1 };
    let mut body = vec![0u8; usize::try_from(length).map_err(|_| ServeError)?];
    if length > 0 {
        file.seek(SeekFrom::Start(start)).map_err(|_| ServeError)?;
        file.read_exact(&mut body).map_err(|_| ServeError)?;
    }

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, length)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, CACHE_CONTROL)
        .header("X-Content-Type-Options", "nosniff");
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    builder.body(body).map_err(|_| ServeError)
}

/// Parses one `bytes=` range against `size`. Returns the inclusive start and end.
pub fn parse_range(value: &str, size: u64) -> Option<(u64, u64)> {
    let spec = value.strip_prefix("bytes=")?;
    if spec.contains(',') || size == 0 {
        return None;
    }
    let (first, last) = spec.split_once('-')?;
    let (start, end) = if first.is_empty() {
        let suffix: u64 = last.parse().ok()?;
        if suffix == 0 {
            return None;
        }
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start: u64 = first.parse().ok()?;
        let end = if last.is_empty() {
            size - 1
        } else {
            last.parse::<u64>().ok()?.min(size - 1)
        };
        (start, end)
    };
    (start <= end && start < size).then_some((start, end))
}

/// The MIME type of a file from its first bytes. Unknown content is `application/octet-stream`.
pub fn sniff_mime(head: &[u8]) -> &'static str {
    let at = |range: std::ops::Range<usize>| head.get(range).unwrap_or(&[]);
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if head.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if head.starts_with(b"GIF8") {
        "image/gif"
    } else if head.starts_with(b"%PDF") {
        "application/pdf"
    } else if head.starts_with(b"OggS") {
        "audio/ogg"
    } else if head.starts_with(b"fLaC") {
        "audio/flac"
    } else if head.starts_with(b"ID3")
        || head.starts_with(&[0xFF, 0xFB])
        || head.starts_with(&[0xFF, 0xF3])
    {
        "audio/mpeg"
    } else if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        "video/webm"
    } else if at(0..4) == b"RIFF" && at(8..12) == b"WEBP" {
        "image/webp"
    } else if at(0..4) == b"RIFF" && at(8..12) == b"WAVE" {
        "audio/wav"
    } else if at(4..8) == b"ftyp" {
        match at(8..12) {
            b"heic" | b"heix" | b"mif1" | b"msf1" => "image/heic",
            b"avif" => "image/avif",
            b"M4A " | b"M4B " => "audio/mp4",
            b"qt  " => "video/quicktime",
            _ => "video/mp4",
        }
    } else {
        "application/octet-stream"
    }
}

fn not_found() -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_LENGTH, 0)
        .body(Vec::new())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mxc_is_decoded_and_validated() {
        let uri: http::Uri = "chirper-media://localhost/mxc%3A%2F%2Fchirper.local%2Fabc_DEF-1"
            .parse()
            .unwrap();
        assert_eq!(
            mxc_from_uri(&uri).as_deref(),
            Some("mxc://chirper.local/abc_DEF-1")
        );
        let bad: http::Uri = "chirper-media://localhost/https%3A%2F%2Fexample.invalid%2Fx"
            .parse()
            .unwrap();
        assert_eq!(mxc_from_uri(&bad), None);
        let traversal: http::Uri = "chirper-media://localhost/mxc%3A%2F%2Fchirper.local%2F..%2Fx"
            .parse()
            .unwrap();
        assert_eq!(mxc_from_uri(&traversal), None);
    }

    #[test]
    fn ranges_parse_and_clamp() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=900-5000", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=5-1", 1000), None);
        assert_eq!(parse_range("bytes=0-1,5-6", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
        assert_eq!(parse_range("bytes=0-1", 0), None);
    }

    #[test]
    fn sniffs_common_media() {
        assert_eq!(sniff_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), "image/jpeg");
        assert_eq!(sniff_mime(b"\x89PNG\r\n\x1a\n"), "image/png");
        assert_eq!(sniff_mime(b"RIFF\x00\x00\x00\x00WEBPVP8 "), "image/webp");
        assert_eq!(sniff_mime(b"RIFF\x00\x00\x00\x00WAVEfmt "), "audio/wav");
        assert_eq!(sniff_mime(b"\x00\x00\x00\x18ftypisom"), "video/mp4");
        assert_eq!(sniff_mime(b"\x00\x00\x00\x18ftypM4A "), "audio/mp4");
        assert_eq!(sniff_mime(b"\x00\x00\x00\x14ftypqt  "), "video/quicktime");
        assert_eq!(sniff_mime(&[0x1A, 0x45, 0xDF, 0xA3]), "video/webm");
        assert_eq!(sniff_mime(b"ID3\x04"), "audio/mpeg");
        assert_eq!(sniff_mime(b"not media"), "application/octet-stream");
        assert_eq!(sniff_mime(&[]), "application/octet-stream");
    }

    #[test]
    fn read_range_serves_partial_content_and_full_file() {
        let dir = std::env::temp_dir().join(format!("chirper-media-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("clip.bin");
        std::fs::write(&file, b"\x89PNG\r\n\x1a\n0123456789").unwrap();

        let full = read_range(&file, None).unwrap();
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(full.body().len(), 18);
        assert!(full.headers().get("access-control-allow-origin").is_none());

        let part = read_range(&file, Some("bytes=8-11")).unwrap();
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(part.body().as_slice(), b"0123");
        assert_eq!(part.headers()[header::CONTENT_RANGE], "bytes 8-11/18");
        assert!(read_range(&file, Some("bytes=50-")).is_err());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn files_outside_the_root_are_refused() {
        let base = std::env::temp_dir().join(format!("chirper-root-test-{}", std::process::id()));
        let root = base.join("core");
        std::fs::create_dir_all(&root).unwrap();
        let outside = base.join("secret.txt");
        std::fs::write(&outside, b"x").unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        assert!(read_inside(&root, &outside, None).is_err());
        std::fs::remove_dir_all(base).unwrap();
    }
}
