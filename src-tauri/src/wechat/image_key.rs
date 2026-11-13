//! Offline decoding of WeChat 4.x `V2` image payloads (`msg/attach/**/Img/*.dat`).
//!
//! Layout (verified against this machine's 4.x account):
//!
//! ```text
//! [6B magic 07 08 "V2" 08 07][u32le aes_size][u32le xor_size][1B pad]
//! [AES-128-ECB ciphertext, PKCS#7 padded][plaintext middle][single-byte-XOR trailer]
//! ```
//!
//! Both keys are *derived*, not scraped from the running client:
//!
//! * `code`     — the number in `<config>/…/kvcomm/key_<code>_*.statistic`
//! * `xor_key`  — `code & 0xFF`
//! * `aes_key`  — `md5("{code}{wxid}").hexdigest()[:16]` used as 16 ASCII bytes
//!
//! Several codes can exist on one machine (they change on some updates), so every candidate
//! is verified against a real ciphertext block before use: decrypting the first block must
//! yield a known image signature. A wrong key effectively never matches three leading bytes.
//!
//! Legacy single-byte-XOR files and the older `V1` magic are handled too. `WxAM` payloads
//! (`wxgf` magic) are HEVC-compressed and are reported as unsupported instead of written out
//! as a broken image.

use aes::cipher::{BlockDecrypt, KeyInit};
use aes::Aes128;
use md5::{Digest, Md5};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

pub const V2_MAGIC: &[u8; 6] = b"\x07\x08V2\x08\x07";
pub const V1_MAGIC: &[u8; 6] = b"\x07\x08V1\x08\x07";
const HEADER: usize = 15;
const BLOCK: usize = 16;

/// Upper bound for a derived key scan: kvcomm trees are shallow in practice.
const MAX_DEPTH: usize = 6;

#[derive(Serialize, Clone, Debug)]
pub struct ImageKeys {
    pub code: u64,
    pub xor_key: u8,
    pub wxid: String,
    pub source: String,
}

#[derive(Clone, Debug)]
pub struct ResolvedKeys {
    pub keys: ImageKeys,
    aes: [u8; BLOCK],
}

pub fn derive(code: u64, wxid: &str) -> ResolvedKeys {
    let digest = md5_hex(&format!("{code}{wxid}"));
    let ascii = digest[..16].as_bytes().to_vec();
    let mut aes = [0u8; BLOCK];
    aes.copy_from_slice(&ascii[..BLOCK]);
    ResolvedKeys {
        keys: ImageKeys {
            code,
            xor_key: (code & 0xff) as u8,
            wxid: wxid.to_string(),
            source: String::new(),
        },
        aes,
    }
}

