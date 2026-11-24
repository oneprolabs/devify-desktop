//! Snapshot copy + page decryption into a read-only cache used by the query layer.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::crypto;
use super::discover::{Account, DbFile};
use super::keys::{self, KeyStore};
use super::wal;

#[derive(Serialize, Clone, Debug)]
pub struct DecryptOutcome {
    pub account: String,
    pub rel: String,
    /// ok | cached | no-key | failed
    pub status: String,
    pub pages: u64,
    pub failed_page: Option<u64>,
    pub error: Option<String>,
    pub wal: Option<wal::WalOutcome>,
    pub output: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct SyncMeta {
    entries: BTreeMap<String, SyncEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
struct SyncEntry {
    src_size: u64,
    src_mtime: u64,
    out_size: u64,
    #[serde(default)]
    wal_size: u64,
    #[serde(default)]
    wal_mtime: u64,
    /// Older entries and snapshots made with WAL replay disabled must retry a nonempty WAL.
    #[serde(default)]
    wal_replayed: bool,
    /// Salt of the key that produced this decrypted copy. The cache is only reusable while the
    /// same salt still unlocks the database: entries written before this field existed carry an
    /// empty value and therefore force one re-decryption instead of trusting a stale copy.
    #[serde(default)]
    key_salt: String,
}

/// Keys are not the only thing that makes a database readable: the decrypted copy sits in the
/// cache and the shortcut below would keep serving it after the keys were cleared. Tie the
/// cached copy to the key that created it so clearing (or re-keying) invalidates the cache.
fn cache_matches_key(db: &DbFile, entry: &SyncEntry, store: &KeyStore, account: &str) -> bool {
    if !db.encrypted {
        return true;
    }
    match keys::lookup(store, account, &db.rel) {
        Some(key) => !entry.key_salt.is_empty() && entry.key_salt == key.salt
            && db.salt.as_deref() == Some(key.salt.as_str()),
        None => false,
    }
}

pub fn cache_dir(app_data_dir: &Path, account: &str) -> PathBuf {
    crate::local_scope::root(app_data_dir).join("wechat-cache").join(account)
}

fn meta_path(cache: &Path) -> PathBuf {
    cache.join("_sync.json")
}

fn load_meta(cache: &Path) -> SyncMeta {
    fs::read_to_string(meta_path(cache))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_meta(cache: &Path, meta: &SyncMeta) {
    if let Ok(text) = serde_json::to_string_pretty(meta) {
        let _ = fs::write(meta_path(cache), text);
    }
}

fn mtime_nanos(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn file_signature(path: &Path) -> (u64, u64) {
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    (size, mtime_nanos(path))
}

fn wal_path(path: &str) -> PathBuf {
    sidecar(Path::new(path), "-wal")
}

fn base_path(path: &Path) -> PathBuf {
    sidecar(path, ".base")
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", path.to_string_lossy(), suffix))
}

fn remove_if_exists(path: &Path) {
    let _ = fs::remove_file(path);
}

fn failed(account: &str, rel: &str, error: impl Into<String>) -> DecryptOutcome {
    DecryptOutcome {
        account: account.to_string(),
        rel: rel.to_string(),
        status: "failed".into(),
        pages: 0,
        failed_page: None,
        error: Some(error.into()),
        wal: None,
        output: None,
    }
}

/// Copy a plaintext database together with its WAL so SQLite can see the latest committed state.
fn copy_plaintext(db: &DbFile, out: &Path) -> Result<u64, String> {
    let size = fs::copy(&db.path, out).map_err(|e| format!("复制明文数据库失败：{e}"))?;
    let src_wal = sidecar(Path::new(&db.path), "-wal");
    let out_wal = sidecar(out, "-wal");
    let src_shm = sidecar(Path::new(&db.path), "-shm");
    let out_shm = sidecar(out, "-shm");
    if src_wal.exists() {
        fs::copy(&src_wal, &out_wal).map_err(|e| format!("复制 WAL 失败：{e}"))?;
    } else {
        remove_if_exists(&out_wal);
    }
    if src_shm.exists() {
        let _ = fs::copy(&src_shm, &out_shm);
    } else {
        remove_if_exists(&out_shm);
    }
    Ok(size)
}

fn decrypt_file(db: &DbFile, out: &Path, enc_key: &[u8], replay_wal: bool) -> DecryptOutcome {
    let account = String::new();
    let mut pages: u64 = 0;

    let salt = match salt_bytes(db.salt.as_deref()) {
        Some(salt) => salt,
        None => return failed(&account, &db.rel, "数据库 salt 不可读"),
    };
    let mac_key = match crypto::derive_mac_key(enc_key, &salt) {
        Ok(mac) => mac,
        Err(error) => return failed(&account, &db.rel, error),
    };

    let source = match File::open(&db.path) {
        Ok(file) => file,
        Err(error) => return failed(&account, &db.rel, format!("无法读取数据库：{error}")),
    };
    let mut reader = BufReader::new(source);
    let tmp = out.with_extension("db.part");
    let sink = match File::create(&tmp) {
        Ok(file) => file,
        Err(error) => return failed(&account, &db.rel, format!("无法创建临时文件：{error}")),
    };
    let mut writer = BufWriter::new(sink);

    let mut page = vec![0u8; crypto::PAGE_SIZE];
    let mut pgno: u32 = 1;
    loop {
        match reader.read_exact(&mut page) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => {
                remove_if_exists(&tmp);
                return failed(&account, &db.rel, format!("读取第 {pgno} 页失败：{error}"));
            }
        }
        if !crypto::verify_page(&page, &mac_key, pgno) {
            remove_if_exists(&tmp);
            let mut outcome = failed(
                &account,
                &db.rel,
                format!("第 {pgno} 页 HMAC 校验失败：密钥与该数据库不匹配，或读取时文件正在被写入。"),
            );
            outcome.failed_page = Some(pgno as u64);
            return outcome;
        }
        match crypto::decrypt_page(&page, enc_key, pgno) {
            Ok(plain) => {
                if let Err(error) = writer.write_all(&plain) {
                    remove_if_exists(&tmp);
                    return failed(&account, &db.rel, format!("写入解密数据失败：{error}"));
                }
            }
            Err(error) => {
                remove_if_exists(&tmp);
                return failed(&account, &db.rel, error);
            }
        }
        pages = pgno as u64;
        pgno = pgno.saturating_add(1);
    }

    if let Err(error) = writer.flush() {
        remove_if_exists(&tmp);
        return failed(&account, &db.rel, format!("刷新解密数据失败：{error}"));
    }
    drop(writer);

    let base = base_path(out);
    let base_part = sidecar(&tmp, ".base.part");
    if let Err(error) = fs::copy(&tmp, &base_part) {
        remove_if_exists(&tmp);
        remove_if_exists(&base_part);
        return failed(&account, &db.rel, format!("保存解密基线失败：{error}"));
    }

    let wal_outcome = if replay_wal {
        match replay_snapshot_wal(db, &tmp, enc_key, &mac_key) {
            Ok(outcome) => Some(outcome),
            Err(error) => {
                remove_if_exists(&tmp);
                remove_if_exists(&base_part);
                return failed(&account, &db.rel, error);
            }
        }
    } else {
        None
    };

    if let Err(error) = fs::rename(&base_part, &base) {
        remove_if_exists(&tmp);
        remove_if_exists(&base_part);
        return failed(&account, &db.rel, format!("保存解密基线失败：{error}"));
    }

    if let Err(error) = fs::rename(&tmp, out) {
        remove_if_exists(&tmp);
        return failed(&account, &db.rel, format!("替换解密文件失败：{error}"));
    }
    remove_if_exists(&sidecar(out, "-wal"));
    remove_if_exists(&sidecar(out, "-shm"));

    DecryptOutcome {
        account,
        rel: db.rel.clone(),
        status: "ok".into(),
        pages,
        failed_page: None,
        error: None,
        wal: wal_outcome,
        output: Some(out.to_string_lossy().to_string()),
    }
}

fn replay_snapshot_wal(db: &DbFile, tmp: &Path, enc_key: &[u8], mac_key: &[u8]) -> Result<wal::WalOutcome, String> {
    let mut handle = File::options()
        .read(true)
        .write(true)
        .open(tmp)
        .map_err(|error| format!("无法打开解密文件以重放 WAL：{error}"))?;
    wal::apply(&mut handle, &wal_path(&db.path), enc_key, mac_key)
}

fn refresh_wal(
    db: &DbFile,
    out: &Path,
    enc_key: &[u8],
    base: &Path,
) -> Result<wal::WalOutcome, String> {
    let salt = salt_bytes(db.salt.as_deref()).ok_or("数据库 salt 不可读")?;
    let mac_key = crypto::derive_mac_key(enc_key, &salt)?;
    let tmp = out.with_extension("db.refresh.part");
    let result = (|| {
        fs::copy(base, &tmp).map_err(|e| format!("复制解密基线失败：{e}"))?;
        let outcome = replay_snapshot_wal(db, &tmp, enc_key, &mac_key)?;
        fs::rename(&tmp, out).map_err(|e| format!("替换增量解密文件失败：{e}"))?;
        Ok(outcome)
    })();
    if result.is_err() {
        remove_if_exists(&tmp);
        return result;
    }
    remove_if_exists(&sidecar(out, "-wal"));
    remove_if_exists(&sidecar(out, "-shm"));
    result
}

fn salt_bytes(salt_hex: Option<&str>) -> Option<Vec<u8>> {
    let bytes = hex::decode(salt_hex?).ok()?;
    if bytes.len() == crypto::SALT_LEN {
        Some(bytes)
    } else {
        None
    }
}

fn is_chat_db(rel: &str) -> bool {
    rel == "session/session.db"
        || rel == "contact/contact.db"
        || rel == "bizchat/bizchat.db"
        // WeChat 4.x also keeps `message/message_0.kvdb` LevelDB sidecars next to the shards.
        // They are not SQLCipher databases, have no derived key, and are not needed to read
        // messages, so the suffix matters here.
        || (rel.starts_with("message/message_") && rel.ends_with(".db"))
        || (rel.starts_with("message/biz_message_") && rel.ends_with(".db"))
        // Voice payloads live in `message/media_0.db` (`VoiceInfo.voice_data`), so the media
        // shard has to be decrypted for transcription and attachment metadata.
        || (rel.starts_with("message/media_") && rel.ends_with(".db"))
        || (rel.starts_with("message/biz_media_") && rel.ends_with(".db"))
        // Avatars live beside the chat databases and are keyed by username, so they are part
        // of rendering a conversation rather than a separate feature.
        || rel.starts_with("head_image/")
}

/// Upgrade an older cache lazily when a business conversation is first opened.
/// Only the previously omitted databases are prepared, never the whole chat history.
pub fn prepare_business_cache(data: &Path, account_id: &str, include_messages: bool) -> Result<(), String> {
    let cache = cache_dir(data, account_id);
    if cache.join("bizchat/bizchat.db").is_file() && (!include_messages || cache.join("message/biz_message_0.db").is_file()) { return Ok(()); }
    let account = super::discover::scan().accounts.into_iter().find(|account| account.id == account_id)
        .ok_or("未找到本机微信账号，请检查微信连接。")?;
    prepare_business_account(data, account, include_messages)
}

fn prepare_business_account(data: &Path, mut account: Account, include_messages: bool) -> Result<(), String> {
    let cache = cache_dir(data, &account.id);
    account.dbs.retain(|db| (db.rel == "bizchat/bizchat.db" || (include_messages && (db.rel.starts_with("message/biz_message_") || db.rel.starts_with("message/biz_media_")))) && db.rel.ends_with(".db") && !cache.join(&db.rel).is_file());
    if account.dbs.is_empty() { return Ok(()); }
    let store = keys::load(&keys::store_path(data));
    let outcomes = decrypt_account(data, &account, &store, true, true);
    if outcomes.iter().any(|outcome| !matches!(outcome.status.as_str(), "ok" | "cached")) {
        return Err("这段微信记录尚未准备好，请在连接器中重新检查微信连接。".into());
    }
    Ok(())
}

pub fn decrypt_account(
    app_data_dir: &Path,
    account: &Account,
    store: &KeyStore,
    apply_wal: bool,
    only_chat: bool,
) -> Vec<DecryptOutcome> {
    // Explicit refreshes and the automatic coordinator share the same atomic outputs.
    static DECRYPT_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _serial = DECRYPT_SERIAL.lock().unwrap_or_else(|error| error.into_inner());
    let cache = cache_dir(app_data_dir, &account.id);
    let _ = fs::create_dir_all(&cache);
    let mut meta = load_meta(&cache);
    let mut outcomes = Vec::new();

    for db in account
        .dbs
        .iter()
        .filter(|db| !only_chat || is_chat_db(&db.rel))
    {
        let out = cache.join(&db.rel);
        if let Some(parent) = out.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let src_size = db.size;
        let src_mtime = mtime_nanos(Path::new(&db.path));
        let (wal_size, wal_mtime) = file_signature(&wal_path(&db.path));
        let replay_wal = apply_wal && db.encrypted && wal_size > 0;

        if let Some(entry) = meta.entries.get(&db.rel) {
            let matching_key = cache_matches_key(db, entry, store, &account.id);
            if entry.src_size == src_size
                && entry.src_mtime == src_mtime
                && entry.wal_size == wal_size
                && entry.wal_mtime == wal_mtime
                && (!replay_wal || entry.wal_replayed)
                && out.exists()
                && matching_key
            {
                outcomes.push(DecryptOutcome {
                    account: account.id.clone(),
                    rel: db.rel.clone(),
                    status: "cached".into(),
                    pages: entry.out_size / crypto::PAGE_SIZE as u64,
                    failed_page: None,
                    error: None,
                    wal: None,
                    output: Some(out.to_string_lossy().to_string()),
                });
                continue;
            }

            if replay_wal
                && matching_key
                && entry.src_size == src_size
                && entry.src_mtime == src_mtime
                && base_path(&out).exists()
            {
                let Some(key) = keys::lookup(store, &account.id, &db.rel) else {
                    let mut outcome = failed(&account.id, &db.rel, "没有该数据库的密钥。请先用密钥工具提取并导入。");
                    outcome.status = "no-key".into();
                    outcomes.push(outcome);
                    continue;
                };
                let Ok(enc_key) = hex::decode(&key.enc_key) else {
                    outcomes.push(failed(&account.id, &db.rel, "保存的密钥格式无效。"));
                    continue;
                };
                match refresh_wal(db, &out, &enc_key, &base_path(&out)) {
                    Ok(wal) => {
                        meta.entries.insert(db.rel.clone(), SyncEntry {
                            src_size,
                            src_mtime,
                            out_size: fs::metadata(&out).map(|m| m.len()).unwrap_or(entry.out_size),
                            wal_size,
                            wal_mtime,
                            wal_replayed: true,
                            key_salt: key.salt.clone(),
                        });
                        outcomes.push(DecryptOutcome {
                            account: account.id.clone(),
                            rel: db.rel.clone(),
                            status: "ok".into(),
                            pages: fs::metadata(&out).map(|m| m.len()).unwrap_or(0) / crypto::PAGE_SIZE as u64,
                            failed_page: None,
                            error: None,
                            wal: Some(wal),
                            output: Some(out.to_string_lossy().to_string()),
                        });
                        continue;
                    }
                    Err(error) => {
                        outcomes.push(failed(&account.id, &db.rel, error));
                        continue;
                    }
                }
            }
        }

        if !db.encrypted {
            match copy_plaintext(db, &out) {
                Ok(size) => {
                    meta.entries.insert(
                        db.rel.clone(),
                        SyncEntry {
                            src_size,
                            src_mtime,
                            out_size: size,
                            wal_size,
                            wal_mtime,
                            wal_replayed: false,
                            key_salt: String::new(),
                        },
                    );
                    outcomes.push(DecryptOutcome {
                        account: account.id.clone(),
                        rel: db.rel.clone(),
                        status: "ok".into(),
                        pages: size / crypto::PAGE_SIZE as u64,
                        failed_page: None,
                        error: None,
                        wal: None,
                        output: Some(out.to_string_lossy().to_string()),
                    });
                }
                Err(error) => outcomes.push(failed(&account.id, &db.rel, error)),
            }
            continue;
        }

        let Some(entry) = keys::lookup(store, &account.id, &db.rel) else {
            let mut outcome = failed(
                &account.id,
                &db.rel,
                "没有该数据库的密钥。请先用密钥工具提取并导入。",
            );
            outcome.status = "no-key".into();
            outcomes.push(outcome);
            continue;
        };

        if let Some(current_salt) = &db.salt {
            if &entry.salt != current_salt {
                let mut outcome = failed(
                    &account.id,
                    &db.rel,
                    "数据库已重新加密，需重新提取密钥。",
                );
                outcome.status = "no-key".into();
                outcomes.push(outcome);
                continue;
            }
        }

        let enc_key = match hex::decode(&entry.enc_key) {
            Ok(key) if key.len() == crypto::KEY_LEN => key,
            _ => {
                outcomes.push(failed(&account.id, &db.rel, "保存的密钥格式无效。"));
                continue;
            }
        };

        let mut outcome = decrypt_file(db, &out, &enc_key, replay_wal);
        outcome.account = account.id.clone();
        if outcome.status == "ok" {
            let out_size = fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
            meta.entries.insert(
                db.rel.clone(),
                SyncEntry {
                    src_size,
                    src_mtime,
                    out_size,
                    wal_size,
                    wal_mtime,
                    wal_replayed: replay_wal,
                    key_salt: entry.salt.clone(),
                },
            );
        }
        outcomes.push(outcome);
    }

    save_meta(&cache, &meta);
    outcomes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn business_metadata_upgrade_defers_messages_and_preserves_existing_cache() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let data = temp.path().join("data");
        let mut dbs = Vec::new();
        for rel in ["bizchat/bizchat.db", "message/biz_message_0.db", "message/message_0.db"] {
            let path = source.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE fixture (value TEXT); INSERT INTO fixture VALUES ('source');").unwrap();
            drop(conn);
            dbs.push(DbFile { rel: rel.into(), path: path.to_string_lossy().into(), size: fs::metadata(&path).unwrap().len(), encrypted: false, salt: None, has_wal: false });
        }
        let account = Account { id: "business-test".into(), dir: source.to_string_lossy().into(), db_count: dbs.len(), encrypted_count: 0, total_bytes: 0, modified_at: None, dbs };
        let cache = cache_dir(&data, &account.id);
        fs::create_dir_all(cache.join("message")).unwrap();
        let ordinary = cache.join("message/message_0.db");
        fs::write(&ordinary, b"existing ordinary cache").unwrap();

        prepare_business_account(&data, account.clone(), false).unwrap();
        assert!(cache.join("bizchat/bizchat.db").is_file());
        assert!(!cache.join("message/biz_message_0.db").exists());
        prepare_business_account(&data, account, true).unwrap();
        assert!(cache.join("message/biz_message_0.db").is_file());
        assert_eq!(fs::read(&ordinary).unwrap(), b"existing ordinary cache");
        let conn = rusqlite::Connection::open(cache.join("message/biz_message_0.db")).unwrap();
        assert_eq!(conn.query_row("SELECT value FROM fixture", [], |row| row.get::<_, String>(0)).unwrap(), "source");
    }

    #[test]
    fn chat_dbs_exclude_kvdb_sidecars() {
        assert!(is_chat_db("message/message_0.db"));
        assert!(is_chat_db("message/message_fts.db"));
        assert!(is_chat_db("message/message_resource.db"));
        assert!(is_chat_db("session/session.db"));
        assert!(is_chat_db("contact/contact.db"));
        assert!(is_chat_db("head_image/head_image.db"));
        // WeChat 4.x LevelDB sidecars have no derived key and are not chat databases.
        assert!(!is_chat_db("message/message_0.kvdb"));
        assert!(!is_chat_db("message/biz_message_0.kvdb"));
        assert!(!is_chat_db("message/media_0.kvdb"));
        assert!(is_chat_db("message/biz_message_0.db"));
        assert!(is_chat_db("message/biz_media_0.db"));
        assert!(is_chat_db("bizchat/bizchat.db"));
        assert!(!is_chat_db("contact/contact_fts.db"));
    }

    fn db(encrypted: bool) -> DbFile {
        DbFile {
            rel: "message/message_0.db".into(),
            path: "/tmp/message_0.db".into(),
            size: 4096,
            encrypted,
            salt: Some("6161".into()),
            has_wal: false,
        }
    }

    fn cached(key_salt: &str) -> SyncEntry {
        SyncEntry {
            src_size: 4096,
            src_mtime: 1,
            out_size: 4096,
            wal_size: 0,
            wal_mtime: 0,
            wal_replayed: false,
            key_salt: key_salt.into(),
        }
    }

    fn store_with(salt: Option<&str>) -> KeyStore {
        let mut store = KeyStore::default();
        if let Some(salt) = salt {
            keys::store_key(
                &mut store,
                "acct",
                "message/message_0.db",
                &[0u8; crypto::KEY_LEN],
                salt.as_bytes(),
                "test",
            );
        }
        store
    }

    #[test]
    fn cached_copy_is_not_usable_after_keys_are_cleared() {
        // Regression: "清除本账号密钥" looked like a no-op because the cached-copy shortcut kept
        // reporting success for a database whose key no longer existed.
        assert!(!cache_matches_key(&db(true), &cached("aa"), &store_with(None), "acct"));
    }

    #[test]
    fn cached_copy_is_not_usable_after_rekeying() {
        let mut rotated = db(true);
        rotated.salt = Some("6262".into());
        // The old cache and saved key agree, but neither matches the source.
        assert!(!cache_matches_key(&rotated, &cached("6161"), &store_with(Some("aa")), "acct"));
        // Updating the saved key still requires rebuilding the old cache.
        assert!(!cache_matches_key(&rotated, &cached("6161"), &store_with(Some("bb")), "acct"));
    }

    #[test]
    fn changed_salt_bypasses_cache_and_wal_shortcuts() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("message_0.db");
        fs::write(&source, [0u8; crypto::PAGE_SIZE]).unwrap();
        let mut rotated = db(true);
        rotated.path = source.to_string_lossy().into();
        rotated.salt = Some("62".repeat(crypto::SALT_LEN));
        rotated.has_wal = true;
        let account = Account {
            id: "acct".into(), dir: temp.path().to_string_lossy().into(), db_count: 1,
            encrypted_count: 1, total_bytes: rotated.size, modified_at: None, dbs: vec![rotated.clone()],
        };
        let cache = cache_dir(temp.path(), &account.id);
        let out = cache.join(&rotated.rel);
        fs::create_dir_all(out.parent().unwrap()).unwrap();
        fs::write(&out, b"previous snapshot").unwrap();
        fs::write(base_path(&out), b"previous base").unwrap();
        let salt = "a".repeat(crypto::SALT_LEN);
        let mut entry = cached(&"61".repeat(crypto::SALT_LEN));
        entry.src_mtime = mtime_nanos(&source);
        save_meta(&cache, &SyncMeta { entries: BTreeMap::from([(rotated.rel, entry)]) });

        for wal_changed in [false, true] {
            if wal_changed {
                fs::write(wal_path(&account.dbs[0].path), b"new WAL").unwrap();
            }
            let outcome = decrypt_account(temp.path(), &account, &store_with(Some(&salt)), true, true);
            assert_eq!(outcome[0].status, "no-key");
            assert_eq!(fs::read(&out).unwrap(), b"previous snapshot");
        }
    }

    #[test]
    fn cached_copy_is_usable_with_the_same_key() {
        assert!(cache_matches_key(&db(true), &cached("6161"), &store_with(Some("aa")), "acct"));
    }

    #[test]
    fn failed_wal_keeps_last_snapshot_and_can_be_retried() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("message_0.db");
        let salt = "a".repeat(crypto::SALT_LEN);
        let key = [0; crypto::KEY_LEN];
        let page = crypto::authenticated_fixture_page(&key, salt.as_bytes(), 1);
        fs::write(&source, page).unwrap();
        let mut account = Account {
            id: "acct".into(),
            dir: temp.path().to_string_lossy().into(),
            db_count: 1,
            encrypted_count: 1,
            total_bytes: page.len() as u64,
            modified_at: None,
            dbs: vec![DbFile {
                path: source.to_string_lossy().into(),
                salt: Some(hex::encode(salt.as_bytes())),
                has_wal: true,
                ..db(true)
            }],
        };
        let cache = cache_dir(temp.path(), &account.id);
        let out = cache.join(&account.dbs[0].rel);
        let wal = wal_path(&account.dbs[0].path);
        let store = store_with(Some(&salt));
        fs::write(&wal, []).unwrap();
        assert_eq!(decrypt_account(temp.path(), &account, &store, true, true)[0].status, "ok");

        // One committed frame makes the current snapshot newer than its base.
        let mut header = [0; 32];
        header[..4].copy_from_slice(&0x377f_0682u32.to_be_bytes());
        header[8..12].copy_from_slice(&(crypto::PAGE_SIZE as u32).to_be_bytes());
        let mut frame = [0; 24];
        frame[..4].copy_from_slice(&2u32.to_be_bytes());
        frame[4..8].copy_from_slice(&2u32.to_be_bytes());
        let page2 = crypto::authenticated_fixture_page(&key, salt.as_bytes(), 2);
        let committed = [header.as_slice(), frame.as_slice(), page2.as_slice()].concat();
        fs::write(&wal, &committed).unwrap();
        // The WAL signature alone doesn't prove it was replayed. Old metadata has no flag.
        assert_eq!(decrypt_account(temp.path(), &account, &store, false, true)[0].status, "ok");
        let mut legacy: serde_json::Value = serde_json::from_slice(&fs::read(meta_path(&cache)).unwrap()).unwrap();
        legacy["entries"][&account.dbs[0].rel].as_object_mut().unwrap().remove("wal_replayed");
        fs::write(meta_path(&cache), serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(decrypt_account(temp.path(), &account, &store, true, true)[0].status, "ok");
        let previous = fs::read(&out).unwrap();
        assert_eq!(previous.len(), 2 * crypto::PAGE_SIZE);
        let previous_base = fs::read(base_path(&out)).unwrap();
        let previous_meta = fs::read(meta_path(&cache)).unwrap();

        for main_file_changed in [false, true] {
            if main_file_changed {
                fs::write(&wal, b"interrupted header").unwrap();
                fs::write(&source, [page.as_slice(), page2.as_slice()].concat()).unwrap();
                account.dbs[0].size = (2 * crypto::PAGE_SIZE) as u64;
            } else {
                fs::write(&wal, [committed.as_slice(), &frame[..12]].concat()).unwrap();
            }
            let result = decrypt_account(temp.path(), &account, &store, true, true);
            assert_eq!(result[0].status, "failed");
            assert!(result[0].error.as_deref().unwrap().contains("WAL"));
            assert_eq!(fs::read(&out).unwrap(), previous);
            assert_eq!(fs::read(base_path(&out)).unwrap(), previous_base);
            assert_eq!(fs::read(meta_path(&cache)).unwrap(), previous_meta);
        }

        // A valid WAL with no committed frames is a successful refresh.
        fs::write(&wal, header).unwrap();
        assert_eq!(decrypt_account(temp.path(), &account, &store, true, true)[0].status, "ok");
        assert_eq!(decrypt_account(temp.path(), &account, &store, true, true)[0].status, "cached");
    }

    #[test]
    fn entries_written_before_the_salt_was_tracked_force_one_decryption() {
        assert!(!cache_matches_key(&db(true), &cached(""), &store_with(Some("aa")), "acct"));
    }

    #[test]
    fn plaintext_databases_do_not_need_a_key() {
        assert!(cache_matches_key(&db(false), &cached(""), &store_with(None), "acct"));
    }
}
