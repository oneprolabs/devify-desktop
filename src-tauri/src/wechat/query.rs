//! Read sessions and messages out of the decrypted cache.
//!
//! The WeChat schema is version-dependent, so every column is discovered with
//! `PRAGMA table_info` instead of being assumed. When a required column is missing the
//! caller gets an explicit error instead of fabricated data.

use md5::{Digest, Md5};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{Local, TimeZone};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SessionItem {
    pub username: String,
    pub display: String,
    pub last_time: Option<i64>,
    pub is_group: bool,
    pub member_count: Option<i64>,
    pub preview: Option<String>,
    pub unread: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub community: Option<CommunityInfo>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CommunityInfo {
    pub id: String,
    pub name: String,
    pub is_parent: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MessageItem {
    pub local_id: i64,
    #[serde(default)]
    pub server_id: i64,
    #[serde(default)]
    pub source_shard: String,
    pub time: i64,
    pub sender: Option<String>,
    pub sender_name: Option<String>,
    pub kind: i64,
    pub text: String,
    /// True when this message was sent by the account being read (right-aligned in the UI).
    pub is_self: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_reader_preserves_cross_shard_boundaries_and_missing_chats() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("message")).unwrap();
        for shard in 0..2 {
            let conn = Connection::open(dir.path().join(format!("message/message_{shard}.db"))).unwrap();
            let table = format!("Msg_{}", md5_hex("person"));
            conn.execute_batch(&format!("CREATE TABLE {table}(create_time INTEGER, local_id INTEGER); INSERT INTO {table} VALUES(10,1),(20,2),(20,{});", 3 + shard)).unwrap();
            let other = format!("Msg_{}", md5_hex("other"));
            conn.execute_batch(&format!("CREATE TABLE {other}(timestamp INTEGER, id INTEGER); INSERT INTO {other} VALUES(30,8);")).unwrap();
        }
        let reader = UploadStateReader::new(dir.path());
        assert_eq!(reader.latest_position("person").unwrap(), Some((20,4)));
        assert_eq!(reader.count_at_time("person",20).unwrap(),4);
        assert_eq!(reader.count_after("person",20,2).unwrap(),2);
        assert_eq!(reader.latest_position("other").unwrap(),Some((30,8)));
        assert_eq!(reader.count_after("other",30,8).unwrap(),0);
        assert!(reader.latest_position("missing").is_err());
        for name in ["person", "other"] {
            assert_eq!(reader.latest_position(name).unwrap(), latest_position(dir.path(),name).unwrap());
            assert_eq!(reader.count_at_time(name,20).unwrap(),count_at_time(dir.path(),name,20).unwrap());
            assert_eq!(reader.count_after(name,20,2).unwrap(),count_after(dir.path(),name,20,2).unwrap());
        }
    }

    #[test]
    fn account_profile_matches_exact_and_suffixed_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("contact")).unwrap();
        let conn = Connection::open(dir.path().join("contact/contact.db")).unwrap();
        conn.execute_batch("CREATE TABLE contact (username TEXT, nick_name TEXT, alias TEXT); INSERT INTO contact VALUES ('wxid_person', 'Name', 'custom-id'), ('wxid_person_abcd', 'Exact', NULL);").unwrap();
        assert_eq!(account_profile(dir.path(), "wxid_person_abcd").unwrap().nickname.as_deref(), Some("Exact"));
        let profile = account_profile(dir.path(), "wxid_person_12af").unwrap();
        assert_eq!(profile.alias.as_deref(), Some("custom-id"));
        assert!(account_profile(dir.path(), "wxid_person_other").is_none());
        assert!(account_profile(dir.path(), "wxid_missing_12af").is_none());
    }

    #[test]
    fn message_dates_counts_records_across_shards() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("message")).unwrap();
        let name = "wxid_test";
        let table = format!("Msg_{}", md5_hex(name));
        for n in 0..2 {
            let conn = Connection::open(dir.path().join(format!("message/message_{n}.db"))).unwrap();
            conn.execute_batch(&format!("CREATE TABLE \"{table}\" (create_time INTEGER); INSERT INTO \"{table}\" VALUES (1793923200); ")).unwrap();
        }
        let days = message_dates(dir.path(), name).unwrap();
        assert_eq!(days.iter().map(|(_, count)| count).sum::<usize>(), 2);
        assert_eq!(days.len(), 1);
    }

    #[test]
    fn group_prefix_is_stripped_only_when_username_shaped() {
        let (who, body) = split_sender_prefix("wxid_abc123:\n早上的会议改到下午".to_string());
        assert_eq!(who.as_deref(), Some("wxid_abc123"));
        assert_eq!(body, "早上的会议改到下午");

        let (who, body) = split_sender_prefix("46316584746@chatroom:\r\n订单已发货".to_string());
        assert_eq!(who.as_deref(), Some("46316584746@chatroom"));
        assert_eq!(body, "订单已发货");

        // Ordinary text must not be mangled.
        let (who, body) = split_sender_prefix("决定：下周一上线\n第二个议题".to_string());
        assert_eq!(who, None);
        assert_eq!(body, "决定：下周一上线\n第二个议题");

        let (who, body) = split_sender_prefix("请看这段话:\n我们再讨论".to_string());
        assert_eq!(who, None, "带空格的自然语言前缀不应被当作发送者");
        assert_eq!(body, "请看这段话:\n我们再讨论");
    }

    #[test]
    fn only_personal_and_group_sessions_are_visible() {
        assert!(is_visible_chat_username("wxid_colleague_123"));
        assert!(is_visible_chat_username("123456789@chatroom"));

        assert!(!is_visible_chat_username("gh_company_service"));
        assert!(!is_visible_chat_username("filehelper"));
        assert!(!is_visible_chat_username("@placeholder_foldgroup"));
        // 服务号 / 订阅号 aggregate buckets and the WeChat service notices.
        assert!(!is_visible_chat_username("brandservicesessionholder"));
        assert!(!is_visible_chat_username("brandsessionholder"));
        assert!(!is_visible_chat_username("notifymessage"));
        assert!(!is_visible_chat_username("  "));
    }

    #[test]
    fn official_account_registry_hides_legacy_service_ids() {
        let root = std::env::temp_dir().join(format!("wc-query-test-{}", std::process::id()));
        let contact = root.join("contact");
        std::fs::create_dir_all(&contact).unwrap();
        {
            let conn = Connection::open(contact.join("contact.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE biz_info (username TEXT);
                 INSERT INTO biz_info VALUES ('cmb4008205555'), ('qqsafe'), ('gh_registered');",
            )
            .unwrap();
        }

        // Legacy 服务号 keep their own id, so the `gh_` prefix rule alone misses them.
        assert!(!is_chat_session(&root, "cmb4008205555"));
        assert!(!is_chat_session(&root, "qqsafe"));
        // Real conversations are untouched, and a missing contact.db degrades to the rule.
        assert!(is_chat_session(&root, "wxid_real_person"));
        assert!(is_chat_session(&root, "123456789@chatroom"));
        assert!(is_chat_session(&root.join("missing"), "wxid_real_person"));

        let _ = std::fs::remove_dir_all(&root);
    }

    fn summary_message(username: &str, time: i64, text: &str) -> SummaryMessage {
        SummaryMessage {
            username: username.to_string(),
            display: format!("会话-{username}"),
            is_group: username.ends_with("@chatroom"),
            time,
            sender: "张三".to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn summary_input_groups_by_session_and_sorts_by_time() {
        let input = summarize_messages(vec![
            summary_message("b@chatroom", 30, "后一条"),
            summary_message("a", 20, "第二条"),
            summary_message("a", 10, "第一条"),
            summary_message("b@chatroom", 10, "前一条"),
        ]);
        assert_eq!(input.sessions, 2);
        assert_eq!(input.messages, 4);
        assert!(!input.truncated);
        assert!(input.transcript.contains("【会话】会话-a（2 条）"));
        assert!(input
            .transcript
            .contains("【会话】会话-b@chatroom（群聊）（2 条）"));
        let first_a = input.transcript.find("第一条").unwrap();
        let second_a = input.transcript.find("第二条").unwrap();
        assert!(first_a < second_a, "同一会话内必须按时间升序");
    }

    #[test]
    fn summary_input_marks_truncation_instead_of_silently_dropping() {
        let messages: Vec<SummaryMessage> = (0..SUMMARY_MAX_MESSAGES_PER_SESSION + 10)
            .map(|index| summary_message("a", index as i64, &format!("消息 {index}")))
            .collect();
        let input = summarize_messages(messages);
        assert!(input.truncated);
        assert_eq!(input.messages, SUMMARY_MAX_MESSAGES_PER_SESSION);
        assert!(!input
            .transcript
            .contains(&format!("消息 {}", SUMMARY_MAX_MESSAGES_PER_SESSION)));
    }

    #[test]
    fn summary_input_ignores_empty_messages() {
        let input = summarize_messages(vec![summary_message("a", 1, "   ")]);
        assert_eq!(input.messages, 0);
        assert_eq!(input.sessions, 0);
        assert!(input.transcript.is_empty());
    }

    #[test]
    fn rich_messages_are_labelled_instead_of_showing_xml() {
        // Images embed a tall `<msg><img>…` blob: the viewer must only see the label.
        let img = prepare_text(
            3,
            Some(b"<msg><img aeskey=\"x\" length=\"120\"><live><duration>0</duration></live></img></msg>".to_vec()),
            None,
            false,
            None,
        );
        assert_eq!(img.1, "[图片]");

        // Links keep their title and description, prefixed with the app type.
        let (_, link) = prepare_text(
            49,
            Some(
                "<msg><appmsg><title>季度复盘</title><des>请看这份文档</des><type>5</type></appmsg></msg>"
                    .as_bytes()
                    .to_vec(),
            ),
            None,
            false,
            None,
        );
        assert_eq!(link, "[链接] 季度复盘（请看这份文档）");

        // Files without a description still get a label from `<type>`.
        let (_, file) = prepare_text(
            49,
            Some(
                "<msg><appmsg><title>预算.xlsx</title><type>6</type></appmsg></msg>"
                    .as_bytes()
                    .to_vec(),
            ),
            None,
            false,
            None,
        );
        assert_eq!(file, "[文件] 预算.xlsx");

        // Large payloads arrive zstd-compressed; they must be labelled after decompression.
        let packed = zstd::encode_all(
            "<msg><appmsg><title>运营看板</title><des /><type>6</type></appmsg></msg>".as_bytes(),
            3,
        )
        .unwrap();
        let (_, board) = prepare_text(49, Some(packed), Some(4), false, None);
        assert_eq!(board, "[文件] 运营看板");

        // Real group payloads look like `<wxid>:\n<?xml …`: the sender prefix sits in
        // front of the markup, so it has to be stripped before the label is chosen.
        let prefixed = zstd::encode_all(
            "wxid_dgi01nkugygc21:\n<?xml version=\"1.0\"?>\n<msg><img aeskey=\"a\" length=\"9\" /></msg>"
                .as_bytes(),
            3,
        )
        .unwrap();
        let (sender, image) = prepare_text(3, Some(prefixed), Some(4), true, None);
        assert_eq!(sender.as_deref(), Some("wxid_dgi01nkugygc21"));
        assert_eq!(
            image, "[图片]",
            "群消息的发送者前缀必须先剥离，否则 XML 会被当成正文"
        );

        // The stored sender id wins over the inline prefix.
        let (sender, _) = prepare_text(
            1,
            Some("wxid_prefix:\n正文".as_bytes().to_vec()),
            None,
            true,
            Some("wxid_column".to_string()),
        );
        assert_eq!(sender.as_deref(), Some("wxid_column"));
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SearchSnippet {
    pub time: i64,
    pub sender: Option<String>,
    pub sender_name: Option<String>,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SearchHit {
    pub username: String,
    pub display: String,
    pub is_group: bool,
    pub hits: i64,
    pub truncated: bool,
    pub last_time: Option<i64>,
    pub snippets: Vec<SearchSnippet>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SearchReport {
    pub keyword: String,
    pub scanned: usize,
    pub skipped: usize,
    pub hits: Vec<SearchHit>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DigestReport {
    pub markdown: String,
    pub sessions: usize,
    pub messages: usize,
    pub todo: usize,
    pub decisions: usize,
    pub issues: usize,
    pub questions: usize,
    pub data: usize,
}

/// Plain-text material for an external agent, plus the numbers needed to tell the user
/// (and the prompt) exactly how much of the conversation was included.
#[derive(Serialize, Clone, Debug, Default)]
pub struct SummaryInput {
    pub transcript: String,
    pub sessions: usize,
    pub messages: usize,
    pub characters: usize,
    pub truncated: bool,
}

fn open_readonly(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("无法打开数据库 {}：{e}", path.to_string_lossy()))
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn tables(conn: &Connection) -> Vec<String> {
    let mut stmt = match conn.prepare("SELECT name FROM sqlite_master WHERE type='table'") {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([], |row| row.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

fn columns(conn: &Connection, table: &str) -> Vec<String> {
    let sql = format!("PRAGMA table_info({})", quote(table));
    let mut stmt = match conn.prepare(&sql) {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([], |row| row.get::<_, String>(1))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

fn pick<'a>(columns: &'a [String], names: &[&str]) -> Option<&'a str> {
    for name in names {
        if let Some(found) = columns.iter().find(|c| c.eq_ignore_ascii_case(name)) {
            return Some(found.as_str());
        }
    }
    None
}

fn table_like(tables: &[String], needle: &str) -> Option<String> {
    tables
        .iter()
        .find(|t| t.to_lowercase().contains(&needle.to_lowercase()))
        .cloned()
}

fn table_exact(tables: &[String], name: &str) -> Option<String> {
    tables
        .iter()
        .find(|t| t.eq_ignore_ascii_case(name))
        .cloned()
}

/// Read a column as raw bytes.
///
/// Compressed message content is stored as a BLOB in some WCDB builds and as TEXT in
/// others. Converting to `String` first would mangle non-UTF-8 bytes, so zstd decoding
/// always starts from the raw bytes.
fn value_to_bytes(value: rusqlite::types::ValueRef<'_>) -> Option<Vec<u8>> {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => None,
        ValueRef::Integer(i) => Some(i.to_string().into_bytes()),
        ValueRef::Real(f) => Some(f.to_string().into_bytes()),
        ValueRef::Text(bytes) => Some(bytes.to_vec()),
        ValueRef::Blob(bytes) => Some(bytes.to_vec()),
    }
}

pub fn is_group_username(username: &str) -> bool {
    username.ends_with("@chatroom")
}

#[derive(Serialize)]
pub struct AccountProfile {
    pub username: String,
    pub nickname: Option<String>,
    pub alias: Option<String>,
}

/// The local folder may append four hexadecimal characters to the WeChat username.
pub fn account_profile(cache: &Path, account: &str) -> Option<AccountProfile> {
    let conn = open_readonly(&cache.join("contact").join("contact.db")).ok()?;
    let table = table_like(&tables(&conn), "contact")?;
    let cols = columns(&conn, &table);
    let username = pick(&cols, &["username", "user_name"])?;
    let nickname = pick(&cols, &["nick_name", "nickname"]);
    let alias = pick(&cols, &["alias"]);
    let sql = format!("SELECT {}, {}, {} FROM {} WHERE {} = ?1 LIMIT 1",
        quote(username), nickname.map(quote).unwrap_or_else(|| "NULL".into()),
        alias.map(quote).unwrap_or_else(|| "NULL".into()), quote(&table), quote(username));
    let lookup = |candidate: &str| conn.query_row(&sql, [candidate], |row| {
        Ok(AccountProfile { username: row.get(0)?, nickname: row.get(1)?, alias: row.get(2)? })
    }).ok();
    lookup(account).or_else(|| {
        let (name, suffix) = account.rsplit_once('_')?;
        if !name.starts_with("wxid_") || suffix.len() != 4 || !suffix.bytes().all(|c| c.is_ascii_hexdigit()) { return None; }
        lookup(name)
    })
}

/// Load `username -> display name` from contact.db when it is available.
fn string_list(values: impl Iterator<Item=String>) -> String {values.map(|v|format!("'{}'",v.replace("'","''"))).collect::<Vec<_>>().join(",")}
fn contacts(cache: &Path) -> HashMap<String, String> { contacts_for(cache,None) }
fn contacts_for(cache: &Path, wanted: Option<&HashSet<String>>) -> HashMap<String,String> {
    let path = cache.join("contact").join("contact.db");
    let mut map = HashMap::new();
    let Ok(conn) = open_readonly(&path) else {
        return map;
    };
    let Some(table) = table_like(&tables(&conn), "contact") else {
        return map;
    };
    let cols = columns(&conn, &table);
    let Some(username) = pick(&cols, &["username", "user_name"]) else {
        return map;
    };
    let name_cols: Vec<&str> = ["remark", "nick_name", "nickname", "alias"]
        .iter()
        .copied()
        .filter(|c| cols.iter().any(|col| col.eq_ignore_ascii_case(c)))
        .collect();
    let sql = format!(
        "SELECT {}, {} FROM {}",
        quote(username),
        name_cols
            .iter()
            .map(|c| quote(c))
            .collect::<Vec<_>>()
            .join(", "),
        quote(&table)
    );
    let sql=if let Some(wanted)=wanted {format!("{sql} WHERE {} IN ({})",quote(username),string_list(wanted.iter().cloned()))} else {sql};
    if name_cols.is_empty() {
        return map;
    }
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return map;
    };
    let Ok(rows) = stmt.query_map([], |row| {
        let user: String = row.get(0)?;
        let mut best: Option<String> = None;
        for idx in 0..name_cols.len() {
            if let Ok(Some(value)) = row.get::<_, Option<String>>(idx + 1) {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    best = Some(trimmed.to_string());
                    break;
                }
            }
        }
        Ok((user, best))
    }) else {
        return map;
    };
    for (user, name) in rows.flatten() {
        if let Some(name) = name {
            map.insert(user, name);
        }
    }
    map
}

/// Usernames registered as official accounts (公众号 / 服务号) in `contact.db`.
///
/// Most of them use the `gh_` namespace, which the username rule already rejects, but
/// older accounts keep their original id (`cmb4008205555`, `qqsafe`, `weixin`) and the
/// 服务号 aggregate itself is `brandservicesessionholder`. `biz_info` is WeChat's own
/// registry for these accounts, so it is the reliable signal; when the table is missing
/// the caller simply falls back to the username rule.
fn official_accounts(cache: &Path) -> HashSet<String> {
    let path = cache.join("contact").join("contact.db");
    let mut set = HashSet::new();
    let Ok(conn) = open_readonly(&path) else {
        return set;
    };
    let available = tables(&conn);
    let Some(table) =
        table_exact(&available, "biz_info").or_else(|| table_like(&available, "biz_info"))
    else {
        return set;
    };
    let cols = columns(&conn, &table);
    let Some(username) = pick(&cols, &["username", "user_name"]) else {
        return set;
    };
    let sql = format!("SELECT {} FROM {}", quote(username), quote(&table));
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return set;
    };
    if let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) {
        for user in rows.flatten() {
            set.insert(user);
        }
    }
    set
}

/// `group username -> member count`, from `chat_room` joined with `chatroom_member`.
///
/// The join is schema-dependent, so it is best-effort: any missing table or column
/// returns an empty map and the UI simply shows no member count.
fn group_member_counts_for(cache:&Path,wanted:Option<&HashSet<String>>)->HashMap<String,i64>{
    let path = cache.join("contact").join("contact.db");
    let mut map = HashMap::new();
    let Ok(conn) = open_readonly(&path) else {
        return map;
    };
    let available = tables(&conn);
    let Some(room) =
        table_exact(&available, "chat_room").or_else(|| table_like(&available, "chat_room"))
    else {
        return map;
    };
    let Some(member) = table_exact(&available, "chatroom_member")
        .or_else(|| table_like(&available, "chatroom_member"))
    else {
        return map;
    };
    let room_cols = columns(&conn, &room);
    let member_cols = columns(&conn, &member);
    let (Some(id), Some(username)) = (pick(&room_cols, &["id"]), pick(&room_cols, &["username"]))
    else {
        return map;
    };
    let Some(room_id) = pick(&member_cols, &["room_id"]) else {
        return map;
    };
    let sql = format!(
        "SELECT r.{}, COUNT(*) FROM {} m JOIN {} r ON r.{} = m.{} GROUP BY r.{}",
        quote(username),
        quote(&member),
        quote(&room),
        quote(id),
        quote(room_id),
        quote(username)
    );
    let sql=if let Some(wanted)=wanted {sql.replace(" GROUP BY",&format!(" WHERE r.{} IN ({}) GROUP BY",quote(username),string_list(wanted.iter().cloned())))} else {sql};
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return map;
    };
    if let Ok(rows) = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    }) {
        for (user, count) in rows.flatten() {
            map.insert(user, count);
        }
    }
    map
}

fn clean_preview(value: Option<String>) -> Option<String> {
    let text = value?.replace('\u{0}', "");
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Guard against binary blobs leaking into the UI.
    if trimmed
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return None;
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() > 120 {
        Some(chars[..120].iter().collect::<String>() + "…")
    } else {
        Some(trimmed.to_string())
    }
}

/// Sessions from `session/session.db`, sorted by most recent activity.
pub fn sessions(cache: &Path) -> Result<Vec<SessionItem>, String> {
    sessions_filtered(cache, None)
}

/// Sessions filtered by an optional account-level whitelist.
pub fn sessions_filtered(
    cache: &Path,
    allowed_usernames: Option<&HashSet<String>>,
) -> Result<Vec<SessionItem>, String> {
    sessions_page(cache,allowed_usernames,None,0)
}

pub fn sessions_page(cache:&Path,allowed_usernames:Option<&HashSet<String>>,limit:Option<usize>,offset:usize)->Result<Vec<SessionItem>,String>{
    let path = cache.join("session").join("session.db");
    if !path.exists() {
        return Err("未找到解密后的 session.db，请先完成解密。".into());
    }
    let conn = open_readonly(&path)?;
    let available = tables(&conn);
    let table = table_like(&available, "session").ok_or("session.db 中没有会话表")?;
    let cols = columns(&conn, &table);
    let Some(username) = pick(&cols, &["username", "user_name"]) else {
        return Err(format!("会话表 {table} 缺少 username 字段"));
    };
    let time_col = pick(
        &cols,
        &["last_timestamp", "sort_timestamp", "last_time", "timestamp"],
    );
    let name_col = pick(&cols, &["nick_name", "nickname", "display_name", "remark"]);
    let summary_col = pick(&cols, &["summary", "digest", "last_msg_content"]);
    let unread_col = pick(&cols, &["unread_count"]);

    let expr = |column: Option<&str>| column.map(quote).unwrap_or_else(|| "NULL".to_string());
    let select = format!(
        "SELECT {}, {}, {}, {}, {} FROM {}",
        quote(username),
        expr(time_col),
        expr(summary_col),
        expr(unread_col),
        expr(name_col),
        quote(&table)
    );

    let officials=official_accounts(cache);
    let select=if let Some(allowed)=allowed_usernames {
        let names=allowed.iter().filter(|n|is_visible_chat_username(n) && !officials.contains(*n)).cloned();
        format!("{select} WHERE {} IN ({})",quote(username),string_list(names))
    } else {select};
    let select=format!("{select} ORDER BY {} DESC, {} ASC",expr(time_col),quote(username));
    let select=if let Some(limit)=limit {format!("{select} LIMIT {} OFFSET {}",limit.clamp(1,100),offset)} else {select};
    let mut stmt = conn
        .prepare(&select)
        .map_err(|e| format!("查询会话失败：{e}"))?;
    let rows = stmt
        .query_map([], |row| {
            let username: String = row.get(0)?;
            let last_time: Option<i64> = row.get(1).ok().flatten();
            let summary: Option<String> = row.get(2).ok().flatten();
            let unread: Option<i64> = row.get(3).ok().flatten();
            let name: Option<String> = row.get(4).ok().flatten();
            Ok((username, last_time, summary, unread, name))
        })
        .map_err(|e| format!("查询会话失败：{e}"))?;

    let rows=rows.collect::<Result<Vec<_>,_>>().map_err(|e|format!("读取会话失败：{e}"))?;
    let wanted:HashSet<_>=rows.iter().map(|r|r.0.clone()).collect();
    let names = contacts_for(cache,Some(&wanted));
    let member_counts = group_member_counts_for(cache,Some(&wanted));
    let mut items: Vec<SessionItem> = rows
        .into_iter()
        .filter(|(username, ..)| {
            is_visible_chat_username(username)
                && !officials.contains(username)
                && allowed_usernames.map_or(true, |allowed| allowed.contains(username))
        })
        .map(|(username, last_time, summary, unread, name)| {
            let display = name
                .filter(|n| !n.trim().is_empty())
                .or_else(|| names.get(&username).cloned())
                .unwrap_or_else(|| username.clone());
            let is_group = is_group_username(&username);
            SessionItem {
                member_count: member_counts.get(&username).copied(),
                preview: clean_preview(summary),
                unread: unread.filter(|n| *n > 0),
                is_group,
                username,
                display,
                last_time,
                community: None,
            }
        })
        .collect();
    items.sort_by(|a, b| b.last_time.unwrap_or(0).cmp(&a.last_time.unwrap_or(0)));
    Ok(items)
}

/// Synthetic sessions that are not real conversations (e.g. the "folded groups" bucket).
fn is_placeholder_username(username: &str) -> bool {
    username.starts_with("@placeholder")
}

/// Keep only one-to-one and group conversations in the viewer.
///
/// Official accounts normally use the `gh_` namespace, and the rest are caught by their
/// `biz_info` registration (see [`official_accounts`]). The explicit list covers WeChat's
/// built-in service sessions, which are neither people nor chat groups — including the
/// 服务号 / 订阅号 aggregate buckets themselves.
/// Whether a conversation is a real one-to-one or group chat rather than a system channel.
/// Combines the username rule with WeChat's own official-account registry, so legacy 服务号
/// ids (`cmb4008205555`, `qqsafe`) are rejected too.
pub fn is_chat_session(cache: &Path, username: &str) -> bool {
    is_visible_chat_username(username) && !official_accounts(cache).contains(username)
}

pub fn is_visible_chat_username(username: &str) -> bool {
    const SYSTEM_USERNAMES: &[&str] = &[
        "weixin",
        "filehelper",
        "fmessage",
        "floatbottle",
        "lbsapp",
        "shakeapp",
        "medianote",
        "qqmail",
        "qmessage",
        "tmessage",
        "facebookapp",
        "masssend",
        "feedsapp",
        "weibo",
        "newsapp",
        "blogapp",
        "mphelper",
        "notification_messages",
        "notifymessage",
        "brandsessionholder",
        "brandservicesessionholder",
        "wa_app_kefu_message",
        "weixinreminder",
        "weixinvoip",
        "weixinguanhaozhushou",
    ];

    let normalized = username.trim().to_ascii_lowercase();
    if normalized.is_empty() || is_placeholder_username(&normalized) {
        return false;
    }
    if is_group_username(&normalized) {
        return true;
    }
    !normalized.starts_with("gh_") && !SYSTEM_USERNAMES.contains(&normalized.as_str())
}

pub fn md5_hex(value: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

fn message_shards(cache: &Path) -> Vec<PathBuf> {
    let dir = cache.join("message");
    let mut shards: Vec<_> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    let name = p
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    name.starts_with("message") && name.ends_with(".db")
                })
                .collect()
        })
        .unwrap_or_default();
    shards.sort();
    shards
}

