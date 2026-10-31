//! End-to-end check of the decrypt + query pipeline against a database produced by the
//! independent `sqlcipher` CLI. Skipped when sqlcipher is not installed.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use wechat_devify_bridge_lib::wechat::decrypt;
use wechat_devify_bridge_lib::wechat::discover::{Account, DbFile};
use wechat_devify_bridge_lib::wechat::keys::{self, KeyStore};
use wechat_devify_bridge_lib::wechat::query;

const USERNAME: &str = "wxid_testuser";

fn md5_hex(value: &str) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

fn sqlcipher_available() -> bool {
    Command::new("sqlcipher")
        .arg("-version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn seed(path: &Path, key_hex: &str, statements: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let sql = format!(
        "PRAGMA key = \"x'{key_hex}'\"; PRAGMA cipher_compatibility = 4; PRAGMA cipher_page_size = 4096; {statements}"
    );
    let output = Command::new("sqlcipher")
        .arg(path)
        .arg(sql)
        .output()
        .expect("运行 sqlcipher");
    assert!(
        output.status.success(),
        "sqlcipher 失败：{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn db_file(root: &Path, rel: &str) -> DbFile {
    let path = root.join("db_storage").join(rel);
    let mut header = [0u8; 16];
    use std::io::Read;
    fs::File::open(&path)
        .unwrap()
        .read_exact(&mut header)
        .unwrap();
    let salt = wechat_devify_bridge_lib::wechat::crypto::read_salt(&header).unwrap();
    DbFile {
        rel: rel.to_string(),
        path: path.to_string_lossy().to_string(),
        size: fs::metadata(&path).unwrap().len(),
        encrypted: true,
        salt: Some(hex::encode(salt)),
        has_wal: false,
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("wechat-devify-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn pipeline_decrypts_and_reads_sessions_and_messages() {
    if !sqlcipher_available() {
        eprintln!("跳过：未安装 sqlcipher，无法独立生成加密夹具");
        return;
    }

    let work = temp_dir("pipeline");
    let account_root = work.join("account");
    fs::create_dir_all(account_root.join("db_storage")).unwrap();

    let key_bytes = [0x42u8; 32];
    let salt_bytes = [0x17u8; 16];
    let key_hex = format!("{}{}", hex::encode(key_bytes), hex::encode(salt_bytes));

    seed(
        &account_root.join("db_storage/message/message_0.db"),
        &key_hex,
        &format!(
            "CREATE TABLE Name2Id(user_name TEXT); \
             INSERT INTO Name2Id(rowid, user_name) VALUES (1, '{USERNAME}'), (2, 'wxid_me'); \
             CREATE TABLE \"Msg_{table}\"(local_id INTEGER PRIMARY KEY, local_type INTEGER, \
               real_sender_id INTEGER, create_time INTEGER, message_content TEXT, \
               WCDB_CT_message_content INTEGER); \
             INSERT INTO \"Msg_{table}\" VALUES (1, 1, 2, 1000, '第一条消息', NULL), \
               (2, 1, 1, 2000, '收到，谢谢', 0), \
               (3, 3, 1, 3000, '', NULL);",
            table = md5_hex(USERNAME)
        ),
    );
    seed(
        &account_root.join("db_storage/session/session.db"),
        &key_hex,
        &format!(
            "CREATE TABLE SessionTable(username TEXT, last_timestamp INTEGER); \
             INSERT INTO SessionTable VALUES ('{USERNAME}', 3000), ('wxid_other', 100);"
        ),
    );
    seed(
        &account_root.join("db_storage/contact/contact.db"),
        &key_hex,
        "CREATE TABLE contact(username TEXT, remark TEXT, nick_name TEXT); \
         INSERT INTO contact VALUES ('wxid_testuser', '测试好友', '昵称');",
    );

    let dbs = vec![
        db_file(&account_root, "message/message_0.db"),
        db_file(&account_root, "session/session.db"),
        db_file(&account_root, "contact/contact.db"),
    ];
    for db in &dbs {
        assert!(db.encrypted, "夹具应为加密库：{}", db.rel);
    }

    let account = Account {
        id: "acct_test".into(),
        dir: account_root.to_string_lossy().to_string(),
        db_count: dbs.len(),
        encrypted_count: dbs.len(),
        total_bytes: dbs.iter().map(|d| d.size).sum(),
        modified_at: None,
        dbs,
    };

    let mut store = KeyStore::default();
    for db in &account.dbs {
        let salt = hex::decode(db.salt.as_ref().unwrap()).unwrap();
        keys::store_key(&mut store, &account.id, &db.rel, &key_bytes, &salt, "test");
    }

    let outcomes = decrypt::decrypt_account(&work.join("app-data"), &account, &store, false, false);
    for outcome in &outcomes {
        assert_eq!(
            outcome.status, "ok",
            "解密 {} 失败：{:?}",
            outcome.rel, outcome.error
        );
    }

    let cache = decrypt::cache_dir(&work.join("app-data"), &account.id);
    let sessions = query::sessions(&cache).expect("读取会话");
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].username, USERNAME);
    assert_eq!(sessions[0].display, "测试好友");
    assert_eq!(sessions[0].last_time, Some(3000));

    let messages = query::messages(&cache, USERNAME, 50, None).expect("读取消息");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].time, 3000);
    assert_eq!(messages[0].text, "[图片]");
    assert_eq!(messages[1].text, "收到，谢谢");
    assert_eq!(messages[1].sender.as_deref(), Some(USERNAME));
    assert_eq!(messages[2].text, "第一条消息");
    assert_eq!(messages[2].sender.as_deref(), Some("wxid_me"));

    // Re-running must hit the cache instead of decrypting again.
    let again = decrypt::decrypt_account(&work.join("app-data"), &account, &store, false, false);
    assert!(again.iter().all(|o| o.status == "cached"));

    // A wrong key must fail on page 1 and must not leave a partial file behind.
    let mut wrong = KeyStore::default();
    for db in &account.dbs {
        let salt = hex::decode(db.salt.as_ref().unwrap()).unwrap();
        keys::store_key(
            &mut wrong,
            &account.id,
            &db.rel,
            &[0x11u8; 32],
            &salt,
            "wrong",
        );
    }
    let bad_dir = work.join("bad-data");
    let bad = decrypt::decrypt_account(&bad_dir, &account, &wrong, false, false);
    assert!(bad.iter().all(|o| o.status == "failed"));
    assert!(bad.iter().all(|o| o.failed_page == Some(1)));

    let _ = fs::remove_dir_all(&work);
}

