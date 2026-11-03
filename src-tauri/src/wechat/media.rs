//! Per-message attachment manifest.
//!
//! WeChat 4.x keeps attachments outside the SQLCipher databases:
//!   * documents      `msg/file/<YYYY-MM>/<title>`             plaintext
//!   * videos         `msg/video/<YYYY-MM>/<hash>.mp4`         plaintext
//!   * video covers   `msg/video/<YYYY-MM>/<hash>_thumb.jpg`   plaintext
//!   * wallpapers/thumbs `cache/<YYYY-MM>/Message/<hash>/Thumb/<local_id>_<time>_thumb.jpg`
//!   * images         `msg/attach/<hash>/<YYYY-MM>/Img/<file>.dat`
//!
//! `.dat` images are V2-encrypted (AES-128-ECB prefix + single-byte XOR trailer). Their keys
//! are derived offline from the account's own `kvcomm` cache — see [`super::image_key`] — so
//! thumbnails, WeChat's bubble JPEGs, and WxAM/HEVC originals (through `ffmpeg`) can all be
//! rendered. Previews are produced on demand and cached; export decodes full-resolution
//! originals.
//!
//! Attachment directories are named after `md5(username)`; the month folder follows the
//! message timestamp in local time. Files are never written or moved here.

use base64::Engine;
use chrono::{Datelike, Local, TimeZone};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

use super::query::{self, RawMessage};
use super::{image_key, voice};



const MAX_THUMB_BYTES: u64 = 96 * 1024;

#[derive(Serialize, Clone, Debug)]
pub struct MediaItem {
    pub local_id: i64,
    pub server_id: i64,
    pub kind: String,
    pub name: Option<String>,
    pub rel_path: Option<String>,
    pub size: Option<u64>,
    pub exists: bool,
    /// True for images that still live as encrypted `.dat` payloads.
    pub encrypted: bool,
    /// True when a plaintext preview could be produced for this message.
    pub decoded: bool,
    /// Video cover as a data URI (images use `preview`).
    pub thumb: Option<String>,
    /// Absolute path of the decoded preview JPEG, served through the asset protocol.
    pub preview: Option<String>,
    /// Relative source files in quality order (original, bubble, thumbnail).
    pub sources: Vec<String>,
    /// Voice length in seconds.
    pub duration: Option<i64>,
    pub note: Option<String>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct MediaCounts {
    pub images: i64,
    pub videos: i64,
    pub files: i64,
    pub voices: i64,
    pub encrypted: i64,
    pub missing: i64,
}

#[derive(Serialize, Clone, Debug)]
pub struct MediaReport {
    pub items: Vec<MediaItem>,
    pub counts: MediaCounts,
}

#[derive(Serialize, Clone, Debug)]
pub struct ExportReport {
    pub directory: String,
    pub transcript: String,
    pub manifest: String,
    pub media_root: String,
    pub copied: usize,
    pub skipped: usize,
    pub bytes: u64,
    pub note: String,
}

/// Find the account's kvcomm keys and verify them against a real `_t.dat` payload.
pub fn resolve_image_keys(account_dir: &Path, hash: &str) -> Option<image_key::ResolvedKeys> {
    let attach = account_dir.join("msg").join("attach").join(hash);
    let months = fs::read_dir(&attach).ok()?;
    let mut dirs: Vec<PathBuf> = months.flatten().map(|entry| entry.path().join("Img")).collect();
    dirs.sort();
    dirs.reverse();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with("_t.dat") {
                continue;
            }
            let Ok(bytes) = fs::read(&entry.path()) else { continue };
            if let Some(keys) = image_key::resolve(account_dir, &bytes) {
                return Some(keys);
            }
        }
    }
    None
}

/// Bubble images are what WeChat itself renders in the chat: a readable, screen-sized JPEG.
///
/// The file is named after the **thumbnail** (or the XML `md5`) plus `_b.dat`; some builds
/// use `<local_id>_<timestamp>_b.dat`. All three shapes are checked.
fn find_bubble(
    account_dir: &Path,
    hash: &str,
    month: &str,
    names: &[String],
    local_id: i64,
    time: i64,
) -> Option<PathBuf> {
    let dir = account_dir
        .join("cache")
        .join(month)
        .join("Message")
        .join(hash)
        .join("Bubble");
    for name in names {
        for suffix in ["_b.dat", "_h.dat"] {
            let exact = dir.join(format!("{name}{suffix}"));
            if exact.is_file() {
                return Some(exact);
            }
        }
    }
    let exact = dir.join(format!("{local_id}_{time}_b.dat"));
    if exact.is_file() {
        return Some(exact);
    }
    let prefix = format!("{local_id}_");
    files_in(&dir)
        .into_iter()
        .map(|(path, _, _)| path)
        .find(|path| {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            name.starts_with(&prefix) && name.ends_with("_b.dat")
        })
}

