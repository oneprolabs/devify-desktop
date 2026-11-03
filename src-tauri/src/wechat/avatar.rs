//! Avatars out of `head_image/head_image.db`.
//!
//! WeChat 4.x keeps contact and group avatars as raw image bytes keyed by username:
//! `head_image(username TEXT PRIMARY KEY, md5 TEXT, image_buffer BLOB, update_time INTEGER)`.
//! The file is one of the encrypted databases, so it only exists in the cache once the account
//! has been decrypted; before that every lookup is simply empty and the UI keeps its letter
//! avatars.

use rusqlite::{Connection, OpenFlags};
use std::path::Path;

pub struct Avatar {
    pub username: String,
    /// `data:` URL — the WebView can render it without exposing the cache directory.
    pub data_url: String,
}

/// Raw avatar bytes plus their media type, for the upload package.
pub struct AvatarImage {
    pub username: String,
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

fn mime_of(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(triple >> 18) as usize & 63] as char);
        out.push(TABLE[(triple >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(triple >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[triple as usize & 63] as char } else { '=' });
    }
    out
}

/// Look up the given usernames. Unknown names, an empty list and a missing database all just
/// produce fewer results — the caller renders letters instead.
pub fn avatars(cache: &Path, usernames: &[String]) -> Vec<Avatar> {
    images(cache, usernames)
        .into_iter()
        .map(|image| Avatar {
            username: image.username,
            data_url: format!(
                "data:{};base64,{}",
                image.content_type,
                base64(&image.bytes)
            ),
        })
        .collect()
}

/// Raw avatar bytes for the upload package, keeping only real images.
pub fn images(cache: &Path, usernames: &[String]) -> Vec<AvatarImage> {
    let path = cache.join("head_image").join("head_image.db");
    if !path.is_file() || usernames.is_empty() {
        return Vec::new();
    }
    let Ok(conn) = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };

    let placeholders = vec!["?"; usernames.len()].join(",");
    let sql = format!("select username, image_buffer from head_image where username in ({placeholders})");
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let params = rusqlite::params_from_iter(usernames.iter());
    let Ok(rows) = stmt.query_map(params, |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<Vec<u8>>>(1)?))
    }) else {
        return Vec::new();
    };

    rows.flatten()
        .filter_map(|(username, bytes)| {
            let bytes = bytes?;
            let content_type = mime_of(&bytes)?;
            Some(AvatarImage {
                username,
                bytes,
                content_type,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"hello, world"), "aGVsbG8sIHdvcmxk");
    }

    #[test]
    fn mime_follows_the_magic_bytes() {
        assert_eq!(mime_of(&[0x89, b'P', b'N', b'G', 0x0d]), Some("image/png"));
        assert_eq!(mime_of(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(mime_of(b"GIF89a"), Some("image/gif"));
        assert_eq!(mime_of(b"RIFF1234WEBPVP8 "), Some("image/webp"));
        assert_eq!(mime_of(b"not an image"), None);
    }

    #[test]
    fn missing_cache_is_not_an_error() {
        assert!(avatars(Path::new("/nonexistent/wechat-cache"), &["wxid_x".into()]).is_empty());
    }
}