/// Pull a human-readable summary out of a WeChat app message (`<msg><appmsg>…`).
///
/// Links, files, quotes, solitaire and transfers are all stored as XML. Showing the raw
/// markup makes both the message list and keyword snippets unreadable, so the title and
/// description are extracted when they exist. When they do not, the raw text is kept.
fn tag_text(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let start = text.find(&open)? + open.len();
    let rest = &text[start..];
    let end = rest.find(&format!("</{tag}>"))?;
    let mut inner = rest[..end].trim();
    if let Some(stripped) = inner
        .strip_prefix("<![CDATA[")
        .and_then(|v| v.strip_suffix("]]>"))
    {
        inner = stripped.trim();
    }
    if inner.is_empty() {
        None
    } else {
        Some(inner.to_string())
    }
}

fn is_markup(text: &str) -> bool {
    text.trim_start().starts_with('<')
}

/// `<type>` of an app message (`<msg><appmsg><type>5</type>…`).
fn app_type(text: &str) -> Option<i64> {
    tag_text(text, "type")?.trim().parse().ok()
}

/// Human label for an `<appmsg><type>` value.
///
/// WeChat stores every rich message (link, file, quote, mini program, transfer …) as
/// XML inside a type-49 message. The numeric `type` tells them apart; anything unknown
/// still gets a neutral label so raw markup never reaches the viewer.
fn app_label(app_type: Option<i64>) -> &'static str {
    match app_type {
        Some(3) => "[音乐]",
        Some(5) => "[链接]",
        Some(6) => "[文件]",
        Some(19) => "[聊天记录]",
        Some(33) | Some(36) => "[小程序]",
        Some(51) | Some(63) => "[视频号]",
        Some(57) => "[引用]",
        Some(2000) => "[转账]",
        Some(2001) => "[红包]",
        Some(10000) => "[系统消息]",
        _ => "[卡片消息]",
    }
}