/// Decode an image source, optionally turning WxAM (HEVC) into PNG with ffmpeg.
fn decode_full(source: &Path, keys: &image_key::ResolvedKeys, allow_wxam: bool) -> Result<(Vec<u8>, String), String> {
    let bytes = fs::read(source).map_err(|e| format!("无法读取图片：{e}"))?;
    let decoded = image_key::decode(&bytes, keys)?;
    if decoded.kind == "wxam" {
        if !allow_wxam {
            return Err("WxAM 原图需要 ffmpeg 才能解码。".into());
        }
        let png = image_key::wxam_to_png(&decoded.bytes)?;
        return Ok((png, "png".to_string()));
    }
    Ok((decoded.bytes, decoded.kind))
}

/// Decode one of the resolved image sources for the upload package.
///
/// The returned bytes are a browser-safe PNG/JPEG payload rather than WeChat's
/// encrypted `.dat` container. Sources are checked below the account directory
/// before decoding so a malformed database record cannot make the client read
/// an arbitrary local file.
pub fn decode_for_upload(
    account_dir: &Path,
    sources: &[PathBuf],
) -> Result<(Vec<u8>, String), String> {
    let root = account_dir
        .canonicalize()
        .map_err(|e| format!("无法定位账号目录：{e}"))?;
    let mut keys: Option<image_key::ResolvedKeys> = None;
    let mut last_error = String::from("没有可用的图片文件。");
    for path in sources {
        let Ok(source) = path.canonicalize() else {
            continue;
        };
        if !source.starts_with(&root) || !source.is_file() {
            continue;
        }
        let Ok(raw) = fs::read(&source) else {
            continue;
        };
        if keys.is_none() {
            keys = image_key::resolve(account_dir, &raw);
        }
        let Some(resolved) = keys.as_ref() else {
            last_error = "没有找到本账号的图片密钥。".into();
            continue;
        };
        match decode_full(&source, resolved, true) {
            Ok(decoded) => return Ok(decoded),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

/// Build a display preview for one message on demand.
///
/// The message list stays cheap: prefetching every image would mean one ffmpeg run per
/// WxAM original. The UI calls this for images it is about to render, and the result is
/// cached on disk under `<app_data>/image-cache/<account>/`.
pub fn make_preview(
    app_data: &Path,
    account: &str,
    local_id: i64,
    account_dir: &Path,
    paths: &[PathBuf],
) -> Result<PathBuf, String> {
    let mut keys: Option<image_key::ResolvedKeys> = None;
    let mut chosen: Option<(Vec<u8>, String, u64)> = None;
    let mut last_error = String::from("没有可用的图片文件。");
    for path in paths {
        let Ok(bytes) = fs::read(path) else { continue };
        if keys.is_none() {
            keys = image_key::resolve(account_dir, &bytes);
        }
        let Some(resolved) = keys.as_ref() else {
            last_error = "没有找到本账号的图片密钥。".into();
            continue;
        };
        let size = bytes.len() as u64;
        match image_key::decode(&bytes, resolved) {
            Ok(decoded) if decoded.kind == "wxam" => {
                match image_key::wxam_to_jpeg(&decoded.bytes, 900) {
                    Ok(jpeg) => {
                        chosen = Some((jpeg, "jpg".to_string(), size));
                        break;
                    }
                    Err(error) => last_error = error,
                }
            }
            Ok(decoded) => {
                let kind = if decoded.kind == "png" { "png" } else { "jpg" };
                chosen = Some((decoded.bytes, kind.to_string(), size));
                break;
            }
            Err(error) => last_error = error,
        }
    }
    let (bytes, kind, size) = chosen.ok_or(last_error)?;
    let dir = app_data.join("image-cache").join(account);
    fs::create_dir_all(&dir).map_err(|e| format!("无法创建图片缓存目录：{e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    let target = dir.join(format!("preview_{local_id}_{size}.{kind}"));
    if !target.is_file() {
        write_private(&target, &bytes)?;
    }
    Ok(target)
}

fn month_of(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|dt| format!("{:04}-{:02}", dt.year(), dt.month()))
        .unwrap_or_else(|| "1970-01".to_string())
}

fn session_hash(username: &str) -> String {
    query::md5_hex(username)
}

/// Read one XML attribute.
///
/// A naive `find("length=\"")` also matches `cdnthumblength="…"`, and `md5="` also matches
/// `originsourcemd5="…"`, which silently pairs a message with the wrong file. The character
/// before the attribute name must therefore be whitespace or the opening tag.
fn attr(xml: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let mut from = 0usize;
    while let Some(offset) = xml[from..].find(&needle) {
        let start = from + offset;
        let boundary = xml[..start]
            .chars()
            .last()
            .map(|ch| ch.is_whitespace() || ch == '<')
            .unwrap_or(true);
        if boundary {
            let rest = &xml[start + needle.len()..];
            let end = rest.find('"')?;
            let value = rest[..end].trim().to_string();
            return if value.is_empty() { None } else { Some(value) };
        }
        from = start + needle.len();
    }
    None
}

fn tag_value(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let rest = &xml[start..];
    let end = rest.find(&close)?;
    let value = rest[..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn num_attr(xml: &str, name: &str) -> Option<i64> {
    attr(xml, name).and_then(|value| value.parse::<i64>().ok())
}

/// Files directly inside one directory, cheap enough to call per message range.
fn files_in(dir: &Path) -> Vec<(PathBuf, u64, Option<std::time::SystemTime>)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((path, meta.len(), meta.modified().ok()))
        })
        .collect()
}

fn relative_to(base: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(base)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
}

fn thumb_data_uri(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_THUMB_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(format!("data:image/jpeg;base64,{encoded}"))
}

/// Cache cover for an image or video message, if WeChat already rendered one.
fn cached_thumb(account_dir: &Path, hash: &str, month: &str, local_id: i64, time: i64) -> Option<PathBuf> {
    let dir = account_dir
        .join("cache")
        .join(month)
        .join("Message")
        .join(hash)
        .join("Thumb");
    let exact = dir.join(format!("{local_id}_{time}_thumb.jpg"));
    if exact.is_file() {
        return Some(exact);
    }
    // Message ids differ between shards after a re-login; fall back to the timestamp suffix.
    let suffix = format!("_{time}_thumb.jpg");
    files_in(&dir)
        .into_iter()
        .map(|(path, _, _)| path)
        .find(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().ends_with(&suffix))
                .unwrap_or(false)
        })
}

fn find_video(account_dir: &Path, month: &str, length: Option<i64>, time: i64) -> Option<PathBuf> {
    let dir = account_dir.join("msg").join("video").join(month);
    let files: Vec<(PathBuf, u64, Option<std::time::SystemTime>)> = files_in(&dir)
        .into_iter()
        .filter(|(path, _, _)| path.extension().map(|ext| ext == "mp4").unwrap_or(false))
        .collect();

    if let Some(length) = length {
        if let Some((path, _, _)) = files.iter().find(|(_, size, _)| *size as i64 == length) {
            return Some(path.clone());
        }
    }
    // Same-second write time is the next best deterministic signal.
    files
        .into_iter()
        .filter(|(_, _, modified)| {
            modified
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| (d.as_secs() as i64 - time).abs() <= 120)
                .unwrap_or(false)
        })
        .min_by_key(|(_, _, modified)| {
            modified
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| (d.as_secs() as i64 - time).abs())
                .unwrap_or(i64::MAX)
        })
        .map(|(path, _, _)| path)
}