fn md5_hex(value: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

/// Strip the `_<hash>` disambiguator WeChat appends to the account directory name.
fn wxid_candidates(account_dir: &Path) -> Vec<String> {
    let name = account_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut out = vec![name.clone()];
    if let Some((head, _)) = name.rsplit_once('_') {
        if !head.is_empty() {
            out.push(head.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn kvcomm_stems(name: &str) -> Option<u64> {
    let rest = name.strip_prefix("key_")?;
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let code = digits.parse::<u64>().ok()?;
    if code == 0 || code > u32::MAX as u64 {
        return None;
    }
    Some(code)
}

fn collect_kvcomm(dir: &Path, depth: usize, codes: &mut Vec<(u64, String)>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            let is_kvcomm = path
                .file_name()
                .map(|name| name == "kvcomm")
                .unwrap_or(false);
            if is_kvcomm {
                if let Ok(files) = fs::read_dir(&path) {
                    for file in files.flatten() {
                        let name = file.file_name().to_string_lossy().to_string();
                        if !name.ends_with(".statistic") {
                            continue;
                        }
                        if let Some(code) = kvcomm_stems(&name) {
                            codes.push((code, file.path().to_string_lossy().to_string()));
                        }
                    }
                }
                continue;
            }
            collect_kvcomm(&path, depth + 1, codes);
        }
    }
}

/// Directories that may hold the `kvcomm` cache, derived from the account path.
fn config_roots(account_dir: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut cursor = Some(account_dir);
    while let Some(dir) = cursor {
        if dir.file_name().map(|name| name == "xwechat_files").unwrap_or(false) {
            if let Some(parent) = dir.parent() {
                roots.push(parent.join("app_data"));
                roots.push(parent.to_path_buf());
            }
            break;
        }
        cursor = dir.parent();
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        roots.push(home.join("Documents/app_data"));
    }
    for key in ["APPDATA", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(key) {
            roots.push(PathBuf::from(base).join("Tencent/xwechat"));
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

fn image_kind(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some("jpg");
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("png");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if bytes.starts_with(b"wxgf") || bytes.starts_with(b"WXGF") {
        return Some("wxam");
    }
    None
}

fn aes_decrypt(key: &[u8; BLOCK], data: &[u8]) -> Vec<u8> {
    let cipher = Aes128::new_from_slice(key).expect("16-byte key");
    let mut out = data.to_vec();
    for chunk in out.chunks_mut(BLOCK) {
        if chunk.len() == BLOCK {
            let block = aes::Block::from_mut_slice(chunk);
            cipher.decrypt_block(block);
        }
    }
    out
}

fn unpad(data: &mut Vec<u8>) {
    if let Some(&last) = data.last() {
        let pad = last as usize;
        if pad > 0 && pad <= BLOCK && data.len() >= pad && data[data.len() - pad..].iter().all(|b| *b == last) {
            data.truncate(data.len() - pad);
        }
    }
}

/// Try every code/wxid candidate against one ciphertext block.
pub fn resolve(account_dir: &Path, sample: &[u8]) -> Option<ResolvedKeys> {
    if sample.len() < HEADER + BLOCK || !sample.starts_with(V2_MAGIC) {
        return None;
    }
    let mut codes = Vec::new();
    for root in config_roots(account_dir) {
        collect_kvcomm(&root, 0, &mut codes);
    }
    codes.sort();
    codes.dedup_by(|a, b| a.0 == b.0);

    let block = &sample[HEADER..HEADER + BLOCK];
    for (code, path) in codes {
        for wxid in wxid_candidates(account_dir) {
            let mut keys = derive(code, &wxid);
            let plain = aes_decrypt(&keys.aes, block);
            if image_kind(&plain).is_some() {
                keys.keys.source = path.clone();
                return Some(keys);
            }
        }
    }
    None
}

#[derive(Serialize, Clone, Debug)]
pub struct DecodedImage {
    pub bytes: Vec<u8>,
    pub kind: String,
}

/// Decode one `.dat` payload. `WxAM` is reported instead of returned as broken data.
pub fn decode(data: &[u8], keys: &ResolvedKeys) -> Result<DecodedImage, String> {
    if data.starts_with(V2_MAGIC) || data.starts_with(V1_MAGIC) {
        if data.len() < HEADER + BLOCK {
            return Err("图片文件头不完整。".into());
        }
        let aes_size = u32::from_le_bytes([data[6], data[7], data[8], data[9]]) as usize;
        let xor_size = u32::from_le_bytes([data[10], data[11], data[12], data[13]]) as usize;
        if aes_size == 0 {
            return Err("图片头缺少加密区长度。".into());
        }
        let aligned = aes_size + BLOCK - (aes_size % BLOCK);
        let body = &data[HEADER..];
        if aligned > body.len() {
            return Err("图片头声明的密文长度超过文件大小。".into());
        }

        let aes_key = if data.starts_with(V1_MAGIC) {
            // V1 used a fixed key; V2 derives it. Try both variants and keep what works.
            let fixed = md5::Md5::digest(b"0");
            let mut raw = [0u8; BLOCK];
            raw.copy_from_slice(&fixed);
            let mut ascii = [0u8; BLOCK];
            ascii.copy_from_slice(hex::encode(fixed)[..16].as_bytes());
            let mut first = aes_decrypt(&raw, &body[..aligned]);
            unpad(&mut first);
            if image_kind(&first).is_some() {
                first
            } else {
                let mut second = aes_decrypt(&ascii, &body[..aligned]);
                unpad(&mut second);
                second
            }
        } else {
            let mut plain = aes_decrypt(&keys.aes, &body[..aligned]);
            unpad(&mut plain);
            plain
        };

        let (middle, trailer) = if xor_size > 0 && xor_size < body.len() {
            (
                &body[aligned..body.len() - xor_size],
                body[body.len() - xor_size..]
                    .iter()
                    .map(|byte| byte ^ keys.keys.xor_key)
                    .collect::<Vec<u8>>(),
            )
        } else {
            (&body[aligned..], Vec::new())
        };

        let mut out = aes_key;
        out.extend_from_slice(middle);
        out.extend_from_slice(&trailer);
        if let Some(kind) = image_kind(&out) {
            return Ok(DecodedImage {
                bytes: out,
                kind: kind.to_string(),
            });
        }
        if out.starts_with(b"wxgf") || out.starts_with(b"WXGF") {
            return Err("WxAM（HEVC 压缩）图片暂不支持解码。".into());
        }
        return Err("解密结果不是已知图片格式。".into());
    }

    // Legacy: one XOR byte for the whole file, recovered from a known signature.
    let xor = data.first().map(|first| first ^ 0xFF);
    if let Some(xor) = xor {
        let guess: Vec<u8> = data.iter().map(|byte| byte ^ xor).collect();
        if let Some(kind) = image_kind(&guess) {
            return Ok(DecodedImage {
                bytes: guess,
                kind: kind.to_string(),
            });
        }
    }
    Err("未知的图片加密格式。".into())
}

/// Convert a `wxgf` (WxAM) payload into a PNG by extracting its HEVC bitstream.
///
/// WxAM wraps a normal HEVC stream behind a private header. Feature-scanning for the Annex-B
/// start code is enough to recover the bitstream, and `ffmpeg` decodes the first frame. The
/// converter is optional: when `ffmpeg` is not installed the caller keeps the thumbnail.
pub fn wxam_to_png(payload: &[u8]) -> Result<Vec<u8>, String> {
    wxam_decode(payload, "png", 0)
}

/// Decode WxAM to JPEG, optionally capping the width (used for lightweight previews).
pub fn wxam_to_jpeg(payload: &[u8], max_width: u32) -> Result<Vec<u8>, String> {
    wxam_decode(payload, "mjpeg", max_width)
}

fn wxam_decode(payload: &[u8], codec: &str, max_width: u32) -> Result<Vec<u8>, String> {
    if !(payload.starts_with(b"wxgf") || payload.starts_with(b"WXGF")) {
        return Err("不是 WxAM 图片。".into());
    }
    let start = payload
        .windows(4)
        .position(|window| window == [0, 0, 0, 1])
        .or_else(|| {
            payload
                .windows(3)
                .position(|window| window == [0, 0, 1])
        })
        .ok_or_else(|| "WxAM 中没有找到 HEVC 码流。".to_string())?;
    let bitstream = &payload[start..];

    let Some(ffmpeg) = ffmpeg_path() else {
        return Err("需要 ffmpeg 才能解码 WxAM 原图（brew install ffmpeg）。".into());
    };

    let mut command = std::process::Command::new(ffmpeg);
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "hevc",
        "-i",
        "pipe:0",
        "-frames:v",
        "1",
    ]);
    if max_width > 0 {
        command.args(["-vf", &format!("scale='min({max_width},iw)':-1")]);
    }
    if codec == "mjpeg" {
        command.args(["-q:v", "3"]);
    }
    command.args(["-f", "image2pipe", "-vcodec", codec, "-"]);
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 ffmpeg：{e}"))?;

    let data = bitstream.to_vec();
    let mut stdin = child.stdin.take().ok_or("ffmpeg 标准输入不可用。")?;
    let writer = std::thread::spawn(move || {
        use std::io::Write;
        let _ = stdin.write_all(&data);
    });
    let output = child
        .wait_with_output()
        .map_err(|e| format!("等待 ffmpeg 失败：{e}"))?;
    let _ = writer.join();

    if !output.status.success() || output.stdout.is_empty() {
        let tail: String = String::from_utf8_lossy(&output.stderr).chars().take(300).collect();
        return Err(format!("ffmpeg 解码 WxAM 失败：{}", tail.trim()));
    }
    Ok(output.stdout)
}

fn ffmpeg_path() -> Option<PathBuf> {
    for candidate in [
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/usr/bin/ffmpeg",
    ] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    let names: &[&str] = if cfg!(windows) {
        &["ffmpeg.exe", "ffmpeg"]
    } else {
        &["ffmpeg"]
    };
    crate::agent::resolve_bins(names)
        .get(names[0])
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_keys_from_code_and_wxid() {
        let keys = derive(1405096200, "zhengweifree");
        assert_eq!(keys.keys.xor_key, (1405096200u64 & 0xff) as u8);
        assert_ne!(keys.keys.code, 0);
        assert_eq!(keys.keys.wxid, "zhengweifree");
    }

    #[test]
    fn account_dir_suffix_is_a_wxid_candidate() {
        let mut dir = PathBuf::from("/tmp");
        dir.push("zhengweifree_b6d1");
        let candidates = wxid_candidates(&dir);
        assert!(candidates.contains(&"zhengweifree".to_string()));
        assert!(candidates.contains(&"zhengweifree_b6d1".to_string()));
    }

    #[test]
    fn kvcomm_file_names_expose_the_code() {
        assert_eq!(kvcomm_stems("key_1405096200_4066647863_1_1793693678_670857875_3600_input.statistic"), Some(1405096200));
        assert_eq!(kvcomm_stems("reportnow_1405096200_0.statistic"), None);
        assert_eq!(kvcomm_stems("key_notanumber_x.statistic"), None);
    }

    #[test]
    fn v2_header_round_trips_through_decode() {
        // Synthetic V2 payload: 16 bytes AES (one block), no XOR trailer.
        let keys = derive(123456, "wxid_test");
        let cipher = Aes128::new_from_slice(&keys.aes).unwrap();
        use aes::cipher::BlockEncrypt;
        // 16-byte plaintext + PKCS#7 padding is two blocks of ciphertext.
        let mut plain = [0u8; 32];
        plain[..4].copy_from_slice(b"wxgf");
        plain[16] = 16;
        plain[17..].fill(16);
        let mut ciphertext = plain;
        for chunk in ciphertext.chunks_mut(16) {
            cipher.encrypt_block(aes::Block::from_mut_slice(chunk));
        }

        let mut dat = Vec::new();
        dat.extend_from_slice(V2_MAGIC);
        dat.extend_from_slice(&16u32.to_le_bytes());
        dat.extend_from_slice(&0u32.to_le_bytes());
        dat.push(1);
        dat.extend_from_slice(&ciphertext);
        // Wrong wxid must not decode as a known image.
        let wrong = derive(123456, "someone_else");
        assert!(decode(&dat, &wrong).is_err());
        // The right key reports WxAM instead of pretending it is a JPEG.
        let decoded = decode(&dat, &keys).expect("right key must decode");
        assert_eq!(decoded.kind, "wxam");
    }
}