fn markup_summary(text: &str) -> Option<String> {
    if !is_markup(text) {
        return None;
    }
    let title = tag_text(text, "title");
    let des = tag_text(text, "des");
    let mut summary = match (title, des) {
        (Some(title), Some(des)) if des != title => format!("{title}（{des}）"),
        (Some(title), _) => title,
        (None, Some(des)) => des,
        (None, None) => return None,
    };
    summary = summary.replace(['\n', '\r', '\t'], " ");
    let chars: Vec<char> = summary.chars().collect();
    if chars.len() > 160 {
        summary = chars[..160].iter().collect::<String>() + "…";
    }
    Some(summary)
}

/// Decompress (when needed) and UTF-8 decode one `message_content` cell.
///
/// Group messages prepend `sender_username:\n` to this raw text, so the prefix must be
/// stripped before the content can be classified as markup or labelled.
fn decode_text(raw: Option<Vec<u8>>, compression: Option<i64>) -> String {
    let bytes = raw.unwrap_or_default();
    if compression == Some(4) && !bytes.is_empty() {
        return match zstd::decode_all(bytes.as_slice()) {
            Ok(plain) => String::from_utf8_lossy(&plain).to_string(),
            Err(_) => "[压缩内容无法解压]".to_string(),
        };
    }
    String::from_utf8_lossy(&bytes).to_string()
}

/// Add a human label to already-decoded, prefix-free message body.
fn label_text(kind: i64, body: &str) -> String {
    let base = kind & 0xFFFF_FFFF;
    if base == 1 {
        if body.trim().is_empty() {
            return "[空文本]".to_string();
        }
        if is_markup(body) {
            return markup_summary(body).unwrap_or_else(|| "[富文本消息]".to_string());
        }
        return body.to_string();
    }

    let label = match base {
        3 => "[图片]",
        34 => "[语音]",
        42 => "[名片]",
        43 => "[视频]",
        47 => "[表情]",
        48 => "[位置]",
        49 => "[链接/文件]",
        50 => "[通话]",
        10000 | 10002 => "[系统消息]",
        37 => "[好友申请]",
        _ => "[非文本消息]",
    };
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return label.to_string();
    }
    if !is_markup(trimmed) {
        return format!("{label} {trimmed}");
    }
    // Rich messages carry no display text beyond their XML: use the app type and the
    // title/description when present, never the raw markup.
    if base == 49 {
        let typed = app_label(app_type(trimmed));
        return match markup_summary(trimmed) {
            Some(summary) => format!("{typed} {summary}"),
            None => typed.to_string(),
        };
    }
    label.to_string()
}

/// Decoded, de-prefixed display text plus the sender recovered from the group prefix.
///
/// The `username:\n` prefix only exists in group sessions, and it sits in front of the
/// payload — including compressed rich messages — so it has to be removed first.
fn prepare_text(
    kind: i64,
    raw: Option<Vec<u8>>,
    compression: Option<i64>,
    group: bool,
    sender: Option<String>,
) -> (Option<String>, String) {
    let decoded = decode_text(raw, compression);
    if !group {
        return (sender, label_text(kind, &decoded));
    }
    let (prefix, body) = split_sender_prefix(decoded);
    (sender.or(prefix), label_text(kind, &body))
}