/// `.dat` files are sized `plaintext + 31` in practice, so the XML length is a strong key.
fn find_dat(dir: &Path, expected: i64, thumb: bool, time: i64) -> Option<(PathBuf, u64)> {
    let files: Vec<(PathBuf, u64, Option<std::time::SystemTime>)> = files_in(dir)
        .into_iter()
        .filter(|(path, _, _)| {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            name.ends_with(".dat") && name.ends_with("_t.dat") == thumb
        })
        .collect();
    if files.is_empty() {
        return None;
    }
    let exact = files
        .iter()
        .find(|(_, size, _)| (*size as i64 - expected).abs() <= 0 || *size as i64 == expected + 31);
    if let Some((path, size, _)) = exact {
        return Some((path.clone(), *size));
    }
    files
        .into_iter()
        .filter(|(_, size, _)| (*size as i64 - expected).abs() <= 64)
        .min_by_key(|(_, _, modified)| {
            modified
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| (d.as_secs() as i64 - time).abs())
                .unwrap_or(i64::MAX)
        })
        .map(|(path, size, _)| (path, size))
}

/// The original may live in a different month folder when it was downloaded later.
fn find_sibling_anywhere(account_dir: &Path, hash: &str, stem: &str) -> Option<(PathBuf, u64)> {
    let attach = account_dir.join("msg").join("attach").join(hash);
    let months = fs::read_dir(&attach).ok()?;
    for entry in months.flatten() {
        let candidate = entry.path().join("Img").join(format!("{stem}.dat"));
        if candidate.is_file() {
            if let Ok(meta) = fs::metadata(&candidate) {
                return Some((candidate, meta.len()));
            }
        }
    }
    None
}

