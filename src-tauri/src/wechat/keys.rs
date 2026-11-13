//! Key storage. Keys never leave the machine and are never written to logs.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::crypto;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct KeyEntry {
    pub enc_key: String,
    pub salt: String,
    pub source: String,
    pub verified_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct KeyStore {
    pub version: u32,
    pub keys: BTreeMap<String, KeyEntry>,
}

impl Default for KeyStore {
    fn default() -> Self {
        KeyStore {
            version: 1,
            keys: BTreeMap::new(),
        }
    }
}

/// A candidate parsed from imported text, optionally bound to a relative db path.
///
/// WeChat 4.x accepts two shapes: a per-database `enc_key`, or the account-wide 32-byte
/// passphrase that has to be run through PBKDF2 with each database's own salt. Pasted
/// 64-hex material is ambiguous, so both interpretations are tried and only the one that
/// passes the page-1 HMAC check is stored.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub rel: Option<String>,
    pub key: Vec<u8>,
    pub label: String,
    pub passphrase: bool,
}

/// Resolve which imported candidate unlocks one database.
///
/// Raw keys are tried first so a passphrase never pays the 256000-round derivation on a
/// database that is already covered by a direct key. Passphrase hits return the derived
/// per-database key, which is what gets stored and used for decryption.
pub fn resolve_db_key(page1: &[u8], candidates: &[Candidate]) -> Option<(Vec<u8>, String)> {
    for candidate in candidates.iter().filter(|c| !c.passphrase) {
        if crypto::verify_key(page1, &candidate.key) {
            return Some((candidate.key.clone(), candidate.label.clone()));
        }
    }
    for candidate in candidates.iter().filter(|c| c.passphrase) {
        if let Some(derived) = crypto::verify_passphrase(page1, &candidate.key) {
            return Some((derived.to_vec(), candidate.label.clone()));
        }
    }
    None
}

pub fn store_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("wechat-keys.json")
}

pub fn load(path: &Path) -> KeyStore {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<KeyStore>(&text).ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, store: &KeyStore) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("无法创建密钥目录：{e}"))?;
    }
    let text = serde_json::to_string_pretty(store).map_err(|e| format!("密钥序列化失败：{e}"))?;
    fs::write(path, text).map_err(|e| format!("无法写入密钥文件：{e}"))?;
    restrict_permissions(path)?;
    Ok(())
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("无法收紧密钥文件权限：{e}"))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

pub fn store_key(store: &mut KeyStore, account: &str, rel: &str, key: &[u8], salt: &[u8], source: &str) {
    store.keys.insert(
        format!("{account}::{rel}"),
        KeyEntry {
            enc_key: hex::encode(key),
            salt: hex::encode(salt),
            source: source.to_string(),
            verified_at: now_iso(),
        },
    );
}

pub fn lookup<'a>(store: &'a KeyStore, account: &str, rel: &str) -> Option<&'a KeyEntry> {
    store.keys.get(&format!("{account}::{rel}"))
}

/// Remove every stored key for one account. Returns how many entries were dropped.
pub fn remove_account(store: &mut KeyStore, account: &str) -> usize {
    let prefix = format!("{account}::");
    let before = store.keys.len();
    store.keys.retain(|id, _| !id.starts_with(&prefix));
    before - store.keys.len()
}

/// Remove all stored keys. Returns how many entries were dropped.
pub fn remove_all(store: &mut KeyStore) -> usize {
    let before = store.keys.len();
    store.keys.clear();
    before
}

/// Stored verification time as Unix seconds, when the audit string uses that format.
pub fn verified_secs(entry: &KeyEntry) -> Option<u64> {
    entry.verified_at.strip_prefix("unix:")?.parse().ok()
}

fn now_iso() -> String {
    // Seconds since epoch is enough for an audit trail and avoids a chrono dependency.
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => format!("unix:{}", d.as_secs()),
        Err(_) => "unknown".to_string(),
    }
}

