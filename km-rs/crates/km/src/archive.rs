//! Just enough of tar.gz and zip to pull the km executable out of a release
//! archive (the same archives `scripts/package_downloads.py` writes).

use flate2::read::{DeflateDecoder, GzDecoder};
use std::io::Read;

/// The bytes of `name` (for example "km") from a gzip-compressed tar archive.
pub fn from_tar_gz(archive: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let mut tar = Vec::new();
    GzDecoder::new(archive)
        .read_to_end(&mut tar)
        .map_err(|e| format!("the download is not a valid archive: {e}"))?;
    let mut offset = 0;
    while offset + 512 <= tar.len() {
        let header = &tar[offset..offset + 512];
        if header.iter().all(|b| *b == 0) {
            break;
        }
        let field = |range: std::ops::Range<usize>| {
            let raw = &header[range];
            let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
            String::from_utf8_lossy(&raw[..end]).trim().to_string()
        };
        let size = usize::from_str_radix(&field(124..136), 8).map_err(|_| "bad tar entry size")?;
        let prefix = field(345..500);
        let short = field(0..100);
        let path = if prefix.is_empty() { short } else { format!("{prefix}/{short}") };
        let kind = header[156];
        let start = offset + 512;
        if start + size > tar.len() {
            return Err("the archive is truncated".into());
        }
        if (kind == b'0' || kind == 0) && path.trim_start_matches("./") == name {
            return Ok(tar[start..start + size].to_vec());
        }
        offset = start + size.div_ceil(512) * 512;
    }
    Err(format!("{name} is missing from the archive"))
}

fn u16_at(data: &[u8], at: usize) -> Option<usize> {
    data.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
}

fn u32_at(data: &[u8], at: usize) -> Option<usize> {
    data.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

/// The bytes of `name` (for example "km.exe") from a zip archive (stored or deflated).
pub fn from_zip(archive: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let bad = || "the download is not a valid zip archive".to_string();
    // End of central directory: the last "PK\x05\x06" record.
    let eocd = (0..archive.len().saturating_sub(21))
        .rev()
        .find(|&i| archive[i..].starts_with(b"PK\x05\x06"))
        .ok_or_else(bad)?;
    let entries = u16_at(archive, eocd + 10).ok_or_else(bad)?;
    let mut at = u32_at(archive, eocd + 16).ok_or_else(bad)?;
    for _ in 0..entries {
        if !archive.get(at..).is_some_and(|rest| rest.starts_with(b"PK\x01\x02")) {
            return Err(bad());
        }
        let method = u16_at(archive, at + 10).ok_or_else(bad)?;
        let packed = u32_at(archive, at + 20).ok_or_else(bad)?;
        let unpacked = u32_at(archive, at + 24).ok_or_else(bad)?;
        let name_len = u16_at(archive, at + 28).ok_or_else(bad)?;
        let extra_len = u16_at(archive, at + 30).ok_or_else(bad)?;
        let comment_len = u16_at(archive, at + 32).ok_or_else(bad)?;
        let local = u32_at(archive, at + 42).ok_or_else(bad)?;
        let entry = archive.get(at + 46..at + 46 + name_len).ok_or_else(bad)?;
        if entry == name.as_bytes() {
            if !archive.get(local..).is_some_and(|rest| rest.starts_with(b"PK\x03\x04")) {
                return Err(bad());
            }
            let skip = 30 + u16_at(archive, local + 26).ok_or_else(bad)?
                + u16_at(archive, local + 28).ok_or_else(bad)?;
            let data = archive.get(local + skip..local + skip + packed).ok_or_else(bad)?;
            let bytes = match method {
                0 => data.to_vec(),
                8 => {
                    let mut out = Vec::with_capacity(unpacked);
                    DeflateDecoder::new(data).read_to_end(&mut out).map_err(|_| bad())?;
                    out
                }
                _ => return Err("the zip archive uses an unsupported compression".into()),
            };
            if bytes.len() != unpacked {
                return Err(bad());
            }
            return Ok(bytes);
        }
        at += 46 + name_len + extra_len + comment_len;
    }
    Err(format!("{name} is missing from the archive"))
}