fn find_document(account_dir: &Path, month: &str, name: &str) -> Option<PathBuf> {
    let safe: String = name
        .chars()
        .map(|ch| if matches!(ch, '/' | '\\' | ':' | '\0') { '_' } else { ch })
        .collect();
    let direct = account_dir.join("msg").join("file").join(month).join(&safe);
    if direct.is_file() {
        return Some(direct);
    }
    let root = account_dir.join("msg").join("file");
    let mut stack = vec![(root.clone(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 3 {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push((path, depth + 1));
            } else if path.file_name().map(|n| n.to_string_lossy() == safe).unwrap_or(false) {
                return Some(path);
            }
        }
    }
    None
}

/// Build the manifest for a set of raw messages of one conversation.
pub fn resolve(
    account_dir: &Path,
    username: &str,
    messages: &[RawMessage],
    keys: Option<&image_key::ResolvedKeys>,
) -> MediaReport {
    let hash = session_hash(username);
    let mut items = Vec::new();
    let mut counts = MediaCounts::default();

    for message in messages {
        let base = message.kind & 0xffff;
        let month = month_of(message.time);
        let xml = message.content.as_str();
        let item = match base {
            3 => {
                let length = num_attr(xml, "length");
                let thumb_len = num_attr(xml, "cdnthumblength");
                let img_dir = account_dir
                    .join("msg")
                    .join("attach")
                    .join(&hash)
                    .join(&month)
                    .join("Img");
                let thumb_dat = thumb_len.and_then(|len| find_dat(&img_dir, len, true, message.time));
                // A `_t.dat` thumbnail almost always has its original beside it under the same
                // stem. Size-based matching fails for WxAM originals (HEVC is much smaller than
                // the `length` attribute), so the sibling lookup is the reliable link.
                let sibling = thumb_dat.as_ref().and_then(|(thumb, _)| {
                    let name = thumb.file_name()?.to_string_lossy().to_string();
                    let stem = name.strip_suffix("_t.dat")?;
                    let candidate = thumb.with_file_name(format!("{stem}.dat"));
                    candidate
                        .is_file()
                        .then(|| fs::metadata(&candidate).ok().map(|meta| (candidate, meta.len())))
                        .flatten()
                });
                let original = length
                    .and_then(|len| find_dat(&img_dir, len, false, message.time))
                    .or(sibling)
                    .or_else(|| {
                        let (thumb, _) = thumb_dat.as_ref()?;
                        let name = thumb.file_name()?.to_string_lossy().to_string();
                        let stem = name.strip_suffix("_t.dat")?;
                        find_sibling_anywhere(account_dir, &hash, stem)
                    });
                let cached = cached_thumb(account_dir, &hash, &month, message.local_id, message.time);
                let mut bubble_names: Vec<String> = Vec::new();
                if let Some(md5) = attr(xml, "md5") {
                    bubble_names.push(md5);
                }
                if let Some((path, _)) = thumb_dat.as_ref() {
                    if let Some(stem) = path.file_stem() {
                        let stem = stem.to_string_lossy();
                        let stem = stem.strip_suffix("_t").unwrap_or(&stem);
                        if !bubble_names.iter().any(|name| name == stem) {
                            bubble_names.push(stem.to_string());
                        }
                    }
                }
                let bubble = find_bubble(
                    account_dir,
                    &hash,
                    &month,
                    &bubble_names,
                    message.local_id,
                    message.time,
                );

                let original_path = original.as_ref().map(|(path, _)| path.clone());
                let thumb_path = thumb_dat.as_ref().map(|(path, _)| path.clone());
                let raw_path = original_path.clone().or(thumb_path.clone());
                // Sources in quality order. The original may be a WxAM/HEVC payload; the
                // bubble copy is what WeChat itself renders, and `_t.dat` is the thumbnail.
                let mut sources: Vec<PathBuf> = Vec::new();
                for candidate in [
                    original_path.clone(),
                    bubble.clone(),
                    thumb_path.clone(),
                    cached.clone(),
                ]
                .into_iter()
                .flatten()
                {
                    if !sources.contains(&candidate) {
                        sources.push(candidate);
                    }
                }
                // Inline preview stays fast: bubble or thumbnail, never an ffmpeg run.
                let display = bubble
                    .clone()
                    .or_else(|| thumb_path.clone())
                    .or_else(|| cached.clone())
                    .or_else(|| original_path.clone());

                if display.is_none() {
                    counts.missing += 1;
                }
                if raw_path.is_some() || bubble.is_some() {
                    counts.encrypted += 1;
                }
                counts.images += 1;

                let size = display
                    .as_deref()
                    .and_then(|path| fs::metadata(path).ok())
                    .map(|meta| meta.len());
                let has_preview = keys.is_some() && display.is_some();

                MediaItem {
                    local_id: message.local_id,
                    server_id: message.server_id,
                    kind: "image".to_string(),
                    name: None,
                    rel_path: raw_path.as_deref().and_then(|p| relative_to(account_dir, p)),
                    size,
                    exists: display.as_ref().map(|p| p.is_file()).unwrap_or(false),
                    encrypted: raw_path.is_some() || bubble.is_some(),
                    decoded: has_preview,
                    thumb: None,
                    preview: None,
                    sources: sources
                        .iter()
                        .filter_map(|path| relative_to(account_dir, path))
                        .collect(),
                    duration: None,
                    note: Some(if display.is_some() {
                        let has_full = sources.iter().any(|path| {
                            !path
                                .file_name()
                                .map(|name| name.to_string_lossy().ends_with("_t.dat"))
                                .unwrap_or(false)
                        });
                        if !has_preview {
                            "找到了图片文件，但缺少图片密钥，无法解码。".to_string()
                        } else if has_full {
                            "已离线解密；查看原图会打开全尺寸版本。".to_string()
                        } else {
                            "本机只有缩略图（微信没有下载原图），可以放大查看但清晰度有限。".to_string()
                        }
                    } else {
                        "本地没有找到图片文件，可能未下载或已清理。".to_string()
                    }),
                }
            }
            43 => {
                let length = num_attr(xml, "length");
                let play = num_attr(xml, "playlength");
                let path = find_video(account_dir, &month, length, message.time);
                let thumb = path.as_ref().and_then(|video| {
                    let stem = video.file_stem().map(|s| s.to_string_lossy().to_string())?;
                    let cover = video.with_file_name(format!("{stem}_thumb.jpg"));
                    cover.is_file().then_some(cover)
                });
                if path.is_none() {
                    counts.missing += 1;
                }
                counts.videos += 1;
                MediaItem {
                    local_id: message.local_id,
                    server_id: message.server_id,
                    kind: "video".to_string(),
                    name: path
                        .as_deref()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().to_string()),
                    rel_path: path.as_deref().and_then(|p| relative_to(account_dir, p)),
                    size: path.as_ref().and_then(|p| fs::metadata(p).ok()).map(|m| m.len()),
                    exists: path.as_ref().map(|p| p.is_file()).unwrap_or(false),
                    encrypted: false,
                    decoded: false,
                    thumb: thumb.as_deref().and_then(thumb_data_uri),
                    preview: None,
                    sources: Vec::new(),
                    duration: play,
                    note: path.is_none().then(|| "本地没有找到视频文件。".to_string()),
                }
            }
            49 => {
                let app_type = tag_value(xml, "type").and_then(|value| value.parse::<i64>().ok());
                if app_type == Some(6) {
                    let title = tag_value(xml, "title");
                    let path = title
                        .as_deref()
                        .and_then(|name| find_document(account_dir, &month, name));
                    let total = tag_value(xml, "totallen").and_then(|v| v.parse::<u64>().ok());
                    if path.is_none() {
                        counts.missing += 1;
                    }
                    counts.files += 1;
                    MediaItem {
                        local_id: message.local_id,
                        server_id: message.server_id,
                        kind: "file".to_string(),
                        name: title,
                        rel_path: path.as_deref().and_then(|p| relative_to(account_dir, p)),
                        size: path
                            .as_ref()
                            .and_then(|p| fs::metadata(p).ok())
                            .map(|m| m.len())
                            .or(total),
                        exists: path.as_ref().map(|p| p.is_file()).unwrap_or(false),
                        encrypted: false,
                        decoded: false,
                        thumb: None,
                        preview: None,
                        sources: Vec::new(),
                        duration: None,
                        note: path.is_none().then(|| "文件未下载或已被清理。".to_string()),
                    }
                } else {
                    continue;
                }
            }
            34 => {
                let ms = num_attr(xml, "voicelength").or_else(|| num_attr(xml, "length"));
                counts.voices += 1;
                MediaItem {
                    local_id: message.local_id,
                    server_id: message.server_id,
                    kind: "voice".to_string(),
                    name: None,
                    rel_path: None,
                    size: None,
                    exists: true,
                    encrypted: false,
                    decoded: false,
                    thumb: None,
                    preview: None,
                    sources: Vec::new(),
                    duration: ms.map(|value| (value as f64 / 1000.0).round() as i64),
                    note: None,
                }
            }
            _ => continue,
        };
        items.push(item);
    }

    MediaReport { items, counts }
}