/// Decode an account passphrase: exactly 32 bytes written as 64 hex characters.
pub fn passphrase_from_text(raw: &str) -> Option<Vec<u8>> {
    let cleaned: String = raw
        .trim()
        .trim_start_matches("0x")
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    if cleaned.len() != crypto::KEY_LEN * 2 {
        return None;
    }
    hex::decode(cleaned).ok()
}

fn key_from_hex(raw: &str) -> Option<Vec<u8>> {
    let cleaned: String = raw.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if cleaned.len() < crypto::KEY_LEN * 2 {
        return None;
    }
    let key_hex = &cleaned[..crypto::KEY_LEN * 2];
    hex::decode(key_hex).ok()
}

/// Parse imported key material. Accepts:
/// - `x'<64 hex key><32 hex salt>'` (memory scanner form)
/// - a bare 64/96+ hex string
/// - `all_keys.json` style objects (flat map of path -> {enc_key, ...} or path -> hex)
pub fn parse_candidates(text: &str) -> Result<Vec<Candidate>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("没有提供任何密钥内容。".into());
    }

    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return parse_json_candidates(trimmed);
    }

    // All x'...' occurrences in pasted text.
    let mut candidates = Vec::new();
    let bytes = trimmed.as_bytes();
    let mut index = 0;
    while let Some(pos) = trimmed[index..].find("x'") {
        let start = index + pos + 2;
        if let Some(end_rel) = trimmed[start..].find('\'') {
            let hex_part = &trimmed[start..start + end_rel];
            if let Some(key) = key_from_hex(hex_part) {
                candidates.push(Candidate {
                    rel: None,
                    key,
                    label: "粘贴的内存密钥".to_string(),
                    passphrase: false,
                });
            }
            index = start + end_rel + 1;
        } else {
            break;
        }
    }
    if candidates.is_empty() {
        if let Some(key) = key_from_hex(trimmed) {
            let hex_only: String = trimmed
                .trim()
                .trim_start_matches("0x")
                .chars()
                .filter(|c| c.is_ascii_hexdigit())
                .collect();
            candidates.push(Candidate {
                rel: None,
                key: key.clone(),
                label: "粘贴的十六进制密钥".to_string(),
                passphrase: false,
            });
            // A bare 64-hex value may equally be the account passphrase rather than a
            // database key. Both are attempted; only the verified one is stored.
            if hex_only.len() == crypto::KEY_LEN * 2 {
                // Same bytes, different meaning: as a key it must decrypt page 1 directly,
                // as a passphrase it is fed through PBKDF2 with each database's salt.
                if let Some(passphrase) = passphrase_from_text(&hex_only) {
                    candidates.push(Candidate {
                        rel: None,
                        key: passphrase,
                        label: "粘贴的口令（PBKDF2 派生）".to_string(),
                        passphrase: true,
                    });
                }
            }
        }
    }
    let _ = bytes;
    if candidates.is_empty() {
        return Err(
            "无法识别密钥格式。支持 x'<hex>'、64 位十六进制密钥或口令、{\"passphrase\":\"…\"}，或 all_keys.json。"
                .into(),
        );
    }
    dedupe(candidates)
}

