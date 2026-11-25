//! Media transfer: downloads from `upload.getFile` (in 512 KiB chunks, from the file's own data
//! centre) and uploads for sends (`upload.saveFilePart` and `upload.saveBigFilePart` from 10 MiB).

use chirper_bridge::{Error, Result};

use crate::client::Api;
use crate::convert::{Download, MAX_MEDIA_BYTES};
use crate::error::CallError;
use crate::mtproto::crypto;
use crate::tl::{enums, functions, types};

/// Bytes per `upload.getFile` and `upload.saveFilePart` chunk.
pub const CHUNK: usize = 512 * 1024;
/// Files at least this large use `upload.saveBigFilePart`.
pub const BIG_FILE: usize = 10 * 1024 * 1024;
/// Most parts a big upload may have.
const MAX_PARTS: usize = 4000;

/// Download a media file completely, refusing anything over [`MAX_MEDIA_BYTES`].
pub async fn download<A: Api>(api: &A, file: &Download) -> Result<Vec<u8>> {
    let declared = i64::try_from(file.info.size).unwrap_or(i64::MAX);
    if declared > MAX_MEDIA_BYTES {
        return Err(Error::Unsupported("files larger than 100 MiB"));
    }
    api.authorize_dc(file.dc).await.map_err(Error::from)?;
    let mut data: Vec<u8> = Vec::new();
    let mut offset: i64 = 0;
    let limit = i32::try_from(CHUNK).unwrap_or(i32::MAX);
    loop {
        let request = functions::upload::GetFile {
            precise: false,
            cdn_supported: false,
            location: Box::new(file.location.clone()),
            offset,
            limit,
        };
        let answer = api.call_on(file.dc, &request).await.map_err(Error::from)?;
        let part = match answer {
            enums::upload::File::File(part) => part,
            enums::upload::File::FileCdnRedirect(_) => {
                return Err(Error::Unsupported("files served from a CDN"));
            }
        };
        let received = part.bytes.len();
        data.extend_from_slice(&part.bytes);
        if i64::try_from(data.len()).unwrap_or(i64::MAX) > MAX_MEDIA_BYTES {
            return Err(Error::Unsupported("files larger than 100 MiB"));
        }
        if received < CHUNK {
            return Ok(data);
        }
        offset += i64::try_from(received).unwrap_or(0);
    }
}

/// Upload a file in parts and return its input handle for `messages.sendMedia`.
pub async fn upload<A: Api>(api: &A, data: &[u8], name: &str) -> Result<enums::InputFile> {
    if data.len() > MAX_MEDIA_BYTES as usize {
        return Err(Error::Unsupported("sending files larger than 100 MiB"));
    }
    let total = data.len().div_ceil(CHUNK).max(1);
    if total > MAX_PARTS {
        return Err(Error::Unsupported("sending files this large"));
    }
    let file_id = i64::from_le_bytes(
        crypto::random_array::<8>().map_err(|e| Error::Internal(e.to_string()))?,
    );
    let big = data.len() >= BIG_FILE;
    let total_parts =
        i32::try_from(total).map_err(|_| Error::Unsupported("sending files this large"))?;
    for (index, chunk) in data.chunks(CHUNK).enumerate() {
        let part = i32::try_from(index).map_err(|_| Error::Protocol("too many parts".into()))?;
        if big {
            let request = functions::upload::SaveBigFilePart {
                file_id,
                file_part: part,
                file_total_parts: total_parts,
                bytes: chunk.to_vec(),
            };
            api.call(&request).await.map_err(to_bridge)?;
        } else {
            let request = functions::upload::SaveFilePart {
                file_id,
                file_part: part,
                bytes: chunk.to_vec(),
            };
            api.call(&request).await.map_err(to_bridge)?;
        }
    }
    let name = name.to_owned();
    Ok(if big {
        enums::InputFile::InputFileBig(types::InputFileBig {
            id: file_id,
            parts: total_parts,
            name,
        })
    } else {
        enums::InputFile::InputFile(types::InputFile {
            id: file_id,
            parts: total_parts,
            name,
            md5_checksum: String::new(),
        })
    })
}

fn to_bridge(error: CallError) -> Error {
    error.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_sizes_follow_the_protocol() {
        assert_eq!(CHUNK, 524_288);
        assert_eq!(BIG_FILE, 10 * 1024 * 1024);
        assert_eq!(10 * 1024 * 1024 / CHUNK, 20);
    }
}