/// Group messages store the sender as a `username:\n` prefix inside `message_content`.
///
/// The prefix is part of the stored string, not a separate column, so it has to be
/// removed before the text is shown, searched or summarised. Only a plausible
/// username-shaped token is stripped, so ordinary text such as `决定：…` is untouched.
fn split_sender_prefix(text: String) -> (Option<String>, String) {
    for marker in [":\n", ":\r\n"] {
        if let Some(pos) = text.find(marker) {
            let head = &text[..pos];
            let plausible = !head.is_empty()
                && head.chars().count() <= 64
                && !head.contains(char::is_whitespace)
                && head
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '@'));
            if plausible {
                return (
                    Some(head.to_string()),
                    text[pos + marker.len()..].to_string(),
                );
            }
            break;
        }
    }
    (None, text)
}

/// A message worth putting in a work digest: plain text only.
///
/// Images, files, links, transfers and system notices carry little work context and, in
/// 4.x, are stored as XML; including them turns the digest into markup noise.
fn is_work_text(kind: i64, text: &str) -> bool {
    if kind & 0xFFFF != 1 {
        return false;
    }
    let trimmed = text.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('<') {
        return false;
    }
    !(trimmed.starts_with('[') && trimmed.ends_with(']'))
}

fn shard_senders(conn: &Connection) -> HashMap<i64, String> {
    let mut map = HashMap::new();
    if !tables(conn)
        .iter()
        .any(|t| t.eq_ignore_ascii_case("Name2Id"))
    {
        return map;
    }
    if let Ok(mut stmt) = conn.prepare("SELECT rowid, user_name FROM Name2Id") {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        }) {
            for (id, name) in rows.flatten() {
                map.insert(id, name);
            }
        }
    }
    map
}

/// Best-effort identification of this account's own wxid.
///
/// A direct conversation has exactly two participants, so any sender that is not the
/// session's own username is this account. Voting across several direct chats keeps a
/// single odd conversation (a service account, a folded session) from deciding it.
pub fn self_username(cache: &Path) -> Option<String> {
    let sessions = sessions(cache).ok()?;
    let direct: Vec<&SessionItem> = sessions.iter().filter(|s| !s.is_group).take(12).collect();
    let mut votes: HashMap<String, usize> = HashMap::new();
    let shards = message_shards(cache);

    for session in direct {
        let table = format!("Msg_{}", md5_hex(&session.username));
        for shard in &shards {
            let Ok(conn) = open_readonly(shard) else {
                continue;
            };
            if !tables(&conn).iter().any(|t| t == &table) {
                continue;
            }
            let senders = shard_senders(&conn);
            let cols = columns(&conn, &table);
            let Some(sender_col) = pick(&cols, &["real_sender_id", "sender_id", "sender"]) else {
                break;
            };
            let sql = format!(
                "SELECT {}, COUNT(*) FROM {} GROUP BY 1 ORDER BY 2 DESC LIMIT 3",
                quote(sender_col),
                quote(&table)
            );
            if let Ok(mut stmt) = conn.prepare(&sql) {
                let lookup = &senders;
                if let Ok(rows) = stmt.query_map([], move |row| {
                    let name = match row.get_ref(0) {
                        Ok(rusqlite::types::ValueRef::Integer(id)) => lookup.get(&id).cloned(),
                        Ok(rusqlite::types::ValueRef::Text(bytes)) => {
                            Some(String::from_utf8_lossy(bytes).to_string())
                        }
                        _ => None,
                    };
                    Ok((name, row.get::<_, i64>(1).unwrap_or(0)))
                }) {
                    for (name, count) in rows.flatten() {
                        if let Some(name) = name {
                            if name != session.username {
                                *votes.entry(name).or_default() += count.max(1) as usize;
                            }
                        }
                    }
                }
            }
            break;
        }
    }

    votes
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(name, _)| name)
}

/// Where a page stopped: the last row already handed out.
///
/// A timestamp alone cannot order a conversation: busy groups routinely land dozens of
/// messages in the same second, and a `create_time <` cursor would then drop the rest of
/// that second at a page boundary. The local id breaks the tie, so every page continues
/// strictly after the previous row and nothing is skipped or repeated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageCursor {
    pub time: i64,
    pub local_id: i64,
    #[serde(default)]
    pub source_shard: String,
}

impl MessageCursor {
    pub fn new(time: i64, local_id: i64) -> Self {
        Self { time, local_id, source_shard: String::new() }
    }

    pub fn from_message(message: &MessageItem) -> Self {
        Self { time: message.time, local_id: message.local_id, source_shard: message.source_shard.clone() }
    }

    /// Cursor for "everything older than this timestamp", the pre-cursor semantics.
    pub fn before_time(time: i64) -> Self {
        Self {
            time,
            local_id: i64::MIN,
            source_shard: String::new(),
        }
    }
}

/// Messages of one conversation, newest first.
///
/// `before` is exclusive: pass the [`MessageCursor`] of the last row already read.
pub fn messages(
    cache: &Path,
    username: &str,
    limit: usize,
    before: Option<MessageCursor>,
) -> Result<Vec<MessageItem>, String> {
    let me = self_username(cache);
    messages_inner(cache, username, limit, before, None, None, me.as_deref())
}

/// IDs are only unique inside a message shard, not across a conversation.
pub fn local_id_inventory(cache: &Path, username: &str) -> Result<HashMap<i64, HashMap<i64, usize>>, String> {
    local_id_inventory_for(cache,username,None)
}
pub fn local_id_inventory_for(cache: &Path,username: &str, ids: Option<&[i64]>) -> Result<HashMap<i64,HashMap<i64,usize>>,String> {
    let table = format!("Msg_{}", md5_hex(username));
    let mut inventory: HashMap<i64, HashMap<i64, usize>> = HashMap::new();
    let mut found = false;
    for shard in message_shards(cache) {
        let conn = open_readonly(&shard).map_err(|error| format!("无法读取消息分片：{error}"))?;
        if !tables(&conn).iter().any(|name| name == &table) { continue; }
        found = true;
        let cols = columns(&conn, &table);
        let Some(time) = pick(&cols, &["create_time", "createTime", "timestamp"]) else { continue };
        let local = pick(&cols, &["local_id", "localId", "id"]).unwrap_or("rowid");
        let sql = format!("SELECT {}, {} FROM {} WHERE {} IS NOT NULL", quote(local), quote(time), quote(&table), quote(time));
        let sql=if let Some(ids)=ids {format!("{sql} AND {} IN ({})",quote(local),ids.iter().map(i64::to_string).collect::<Vec<_>>().join(","))} else {sql};
        let mut stmt = conn.prepare(&sql).map_err(|error| format!("无法检查消息编号：{error}"))?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))).map_err(|error| format!("无法检查消息编号：{error}"))?;
        for row in rows {
            let (id, timestamp) = row.map_err(|error| format!("无法读取消息编号：{error}"))?;
            *inventory.entry(id).or_default().entry(timestamp).or_default() += 1;
        }
    }
    if !found { return Err("未找到该会话的消息库。".into()); }
    Ok(inventory)
}

pub fn server_id_counts(cache: &Path, username: &str) -> Result<HashMap<i64, usize>, String> {
    server_id_counts_for(cache,username,None)
}
pub fn server_id_counts_for(cache: &Path,username: &str,ids: Option<&[i64]>) -> Result<HashMap<i64,usize>,String> {
    let table = format!("Msg_{}", md5_hex(username));
    let mut counts = HashMap::new();
    for shard in message_shards(cache) {
        let conn = open_readonly(&shard).map_err(|e| format!("无法读取消息分片：{e}"))?;
        if !tables(&conn).iter().any(|name| name == &table) { continue; }
        let cols = columns(&conn, &table);
        let Some(server) = pick(&cols, &["server_id", "serverId"]) else { continue };
        let Some(time) = pick(&cols, &["create_time", "createTime", "timestamp"]) else { continue };
        let sql = format!("SELECT {}, COUNT(*) FROM {} WHERE {} IS NOT NULL AND {} IS NOT NULL AND {} != 0 GROUP BY {}", quote(server), quote(&table), quote(time), quote(server), quote(server), quote(server));
        let sql=if let Some(ids)=ids {sql.replace(" GROUP BY",&format!(" AND {} IN ({}) GROUP BY",quote(server),ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")))} else {sql};
        let mut stmt = conn.prepare(&sql).map_err(|e| format!("无法核对服务端消息编号：{e}"))?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, usize>(1)?))).map_err(|e| format!("无法核对服务端消息编号：{e}"))?;
        for row in rows {
            let (id, count) = row.map_err(|e| format!("无法核对服务端消息编号：{e}"))?;
            *counts.entry(id).or_default() += count;
        }
    }
    Ok(counts)
}

/// Messages inside an explicit time window, newest first. Used by media export.
pub fn messages_between(
    cache: &Path,
    username: &str,
    since: i64,
    until: i64,
    limit: usize,
) -> Result<Vec<MessageItem>, String> {
    let me = self_username(cache);
    messages_inner(cache, username, limit, None, Some(since), Some(until), me.as_deref())
}

/// Newest message time and local id of one conversation, without the self-identification
/// vote.
///
/// Automatic upload uses this to skip a conversation before packaging when nothing is new.
pub fn message_dates(cache: &Path, username: &str) -> Result<Vec<(String, usize)>, String> {
    let table = format!("Msg_{}", md5_hex(username));
    let mut dates = std::collections::BTreeMap::<String, usize>::new();
    let mut found = false;
    for shard in message_shards(cache) {
        let conn = open_readonly(&shard)?;
        if !tables(&conn).iter().any(|t| t == &table) { continue; }
        found = true;
        let cols = columns(&conn, &table);
        let time = pick(&cols, &["create_time", "createTime", "timestamp"]).ok_or("消息库缺少时间字段。")?;
        let sql = format!("SELECT date({}, 'unixepoch', 'localtime'), COUNT(*) FROM {} WHERE {} > 0 GROUP BY 1", quote(time), quote(&table), quote(time));
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, usize>(1)?))).map_err(|e| e.to_string())?;
        for row in rows { let (day, count) = row.map_err(|e| e.to_string())?; if let Some(day) = day { *dates.entry(day).or_default() += count; } }
    }
    if !found { return Err("没有找到会话消息库。".into()); }
    Ok(dates.into_iter().collect())
}

pub fn latest_message(cache: &Path, username: &str) -> Result<Option<(i64, i64)>, String> {
    let messages = messages_inner(cache, username, 1, None, None, None, None)?;
    Ok(messages.first().map(|message| (message.time, message.local_id)))
}