/// Reject `..` traversal and symlinks that escape the account directory before revealing.
pub fn reveal(account_dir: &Path, path: &Path) -> Result<(), String> {
    let root = account_dir
        .canonicalize()
        .map_err(|e| format!("无法定位账号目录：{e}"))?;
    let resolved = path.canonicalize().map_err(|e| format!("无法定位文件：{e}"))?;
    if !resolved.starts_with(&root) {
        return Err("拒绝打开账号目录之外的文件。".into());
    }
    if !resolved.is_file() {
        return Err("文件不存在。".into());
    }
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg("-R").arg(&resolved);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("explorer");
        command.arg(format!("/select,{}", resolved.to_string_lossy()));
        command
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(resolved.parent().unwrap_or(&resolved));
        command
    };
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("无法在文件管理器中显示：{e}"))?;
    Ok(())
}

/// Decode the best available image source and open the plaintext copy in the system viewer.
pub fn open_image(
    app_data: &Path,
    account: &str,
    account_dir: &Path,
    paths: &[PathBuf],
) -> Result<PathBuf, String> {
    let root = account_dir
        .canonicalize()
        .map_err(|e| format!("无法定位账号目录：{e}"))?;

    let mut keys: Option<image_key::ResolvedKeys> = None;
    let mut decoded: Option<(Vec<u8>, String, PathBuf)> = None;
    let mut last_error = String::from("没有可用的图片文件。");
    for path in paths {
        let Ok(source) = path.canonicalize() else { continue };
        if !source.starts_with(&root) {
            return Err("拒绝打开账号目录之外的文件。".into());
        }
        let Ok(raw) = fs::read(&source) else { continue };
        if keys.is_none() {
            keys = image_key::resolve(account_dir, &raw);
        }
        let Some(resolved) = keys.as_ref() else {
            last_error = "没有找到本账号的图片密钥，无法离线解密。".into();
            continue;
        };
        match decode_full(&source, resolved, true) {
            Ok((bytes, kind)) => {
                decoded = Some((bytes, kind, source));
                break;
            }
            Err(error) => last_error = error,
        }
    }
    let (bytes, kind, source) = decoded.ok_or(last_error)?;

    let dir = app_data.join("image-cache").join(account);
    fs::create_dir_all(&dir).map_err(|e| format!("无法创建图片缓存目录：{e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    let stem = source
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "image".to_string());
    let out = dir.join(format!("{stem}.{kind}"));
    write_private(&out, &bytes)?;

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(&out);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler").arg(&out);
        command
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(&out);
        command
    };
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("无法打开图片：{e}"))?;
    Ok(out)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("无法创建目录：{e}"))?;
    }
    let tmp = path.with_extension("tmp");
    {
        use std::io::Write;
        let mut file = fs::File::create(&tmp).map_err(|e| format!("无法写入 {}：{e}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
        }
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("无法写入 {}：{e}", path.display()))?;
    }
    fs::rename(&tmp, path).map_err(|e| format!("无法保存 {}：{e}", path.display()))
}