fn parse_json_candidates(text: &str) -> Result<Vec<Candidate>, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("JSON 解析失败：{e}"))?;
    let mut candidates = Vec::new();

    // `~/.wcdb-key-tool/wechat-passphrase.json` and similar exports.
    if let Some(passphrase) = value.get("passphrase").and_then(|v| v.as_str()) {
        if let Some(bytes) = passphrase_from_text(passphrase) {
            candidates.push(Candidate {
                rel: None,
                key: bytes,
                label: "导入的口令（PBKDF2 派生）".to_string(),
                passphrase: true,
            });
        }
    }

    match &value {
        serde_json::Value::Object(map) => {
            for (key, entry) in map {
                if key.starts_with('_') || key.eq_ignore_ascii_case("passphrase") {
                    continue;
                }
                let raw = match entry {
                    serde_json::Value::String(s) => Some(s.clone()),
                    serde_json::Value::Object(inner) => ["enc_key", "encKey", "key", "hex"]
                        .iter()
                        .find_map(|field| inner.get(*field).and_then(|v| v.as_str()).map(str::to_string)),
                    _ => None,
                };
                if let Some(raw) = raw {
                    if let Some(bytes) = key_from_hex(&raw) {
                        candidates.push(Candidate {
                            rel: Some(key.clone()),
                            key: bytes,
                            label: key.clone(),
                            passphrase: false,
                        });
                    }
                }
            }
        }
        serde_json::Value::Array(items) => {
            for (idx, entry) in items.iter().enumerate() {
                if let Some(raw) = entry.get("enc_key").and_then(|v| v.as_str()) {
                    if let Some(bytes) = key_from_hex(raw) {
                        candidates.push(Candidate {
                            rel: entry.get("rel").and_then(|v| v.as_str()).map(str::to_string),
                            key: bytes,
                            label: format!("数组第 {} 项", idx + 1),
                            passphrase: false,
                        });
                    }
                }
            }
        }
        _ => {}
    }
    if candidates.is_empty() {
        return Err("JSON 中没有找到可用的 enc_key 或 passphrase。".into());
    }
    dedupe(candidates)
}

fn dedupe(candidates: Vec<Candidate>) -> Result<Vec<Candidate>, String> {
    let mut seen: Vec<(Option<String>, Vec<u8>, bool)> = Vec::new();
    let mut out = Vec::new();
    for candidate in candidates {
        let marker = (candidate.rel.clone(), candidate.key.clone(), candidate.passphrase);
        if seen.contains(&marker) {
            continue;
        }
        seen.push(marker);
        out.push(candidate);
    }
    if out.is_empty() {
        return Err("没有可用的密钥候选。".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    #[test]
    fn parses_reference_passphrase_file() {
        let text = format!(r#"{{"passphrase":"{HEX}"}}"#);
        let candidates = parse_candidates(&text).expect("reference file must parse");
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].passphrase);
        assert_eq!(candidates[0].key.len(), crypto::KEY_LEN);
    }

    #[test]
    fn bare_hex_is_tried_as_key_and_passphrase() {
        let candidates = parse_candidates(HEX).expect("bare hex must parse");
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|c| c.passphrase));
        assert!(candidates.iter().any(|c| !c.passphrase));
    }

    #[test]
    fn rejects_short_and_empty_input() {
        assert!(parse_candidates("").is_err());
        assert!(parse_candidates("abcd").is_err());
    }

    #[test]
    fn parses_all_keys_map() {
        let text = format!(r#"{{"message/message_0.db":{{"enc_key":"{HEX}"}}}}"#);
        let candidates = parse_candidates(&text).expect("all_keys map must parse");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].rel.as_deref(), Some("message/message_0.db"));
        assert!(!candidates[0].passphrase);
    }

    #[test]
    fn removes_keys_per_account_or_all() {
        let mut store = KeyStore::default();
        store_key(&mut store, "a", "session/session.db", &[1u8; 32], &[2u8; 16], "test");
        store_key(&mut store, "a", "contact/contact.db", &[1u8; 32], &[2u8; 16], "test");
        store_key(&mut store, "b", "session/session.db", &[1u8; 32], &[2u8; 16], "test");

        assert_eq!(remove_account(&mut store, "a"), 2);
        assert!(lookup(&store, "b", "session/session.db").is_some());
        assert_eq!(remove_account(&mut store, "a"), 0);
        assert_eq!(remove_all(&mut store), 1);
        assert!(store.keys.is_empty());
    }

    #[test]
    fn reads_unix_verification_time() {
        let entry = KeyEntry {
            enc_key: HEX.to_string(),
            salt: HEX[..32].to_string(),
            source: "test".into(),
            verified_at: "unix:1700000000".into(),
        };
        assert_eq!(verified_secs(&entry), Some(1_700_000_000));
    }
}