pub fn count_at_time(cache: &Path, username: &str, timestamp: i64) -> Result<usize, String> {
    let table = format!("Msg_{}", md5_hex(username));
    let mut count = 0usize;
    let mut found = false;
    for shard in message_shards(cache) {
        let conn = open_readonly(&shard).map_err(|e| format!("无法读取消息分片：{e}"))?;
        if !tables(&conn).iter().any(|t| t == &table) { continue; }
        found = true;
        let cols = columns(&conn, &table);
        let time = pick(&cols, &["create_time", "createTime", "timestamp"]).ok_or("消息库缺少时间字段。")?;
        let sql = format!("SELECT COUNT(*) FROM {} WHERE {} = ?1", quote(&table), quote(time));
        count += conn.query_row(&sql, [timestamp], |row| row.get::<_, usize>(0)).map_err(|e| format!("无法核对消息数量：{e}"))?;
    }
    if !found { return Err("没有找到会话消息库。".into()); }
    Ok(count)
}

/// Newest `(time, local id)` of one conversation without decoding message bodies.
///
/// The chat list asks for this on every refresh, so it skips the contact table and the text
/// pipeline that [`latest_message`] needs to build full records.
pub fn latest_position(cache: &Path, username: &str) -> Result<Option<(i64, i64)>, String> {
    let table = format!("Msg_{}", md5_hex(username));
    let mut newest: Option<(i64, i64)> = None;
    let mut found_table = false;
    for shard in message_shards(cache) {
        let Ok(conn) = open_readonly(&shard) else {
            continue;
        };
        if !tables(&conn).iter().any(|t| t == &table) {
            continue;
        }
        found_table = true;
        let cols = columns(&conn, &table);
        let Some(create_time) = pick(&cols, &["create_time", "createTime", "timestamp"]) else {
            continue;
        };
        let local = pick(&cols, &["local_id", "localId", "id"]).unwrap_or("rowid");
        let sql = format!(
            "SELECT {}, {} FROM {} WHERE {} IS NOT NULL ORDER BY {} DESC, {} DESC LIMIT 1",
            quote(create_time),
            quote(local),
            quote(&table),
            quote(create_time),
            quote(create_time),
            quote(local)
        );
        let Ok(row) = conn.query_row(&sql, [], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        }) else {
            continue;
        };
        if newest.map_or(true, |current| (row.0, row.1) > current) {
            newest = Some(row);
        }
    }
    if !found_table {
        return Err(format!(
            "在解密后的消息库中没有找到会话 {username} 对应的表（{table}）。"
        ));
    }
    Ok(newest)
}

/// One bounded badge refresh shares read-only shard connections. Reopening a
/// WeChat shard for every badge reparses its thousands of table definitions.
/// Keep these connections only for this request, never across account changes
/// or replacement of a decrypted database.
pub struct UploadStateReader {
    shards: Vec<(Connection, HashSet<String>)>,
    unreadable_shard: bool,
}

impl UploadStateReader {
    pub fn new(cache: &Path) -> Self {
        let mut unreadable_shard = false;
        let shards = message_shards(cache).into_iter().filter_map(|path| {
            let conn = match open_readonly(&path) {
                Ok(conn) => conn,
                Err(_) => { unreadable_shard = true; return None; }
            };
            let names = tables(&conn).into_iter().collect();
            Some((conn, names))
        }).collect();
        Self { shards, unreadable_shard }
    }

    fn rows<T>(&self, username: &str, strict: bool, read: impl Fn(&Connection, &str, &str, &str) -> Option<T>) -> Result<Vec<T>, String> {
        if strict && self.unreadable_shard { return Err("无法读取消息分片。".into()); }
        let table = format!("Msg_{}", md5_hex(username));
        let mut found = false;
        let mut rows = Vec::new();
        for (conn, names) in &self.shards {
            if !names.contains(&table) { continue; }
            found = true;
            let cols = columns(conn, &table);
            let Some(time) = pick(&cols, &["create_time", "createTime", "timestamp"]) else {
                if strict { return Err("消息库缺少时间字段。".into()); }
                continue;
            };
            let local = pick(&cols, &["local_id", "localId", "id"]).unwrap_or("rowid");
            if let Some(row) = read(conn, &quote(&table), &quote(time), &quote(local)) { rows.push(row); }
            else if strict { return Err("无法核对消息数量。".into()); }
        }
        if !found { return Err("没有找到会话消息库。".into()); }
        Ok(rows)
    }

    pub fn latest_position(&self, username: &str) -> Result<Option<(i64, i64)>, String> {
        Ok(self.rows(username, false, |conn, table, time, local| {
            conn.query_row(&format!("SELECT {time}, {local} FROM {table} WHERE {time} IS NOT NULL ORDER BY {time} DESC, {local} DESC LIMIT 1"), [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))).ok()
        })?.into_iter().max())
    }

    pub fn count_at_time(&self, username: &str, timestamp: i64) -> Result<usize, String> {
        Ok(self.rows(username, true, |conn, table, time, _| {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE {time} = ?1"), [timestamp], |row| row.get::<_, usize>(0)).ok()
        })?.into_iter().fold(0usize, usize::saturating_add))
    }

    pub fn count_after(&self, username: &str, timestamp: i64, id: i64) -> Result<usize, String> {
        Ok(self.rows(username, false, |conn, table, time, local| {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE {time} IS NOT NULL AND ({time} > ?1 OR ({time} = ?1 AND {local} > ?2))"), params![timestamp, id], |row| row.get::<_, usize>(0)).ok()
        })?.into_iter().fold(0usize, usize::saturating_add))
    }
}

/// Number of messages strictly newer than an upload position.
///
/// Mirrors [`messages`]' shard and column discovery without materialising the rows, so the
/// chat list can report "N new records" without paging through the conversation.
pub fn count_after(
    cache: &Path,
    username: &str,
    time: i64,
    local_id: i64,
) -> Result<usize, String> {
    let table = format!("Msg_{}", md5_hex(username));
    let mut total = 0usize;
    let mut found_table = false;
    for shard in message_shards(cache) {
        let Ok(conn) = open_readonly(&shard) else {
            continue;
        };
        if !tables(&conn).iter().any(|t| t == &table) {
            continue;
        }
        found_table = true;
        let cols = columns(&conn, &table);
        let Some(create_time) = pick(&cols, &["create_time", "createTime", "timestamp"]) else {
            continue;
        };
        let local = pick(&cols, &["local_id", "localId", "id"]).unwrap_or("rowid");
        let sql = format!(
            "SELECT COUNT(*) FROM {} WHERE {} IS NOT NULL AND ({} > {} OR ({} = {} AND {} > {}))",
            quote(&table),
            quote(create_time),
            quote(create_time),
            time,
            quote(create_time),
            time,
            quote(local),
            local_id
        );
        // A shard that cannot count is skipped: the label only needs a lower bound, and the
        // uploader still reports real read failures.
        let Ok(count) = conn.query_row(&sql, [], |row| row.get::<_, i64>(0)) else {
            continue;
        };
        total = total.saturating_add(count.max(0) as usize);
    }
    if !found_table {
        return Err(format!(
            "在解密后的消息库中没有找到会话 {username} 对应的表（{table}）。"
        ));
    }
    Ok(total)
}

fn messages_inner(
    cache: &Path,
    username: &str,
    limit: usize,
    before: Option<MessageCursor>,
    since: Option<i64>,
    until: Option<i64>,
    me: Option<&str>,
) -> Result<Vec<MessageItem>, String> {
    messages_ordered(cache,username,limit,before,since,until,me,false,None)
}

pub fn upload_page(cache: &Path, username: &str, since: i64, until: Option<i64>, after: Option<MessageCursor>, limit: usize) -> Result<Vec<MessageItem>,String> {
    messages_ordered(cache,username,limit,after,Some(since),until.map(|v|v.saturating_sub(1)),None,true,None)
}

pub fn messages_selected(cache: &Path, username: &str, selected: &[crate::settings::MessageRef]) -> Result<Vec<MessageItem>,String> {
    if selected.is_empty() {return Ok(Vec::new());}
    if selected.len()>1000 {return Err("单批消息超过读取上限。".into());}
    let me=self_username(cache);
    let rows=messages_ordered(cache,username,selected.len(),None,None,None,me.as_deref(),false,Some(selected))?;
    if rows.len()!=selected.len() {return Err("部分待上传消息已变化，请重新读取。".into());}
    Ok(rows)
}