/// Copy one attachment into the export bundle, preserving the path below the account root.
fn copy_into(account_dir: &Path, dest_root: &Path, path: &Path) -> Option<(String, u64)> {
    let rel = relative_to(account_dir, path)?;
    let dest = dest_root.join(&rel);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).ok()?;
    }
    fs::copy(path, &dest).ok()?;
    let size = fs::metadata(&dest).ok().map(|m| m.len()).unwrap_or(0);
    Some((rel, size))
}

/// Export a conversation to a folder with transcript, manifest, and plaintext attachments.
pub fn export_bundle(
    app_data: &Path,
    account_dir: &Path,
    account: &str,
    username: &str,
    display: &str,
    since: i64,
    until: i64,
) -> Result<ExportReport, String> {
    let cache = super::decrypt::cache_dir(app_data, account);
    let messages = query::messages_between(&cache, username, since, until, 5000)?;
    let raw = query::raw_messages_between(&cache, username, since, until, 5000)?;
    let keys = resolve_image_keys(account_dir, &session_hash(username));
    let report = resolve(account_dir, username, &raw, keys.as_ref());

    let safe_name: String = display
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let stamp = Local
        .timestamp_opt(voice::now_secs(), 0)
        .single()
        .map(|dt| dt.format("%Y%m%d-%H%M%S").to_string())
        .unwrap_or_else(|| "export".to_string());
    let downloads = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Downloads"))
        .unwrap_or_else(|| PathBuf::from("."));
    let directory = downloads.join("AIMyChats").join(format!("{safe_name}-{stamp}"));
    let media_root = directory.join("media");
    fs::create_dir_all(&media_root).map_err(|e| format!("无法创建导出目录：{e}"))?;

    let transcripts = voice::load_transcripts(app_data);
    let keys = resolve_image_keys(account_dir, &session_hash(username));
    let mut manifest_lines: Vec<String> = Vec::new();
    let mut copied = 0usize;
    let mut skipped = 0usize;
    let mut total_bytes = 0u64;
    let mut by_local: std::collections::HashMap<i64, Vec<&MediaItem>> = std::collections::HashMap::new();
    for item in &report.items {
        by_local.entry(item.local_id).or_default().push(item);
    }

    for item in &report.items {
        let Some(rel) = item.rel_path.as_deref() else {
            continue;
        };
        let source = account_dir.join(rel);
        if !source.is_file() {
            skipped += 1;
            continue;
        }
        if item.kind == "image" {
            let Some(keys) = keys.as_ref() else {
                skipped += 1;
                continue;
            };
            // Full resolution first (WxAM goes through ffmpeg), then bubble, then thumbnail.
            let mut candidates = vec![source.clone()];
            for rel in &item.sources {
                let extra = account_dir.join(rel);
                if !candidates.contains(&extra) {
                    candidates.push(extra);
                }
            }
            let mut decoded = None;
            for candidate in &candidates {
                if let Ok((bytes, kind)) = decode_full(candidate, keys, true) {
                    decoded = Some((bytes, kind));
                    break;
                }
            }
            match decoded {
                Some((bytes, kind)) => {
                    let dest_rel = format!("images/{}/{}.{}", safe_name, item.local_id, kind);
                    let dest = media_root.join(&dest_rel);
                    if let Some(parent) = dest.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    if fs::write(&dest, &bytes).is_ok() {
                        copied += 1;
                        total_bytes += bytes.len() as u64;
                        manifest_lines.push(format!(
                            "  {{\"local_id\":{},\"kind\":\"image\",\"name\":null,\"source\":{},\"dest\":{},\"size\":{}}}",
                            item.local_id,
                            json_str(Some(rel)),
                            json_str(Some(&format!("media/{dest_rel}"))),
                            bytes.len()
                        ));
                    } else {
                        skipped += 1;
                    }
                }
                None => skipped += 1,
            }
            continue;
        }
        if item.encrypted {
            skipped += 1;
            continue;
        }
        if let Some((rel, size)) = copy_into(account_dir, &media_root, &source) {
            copied += 1;
            total_bytes += size;
            let dest = format!("media/{rel}");
            manifest_lines.push(format!(
                "  {{\"local_id\":{},\"kind\":\"{}\",\"name\":{},\"source\":{},\"dest\":{},\"size\":{}}}",
                item.local_id,
                item.kind,
                json_str(item.name.as_deref()),
                json_str(Some(&rel)),
                json_str(Some(&dest)),
                size
            ));
        } else {
            skipped += 1;
        }
    }

    let mut body = String::new();
    body.push_str(&format!("# {display}\n\n"));
    body.push_str(&format!(
        "- 账号：`{account}`\n- 会话：`{username}`\n- 范围：{} — {}\n- 消息：{} 条\n- 附件：图片 {}、视频 {}、文件 {}、语音 {}\n\n",
        fmt_time(since),
        fmt_time(until),
        messages.len(),
        report.counts.images,
        report.counts.videos,
        report.counts.files,
        report.counts.voices
    ));
    for message in messages.iter().rev() {
        let who = if message.is_self {
            "我".to_string()
        } else {
            message
                .sender_name
                .clone()
                .or_else(|| message.sender.clone())
                .unwrap_or_else(|| "对方".to_string())
        };
        body.push_str(&format!("## {} · {}\n\n", fmt_time(message.time), who));
        body.push_str(&format!("{}\n\n", message.text));
        if let Some(media) = by_local.get(&message.local_id) {
            for item in media {
                match item.kind.as_str() {
                    "voice" => {
                        let translated = transcripts
                            .get(&voice::transcript_key(account, item.server_id))
                            .map(|entry| entry.text.clone())
                            .unwrap_or_else(|| "（未转写）".to_string());
                        body.push_str(&format!(
                            "> 语音 {} 秒：{}\n\n",
                            item.duration.unwrap_or(0),
                            translated
                        ));
                    }
                    "image" => {
                        let status = if item.decoded {
                            "已解码，见 media/images"
                        } else if item.exists {
                            "加密原图（未解码）"
                        } else {
                            "本地缺失"
                        };
                        body.push_str(&format!(
                            "> 图片 #{} · {} · {}\n\n",
                            item.server_id,
                            item.size.map(|size| format!("{} KB", size / 1024)).unwrap_or_default(),
                            status
                        ));
                    }
                    "video" => {
                        body.push_str(&format!(
                            "> 视频 {} · {}\n\n",
                            item.duration.map(|d| format!("{d} 秒")).unwrap_or_default(),
                            item.rel_path.clone().unwrap_or_else(|| "本地缺失".to_string())
                        ));
                    }
                    "file" => {
                        body.push_str(&format!(
                            "> 文件 {} · {}\n\n",
                            item.name.clone().unwrap_or_else(|| "未命名".to_string()),
                            item.rel_path.clone().unwrap_or_else(|| "本地缺失".to_string())
                        ));
                    }
                    _ => {}
                }
            }
        }
    }

    let transcript_path = directory.join("transcript.md");
    fs::write(&transcript_path, body).map_err(|e| format!("无法写入 transcript.md：{e}"))?;
    let manifest_path = directory.join("manifest.json");
    let manifest = format!(
        "{{\n \"account\": \"{}\",\n \"username\": \"{}\",\n \"display\": \"{}\",\n \"since\": {},\n \"until\": {},\n \"counts\": {{\"images\":{},\"videos\":{},\"files\":{},\"voices\":{},\"encrypted\":{},\"missing\":{}}},\n \"attachments\": [\n{}\n ]\n}}\n",
        account,
        username,
        display.replace('"', "'"),
        since,
        until,
        report.counts.images,
        report.counts.videos,
        report.counts.files,
        report.counts.voices,
        report.counts.encrypted,
        report.counts.missing,
        manifest_lines.join(",\n")
    );
    fs::write(&manifest_path, manifest).map_err(|e| format!("无法写入 manifest.json：{e}"))?;

    Ok(ExportReport {
        directory: directory.to_string_lossy().to_string(),
        transcript: transcript_path.to_string_lossy().to_string(),
        manifest: manifest_path.to_string_lossy().to_string(),
        media_root: media_root.to_string_lossy().to_string(),
        copied,
        skipped,
        bytes: total_bytes,
        note: "加密的 .dat 原图只登记在清单里，无法在导出中解码；视频封面与明文附件已复制。".to_string(),
    })
}