/// A passphrase-shaped fixture: SQLCipher derives the per-database key from the password
/// with PBKDF2-HMAC-SHA512 (256000 rounds) exactly like WeChat 4.x does.
fn seed_with_passphrase(path: &Path, passphrase: &str, statements: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    assert!(!passphrase.contains('\''), "夹具口令不能包含单引号");
    let sql = format!(
        "PRAGMA key = '{passphrase}'; PRAGMA cipher_compatibility = 4; PRAGMA cipher_page_size = 4096; {statements}"
    );
    let output = Command::new("sqlcipher")
        .arg(path)
        .arg(sql)
        .output()
        .expect("运行 sqlcipher");
    assert!(
        output.status.success(),
        "sqlcipher 失败：{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// 32 ASCII bytes, so SQLCipher's text password and our hex-decoded passphrase agree.
const PASS_PHRASE: &str = "0123456789abcdef0123456789abcdef";

#[test]
fn passphrase_candidate_derives_per_database_keys() {
    if !sqlcipher_available() {
        eprintln!("跳过：未安装 sqlcipher，无法独立生成加密夹具");
        return;
    }

    let work = temp_dir("passphrase");
    let root = work.join("account");
    let statements = "CREATE TABLE t(v TEXT); INSERT INTO t VALUES ('hello');";
    seed_with_passphrase(
        &root.join("db_storage/message/message_0.db"),
        PASS_PHRASE,
        statements,
    );
    seed_with_passphrase(
        &root.join("db_storage/session/session.db"),
        PASS_PHRASE,
        statements,
    );
    seed_with_passphrase(
        &root.join("db_storage/contact/contact.db"),
        PASS_PHRASE,
        statements,
    );

    let hex_passphrase = hex::encode(PASS_PHRASE.as_bytes());
    let imported =
        keys::parse_candidates(&format!("{{\"passphrase\":\"{hex_passphrase}\"}}")).unwrap();
    assert_eq!(imported.len(), 1);
    assert!(imported[0].passphrase, "JSON passphrase 应被标记为口令候选");

    // A bare 64-hex value is ambiguous, so both interpretations are offered.
    let ambiguous = keys::parse_candidates(&hex_passphrase).unwrap();
    assert_eq!(
        ambiguous.len(),
        2,
        "裸 64 位十六进制应同时提供密钥与口令两种候选"
    );
    assert!(ambiguous.iter().any(|c| c.passphrase));
    assert!(ambiguous.iter().any(|c| !c.passphrase));

    let rels = [
        "message/message_0.db",
        "session/session.db",
        "contact/contact.db",
    ];
    let mut store = KeyStore::default();
    use std::io::Read;
    for rel in rels {
        let path = root.join("db_storage").join(rel);
        let mut header = [0u8; wechat_devify_bridge_lib::wechat::crypto::PAGE_SIZE];
        fs::File::open(&path)
            .unwrap()
            .read_exact(&mut header)
            .unwrap();
        let (key, label) = keys::resolve_db_key(&header, &imported)
            .unwrap_or_else(|| panic!("{rel} 的口令未通过第 1 页 HMAC 校验"));
        assert!(label.contains("口令"));
        assert_ne!(
            key,
            PASS_PHRASE.as_bytes().to_vec(),
            "落库的应是派生后的库密钥"
        );
        let salt = wechat_devify_bridge_lib::wechat::crypto::read_salt(&header).unwrap();
        keys::store_key(&mut store, "acct_phrase", rel, &key, &salt, &label);
    }

    // A wrong passphrase must not unlock anything.
    let wrong = keys::parse_candidates(&format!(
        "{{\"passphrase\":\"{}\"}}",
        hex::encode([0x00u8; 32])
    ))
    .unwrap();
    let path = root.join("db_storage/session/session.db");
    let mut header = [0u8; wechat_devify_bridge_lib::wechat::crypto::PAGE_SIZE];
    fs::File::open(&path)
        .unwrap()
        .read_exact(&mut header)
        .unwrap();
    assert!(keys::resolve_db_key(&header, &wrong).is_none());

    let dbs: Vec<DbFile> = rels.iter().map(|rel| db_file(&root, rel)).collect();
    let account = Account {
        id: "acct_phrase".into(),
        dir: root.to_string_lossy().to_string(),
        db_count: dbs.len(),
        encrypted_count: dbs.len(),
        total_bytes: dbs.iter().map(|d| d.size).sum(),
        modified_at: None,
        dbs,
    };
    let outcomes = decrypt::decrypt_account(&work.join("app-data"), &account, &store, false, false);
    for outcome in &outcomes {
        assert_eq!(
            outcome.status, "ok",
            "解密 {} 失败：{:?}",
            outcome.rel, outcome.error
        );
    }
    // The decrypted copy must actually be readable SQLite with our table in it.
    let cache = decrypt::cache_dir(&work.join("app-data"), &account.id);
    let conn = rusqlite::Connection::open_with_flags(
        cache.join("session").join("session.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let value: String = conn
        .query_row("SELECT v FROM t", [], |row| row.get(0))
        .unwrap();
    assert_eq!(value, "hello");

    let _ = fs::remove_dir_all(&work);
}

const GROUP: &str = "88888888@chatroom";
const TEAMMATE: &str = "wxid_teammate";

fn zstd_hex(text: &str) -> String {
    hex::encode(zstd::encode_all(text.as_bytes(), 3).unwrap())
}

#[test]
fn groups_search_and_digest_work_on_group_chats() {
    if !sqlcipher_available() {
        eprintln!("跳过：未安装 sqlcipher，无法独立生成加密夹具");
        return;
    }

    let work = temp_dir("groups");
    let root = work.join("account");
    fs::create_dir_all(root.join("db_storage")).unwrap();

    let key_bytes = [0x5au8; 32];
    let salt_bytes = [0x2bu8; 16];
    let key_hex = format!("{}{}", hex::encode(key_bytes), hex::encode(salt_bytes));

    let compressed = zstd_hex("wxid_alice:\n预算复核结果：完成率 82%");
    seed(
        &root.join("db_storage/message/message_0.db"),
        &key_hex,
        &format!(
            "CREATE TABLE Name2Id(user_name TEXT); \
             INSERT INTO Name2Id(rowid, user_name) VALUES (1, '{GROUP}'), (2, 'wxid_alice'), (3, 'wxid_me'); \
             CREATE TABLE \"Msg_{group_table}\"(local_id INTEGER PRIMARY KEY, local_type INTEGER, \
               real_sender_id INTEGER, create_time INTEGER, message_content TEXT, \
               WCDB_CT_message_content INTEGER); \
             INSERT INTO \"Msg_{group_table}\" VALUES \
               (1, 1, 2, 1700000100, 'wxid_alice:
请大家今天下班前提交周报', NULL), \
               (2, 1, 3, 1700000200, '决定：下周一上线新版本', 0), \
               (3, 1, 2, 1700000300, 'wxid_alice:
支付接口一直在报错，还没解决', NULL), \
               (4, 1, 3, 1700000400, '这个方案由谁来确认？', NULL), \
               (5, 1, 2, 1700000500, 'wxid_alice:
预算表已更新：https://example.com/budget', NULL), \
               (6, 1, 2, 1700000600, x'{compressed}', 4); \
             CREATE TABLE \"Msg_{dm_table}\"(local_id INTEGER PRIMARY KEY, local_type INTEGER, \
               real_sender_id INTEGER, create_time INTEGER, message_content TEXT, \
               WCDB_CT_message_content INTEGER); \
             INSERT INTO \"Msg_{dm_table}\" VALUES \
               (1, 1, 2, 1700000250, '我这边的预算也发你了', 0), \
               (2, 1, 3, 1700000350, '收到，辛苦', 0), \
               (3, 1, 3, 1700000360, '明早九点碰一下', 0);",
            group_table = md5_hex(GROUP),
            dm_table = md5_hex(TEAMMATE)
        ),
    );
    seed(
        &root.join("db_storage/session/session.db"),
        &key_hex,
        &format!(
            "CREATE TABLE SessionTable(username TEXT, last_timestamp INTEGER, unread_count INTEGER, summary TEXT); \
             INSERT INTO SessionTable VALUES \
               ('{GROUP}', 1700000600, 3, '这周进度同步一下'), \
               ('{TEAMMATE}', 1700000350, 0, '收到，辛苦');"
        ),
    );
    seed(
        &root.join("db_storage/contact/contact.db"),
        &key_hex,
        &format!(
            "CREATE TABLE contact(username TEXT, alias TEXT, remark TEXT, nick_name TEXT); \
             INSERT INTO contact VALUES ('{GROUP}', '', '项目攻坚组', ''), ('{TEAMMATE}', '', '同事小王', '小王'), ('wxid_alice', '', '爱丽丝', 'Alice'); \
             CREATE TABLE chat_room(id INTEGER, username TEXT, owner TEXT, ext_buffer BLOB); \
             INSERT INTO chat_room VALUES (1, '{GROUP}', 'wxid_me', NULL); \
             CREATE TABLE chatroom_member(room_id INTEGER, member_id TEXT); \
             INSERT INTO chatroom_member VALUES (1, 'wxid_me'), (1, 'wxid_alice'), (1, 'wxid_bob');"
        ),
    );

    let dbs = vec![
        db_file(&root, "message/message_0.db"),
        db_file(&root, "session/session.db"),
        db_file(&root, "contact/contact.db"),
    ];
    let account = Account {
        id: "acct_group".into(),
        dir: root.to_string_lossy().to_string(),
        db_count: dbs.len(),
        encrypted_count: dbs.len(),
        total_bytes: dbs.iter().map(|d| d.size).sum(),
        modified_at: None,
        dbs,
    };
    let mut store = KeyStore::default();
    for db in &account.dbs {
        let salt = hex::decode(db.salt.as_ref().unwrap()).unwrap();
        keys::store_key(&mut store, &account.id, &db.rel, &key_bytes, &salt, "test");
    }
    let outcomes = decrypt::decrypt_account(&work.join("app-data"), &account, &store, false, false);
    for outcome in &outcomes {
        assert_eq!(
            outcome.status, "ok",
            "解密 {} 失败：{:?}",
            outcome.rel, outcome.error
        );
    }
    let cache = decrypt::cache_dir(&work.join("app-data"), &account.id);

    // ── groups are identified, named, sized and previewed ──────────────────────────
    let sessions = query::sessions(&cache).expect("读取会话");
    assert_eq!(sessions.len(), 2);
    let group = &sessions[0];
    assert_eq!(group.username, GROUP);
    assert!(group.is_group);
    assert_eq!(group.display, "项目攻坚组");
    assert_eq!(group.member_count, Some(3));
    assert_eq!(group.preview.as_deref(), Some("这周进度同步一下"));
    assert_eq!(group.unread, Some(3));
    let dm = &sessions[1];
    assert!(!dm.is_group);
    assert_eq!(dm.display, "同事小王");
    assert_eq!(dm.member_count, None);
    assert_eq!(dm.unread, None);

    // The allowlist intentionally supports both group and one-to-one conversations.
    let allowed: HashSet<String> = [GROUP, TEAMMATE].into_iter().map(str::to_string).collect();
    let filtered = query::sessions_filtered(&cache, Some(&allowed)).expect("读取白名单会话");
    assert_eq!(filtered.len(), 2);
    let only_group: HashSet<String> = [GROUP].into_iter().map(str::to_string).collect();
    let filtered_group =
        query::sessions_filtered(&cache, Some(&only_group)).expect("读取群聊白名单");
    assert_eq!(filtered_group.len(), 1);
    assert!(filtered_group[0].is_group);
    let empty = HashSet::new();
    assert!(query::sessions_filtered(&cache, Some(&empty))
        .expect("读取空白名单")
        .is_empty());

    // ── messages decode plain and zstd-compressed content ─────────────────────────
    let messages = query::messages(&cache, GROUP, 50, None).expect("读取群消息");
    assert_eq!(messages.len(), 6);
    assert_eq!(messages[0].time, 1700000600);
    assert_eq!(
        messages[0].text, "预算复核结果：完成率 82%",
        "zstd 压缩内容必须解压，且群聊的 sender 前缀要被剥离"
    );
    assert!(
        messages.iter().all(|m| !m.text.starts_with("wxid_alice:")),
        "群聊消息正文不应残留 sender 前缀"
    );
    assert_eq!(messages[0].sender.as_deref(), Some("wxid_alice"));
    assert_eq!(messages[0].sender_name.as_deref(), Some("爱丽丝"));

    // ── keyword search covers both plain and compressed messages ──────────────────
    let report = query::search(&cache, "预算", false, 10).expect("搜索");
    assert_eq!(report.keyword, "预算");
    assert_eq!(report.scanned, 2);
    assert_eq!(report.skipped, 0);
    assert_eq!(report.hits.len(), 2);
    let group_hit = report.hits.iter().find(|h| h.username == GROUP).unwrap();
    assert!(group_hit.is_group);
    assert_eq!(group_hit.hits, 2, "明文与压缩内容中的关键字都应命中");
    assert!(!group_hit.snippets.is_empty());
    assert!(group_hit.snippets.iter().all(|s| s.text.contains("预算")));
    assert!(group_hit
        .snippets
        .iter()
        .all(|s| !s.text.contains("wxid_alice:")));
    let dm_hit = report.hits.iter().find(|h| h.username == TEAMMATE).unwrap();
    assert_eq!(dm_hit.hits, 1);

    // Own messages are flagged so the viewer can right-align them.
    assert!(messages.iter().any(|m| m.is_self), "应识别出本人发送的消息");
    assert!(messages
        .iter()
        .filter(|m| m.is_self)
        .all(|m| m.sender.as_deref() == Some("wxid_me")));
    assert!(messages
        .iter()
        .filter(|m| !m.is_self)
        .all(|m| m.sender.as_deref() != Some("wxid_me")));

    let groups_only = query::search(&cache, "预算", true, 10).expect("只搜群聊");
    assert!(groups_only.hits.iter().all(|h| h.is_group));
    assert_eq!(groups_only.hits.len(), 1);

    // The bound is reported honestly instead of silently truncating.
    let bounded = query::search(&cache, "预算", false, 1).expect("受限搜索");
    assert_eq!(bounded.scanned, 1);
    assert_eq!(bounded.skipped, 1);
    assert!(query::search(&cache, "   ", false, 10).is_err());

    // ── digest groups messages into work categories ───────────────────────────────
    let digest = query::digest(
        &cache,
        &[GROUP.to_string(), TEAMMATE.to_string()],
        None,
        None,
    )
    .expect("整理工作内容");
    assert_eq!(digest.sessions, 2);
    assert_eq!(digest.messages, 9);
    assert!(digest.todo >= 1, "应识别出待办：{digest:?}");
    assert!(digest.decisions >= 1, "应识别出决定：{digest:?}");
    assert!(digest.issues >= 1, "应识别出问题：{digest:?}");
    assert!(digest.questions >= 1, "应识别出待确认问题：{digest:?}");
    assert!(digest.data >= 2, "应识别出数据/链接：{digest:?}");
    for section in [
        "# 工作整理",
        "## 待办与跟进",
        "## 结论与决定",
        "## 问题与风险",
        "## 参与人",
        "关键词与结构规则",
    ] {
        assert!(digest.markdown.contains(section), "缺少 {section}");
    }
    assert!(digest.markdown.contains("项目攻坚组"));

    // Date range narrows the digest to the requested window.
    let windowed = query::digest(&cache, &[GROUP.to_string()], Some(1_700_000_450), None).unwrap();
    assert_eq!(windowed.messages, 2);
    assert_eq!(windowed.sessions, 1);

    let empty = query::digest(&cache, &[], None, None);
    assert!(empty.is_err(), "没有选择会话时必须报错，不能返回空报告");

    let _ = fs::remove_dir_all(&work);
}