fn messages_ordered(cache: &Path,username: &str,limit: usize,before: Option<MessageCursor>,since: Option<i64>,until: Option<i64>,me: Option<&str>,ascending: bool,selected: Option<&[crate::settings::MessageRef]>) -> Result<Vec<MessageItem>,String> {
    let table = format!("Msg_{}", md5_hex(username));
    let limit = limit.clamp(1, 5000);
    let mut collected: Vec<MessageItem> = Vec::new();
    let mut found_table = false;
    let names = contacts(cache);
    let group = is_group_username(username);

    for shard in message_shards(cache) {
        let source_shard = shard.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        let Ok(conn) = open_readonly(&shard) else {
            continue;
        };
        if !tables(&conn).iter().any(|t| t == &table) {
            continue;
        }
        found_table = true;
        let cols = columns(&conn, &table);
        let Some(create_time) = pick(&cols, &["create_time", "createTime", "timestamp"]) else {
            continue;
        };
        let Some(content) = pick(&cols, &["message_content", "content"]) else {
            continue;
        };
        let local_id = pick(&cols, &["local_id", "localId", "id"]).unwrap_or("rowid");
        let kind_col = pick(&cols, &["local_type", "type"]);
        let sender_col = pick(&cols, &["real_sender_id", "sender_id", "sender"]);
        let compress_col = pick(&cols, &["WCDB_CT_message_content", "compress_flag"]);
        let server_col = pick(&cols, &["server_id", "serverId"]);

        let sender_map = if sender_col.map(|c| c != "sender").unwrap_or(false) {
            shard_senders(&conn)
        } else {
            HashMap::new()
        };

        let projection = vec![
            quote(local_id),
            quote(create_time),
            quote(content),
            kind_col.map(quote).unwrap_or_else(|| "0".to_string()),
            sender_col.map(quote).unwrap_or_else(|| "NULL".to_string()),
            compress_col
                .map(quote)
                .unwrap_or_else(|| "NULL".to_string()),
            server_col.map(quote).unwrap_or_else(|| "0".to_string()),
        ];
        let mut sql = format!("SELECT {} FROM {}", projection.join(", "), quote(&table));
        sql.push_str(&format!(" WHERE {} IS NOT NULL", quote(create_time)));
        if let Some(selected)=selected {
            let conditions:Vec<_>=selected.iter().filter(|m|m.source_shard.is_empty() || m.source_shard==source_shard).map(|m|format!("({}={} AND {}={})",quote(local_id),m.local_id,quote(create_time),m.time)).collect();
            if conditions.is_empty() {continue;}
            sql.push_str(&format!(" AND ({})",conditions.join(" OR ")));
        }
        if let Some(ref before) = before {
            // Keyset pagination on (time, local id): a second shared by more messages than
            // one page holds still resumes exactly after the last row handed out.
            sql.push_str(&format!(
                " AND ({} < {} OR ({} = {} AND {} < {}))",
                quote(create_time),
                before.time,
                quote(create_time),
                before.time,
                quote(local_id),
                before.local_id
            ));
            if !before.source_shard.is_empty() && (if ascending {source_shard > before.source_shard} else {source_shard < before.source_shard}) {
                let suffix = format!(" OR ({} = {} AND {} = {})", quote(create_time), before.time, quote(local_id), before.local_id);
                let end = sql.len() - 1;
                sql.insert_str(end, &suffix);
            }
        }
        if ascending {sql=sql.replace(" < "," > ");}
        if let Some(since) = since {
            sql.push_str(&format!(" AND {} >= {}", quote(create_time), since));
        }
        if let Some(until) = until {
            sql.push_str(&format!(" AND {} <= {}", quote(create_time), until));
        }
        sql.push_str(&format!(
            " ORDER BY {} {order}, {} {order} LIMIT {}",
            quote(create_time),
            quote(local_id),
            limit, order=if ascending {"ASC"} else {"DESC"}
        ));

        let Ok(mut stmt) = conn.prepare(&sql) else {
            continue;
        };
        let rows = stmt.query_map([], |row| {
            let local: i64 = row.get(0).unwrap_or(0);
            let time: i64 = row.get(1).unwrap_or(0);
            let raw = value_to_bytes(row.get_ref(2)?).filter(|b| !b.is_empty());
            let kind: i64 = row.get(3).unwrap_or(0);
            let sender_id: Option<i64> = row.get(4).ok();
            let compress: Option<i64> = row.get(5).ok();
            let server: i64 = row.get(6).unwrap_or(0);
            Ok((local, time, raw, kind, sender_id, compress, server))
        });
        let Ok(rows) = rows else {
            continue;
        };
        for (local, time, raw, kind, sender_id, compress, server) in rows.flatten() {
            let stored = sender_id.and_then(|id| sender_map.get(&id).cloned());
            let (sender, text) = prepare_text(kind, raw, compress, group, stored);
            let sender_name = sender.as_ref().and_then(|s| names.get(s).cloned());
            let is_self = match (sender.as_deref(), me) {
                (Some(sender), Some(me)) => sender == me,
                _ => false,
            };
            collected.push(MessageItem {
                local_id: local,
                server_id: server,
                source_shard: source_shard.clone(),
                time,
                sender,
                sender_name,
                kind,
                text,
                is_self,
            });
        }
    }

    if !found_table {
        return Err(format!(
            "在解密后的消息库中没有找到会话 {username} 对应的表（{table}）。可能该会话没有本地消息，或尚未解密对应的消息分片。"
        ));
    }

    collected.sort_by(|a, b| b.time.cmp(&a.time).then(b.local_id.cmp(&a.local_id)).then(b.source_shard.cmp(&a.source_shard)));
    if ascending {collected.reverse();}
    collected.truncate(limit);
    Ok(collected)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RawMessage {
    pub local_id: i64,
    pub server_id: i64,
    #[serde(default)]
    pub source_shard: String,
    pub time: i64,
    pub kind: i64,
    /// Decompressed, sender-prefix-free payload. Rich messages stay as XML.
    pub content: String,
}

/// Fetch exactly the selected rows, so an upload never associates media with another shard.
pub fn raw_messages_selected(cache: &Path, username: &str, selected: &[MessageItem]) -> Result<Vec<RawMessage>, String> {
    let table = format!("Msg_{}", md5_hex(username));
    let wanted: std::collections::HashSet<_> = selected.iter().map(|m| (m.source_shard.clone(), m.local_id, m.time)).collect();
    if wanted.len() != selected.len() { return Err("所选消息标识不唯一，无法安全关联附件。".into()); }
    let mut found = std::collections::HashSet::new();
    let mut out = Vec::new();
    for shard in message_shards(cache) {
        let shard_name = shard.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let mut ids: Vec<i64> = selected.iter().filter(|m| m.source_shard == shard_name).map(|m| m.local_id).collect();
        if ids.is_empty() { continue; }
        ids.sort_unstable(); ids.dedup();
        let conn = open_readonly(&shard).map_err(|e| format!("无法读取附件来源：{e}"))?;
        let cols = columns(&conn, &table);
        let local = pick(&cols, &["local_id", "localId", "id"]).unwrap_or("rowid");
        let time = pick(&cols, &["create_time", "createTime", "timestamp"]).ok_or("消息库缺少时间字段。")?;
        let content = pick(&cols, &["message_content", "content"]).ok_or("消息库缺少内容字段。")?;
        let kind = pick(&cols, &["local_type", "type"]).map(quote).unwrap_or_else(|| "0".into());
        let server = pick(&cols, &["server_id", "serverId"]).map(quote).unwrap_or_else(|| "0".into());
        let compressed = pick(&cols, &["WCDB_CT_message_content", "compress_flag"]).map(quote).unwrap_or_else(|| "NULL".into());
        for chunk in ids.chunks(400) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!("SELECT {}, {}, {}, {}, {}, {} FROM {} WHERE {} IN ({})", quote(local), server, quote(time), kind, quote(content), compressed, quote(&table), quote(local), placeholders);
            let mut statement = conn.prepare(&sql).map_err(|e| format!("无法读取附件来源：{e}"))?;
            let rows = statement.query_map(rusqlite::params_from_iter(chunk.iter()), |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1).unwrap_or(0), row.get::<_, i64>(2)?, row.get::<_, i64>(3).unwrap_or(0), value_to_bytes(row.get_ref(4)?), row.get::<_, Option<i64>>(5).ok().flatten())))
                .map_err(|e| format!("无法读取附件来源：{e}"))?;
            for row in rows {
                let (local_id, server_id, timestamp, kind, raw, compressed) = row.map_err(|e| format!("无法读取附件来源：{e}"))?;
                let key = (shard_name.clone(), local_id, timestamp);
                if !wanted.contains(&key) { continue; }
                if !found.insert(key) { return Err("附件来源存在重复记录，无法安全上传。".into()); }
                out.push(RawMessage { local_id, server_id, source_shard: shard_name.clone(), time: timestamp, kind, content: split_sender_prefix(decode_text(raw, compressed)).1 });
            }
        }
    }
    if found.len() != wanted.len() { return Err("部分消息的附件来源不可用，请重新解密后重试。".into()); }
    Ok(out)
}

/// Raw message payloads inside a time window, used by the attachment manifest and export.
///
/// Unlike [`messages`] this keeps the original XML so the media layer can read `md5`,
/// `length`, `title` and the other attachment attributes.
pub fn raw_messages_between(
    cache: &Path,
    username: &str,
    since: i64,
    until: i64,
    limit: usize,
) -> Result<Vec<RawMessage>, String> {
    let table = format!("Msg_{}", md5_hex(username));
    let limit = limit.clamp(1, 20000);
    let mut out = Vec::new();
    let mut found_table = false;

    for shard in message_shards(cache) {
        let Ok(conn) = open_readonly(&shard) else {
            continue;
        };
        if !tables(&conn).iter().any(|t| t == &table) {
            continue;
        }
        found_table = true;
        let cols = columns(&conn, &table);
        let Some(create_time) = pick(&cols, &["create_time", "createTime", "timestamp"]) else {
            continue;
        };
        let Some(content) = pick(&cols, &["message_content", "content"]) else {
            continue;
        };
        let local_id = pick(&cols, &["local_id", "localId", "id"]).unwrap_or("rowid");
        let kind_col = pick(&cols, &["local_type", "type"]).unwrap_or("0");
        let server_col = pick(&cols, &["server_id", "serverId"]);
        let compress_col = pick(&cols, &["WCDB_CT_message_content", "compress_flag"])
            .unwrap_or("NULL");

        let sql = format!(
            "SELECT {}, {}, {}, {}, {}, {} FROM {} WHERE {} >= {} AND {} <= {} ORDER BY {} DESC LIMIT {}",
            quote(local_id),
            server_col.map(quote).unwrap_or_else(|| "0".to_string()),
            quote(create_time),
            quote(kind_col),
            quote(content),
            quote(compress_col),
            quote(&table),
            quote(create_time),
            since,
            quote(create_time),
            until,
            quote(create_time),
            limit
        );
        let Ok(mut stmt) = conn.prepare(&sql) else {
            continue;
        };
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0).unwrap_or(0),
                row.get::<_, i64>(1).unwrap_or(0),
                row.get::<_, i64>(2).unwrap_or(0),
                row.get::<_, i64>(3).unwrap_or(0),
                value_to_bytes(row.get_ref(4)?),
                row.get::<_, Option<i64>>(5).ok().flatten(),
            ))
        });
        let Ok(rows) = rows else {
            continue;
        };
        for (local, server, time, kind, raw, compress) in rows.flatten() {
            let decoded = decode_text(raw, compress);
            let content = split_sender_prefix(decoded).1;
            out.push(RawMessage {
                local_id: local,
                server_id: server,
                source_shard: shard.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default(),
                time,
                kind,
                content,
            });
        }
    }

    if !found_table {
        return Err(format!(
            "在解密后的消息库中没有找到会话 {username} 对应的表（{table}）。"
        ));
    }
    out.sort_by(|a, b| b.time.cmp(&a.time).then(b.local_id.cmp(&a.local_id)));
    out.truncate(limit);
    Ok(out)
}

fn snippet_around(text: &str, needle: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    let needle_lower: Vec<char> = needle.to_lowercase().chars().collect();
    let pos = if needle_lower.is_empty()
        || lower.len() != chars.len()
        || needle_lower.len() > lower.len()
    {
        None
    } else {
        lower
            .windows(needle_lower.len())
            .position(|w| w == needle_lower.as_slice())
    };
    let pos = pos.unwrap_or(0);
    let start = pos.saturating_sub(24);
    let end = (pos + needle_lower.len().max(1) + 70).min(chars.len());
    let mut out: String = chars[start..end].iter().collect();
    if start > 0 {
        out.insert(0, '…');
    }
    if end < chars.len() {
        out.push('…');
    }
    out.replace(['\n', '\r'], " ")
}

struct HitRow {
    time: i64,
    text: String,
    sender: Option<String>,
    sender_name: Option<String>,
}