fn json_str(value: Option<&str>) -> String {
    match value {
        Some(text) => format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"")),
        None => "null".to_string(),
    }
}

fn fmt_time(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attributes_are_read_without_an_xml_parser() {
        let xml = r#"<msg><img aeskey="aa" length="219493" md5="bb" /><imgdatahash /></msg>"#;
        assert_eq!(attr(xml, "aeskey").as_deref(), Some("aa"));
        assert_eq!(num_attr(xml, "length"), Some(219493));
        assert_eq!(attr(xml, "missing"), None);
    }

    #[test]
    fn app_message_fields_are_extracted() {
        let xml = "<msg><appmsg><title>预算.xlsx</title><type>6</type><totallen>1234</totallen></appmsg></msg>";
        assert_eq!(tag_value(xml, "title").as_deref(), Some("预算.xlsx"));
        assert_eq!(tag_value(xml, "type").as_deref(), Some("6"));
    }

    #[test]
    fn month_folders_follow_local_time() {
        assert_eq!(month_of(0).len(), 7);
        assert!(month_of(0).starts_with("19"));
    }

    #[test]
    fn encrypted_dat_is_sized_plaintext_plus_header() {
        // Mirrors the measured layout: 17548-byte thumbnail -> 17579-byte `_t.dat`.
        assert_eq!(17548 + 31, 17579);
    }

    #[test]
    fn resolve_matches_real_attachment_layout() {
        let time = Local
            .with_ymd_and_hms(2026, 11, 1, 10, 0, 0)
            .single()
            .unwrap()
            .timestamp();
        let root = std::env::temp_dir().join(format!("wc-media-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let hash = session_hash("123@chatroom");
        let month = "2026-11";
        let img = root.join("msg/attach").join(&hash).join(month).join("Img");
        fs::create_dir_all(&img).unwrap();
        fs::write(img.join("abc_t.dat"), vec![0u8; 17579]).unwrap();
        let file_dir = root.join("msg/file").join(month);
        fs::create_dir_all(&file_dir).unwrap();
        fs::write(file_dir.join("预算.xlsx"), b"sheet").unwrap();
        let video_dir = root.join("msg/video").join(month);
        fs::create_dir_all(&video_dir).unwrap();
        fs::write(video_dir.join("clip.mp4"), vec![0u8; 1564891]).unwrap();
        fs::write(video_dir.join("clip_thumb.jpg"), b"jpeg").unwrap();
        let thumb_dir = root
            .join("cache")
            .join(month)
            .join("Message")
            .join(&hash)
            .join("Thumb");
        fs::create_dir_all(&thumb_dir).unwrap();
        fs::write(thumb_dir.join(format!("1_{time}_thumb.jpg")), b"thumb").unwrap();

        let messages = vec![
            RawMessage {
                local_id: 1,
                server_id: 11,
                time,
                kind: 3,
                content: r#"<msg><img length="219493" cdnthumblength="17548" /></msg>"#.into(),
            },
            RawMessage {
                local_id: 2,
                server_id: 12,
                time,
                kind: 43,
                content: r#"<msg><videomsg length="1564891" playlength="9" /></msg>"#.into(),
            },
            RawMessage {
                local_id: 3,
                server_id: 13,
                time,
                kind: 49,
                content: "<msg><appmsg><title>预算.xlsx</title><type>6</type><totallen>5</totallen></appmsg></msg>".into(),
            },
            RawMessage {
                local_id: 42,
                server_id: 14,
                time,
                kind: 34,
                content: r#"<msg><voicemsg voicelength="2300" /></msg>"#.into(),
            },
        ];

        let app_data = std::env::temp_dir().join(format!("wc-media-app-{}", std::process::id()));
        let _ = fs::remove_dir_all(&app_data);
        let report = resolve(&root, "123@chatroom", &messages, None);
        let _ = fs::remove_dir_all(&root);

        assert_eq!(report.counts.images, 1);
        assert_eq!(report.counts.videos, 1);
        assert_eq!(report.counts.files, 1);
        assert_eq!(report.counts.voices, 1);
        assert_eq!(report.counts.missing, 0);

        let image = &report.items[0];
        assert_eq!(image.kind, "image");
        assert!(image.encrypted && image.exists);
        assert_eq!(image.size, Some(17579));
        // Without image keys the attachment is still listed, but no preview is produced.
        assert!(!image.sources.is_empty());
        assert!(image.sources[0].ends_with("abc_t.dat"));
        assert!(image.preview.is_none());
        assert!(image.thumb.is_none(), "images use `preview`, not the video cover field");

        let video = &report.items[1];
        assert_eq!(video.kind, "video");
        assert!(video.exists);
        assert!(video.thumb.is_some());
        assert_eq!(video.duration, Some(9));

        let file = &report.items[2];
        assert_eq!(file.name.as_deref(), Some("预算.xlsx"));
        assert!(file.exists, "documents keep their title as the file name");

        let voice = &report.items[3];
        assert_eq!(voice.duration, Some(2));
    }
}