fn table_hits(
    conn: &Connection,
    table: &str,
    keyword: &str,
    limit: usize,
    senders: &HashMap<i64, String>,
    names: &HashMap<String, String>,
    group: bool,
) -> (Vec<HitRow>, bool) {
    let cols = columns(conn, table);
    let Some(create_time) = pick(&cols, &["create_time", "createTime", "timestamp"]) else {
        return (Vec::new(), false);
    };
    let Some(content) = pick(&cols, &["message_content", "content"]) else {
        return (Vec::new(), false);
    };
    let local_id = pick(&cols, &["local_id", "localId", "id"]).unwrap_or("rowid");
    let kind_col = pick(&cols, &["local_type", "type"]);
    let sender_col = pick(&cols, &["real_sender_id", "sender_id", "sender"]);
    let compress_col = pick(&cols, &["WCDB_CT_message_content", "compress_flag"]);

    let projection = format!(
        "{}, {}, {}, {}, {}, {}",
        quote(local_id),
        quote(create_time),
        quote(content),
        kind_col.map(quote).unwrap_or_else(|| "0".to_string()),
        sender_col.map(quote).unwrap_or_else(|| "NULL".to_string()),
        compress_col
            .map(quote)
            .unwrap_or_else(|| "NULL".to_string()),
    );

    let mut seen: HashSet<i64> = HashSet::new();
    let mut rows_out = Vec::new();
    let mut truncated = false;

    let like = format!("%{}%", keyword);
    let sql = format!(
        "SELECT {} FROM {} WHERE {} LIKE ?1 ORDER BY {} DESC LIMIT {}",
        projection,
        quote(table),
        quote(content),
        quote(create_time),
        limit + 1
    );
    if let Ok(mut stmt) = conn.prepare(&sql) {
        if let Ok(rows) = stmt.query_map(params![like], |row| {
            let local: i64 = row.get(0).unwrap_or(0);
            let time: i64 = row.get(1).unwrap_or(0);
            let raw = value_to_bytes(row.get_ref(2)?).filter(|b| !b.is_empty());
            let kind: i64 = row.get(3).unwrap_or(0);
            let sender_id: Option<i64> = row.get(4).ok();
            let compress: Option<i64> = row.get(5).ok();
            Ok((local, time, raw, kind, sender_id, compress))
        }) {
            for (local, time, raw, kind, sender_id, compress) in rows.flatten() {
                if rows_out.len() >= limit {
                    truncated = true;
                    break;
                }
                if !seen.insert(local) {
                    continue;
                }
                let stored = sender_id.and_then(|id| senders.get(&id).cloned());
                let (sender, text) = prepare_text(kind, raw, compress, group, stored);
                if !text.to_lowercase().contains(&keyword.to_lowercase()) {
                    continue;
                }
                let sender_name = sender.as_ref().and_then(|s| names.get(s).cloned());
                rows_out.push(HitRow {
                    time,
                    text,
                    sender,
                    sender_name,
                });
            }
        }
    }

    // Large messages are stored zstd-compressed, so `LIKE` cannot see them.
    if let Some(compress) = compress_col {
        if rows_out.len() < limit {
            let sql = format!(
                "SELECT {} FROM {} WHERE {} = 4 ORDER BY {} DESC LIMIT 400",
                projection,
                quote(table),
                quote(compress),
                quote(create_time)
            );
            if let Ok(mut stmt) = conn.prepare(&sql) {
                if let Ok(rows) = stmt.query_map([], |row| {
                    let local: i64 = row.get(0).unwrap_or(0);
                    let time: i64 = row.get(1).unwrap_or(0);
                    let raw = value_to_bytes(row.get_ref(2)?).filter(|b| !b.is_empty());
                    let kind: i64 = row.get(3).unwrap_or(0);
                    let sender_id: Option<i64> = row.get(4).ok();
                    Ok((local, time, raw, kind, sender_id))
                }) {
                    for (local, time, raw, kind, sender_id) in rows.flatten() {
                        if rows_out.len() >= limit {
                            truncated = true;
                            break;
                        }
                        if !seen.insert(local) {
                            continue;
                        }
                        let stored = sender_id.and_then(|id| senders.get(&id).cloned());
                        let (sender, text) = prepare_text(kind, raw, Some(4), group, stored);
                        if !text.to_lowercase().contains(&keyword.to_lowercase()) {
                            continue;
                        }
                        let sender_name = sender.as_ref().and_then(|s| names.get(s).cloned());
                        rows_out.push(HitRow {
                            time,
                            text,
                            sender,
                            sender_name,
                        });
                    }
                }
            }
        }
    }

    (rows_out, truncated)
}

/// Search message content across conversations.
///
/// Only the most active `max_sessions` conversations are scanned, and each conversation
/// contributes at most 50 hits. The report states how many were scanned so the caller can
/// tell the user this is a bounded search rather than an exhaustive one.
pub fn search(
    cache: &Path,
    keyword: &str,
    groups_only: bool,
    max_sessions: usize,
) -> Result<SearchReport, String> {
    search_filtered(cache, keyword, groups_only, max_sessions, None)
}

/// Search message content while enforcing an optional account-level whitelist.
pub fn search_filtered(
    cache: &Path,
    keyword: &str,
    groups_only: bool,
    max_sessions: usize,
    allowed_usernames: Option<&HashSet<String>>,
) -> Result<SearchReport, String> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Err("请输入要搜索的关键字。".into());
    }
    let all = sessions_filtered(cache, allowed_usernames)?;
    let total = all.len();
    let mut candidates: Vec<SessionItem> = all
        .into_iter()
        .filter(|s| !groups_only || s.is_group)
        .collect();
    let max_sessions = max_sessions.clamp(1, 500);
    candidates.truncate(max_sessions);
    let scanned = candidates.len();

    let names = contacts(cache);
    let mut hits: HashMap<String, SearchHit> = HashMap::new();

    for shard in message_shards(cache) {
        let Ok(conn) = open_readonly(&shard) else {
            continue;
        };
        let available = tables(&conn);
        let available_set: HashSet<&String> = available.iter().collect();
        let senders = shard_senders(&conn);
        for session in &candidates {
            let table = format!("Msg_{}", md5_hex(&session.username));
            if !available_set.contains(&table) {
                continue;
            }
            let (rows, truncated) = table_hits(
                &conn,
                &table,
                keyword,
                50,
                &senders,
                &names,
                session.is_group,
            );
            if rows.is_empty() {
                continue;
            }
            let entry = hits
                .entry(session.username.clone())
                .or_insert_with(|| SearchHit {
                    username: session.username.clone(),
                    display: session.display.clone(),
                    is_group: session.is_group,
                    hits: 0,
                    truncated: false,
                    last_time: session.last_time,
                    snippets: Vec::new(),
                });
            entry.hits += rows.len() as i64;
            entry.truncated |= truncated;
            for row in rows {
                if entry.snippets.len() >= 3 {
                    break;
                }
                entry.snippets.push(SearchSnippet {
                    time: row.time,
                    sender: row.sender,
                    sender_name: row.sender_name,
                    text: snippet_around(&row.text, keyword),
                });
            }
        }
    }

    let mut hits: Vec<SearchHit> = hits.into_values().collect();
    hits.sort_by(|a, b| {
        b.hits
            .cmp(&a.hits)
            .then(b.last_time.unwrap_or(0).cmp(&a.last_time.unwrap_or(0)))
    });
    Ok(SearchReport {
        keyword: keyword.to_string(),
        scanned,
        skipped: total.saturating_sub(scanned),
        hits,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    Todo,
    Decision,
    Issue,
    Question,
    Data,
}

struct DigestMessage {
    display: String,
    time: i64,
    who: String,
    text: String,
    category: Option<Category>,
}

fn classify(text: &str) -> Option<Category> {
    let lower = text.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    if has(&[
        "问题", "风险", "异常", "报错", "失败", "阻塞", "卡住", "延期", "延迟", "缺少", "缺口",
        "bug", "故障", "回滚",
    ]) {
        return Some(Category::Issue);
    }
    if has(&[
        "决定", "确定", "定了", "结论", "同意", "通过", "就按", "最终", "敲定", "拍板",
    ]) {
        return Some(Category::Decision);
    }
    if has(&[
        "需要",
        "请",
        "麻烦",
        "尽快",
        "务必",
        "安排",
        "负责",
        "跟进",
        "推进",
        "提交",
        "截止",
        "deadline",
        "之前完成",
        "今天",
        "明天",
        "后天",
        "本周",
        "下周",
        "月底",
    ]) {
        return Some(Category::Todo);
    }
    if has(&["http://", "https://", "www."]) {
        return Some(Category::Data);
    }
    if has(&[
        "%",
        "％",
        "万元",
        "完成率",
        "gmv",
        "roi",
        "成本",
        "预算",
        "收入",
        "转化",
    ]) {
        return Some(Category::Data);
    }
    let trimmed = text.trim_end();
    if trimmed.chars().count() >= 6 && (trimmed.ends_with('？') || trimmed.ends_with('?')) {
        return Some(Category::Question);
    }
    None
}

fn fmt_ts(ts: i64) -> String {
    let seconds = if ts > 1_000_000_000_000 {
        ts / 1000
    } else {
        ts
    };
    if seconds <= 0 {
        return "—".into();
    }
    match Local.timestamp_opt(seconds, 0).single() {
        Some(dt) => dt.format("%m-%d %H:%M").to_string(),
        None => format!("{seconds}"),
    }
}

fn push_section(out: &mut String, title: &str, items: &[&DigestMessage], multi: bool, max: usize) {
    if items.is_empty() {
        return;
    }
    out.push_str(&format!("## {title}（{}）\n\n", items.len()));
    for item in items.iter().take(max) {
        let prefix = if multi {
            format!("[{}] ", item.display)
        } else {
            String::new()
        };
        let text: String = item.text.chars().take(200).collect();
        let suffix = if item.text.chars().count() > 200 {
            "…"
        } else {
            ""
        };
        out.push_str(&format!(
            "- {}{} {}：{}{}\n",
            prefix,
            fmt_ts(item.time),
            item.who,
            text.replace('\n', " "),
            suffix
        ));
    }
    if items.len() > max {
        out.push_str(&format!("- （另有 {} 条未列出）\n", items.len() - max));
    }
    out.push('\n');
}

/// Build a rule-based work digest from selected conversations.
///
/// This is deterministic keyword/structure extraction, not semantic summarisation. The
/// output says so explicitly so nobody mistakes it for an AI-written summary.
pub fn digest(
    cache: &Path,
    usernames: &[String],
    since: Option<i64>,
    until: Option<i64>,
) -> Result<DigestReport, String> {
    digest_filtered(cache, usernames, since, until, None)
}

/// Build a digest while enforcing an optional account-level whitelist.
pub fn digest_filtered(
    cache: &Path,
    usernames: &[String],
    since: Option<i64>,
    until: Option<i64>,
    allowed_usernames: Option<&HashSet<String>>,
) -> Result<DigestReport, String> {
    if usernames.is_empty() {
        return Err("请先选择至少一个会话。".into());
    }
    let session_list = sessions_filtered(cache, allowed_usernames).unwrap_or_default();
    let me = self_username(cache);
    let lookup: HashMap<&str, &SessionItem> = session_list
        .iter()
        .map(|s| (s.username.as_str(), s))
        .collect();

    let mut all_messages: Vec<DigestMessage> = Vec::new();
    let mut used_sessions = 0usize;
    let mut capped: Vec<String> = Vec::new();
    let per_session_limit = 800usize;

    for username in usernames {
        let Some(session) = lookup.get(username.as_str()) else {
            continue;
        };
        let display = session.display.clone();
        let Ok(list) = messages_inner(cache, username, per_session_limit, None, None, None, me.as_deref())
        else {
            continue;
        };
        if list.len() == per_session_limit {
            capped.push(display.clone());
        }
        let mut kept = 0usize;
        for message in list {
            if let Some(since) = since {
                if message.time < since {
                    continue;
                }
            }
            if let Some(until) = until {
                if message.time > until {
                    continue;
                }
            }
            if !is_work_text(message.kind, &message.text) {
                continue;
            }
            let who = message
                .sender_name
                .or(message.sender)
                .unwrap_or_else(|| "未知".to_string());
            let category = classify(&message.text);
            all_messages.push(DigestMessage {
                display: display.clone(),
                time: message.time,
                who,
                text: message.text,
                category,
            });
            kept += 1;
        }
        if kept > 0 {
            used_sessions += 1;
        }
    }

    all_messages.sort_by_key(|m| m.time);

    let todo: Vec<&DigestMessage> = all_messages
        .iter()
        .filter(|m| m.category == Some(Category::Todo))
        .collect();
    let decisions: Vec<&DigestMessage> = all_messages
        .iter()
        .filter(|m| m.category == Some(Category::Decision))
        .collect();
    let issues: Vec<&DigestMessage> = all_messages
        .iter()
        .filter(|m| m.category == Some(Category::Issue))
        .collect();
    let questions: Vec<&DigestMessage> = all_messages
        .iter()
        .filter(|m| m.category == Some(Category::Question))
        .collect();
    let data: Vec<&DigestMessage> = all_messages
        .iter()
        .filter(|m| m.category == Some(Category::Data))
        .collect();

    let mut speakers: HashMap<&str, usize> = HashMap::new();
    for message in &all_messages {
        *speakers.entry(message.who.as_str()).or_default() += 1;
    }
    let mut speakers: Vec<(&str, usize)> = speakers.into_iter().collect();
    speakers.sort_by(|a, b| b.1.cmp(&a.1));

    let multi = used_sessions > 1;
    let range = match (all_messages.first(), all_messages.last()) {
        (Some(first), Some(last)) if first.time != last.time => {
            format!("{} ~ {}", fmt_ts(first.time), fmt_ts(last.time))
        }
        (Some(first), _) => fmt_ts(first.time),
        _ => "—".to_string(),
    };
    let session_names: Vec<String> = usernames
        .iter()
        .map(|u| {
            lookup
                .get(u.as_str())
                .map(|s| s.display.clone())
                .unwrap_or_else(|| u.clone())
        })
        .collect();

    let mut markdown = String::new();
    markdown.push_str("# 工作整理\n\n");
    markdown.push_str(&format!(
        "- 会话：{}（{} 个）\n",
        session_names.join("、"),
        used_sessions
    ));
    markdown.push_str(&format!("- 消息：{} 条（{}）\n", all_messages.len(), range));
    markdown.push_str(&format!(
        "- 生成时间：{}\n\n",
        Local::now().format("%Y-%m-%d %H:%M:%S")
    ));
    markdown.push_str(
        "> 以下内容是**关键词与结构规则**抽取，不是语义总结；可能漏掉上下文，也可能把闲聊判成待办。发布前请核对原文。\n\n",
    );

    push_section(&mut markdown, "待办与跟进", &todo, multi, 30);
    push_section(&mut markdown, "结论与决定", &decisions, multi, 20);
    push_section(&mut markdown, "问题与风险", &issues, multi, 20);
    push_section(&mut markdown, "待确认问题", &questions, multi, 15);
    push_section(&mut markdown, "数据与链接", &data, multi, 20);

    if !speakers.is_empty() {
        markdown.push_str(&format!("## 参与人（{}）\n\n", speakers.len()));
        for (who, count) in speakers.iter().take(15) {
            markdown.push_str(&format!("- {who}：{count} 条\n"));
        }
        if speakers.len() > 15 {
            markdown.push_str(&format!("- （另有 {} 人未列出）\n", speakers.len() - 15));
        }
        markdown.push('\n');
    }

    if !capped.is_empty() {
        markdown.push_str(&format!(
            "> 注意：{} 只读取了最近 {per_session_limit} 条消息，更早的内容未纳入。\n\n",
            capped.join("、")
        ));
    }

    if all_messages.is_empty() {
        markdown.push_str("_所选时间范围内没有可整理的文字消息。_\n");
    }

    Ok(DigestReport {
        markdown,
        sessions: used_sessions,
        messages: all_messages.len(),
        todo: todo.len(),
        decisions: decisions.len(),
        issues: issues.len(),
        questions: questions.len(),
        data: data.len(),
    })
}

const SUMMARY_MAX_CHARS: usize = 45_000;
const SUMMARY_MAX_MESSAGES_PER_SESSION: usize = 400;
const SUMMARY_MAX_MESSAGE_CHARS: usize = 1_500;
const SUMMARY_FETCH_LIMIT: usize = 500;

/// One message handed to an agent. The interactive flow builds these from the preview the
/// user just reviewed; the automated flow builds them from the decrypted cache.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SummaryMessage {
    pub username: String,
    pub display: String,
    pub is_group: bool,
    pub time: i64,
    pub sender: String,
    pub text: String,
}

/// Format reviewed messages into a bounded transcript grouped by conversation.
///
/// Bounds are enforced here rather than at each call site: per message, per conversation,
/// and for the whole prompt. `truncated` is reported so neither the prompt nor the UI can
/// pretend the material was complete.
pub fn summarize_messages(messages: Vec<SummaryMessage>) -> SummaryInput {
    let mut order: Vec<String> = Vec::new();
    let mut grouped: HashMap<String, Vec<SummaryMessage>> = HashMap::new();
    for message in messages {
        if !grouped.contains_key(&message.username) {
            order.push(message.username.clone());
        }
        grouped
            .entry(message.username.clone())
            .or_default()
            .push(message);
    }

    let mut transcript = String::new();
    let mut sessions = 0usize;
    let mut total = 0usize;
    let mut truncated = false;

    for username in order {
        if transcript.chars().count() >= SUMMARY_MAX_CHARS {
            truncated = true;
            break;
        }
        let Some(mut group) = grouped.remove(&username) else {
            continue;
        };
        group.sort_by_key(|message| message.time);
        let display = group[0].display.clone();
        let is_group = group[0].is_group;
        let mut section = String::new();
        let mut included = 0usize;

        for message in group {
            let text = message.text.replace(['\r', '\n'], " ");
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            if included >= SUMMARY_MAX_MESSAGES_PER_SESSION {
                truncated = true;
                break;
            }
            let text: String = text.chars().take(SUMMARY_MAX_MESSAGE_CHARS).collect();
            if text.chars().count() >= SUMMARY_MAX_MESSAGE_CHARS {
                truncated = true;
            }
            let line = format!("[{}] {}：{}\n", fmt_ts(message.time), message.sender, text);
            if transcript.chars().count() + section.chars().count() + line.chars().count()
                > SUMMARY_MAX_CHARS
            {
                truncated = true;
                break;
            }
            section.push_str(&line);
            included += 1;
        }

        if included == 0 {
            continue;
        }
        transcript.push_str(&format!(
            "【会话】{}{}（{} 条）\n",
            display,
            if is_group { "（群聊）" } else { "" },
            included
        ));
        transcript.push_str(&section);
        transcript.push('\n');
        sessions += 1;
        total += included;
    }

    SummaryInput {
        characters: transcript.chars().count(),
        transcript,
        sessions,
        messages: total,
        truncated,
    }
}

/// Build the transcript handed to a local agent from the decrypted cache.
///
/// Whitelist enforcement is the same as every other read path: callers pass the account's
/// allowed usernames, and sessions outside that set are never read.
pub fn summary_input(
    cache: &Path,
    usernames: &[String],
    since: Option<i64>,
    until: Option<i64>,
    allowed_usernames: Option<&HashSet<String>>,
) -> Result<SummaryInput, String> {
    if usernames.is_empty() {
        return Err("请先选择至少一个会话。".into());
    }
    let session_list = sessions_filtered(cache, allowed_usernames).unwrap_or_default();
    let me = self_username(cache);
    let lookup: HashMap<&str, &SessionItem> = session_list
        .iter()
        .map(|session| (session.username.as_str(), session))
        .collect();

    let mut collected: Vec<SummaryMessage> = Vec::new();
    let mut truncated = false;
    for username in usernames {
        let Some(session) = lookup.get(username.as_str()) else {
            continue;
        };
        let Ok(list) = messages_inner(cache, username, SUMMARY_FETCH_LIMIT, None, None, None, me.as_deref())
        else {
            continue;
        };
        if list.len() == SUMMARY_FETCH_LIMIT {
            truncated = true;
        }
        for message in list {
            if since.is_some_and(|since| message.time < since)
                || until.is_some_and(|until| message.time > until)
            {
                continue;
            }
            if !is_work_text(message.kind, &message.text) {
                continue;
            }
            let sender = message
                .sender_name
                .or(message.sender)
                .unwrap_or_else(|| "未知".to_string());
            collected.push(SummaryMessage {
                username: username.clone(),
                display: session.display.clone(),
                is_group: session.is_group,
                time: message.time,
                sender,
                text: message.text,
            });
        }
    }

    if collected.is_empty() {
        return Err("所选范围内没有可总结的文字消息。".into());
    }
    // Keep the most recent messages per conversation: an old backlog is both less useful
    // and more likely to blow the prompt budget.
    let per_session_cap = SUMMARY_MAX_MESSAGES_PER_SESSION;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut kept: Vec<SummaryMessage> = Vec::new();
    for message in collected.iter() {
        let count = counts.entry(message.username.as_str()).or_default();
        if *count >= per_session_cap {
            truncated = true;
            continue;
        }
        *count += 1;
        kept.push(message.clone());
    }
    kept.reverse();

    let mut input = summarize_messages(kept);
    input.truncated |= truncated;
    if input.messages == 0 {
        return Err("所选范围内没有可总结的文字消息。".into());
    }
    Ok(input)
}

/// Absence is separate from unreadable/corrupt data, which must never trigger a cloud read.
pub fn chat_present(cache:&Path,username:&str)->Result<bool,String>{
    let entries=match std::fs::read_dir(cache.join("message")) {
        Ok(v)=>v,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(false),Err(e)=>return Err(format!("本地消息目录无法读取：{e}")),
    };
    let table=format!("Msg_{}",md5_hex(username));let mut count=0;
    for entry in entries {
        let entry=entry.map_err(|e|e.to_string())?;let name=entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("message_") || !name.ends_with(".db") {continue;}
        count+=1;if count>64{return Err("本地消息分片过多，请缩小读取范围。".into());}
        let c=open_readonly(&entry.path())?;
        let present:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",[&table],|r|r.get(0)).map_err(|e|format!("本地消息库无法读取：{e}"))?;
        if present {return Ok(true);}
    }
    Ok(false)
}
