//! Read-only access to the local WhatsApp Mac app's Core Data store.
//!
//! WhatsApp keeps the chat database in plaintext SQLite inside its group container
//! (`~/Library/Group Containers/group.net.whatsapp.WhatsApp.shared/ChatStorage.sqlite`),
//! so unlike WeChat there is no key capture, no decryption and no re-signing. The live
//! database is never opened directly: every read works on a copy under the app data
//! directory, which also replays the source `-wal` without touching the original files.
//!
//! The Core Data schema moves between versions, so every column is resolved with
//! `PRAGMA table_info`; a missing required column is reported instead of being papered
//! over with an empty result.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::settings;
use crate::wechat::query::{CommunityInfo, MessageCursor, MessageItem, SearchHit, SearchReport, SearchSnippet, SessionItem};

/// Seconds between the Unix epoch and Core Data's reference date (2001-01-01 UTC).
const CORE_DATA_EPOCH: i64 = 978_307_200;

pub const APP_BUNDLE: &str = "/Applications/WhatsApp.app";
const GROUP_CONTAINER: &str = "Library/Group Containers/group.net.whatsapp.WhatsApp.shared";
const APP_CONTAINER: &str = "Library/Containers/net.whatsapp.WhatsApp";
const DB_FILE: &str = "ChatStorage.sqlite";
const PREFERENCES: &str = "Library/Group Containers/group.net.whatsapp.WhatsApp.shared/Library/Preferences/group.net.whatsapp.WhatsApp.shared.plist";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// `8615652425800@s.whatsapp.net`
    pub id: String,
    pub phone: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Status {
    pub installed: bool,
    pub app_path: Option<String>,
    pub version: Option<String>,
    pub data_dir: Option<String>,
    pub account: Option<String>,
    pub name: Option<String>,
    pub sessions: usize,
    pub messages: usize,
    pub last_message_at: Option<i64>,
    /// Set when the app is present but the database cannot be read (permissions, new schema).
    pub error: Option<String>,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn data_dir() -> Option<PathBuf> {
    home().map(|home| home.join(GROUP_CONTAINER))
}

pub fn app_data_dir() -> Option<PathBuf> {
    home().map(|home| home.join(APP_CONTAINER))
}

pub fn is_installed() -> bool {
    data_dir().is_some_and(|dir| dir.join(DB_FILE).is_file()) || Path::new(APP_BUNDLE).is_dir()
}

/// A WhatsApp account id is always an E.164 JID; WeChat ids never contain `@`.
pub fn is_account(id: &str) -> bool {
    let Some((phone, domain)) = id.split_once('@') else {
        return false;
    };
    domain == "s.whatsapp.net"
        && phone.len() >= 8
        && phone.len() <= 16
        && phone.chars().all(|c| c.is_ascii_digit())
}

/// The account currently logged in, read from the app's own preferences.
pub fn account() -> Option<Account> {
    let jid = std::process::Command::new("plutil")
        .args(["-extract", "LastOwnJabberID", "raw", "-o", "-"])
        .arg(home()?.join(PREFERENCES))
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| is_account(value))
        .or_else(phone_from_cask_dir);
    let id = jid?;
    let phone = id.split('@').next().unwrap_or_default().to_string();
    let name = std::process::Command::new("plutil")
        .args(["-extract", "FullUserName", "raw", "-o", "-"])
        .arg(home()?.join(PREFERENCES))
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty());
    Some(Account { id, phone, name })
}

/// `~/Library/Containers/net.whatsapp.WhatsApp/Data/Documents/cask/<phone>/`
fn phone_from_cask_dir() -> Option<String> {
    let cask = app_data_dir()?.join("Data/Documents/cask");
    for entry in fs::read_dir(cask).ok()? {
        let entry = entry.ok()?;
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.path().is_dir() && name.len() >= 8 && name.chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("{name}@s.whatsapp.net"));
        }
    }
    None
}

pub fn bundle_version() -> Option<String> {
    std::process::Command::new("plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(Path::new(APP_BUNDLE).join("Contents/Info.plist"))
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn meta_path(cache: &Path) -> PathBuf {
    cache.join("_source.json")
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct SourceSignature {
    mtime: u64,
    size: u64,
    wal_mtime: u64,
    wal_size: u64,
}

fn signature(path: &Path) -> SourceSignature {
    let stat = |path: &Path| -> (u64, u64) {
        let Ok(meta) = fs::metadata(path) else {
            return (0, 0);
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|value| value.as_millis() as u64)
            .unwrap_or_default();
        (mtime, meta.len())
    };
    let (mtime, size) = stat(path);
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    let (wal_mtime, wal_size) = stat(&wal);
    SourceSignature { mtime, size, wal_mtime, wal_size }
}

fn copy_if_changed(from: &Path, to: &Path) -> Result<(), String> {
    if !from.is_file() {
        return Ok(());
    }
    let temp = to.with_extension("copying");
    fs::copy(from, &temp).map_err(|error| format!("复制 WhatsApp 数据失败：{error}"))?;
    fs::rename(&temp, to).map_err(|error| format!("写入 WhatsApp 副本失败：{error}"))?;
    Ok(())
}

pub fn source_db() -> Result<PathBuf, String> {
    // Escape hatch for tests and for a container moved outside the default group path.
    if let Some(value) = std::env::var_os("WECHATIFY_WHATSAPP_DB") {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Ok(path);
        }
    }
    let source = data_dir()
        .ok_or("找不到 WhatsApp 数据目录。")?
        .join(DB_FILE);
    match fs::metadata(&source) {
        Ok(metadata) if metadata.is_file() => Ok(source),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            Err(format!("WHATSAPP_ACCESS_DENIED: 无法读取 WhatsApp 本地数据：{error}"))
        }
        _ => Err("未找到 WhatsApp 的 ChatStorage.sqlite，请先打开并登录 WhatsApp。".into()),
    }
}

/// Copy the live store into the app data directory and return the copy's path.
///
/// `-wal`/`-shm` are copied next to the database so a replica that was not checkpointed
/// still reads the transactions WhatsApp had already committed. The copy is only redone
/// when the source changed, which keeps a 30-second sync from rewriting gigabytes.
static SNAPSHOT_READ:std::sync::Mutex<()>=std::sync::Mutex::new(());
pub fn snapshot(app_data: &Path) -> Result<PathBuf, String> {
    let _guard=SNAPSHOT_READ.lock().map_err(|_|"无法锁定 WhatsApp 副本。")?;
    snapshot_from(&source_db()?, app_data)
}

fn snapshot_from(source: &Path, app_data: &Path) -> Result<PathBuf, String> {
    if !source.is_file() {
        return Err("未找到 WhatsApp 的 ChatStorage.sqlite，请先打开并登录 WhatsApp。".into());
    }
    let cache = crate::local_scope::root(app_data).join("whatsapp-cache");
    fs::create_dir_all(&cache).map_err(|error| format!("创建 WhatsApp 缓存目录失败：{error}"))?;
    let target = cache.join(DB_FILE);
    let source_signature = signature(source);
    let meta_file = meta_path(&cache);
    let cached: Option<SourceSignature> = fs::read_to_string(&meta_file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let stale = cached.as_ref() != Some(&source_signature) || !target.is_file();
    if stale {
        // A leftover WAL from an older database would replay the wrong frames; drop it
        // before the new pair is written.
        for suffix in ["-wal", "-shm"] {
            let _ = fs::remove_file(PathBuf::from(format!("{}{suffix}", target.display())));
        }
        copy_if_changed(source, &target)?;
        for suffix in ["-wal", "-shm"] {
            let from = PathBuf::from(format!("{}{suffix}", source.display()));
            let to = PathBuf::from(format!("{}{suffix}", target.display()));
            copy_if_changed(&from, &to)?;
        }
        let _ = fs::write(&meta_file, serde_json::to_vec(&source_signature).unwrap_or_default());
    }
    Ok(target)
}

/// Open a readable connection on the snapshot. Every public reader goes through here so
/// the live database is never held open by the app.
struct SnapshotConnection {connection:Connection,_guard:std::sync::MutexGuard<'static,()>}
impl std::ops::Deref for SnapshotConnection {type Target=Connection;fn deref(&self)->&Connection {&self.connection}}
fn open(app_data: &Path) -> Result<SnapshotConnection, String> {
    let guard=SNAPSHOT_READ.lock().map_err(|_|"无法锁定 WhatsApp 副本。")?;
    let db = snapshot_from(&source_db()?,app_data)?;
    Ok(SnapshotConnection {connection:open_readonly(&db)?,_guard:guard})
}

fn open_readonly(path: &Path) -> Result<Connection, String> {
    // The copy has to be opened read-write: recycling a hot `-wal` needs write access to
    // the companion files. It is our own copy, never the live database.
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("无法打开 WhatsApp 数据库副本：{error}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|error| format!("初始化 WhatsApp 数据库失败：{error}"))?;
    Ok(conn)
}

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |_| Ok(()),
    )
    .is_ok()
}

fn columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(mut statement) = conn.prepare(&format!("PRAGMA table_info(\"{table}\")")) else {
        return names;
    };
    if let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(1)) {
        names.extend(rows.flatten());
    }
    names
}

fn require<'a>(columns: &'a [String], names: &[&str], table: &str) -> Result<&'a str, String> {
    for name in names {
        if let Some(found) = columns.iter().find(|column| column.eq_ignore_ascii_case(name)) {
            return Ok(found.as_str());
        }
    }
    Err(format!("WHATSAPP_DATA_UNSUPPORTED: 这个 WhatsApp 版本的 {table} 缺少 {names:?} 字段，暂不支持读取。"))
}

fn core_data_time(value: f64) -> i64 {
    (value + CORE_DATA_EPOCH as f64) as i64
}

/// Remove the bidi control characters WhatsApp wraps around some push names.
fn clean(value: &str) -> String {
    value
        .chars()
        .filter(|c| !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}'))
        .collect::<String>()
        .trim()
        .to_string()
}

fn is_group(chat: &str) -> bool {
    chat.ends_with("@g.us")
}

fn is_visible(chat: &str) -> bool {
    !chat.is_empty()
        && !chat.starts_with("status@")
        && !chat.ends_with("@broadcast")
        && !chat.ends_with("@newsletter")
        && (is_group(chat) || chat.ends_with("@s.whatsapp.net") || chat.ends_with("@lid"))
}

/// Sessions newest first, mapped onto the same shape the WeChat reader returns.
pub fn sessions(app_data: &Path) -> Result<Vec<SessionItem>, String> {
    sessions_from(&*open(app_data)?)
}

fn sessions_from(conn: &Connection) -> Result<Vec<SessionItem>, String> {sessions_page_from(conn,None,None,0)}
pub fn sessions_page(data:&Path,allowed:&HashSet<String>,limit:usize,offset:usize)->Result<Vec<SessionItem>,String>{sessions_page_from(&*open(data)?,Some(allowed),Some(limit),offset)}
fn sessions_page_from(conn:&Connection,allowed:Option<&HashSet<String>>,limit:Option<usize>,offset:usize)->Result<Vec<SessionItem>,String>{
    if !table_exists(conn, "ZWACHATSESSION") {
        return Err("WHATSAPP_DATA_UNSUPPORTED: WhatsApp 数据版本不受支持（缺少 ZWACHATSESSION）。".into());
    }
    let cols = columns(conn, "ZWACHATSESSION");
    let jid = require(&cols, &["ZCONTACTJID"], "ZWACHATSESSION")?.to_string();
    let name = cols.iter().find(|column| *column == "ZPARTNERNAME").cloned();
    let last = cols.iter().find(|column| *column == "ZLASTMESSAGEDATE").cloned();
    let preview = cols.iter().find(|column| *column == "ZLASTMESSAGETEXT").cloned();
    let unread = cols.iter().find(|column| *column == "ZUNREADCOUNT").cloned();
    let expr = |column: Option<String>| column.map(|value| format!("s.\"{value}\"")).unwrap_or_else(|| "NULL".into());
    let group_members = if table_exists(&conn, "ZWAGROUPMEMBER") {
        "COALESCE((SELECT COUNT(*) FROM ZWAGROUPMEMBER m WHERE m.ZCHATSESSION = s.Z_PK AND COALESCE(m.ZISACTIVE, 1) = 1), 0)"
    } else {
        "0"
    };
    let message_cols = columns(conn, "ZWAMESSAGE");
    let has_last_message = cols.iter().any(|column| column == "ZLASTMESSAGE")
        && ["Z_PK", "ZCHATSESSION", "ZMESSAGETYPE", "ZTEXT"]
            .iter().all(|name| message_cols.iter().any(|column| column == name));
    let message_fields = if has_last_message {
        if message_cols.iter().any(|column| column == "ZMEDIAITEM") {
            "m.Z_PK, m.ZMESSAGETYPE, m.ZTEXT, m.ZMEDIAITEM"
        } else {
            "m.Z_PK, m.ZMESSAGETYPE, m.ZTEXT, NULL"
        }
    } else {
        "NULL, NULL, NULL, NULL"
    };
    let message_join = if has_last_message {
        "LEFT JOIN ZWAMESSAGE m ON m.Z_PK = s.ZLASTMESSAGE AND m.ZCHATSESSION = s.Z_PK"
    } else {
        ""
    };
    let representative_order = if cols.iter().any(|column| column == "ZLASTMESSAGEDATE") {
        "duplicate.ZLASTMESSAGEDATE DESC, duplicate.Z_PK DESC"
    } else {
        "duplicate.Z_PK DESC"
    };
    let allowed_clause = allowed.map(|allowed| format!(" AND s.\"{jid}\" IN ({})", allowed.iter().map(|value| format!("'{}'", value.replace("'", "''"))).collect::<Vec<_>>().join(","))).unwrap_or_default();
    let sql = format!(
        "SELECT s.\"{jid}\", {}, {}, {}, {}, {group_members}, {message_fields} FROM ZWACHATSESSION s {message_join} \
         WHERE s.\"{jid}\" IS NOT NULL AND s.\"{jid}\" != '' {allowed_clause} \
         AND s.Z_PK = (SELECT duplicate.Z_PK FROM ZWACHATSESSION duplicate WHERE duplicate.\"{jid}\" = s.\"{jid}\" ORDER BY {representative_order} LIMIT 1) \
         ORDER BY {} DESC",
        expr(name),
        expr(last.clone()),
        expr(preview),
        expr(unread),
        last.map(|value| format!("s.\"{value}\"")).unwrap_or_else(|| "0".into()),
    );
    let sql=if let Some(limit)=limit {format!("{sql}, s.Z_PK DESC LIMIT {} OFFSET {offset}",limit.clamp(1,100))} else {sql};
    let mut statement = conn.prepare(&sql).map_err(|error| format!("读取 WhatsApp 会话失败：{error}"))?;
    let rows = statement
        .query_map([], |row| {
            let chat: String = row.get(0)?;
            let display: Option<String> = row.get(1)?;
            let last: Option<f64> = row.get(2)?;
            let preview: Option<String> = row.get(3)?;
            let unread: Option<i64> = row.get(4)?;
            let members: i64 = row.get(5)?;
            let message_id: Option<i64> = row.get(6)?;
            let kind: Option<i64> = row.get(7)?;
            let text: Option<String> = row.get(8)?;
            let media: Option<i64> = row.get(9)?;
            Ok((chat, display, last, preview, unread, members, message_id, kind, text, media))
        })
        .map_err(|error| format!("读取 WhatsApp 会话失败：{error}"))?;
    let raw: Vec<_> = rows.flatten().collect();
    let media_ids: Vec<_> = raw.iter().filter_map(|row| row.9).collect();
    let mut media = HashMap::new();
    for chunk in media_ids.chunks(400) {
        media.extend(media_info(conn, chunk));
    }
    let mut items = Vec::new();
    for (chat, display, last, preview, unread, members, message_id, kind, text, media_id) in raw {
        if !is_visible(&chat) {
            continue;
        }
        let display = clean(&display.unwrap_or_default());
        let message_preview = message_id.map(|_| render_text(kind.unwrap_or(0), text.as_deref().unwrap_or_default(), media_id.and_then(|id| media.get(&id))));
        let preview = message_preview.filter(|value| !value.is_empty())
            .or_else(|| preview.map(|value| render_preview(&value)))
            .map(|value| clean(&value)).filter(|value| !value.is_empty());
        items.push(SessionItem {
            display: if display.is_empty() { chat.clone() } else { display },
            is_group: is_group(&chat),
            member_count: is_group(&chat).then_some(members),
            last_time: last.map(core_data_time),
            preview,
            unread,
            username: chat,
            community: None,
        });
    }
    attach_communities(conn, &mut items);
    Ok(items)
}

/// Community metadata comes from WhatsApp's own group-event relationships, never names.
/// Messages remain attached to the original JID and all access checks remain per session.
fn attach_communities(conn: &Connection, sessions: &mut [SessionItem]) {
    let cols = columns(conn, "ZWACHATSESSION");
    if !cols.iter().any(|column| column == "ZSESSIONTYPE") {
        return;
    }
    let Ok(mut statement) = conn.prepare("SELECT ZCONTACTJID FROM ZWACHATSESSION WHERE ZSESSIONTYPE = 4") else { return };
    let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(0)) else { return };
    let parents: HashSet<String> = rows.flatten().collect();
    let parent_names: HashMap<String, String> = sessions.iter().filter(|s| parents.contains(&s.username))
        .map(|s| (s.username.clone(), s.display.clone())).collect();
    let mut memberships: HashMap<String, HashSet<String>> = HashMap::new();
    if table_exists(conn, "ZWAMESSAGE") {
        let sql = "SELECT s.ZCONTACTJID, m.ZTEXT FROM ZWAMESSAGE m JOIN ZWACHATSESSION s ON s.Z_PK = m.ZCHATSESSION WHERE m.ZMESSAGETYPE = 6 AND m.ZTEXT IS NOT NULL";
        if let Ok(mut statement) = conn.prepare(sql) {
            if let Ok(rows) = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))) {
                for (chat, text) in rows.flatten() {
                    let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
                    if let Some(parent) = event.get("parent_group_jid").and_then(|value| value.as_str()).filter(|parent| parent_names.contains_key(*parent)) {
                        if chat != parent { memberships.entry(chat.clone()).or_default().insert(parent.to_string()); }
                    }
                    if parent_names.contains_key(&chat) {
                        if let Some(groups) = event.get("linked_groups").and_then(|value| value.as_array()) {
                            for group in groups {
                                if let Some(jid) = group.get("groupJID").and_then(|value| value.as_str()).filter(|jid| *jid != chat) {
                                    memberships.entry(jid.to_string()).or_default().insert(chat.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    for session in sessions {
        let parent = if parent_names.contains_key(&session.username) {
            Some(session.username.as_str())
        } else {
            memberships.get(&session.username).filter(|ids| ids.len() == 1).and_then(|ids| ids.iter().next()).map(String::as_str)
        };
        if let Some(parent) = parent {
            session.community = Some(CommunityInfo {
                id: parent.to_string(),
                name: parent_names[parent].clone(),
                is_parent: parent == session.username,
            });
        }
    }
}

/// Newest-first page, matching `wechat_messages` so the chat pane can reuse its paging.
pub fn messages(
    app_data: &Path,
    chat: &str,
    limit: usize,
    before: Option<MessageCursor>,
) -> Result<Vec<MessageItem>, String> {
    messages_from(&*open(app_data)?, chat, limit, before, None, None)
}

pub fn messages_between(
    app_data: &Path,
    chat: &str,
    since: Option<i64>,
    until: Option<i64>,
    limit: usize,
) -> Result<Vec<MessageItem>, String> {
    messages_from(&*open(app_data)?, chat, limit, None, since, until)
}

fn messages_from(
    conn: &Connection,
    chat: &str,
    limit: usize,
    before: Option<MessageCursor>,
    since: Option<i64>,
    until: Option<i64>,
) -> Result<Vec<MessageItem>, String> {
    messages_ordered(conn,chat,limit,before,since,until,false,None)
}

pub fn upload_page(data: &Path, chat: &str, since: i64, until: Option<i64>, after: Option<MessageCursor>,limit:usize)->Result<Vec<MessageItem>,String>{
    messages_ordered(&*open(data)?,chat,limit,after,Some(since),until,true,None)
}

fn messages_ordered(conn:&Connection,chat:&str,limit:usize,before:Option<MessageCursor>,since:Option<i64>,until:Option<i64>,ascending:bool,selection:Option<&[crate::settings::MessageRef]>)->Result<Vec<MessageItem>,String>{
    if !table_exists(&conn, "ZWACHATSESSION") || !table_exists(&conn, "ZWAMESSAGE") {
        return Err("WHATSAPP_DATA_UNSUPPORTED: WhatsApp 数据版本不受支持（缺少会话或消息表）。".into());
    }
    let limit = limit.clamp(1, 5000);
    let session_pk: i64 = conn
        .query_row(
            "SELECT Z_PK FROM ZWACHATSESSION WHERE ZCONTACTJID = ?1 LIMIT 1",
            [chat],
            |row| row.get(0),
        )
        .optional().map_err(|e|format!("读取 WhatsApp 会话失败：{e}"))?
        .ok_or_else(||format!("LOCAL_CHAT_MISSING: WhatsApp 中找不到会话 {chat}。"))?;
    let cols = columns(&conn, "ZWAMESSAGE");
    let time = require(&cols, &["ZMESSAGEDATE"], "ZWAMESSAGE")?.to_string();
    let text = require(&cols, &["ZTEXT"], "ZWAMESSAGE")?.to_string();
    let kind = require(&cols, &["ZMESSAGETYPE"], "ZWAMESSAGE")?.to_string();
    let from_me = cols.iter().any(|column| column == "ZISFROMME");
    let sender = cols.iter().find(|column| *column == "ZFROMJID").cloned();
    let media = cols.iter().find(|column| *column == "ZMEDIAITEM").cloned();
    let group = is_group(chat);
    let member_columns = if group && cols.iter().any(|column| column == "ZGROUPMEMBER") {
        columns(conn, "ZWAGROUPMEMBER")
    } else {
        Vec::new()
    };
    let has_member_relation = ["Z_PK", "ZCHATSESSION", "ZMEMBERJID"]
        .iter()
        .all(|required| member_columns.iter().any(|column| column == required));
    let (member_jid, member_join) = if has_member_relation {
        (
            "g.ZMEMBERJID",
            " LEFT JOIN ZWAGROUPMEMBER g ON g.Z_PK = m.ZGROUPMEMBER \
             AND g.ZCHATSESSION = m.ZCHATSESSION",
        )
    } else {
        ("NULL", "")
    };
    let push_name = if group && cols.iter().any(|column| column == "ZPUSHNAME") {
        "m.ZPUSHNAME"
    } else {
        "NULL"
    };
    let mut sql = format!(
        "SELECT m.Z_PK, m.\"{time}\", m.\"{kind}\", m.\"{text}\", {}, {}, {}, {member_jid}, {push_name} \
         FROM ZWAMESSAGE m{member_join} WHERE m.ZCHATSESSION = ?1 AND m.\"{time}\" IS NOT NULL",
        if from_me { "m.ZISFROMME" } else { "0" },
        sender.map(|value| format!("m.\"{value}\"")).unwrap_or_else(|| "NULL".into()),
        media.map(|value| format!("m.\"{value}\"")).unwrap_or_else(|| "NULL".into()),
    );
    let mut params: Vec<i64> = vec![session_pk];
    if let Some(before) = before {
        // Core Data keeps fractional seconds; SQLite `CAST` truncates them toward zero, which
        // matches the Unix second handed to the UI. Paging and ordering must use the same
        // whole-second + row-id pair, otherwise a row could fall between two pages.
        let boundary = before.time.saturating_sub(CORE_DATA_EPOCH);
        sql.push_str(&format!(
            " AND (CAST(m.\"{time}\" AS INTEGER) < ?2 OR (CAST(m.\"{time}\" AS INTEGER) = ?2 AND m.Z_PK < ?3))"
        ));
        params.push(boundary);
        params.push(before.local_id);
    }
    if ascending {sql=sql.replace(" < "," > ");}
    if let Some(refs)=selection {
        if refs.is_empty() {return Ok(Vec::new());}
        if refs.len()>1000 {return Err("单批消息超过读取上限。".into());}
        sql.push_str(&format!(" AND m.Z_PK IN ({})",refs.iter().map(|m|m.local_id.to_string()).collect::<Vec<_>>().join(",")));
    }
    if let Some(since) = since {
        sql.push_str(&format!(" AND m.\"{time}\" >= ?{}", params.len() + 1));
        params.push(since.saturating_sub(CORE_DATA_EPOCH));
    }
    if let Some(until) = until {
        sql.push_str(&format!(" AND m.\"{time}\" < ?{}", params.len() + 1));
        params.push(until.saturating_sub(CORE_DATA_EPOCH));
    }
    sql.push_str(&format!(
        " ORDER BY CAST(m.\"{time}\" AS INTEGER) {order}, m.Z_PK {order} LIMIT {limit}",order=if ascending {"ASC"} else {"DESC"}
    ));
    let mut statement = conn.prepare(&sql).map_err(|error| format!("读取 WhatsApp 消息失败：{error}"))?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                row.get::<_, Option<i64>>(4)?.unwrap_or(0) == 1,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })
        .map_err(|error| format!("读取 WhatsApp 消息失败：{error}"))?;
    let raw: Vec<_> = rows.flatten().collect();
    let media = media_info(conn, &raw.iter().filter_map(|row| row.6).collect::<Vec<_>>());
    let names = name_map(conn);
    let partner = session_partner(conn, session_pk);
    let mut items = Vec::new();
    for (pk, time, kind, text, self_sent, from, media_pk, member, push_name) in raw {
        let rendered = render_text(kind, &text, media_pk.and_then(|id| media.get(&id)));
        if rendered.is_empty() {
            continue;
        }
        let sender = if group {
            // For group messages ZFROMJID can point to the conversation itself.
            member
                .into_iter()
                .chain(from)
                .map(|value| clean(&value))
                .find(|jid| jid.ends_with("@s.whatsapp.net") || jid.ends_with("@lid"))
        } else {
            from.map(|value| clean(&value)).filter(|value| !value.is_empty())
        };
        let sender_name = sender
            .as_ref()
            .and_then(|jid| names.get(jid).cloned())
            .or_else(|| {
                if group {
                    push_name
                        .map(|value| clean(&value))
                        .filter(|value| !value.is_empty())
                } else {
                    partner.clone()
                }
            });
        items.push(MessageItem {
            local_id: pk,
            server_id: 0,
            source_shard: "whatsapp".into(),
            time: core_data_time(time),
            sender,
            sender_name,
            kind,
            text: rendered,
            is_self: self_sent,
        });
    }
    if let Some(refs)=selection {if items.len()!=refs.len() || items.iter().any(|m|!refs.iter().any(|r|r.local_id==m.local_id && r.time==m.time)) {return Err("部分待上传消息已变化，请重新读取。".into());}}
    Ok(items)
}

fn session_partner(conn: &Connection, session_pk: i64) -> Option<String> {
    let value: Option<String> = conn
        .query_row(
            "SELECT ZPARTNERNAME FROM ZWACHATSESSION WHERE Z_PK = ?1",
            [session_pk],
            |row| row.get(0),
        )
        .ok()
        .flatten();
    value.map(|value| clean(&value)).filter(|value| !value.is_empty())
}

struct MediaInfo {
    title: String,
    vcard: String,
    latitude: f64,
    longitude: f64,
}

fn media_info(conn: &Connection, ids: &[i64]) -> HashMap<i64, MediaInfo> {
    let mut map = HashMap::new();
    if ids.is_empty() || !table_exists(&conn, "ZWAMEDIAITEM") {
        return map;
    }
    let cols = columns(&conn, "ZWAMEDIAITEM");
    let get = |name: &str| cols.iter().any(|column| column == name).then(|| format!("\"{name}\"")).unwrap_or_else(|| "NULL".into());
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let sql = format!(
        "SELECT Z_PK, {}, {}, {}, {} FROM ZWAMEDIAITEM WHERE Z_PK IN ({placeholders})",
        get("ZTITLE"),
        get("ZVCARDNAME"),
        get("ZLATITUDE"),
        get("ZLONGITUDE"),
    );
    let Ok(mut statement) = conn.prepare(&sql) else {
        return map;
    };
    let Ok(rows) = statement.query_map(rusqlite::params_from_iter(ids.iter()), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            row.get::<_, Option<f64>>(3)?.unwrap_or_default(),
            row.get::<_, Option<f64>>(4)?.unwrap_or_default(),
        ))
    }) else {
        return map;
    };
    for (id, title, vcard, latitude, longitude) in rows.flatten() {
        map.insert(id, MediaInfo { title: clean(&title), vcard: clean(&vcard), latitude, longitude });
    }
    map
}

fn name_map(conn: &Connection) -> HashMap<String, String> {
    let mut names = HashMap::new();
    if table_exists(&conn, "ZWAPROFILEPUSHNAME") {
        if let Ok(mut statement) = conn.prepare("SELECT ZJID, ZPUSHNAME FROM ZWAPROFILEPUSHNAME") {
            if let Ok(rows) = statement.query_map([], |row| {
                Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?))
            }) {
                for (jid, name) in rows.flatten() {
                    if let (Some(jid), Some(name)) = (jid, name) {
                        let name = clean(&name);
                        if !name.is_empty() {
                            names.insert(clean(&jid), name);
                        }
                    }
                }
            }
        }
    }
    if table_exists(&conn, "ZWAGROUPMEMBER") {
        if let Ok(mut statement) = conn.prepare(
            "SELECT ZMEMBERJID, ZCONTACTNAME, ZFIRSTNAME FROM ZWAGROUPMEMBER",
        ) {
            if let Ok(rows) = statement.query_map([], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            }) {
                for (jid, contact, first) in rows.flatten() {
                    let Some(jid) = jid else { continue };
                    let name = clean(&contact.unwrap_or_default());
                    let name = if name.is_empty() { clean(&first.unwrap_or_default()) } else { name };
                    if !name.is_empty() {
                        names.entry(clean(&jid)).or_insert(name);
                    }
                }
            }
        }
    }
    names
}

/// WhatsApp stores group events as JSON in the text column; everything else is plain text.
fn render_text(kind: i64, text: &str, media: Option<&MediaInfo>) -> String {
    let text = text.trim();
    let caption = media.map(|item| item.title.clone()).unwrap_or_default();
    let with_caption = |label: &str| {
        if caption.is_empty() { label.to_string() } else { format!("{label} {caption}") }
    };
    match kind {
        0 => text.to_string(),
        1 => with_caption("[图片]"),
        2 => with_caption("[视频]"),
        3 => "[语音]".to_string(),
        4 => {
            let name = media.map(|item| item.vcard.clone()).unwrap_or_default();
            if name.is_empty() { "[名片]".into() } else { format!("[名片] {name}") }
        }
        5 => {
            let point = media.filter(|item| item.latitude != 0.0 || item.longitude != 0.0);
            match point {
                Some(item) => format!("[位置] {:.4}, {:.4}", item.latitude, item.longitude),
                None => "[位置]".into(),
            }
        }
        6 => group_event(text).unwrap_or_else(|| "[群事件]".into()),
        8 => with_caption("[文件]"),
        20 => "[贴纸]".into(),
        // Type 10 carries encrypted-system housekeeping whose body is empty; the rest are
        // either plain text or a media type this build does not name yet.
        _ if !text.is_empty() && !text.starts_with('{') => text.to_string(),
        _ if media.is_some() => "[附件]".into(),
        _ => String::new(),
    }
}

/// Older databases may store plain text or a group-event JSON directly in this field.
fn render_preview(text: &str) -> String {
    let text = text.trim();
    if text.starts_with('{') {
        return group_event(text).unwrap_or_default();
    }
    if is_encoded_session_record(text) {
        return String::new();
    }
    render_text(0, text, None)
}

/// Newer stores can contain a Base64-encoded binary record, not a message body.
/// Recognize its UUID field rather than hiding arbitrary Base64-like user text.
fn is_encoded_session_record(text: &str) -> bool {
    use base64::Engine as _;
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(text) else { return false };
    bytes.windows(38).any(|field| {
        field[0..2] == [0x62, 0x24]
            && field[2..].iter().enumerate().all(|(index, byte)| {
                if matches!(index, 8 | 13 | 18 | 23) { *byte == b'-' } else { byte.is_ascii_hexdigit() }
            })
    }) && bytes.iter().any(|byte| *byte < 0x20 || *byte >= 0x7f)
}

fn group_event(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let subject = value.get("subject").and_then(|item| item.as_str()).unwrap_or_default();
    if subject.is_empty() {
        return None;
    }
    if value.get("is_parent_group_general_chat").and_then(|item| item.as_bool()) == Some(true) {
        return Some(format!("[群事件] 创建子群「{subject}」"));
    }
    Some(format!("[群事件]「{subject}」"))
}

/// Day buckets for the date picker, newest first, using local dates like the WeChat reader.
pub fn message_dates(app_data: &Path, chat: &str) -> Result<Vec<(String, usize)>, String> {
    message_dates_from(&*open(app_data)?, chat)
}

fn message_dates_from(conn: &Connection, chat: &str) -> Result<Vec<(String, usize)>, String> {
    if !table_exists(conn, "ZWACHATSESSION") || !table_exists(conn, "ZWAMESSAGE") {
        return Err("WHATSAPP_DATA_UNSUPPORTED: WhatsApp 数据版本不受支持。".into());
    }
    let session_pk: i64 = conn
        .query_row(
            "SELECT Z_PK FROM ZWACHATSESSION WHERE ZCONTACTJID = ?1 LIMIT 1",
            [chat],
            |row| row.get(0),
        )
        .map_err(|_| format!("WhatsApp 中找不到会话 {chat}。"))?;
    let mut statement = conn
        .prepare("SELECT ZMESSAGEDATE FROM ZWAMESSAGE WHERE ZCHATSESSION = ?1 AND ZMESSAGEDATE IS NOT NULL")
        .map_err(|error| format!("读取 WhatsApp 消息日期失败：{error}"))?;
    let rows = statement
        .query_map([session_pk], |row| row.get::<_, f64>(0))
        .map_err(|error| format!("读取 WhatsApp 消息日期失败：{error}"))?;
    let mut days: std::collections::BTreeMap<String, usize> = Default::default();
    for time in rows.flatten() {
        let Some(date) = chrono::DateTime::from_timestamp(core_data_time(time), 0)
            .map(|value| value.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
        else {
            continue;
        };
        *days.entry(date).or_default() += 1;
    }
    let mut list: Vec<(String, usize)> = days.into_iter().collect();
    list.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(list)
}

/// Only the keyword scan the chat pane needs; sessions are capped so a big store stays fast.
pub fn search(
    app_data: &Path,
    keyword: &str,
    max_sessions: usize,
    allowed: Option<&std::collections::HashSet<String>>,
) -> Result<SearchReport, String> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Err("请输入要搜索的关键词。".into());
    }
    let conn = open(app_data)?;
    let sessions = sessions_from(&conn)?;
    let mut report = SearchReport { keyword: keyword.to_string(), scanned: 0, skipped: 0, hits: Vec::new() };
    let needle = keyword.to_lowercase();
    let sessions = sessions.iter().filter(|session| allowed.is_none_or(|allowed| allowed.contains(&session.username)));
    for session in sessions.take(max_sessions.clamp(1, 500)) {
        let Ok(list) = messages_from(&conn, &session.username, 2000, None, None, None) else {
            report.skipped += 1;
            continue;
        };
        report.scanned += 1;
        let messages_truncated = list.len() >= 2000;
        let mut snippets = Vec::new();
        let mut hits = 0;
        for message in list {
            if !message.text.to_lowercase().contains(&needle) {
                continue;
            }
            hits += 1;
            if snippets.len() < 5 {
                snippets.push(SearchSnippet {
                    time: message.time,
                    sender: message.sender,
                    sender_name: message.sender_name,
                    text: message.text,
                });
            }
        }
        if hits > 0 {
            report.hits.push(SearchHit {
                username: session.username.clone(),
                display: session.display.clone(),
                is_group: session.is_group,
                hits,
                truncated: messages_truncated,
                last_time: session.last_time,
                snippets,
            });
        }
    }
    Ok(report)
}

/// Counts and newest activity, used by the connector card and the About page.
pub fn status(app_data: &Path) -> Status {
    let account = account();
    let mut status = Status {
        installed: is_installed(),
        app_path: Path::new(APP_BUNDLE).is_dir().then(|| APP_BUNDLE.to_string()),
        version: bundle_version(),
        data_dir: data_dir().map(|path| path.to_string_lossy().to_string()),
        account: account.as_ref().map(|value| value.id.clone()),
        name: account.and_then(|value| value.name),
        ..Default::default()
    };
    if !status.installed {
        status.error = Some("未检测到 WhatsApp，请先安装并登录一次。".into());
        return status;
    }
    match sessions(app_data) {
        Ok(list) => {
            status.sessions = list.len();
            status.last_message_at = list.iter().filter_map(|item| item.last_time).max();
            status.messages = open(app_data)
                .and_then(|conn| {
                    conn.query_row("SELECT COUNT(*) FROM ZWAMESSAGE", [], |row| row.get::<_, i64>(0))
                        .map(|count| count as usize)
                        .map_err(|error| error.to_string())
                })
                .unwrap_or_default();
        }
        Err(error) => status.error = Some(error),
    }
    status
}


/// One conversation prepared for an upload, messages oldest first.
pub struct UploadConversation {
    pub session: SessionItem,
    /// `(stable message key, message)` in upload order.
    pub entries: Vec<(String, MessageItem)>,
}

const UPLOAD_PAGE: usize = 500;
const UPLOAD_MESSAGE_LIMIT: usize = 5000;

/// Read every uploadable message of the given conversations in one snapshot.
///
/// The whole batch shares a single copy of the database: a multi-conversation upload must
/// not re-stat and re-copy the store once per chat. Pages run newest-first like the chat
/// reader, then the result is ordered oldest-first for the archive.
pub fn upload_conversations(
    app_data: &Path,
    usernames: &[String],
    since: Option<i64>,
    until: Option<i64>,
    selection: &BTreeMap<String, Vec<settings::MessageRef>>,
) -> Result<Vec<UploadConversation>, String> {
    if usernames.is_empty() {
        return Err("请至少选择一个会话。".into());
    }
    if since.zip(until).is_some_and(|(start, end)| start >= end) {
        return Err("上传日期范围无效。".into());
    }
    let conn = open(app_data)?;
    let all = sessions_from(&conn)?;
    let mut result = Vec::new();
    for username in usernames {
        let session = all
            .iter()
            .find(|item| &item.username == username)
            .cloned()
            .ok_or("部分所选会话已不可用，请重新选择。")?;
        let mut before = None;
        let mut messages: Vec<MessageItem> = Vec::new();
        if let Some(refs)=selection.get(username) {
            messages=messages_ordered(&conn,username,refs.len().max(1),None,since,until,false,Some(refs))?;
        } else {
        loop {
            let page = messages_from(&conn, username, UPLOAD_PAGE, before, None, None)?;
            if page.is_empty() {
                break;
            }
            let oldest = page.last().map(|message| message.time);
            before = page
                .last()
                .map(|message| MessageCursor { time: message.time, local_id: message.local_id, source_shard: "whatsapp".into() });
            messages.extend(page.into_iter().filter(|message| {
                since.map(|value| message.time >= value).unwrap_or(true)
                    && until.map(|value| message.time < value).unwrap_or(true)
            }));
            if messages.len() > UPLOAD_MESSAGE_LIMIT {
                return Err("此范围的消息超过 5000 条，请缩小日期范围后逐条选择。".into());
            }
            if since.zip(oldest).map(|(start, oldest)| oldest <= start).unwrap_or(false) {
                break;
            }
        }
        }
        if let Some(refs) = selection.get(username) {
            messages.retain(|message| refs.iter().any(|item| item.local_id == message.local_id && item.time == message.time));
        }
        messages.sort_by(|a, b| a.time.cmp(&b.time).then(a.local_id.cmp(&b.local_id)));
        let keys = stanza_keys(&conn, &messages);
        // A repeated stanza id would make the server merge two messages, so any duplicate
        // falls back to the row id instead of being uploaded under an ambiguous key.
        let mut used: HashSet<String> = HashSet::new();
        let entries = messages.into_iter().map(|message| {
            let mut key = keys
                .get(&message.local_id)
                .cloned()
                .filter(|value| !value.is_empty())
                .map(|value| format!("srv:{value}"))
                .unwrap_or_else(|| format!("local:{}:{}", message.local_id, message.time));
            if !used.insert(key.clone()) {
                key = format!("local:{}:{}", message.local_id, message.time);
                let mut suffix = 2;
                while !used.insert(key.clone()) {
                    key = format!("local:{}:{}:{suffix}", message.local_id, message.time);
                    suffix += 1;
                }
            }
            (key, message)
        }).collect();
        result.push(UploadConversation { session, entries });
    }
    Ok(result)
}

/// Server-assigned stanza ids for the selected rows; used as the stable upload key.
fn stanza_keys(conn: &Connection, messages: &[MessageItem]) -> HashMap<i64, String> {
    let mut keys = HashMap::new();
    if messages.is_empty() || !columns(conn, "ZWAMESSAGE").iter().any(|column| column == "ZSTANZAID") {
        return keys;
    }
    for chunk in messages.chunks(400) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!("SELECT Z_PK, ZSTANZAID FROM ZWAMESSAGE WHERE Z_PK IN ({placeholders})");
        let Ok(mut statement) = conn.prepare(&sql) else {
            return keys;
        };
        let ids = chunk.iter().map(|message| message.local_id);
        let Ok(rows) = statement.query_map(rusqlite::params_from_iter(ids), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?.unwrap_or_default()))
        }) else {
            return keys;
        };
        for (id, stanza) in rows.flatten() {
            let stanza = clean(&stanza);
            if !stanza.is_empty() {
                keys.insert(id, stanza);
            }
        }
    }
    keys
}

/// Messages sharing the given second, for a correct coverage boundary.
pub fn count_at_time(app_data: &Path, chat: &str, time: i64) -> Result<usize, String> {
    let conn = open(app_data)?;
    let session_pk: i64 = conn
        .query_row(
            "SELECT Z_PK FROM ZWACHATSESSION WHERE ZCONTACTJID = ?1 LIMIT 1",
            [chat],
            |row| row.get(0),
        )
        .map_err(|_| format!("WhatsApp 中找不到会话 {chat}。"))?;
    let stored = (time - CORE_DATA_EPOCH) as f64;
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM ZWAMESSAGE WHERE ZCHATSESSION = ?1 AND CAST(ZMESSAGEDATE AS INTEGER) = ?2",
            rusqlite::params![session_pk, stored],
            |row| row.get(0),
        )
        .map_err(|error| format!("读取 WhatsApp 消息数量失败：{error}"))?;
    Ok(count.max(0) as usize)
}


/// Messages newer than a `(time, local id)` position, used for the "new since last upload" badge.
pub fn count_after(app_data: &Path, chat: &str, time: i64, local_id: i64) -> Result<usize, String> {
    let conn = open(app_data)?;
    let session_pk: i64 = conn
        .query_row(
            "SELECT Z_PK FROM ZWACHATSESSION WHERE ZCONTACTJID = ?1 LIMIT 1",
            [chat],
            |row| row.get(0),
        )
        .map_err(|_| format!("WhatsApp 中找不到会话 {chat}。"))?;
    let stored = (time - CORE_DATA_EPOCH) as f64;
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM ZWAMESSAGE WHERE ZCHATSESSION = ?1 AND (CAST(ZMESSAGEDATE AS INTEGER) > ?2 OR (CAST(ZMESSAGEDATE AS INTEGER) = ?2 AND Z_PK > ?3))",
            rusqlite::params![session_pk, stored, local_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("读取 WhatsApp 消息数量失败：{error}"))?;
    Ok(count.max(0) as usize)
}

/// Newest `(time, local id)`; `None` when the conversation has no readable message.
pub fn latest_position(app_data: &Path, chat: &str) -> Result<Option<(i64, i64)>, String> {
    latest_position_from(&*open(app_data)?, chat)
}

fn latest_position_from(conn: &Connection, chat: &str) -> Result<Option<(i64, i64)>, String> {
    let session_pk: i64 = conn
        .query_row(
            "SELECT Z_PK FROM ZWACHATSESSION WHERE ZCONTACTJID = ?1 LIMIT 1",
            [chat],
            |row| row.get(0),
        )
        .map_err(|_| format!("WhatsApp 中找不到会话 {chat}。"))?;
    let mut statement = conn
        .prepare("SELECT ZMESSAGEDATE, Z_PK FROM ZWAMESSAGE WHERE ZCHATSESSION = ?1 AND ZMESSAGEDATE IS NOT NULL ORDER BY CAST(ZMESSAGEDATE AS INTEGER) DESC, Z_PK DESC LIMIT 1")
        .map_err(|error| format!("读取 WhatsApp 最新消息失败：{error}"))?;
    let row: Option<(f64, i64)> = statement
        .query_row([session_pk], |row| Ok((row.get(0)?, row.get(1)?)))
        .ok();
    Ok(row.map(|(time, id)| (core_data_time(time), id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Connection) {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join(DB_FILE);
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE ZWACHATSESSION (Z_PK INTEGER PRIMARY KEY, ZCONTACTJID VARCHAR, ZPARTNERNAME VARCHAR,
                ZLASTMESSAGEDATE TIMESTAMP, ZLASTMESSAGETEXT VARCHAR, ZUNREADCOUNT INTEGER);
            CREATE TABLE ZWAMESSAGE (Z_PK INTEGER PRIMARY KEY, ZCHATSESSION INTEGER, ZISFROMME INTEGER,
                ZMESSAGETYPE INTEGER, ZMESSAGEDATE TIMESTAMP, ZFROMJID VARCHAR, ZTEXT VARCHAR, ZMEDIAITEM INTEGER);
            CREATE TABLE ZWAMEDIAITEM (Z_PK INTEGER PRIMARY KEY, ZTITLE VARCHAR, ZVCARDNAME VARCHAR,
                ZLATITUDE FLOAT, ZLONGITUDE FLOAT);
            CREATE TABLE ZWAPROFILEPUSHNAME (ZJID VARCHAR, ZPUSHNAME VARCHAR);
            CREATE TABLE ZWAGROUPMEMBER (ZCHATSESSION INTEGER, ZMEMBERJID VARCHAR, ZCONTACTNAME VARCHAR,
                ZFIRSTNAME VARCHAR, ZISACTIVE INTEGER);
            INSERT INTO ZWACHATSESSION VALUES (1, '8613800000000@s.whatsapp.net', char(8234) || '张杭' || char(8236), 816590670, 'hello', 0);
            INSERT INTO ZWACHATSESSION VALUES (2, '120363000000000001@g.us', '项目群', 816590732, '{"subject":"项目群"}', 2);
            INSERT INTO ZWACHATSESSION VALUES (3, 'status@broadcast', NULL, 816140703, NULL, 0);
            INSERT INTO ZWAMESSAGE VALUES (1, 1, 0, 0, 816590670, '8613800000000@s.whatsapp.net', 'hello', NULL);
            INSERT INTO ZWAMESSAGE VALUES (2, 1, 1, 1, 816590671, '8615652425800@s.whatsapp.net', '看这个', 1);
            INSERT INTO ZWAMESSAGE VALUES (3, 1, 0, 10, 816590672, '8613800000000@s.whatsapp.net', NULL, NULL);
            INSERT INTO ZWAMESSAGE VALUES (4, 2, 0, 6, 816590732, '120363000000000001@g.us', '{"subject":"项目群","is_parent_group_general_chat":false}', NULL);
            INSERT INTO ZWAMEDIAITEM VALUES (1, '报价.png', NULL, 0, 0);
            INSERT INTO ZWAPROFILEPUSHNAME VALUES ('8613800000000@s.whatsapp.net', '张杭');
            INSERT INTO ZWAGROUPMEMBER VALUES (2, '8613800000000@s.whatsapp.net', '', '', 1);
            INSERT INTO ZWAGROUPMEMBER VALUES (2, '8613700000000@s.whatsapp.net', '', '', 0);
            "#,
        )
        .unwrap();
        (root, conn)
    }

    #[test]
    fn group_messages_resolve_members_without_treating_group_ids_as_senders() {
        let (_root, conn) = fixture();
        let chat = "120363000000000001@g.us";
        conn.execute_batch(
            r#"
            ALTER TABLE ZWAMESSAGE ADD COLUMN ZGROUPMEMBER INTEGER;
            ALTER TABLE ZWAMESSAGE ADD COLUMN ZPUSHNAME VARCHAR;
            ALTER TABLE ZWAGROUPMEMBER ADD COLUMN Z_PK INTEGER;
            UPDATE ZWAGROUPMEMBER SET Z_PK = 17 WHERE ZISACTIVE = 1;
            INSERT INTO ZWAGROUPMEMBER VALUES (1, 'foreign@lid', '另一会话', '', 1, 19);
            INSERT INTO ZWAMESSAGE VALUES
                (5, 2, 0, 0, 816590733, '120363000000000001@g.us', 'hello', NULL, 17, NULL),
                (6, 2, 0, 0, 816590734, '120363000000000001@g.us', 'unknown', NULL, 19, NULL),
                (7, 2, 0, 0, 816590735, 'member@lid', 'hello', NULL, NULL, '群成员');
            "#,
        )
        .unwrap();
        let page = messages_from(&conn, chat, 10, None, None, None).unwrap();
        let find = |id| page.iter().find(|message| message.local_id == id).unwrap();
        assert_eq!(find(5).sender.as_deref(), Some("8613800000000@s.whatsapp.net"));
        assert_eq!(find(5).sender_name.as_deref(), Some("张杭"));
        assert_eq!(find(6).sender, None);
        assert_eq!(find(7).sender_name.as_deref(), Some("群成员"));
        assert_eq!(find(4).sender, None);
    }

    #[test]
    fn account_ids_are_phone_jids_only() {
        assert!(is_account("8615652425800@s.whatsapp.net"));
        assert!(!is_account("zhengweifree_b6d1"));
        assert!(!is_account("abc@s.whatsapp.net"));
        assert!(!is_account("123@g.us"));
    }

    #[test]
    fn sessions_skip_status_and_count_active_members() {
        let (_root, conn) = fixture();
        let list = sessions_from(&conn).unwrap();
        assert_eq!(list.len(), 2);
        let direct = list.iter().find(|item| item.username.ends_with("@s.whatsapp.net")).unwrap();
        assert_eq!(direct.display, "张杭");
        assert!(!direct.is_group);
        let group = list.iter().find(|item| item.is_group).unwrap();
        assert_eq!(group.display, "项目群");
        assert_eq!(group.member_count, Some(1));
        assert_eq!(group.preview.as_deref(), Some("[群事件]「项目群」"));
    }

    #[test]
    fn duplicate_session_rows_do_not_duplicate_conversations_or_pages() {
        let (_root, conn) = fixture();
        conn.execute_batch("INSERT INTO ZWACHATSESSION (Z_PK,ZCONTACTJID,ZPARTNERNAME) VALUES (4,'8613800000000@s.whatsapp.net','empty duplicate');").unwrap();
        let list = sessions_from(&conn).unwrap();
        assert_eq!(list.len(), 2);
        let direct = list.iter().find(|item| !item.is_group).unwrap();
        assert_eq!(direct.display, "张杭");
        let allowed = list.iter().map(|item| item.username.clone()).collect();
        let first = sessions_page_from(&conn, Some(&allowed), Some(1), 0).unwrap();
        let second = sessions_page_from(&conn, Some(&allowed), Some(1), 1).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_ne!(first[0].username, second[0].username);
        assert!(sessions_page_from(&conn, Some(&allowed), Some(1), 2).unwrap().is_empty());
    }

    #[test]
    fn session_previews_use_the_referenced_message_in_the_same_chat() {
        let (_root, conn) = fixture();
        conn.execute_batch(r#"
            ALTER TABLE ZWACHATSESSION ADD COLUMN ZLASTMESSAGE INTEGER;
            UPDATE ZWACHATSESSION SET ZLASTMESSAGETEXT = 'YiQwMDAwMDAwMC0wMDAwLTAwMDAtMDAwMC0wMDAwMDAwMDAwMDCYCAE=', ZLASTMESSAGE = 1 WHERE Z_PK = 1;
            UPDATE ZWACHATSESSION SET ZLASTMESSAGETEXT = 'YiQwMDAwMDAwMC0wMDAwLTAwMDAtMDAwMC0wMDAwMDAwMDAwMDCYCAE=', ZLASTMESSAGE = 4 WHERE Z_PK = 2;
            INSERT INTO ZWACHATSESSION (Z_PK,ZCONTACTJID,ZLASTMESSAGETEXT,ZLASTMESSAGE) VALUES
                (4,'other@g.us','YiQwMDAwMDAwMC0wMDAwLTAwMDAtMDAwMC0wMDAwMDAwMDAwMDCYCAE=',1),
                (5,'missing@g.us','YiQwMDAwMDAwMC0wMDAwLTAwMDAtMDAwMC0wMDAwMDAwMDAwMDCYCAE=',999),
                (6,'empty@g.us','YiQwMDAwMDAwMC0wMDAwLTAwMDAtMDAwMC0wMDAwMDAwMDAwMDCYCAE=',NULL),
                (7,'text@g.us','SGVsbG8=',NULL);
        "#).unwrap();
        let sessions = sessions_from(&conn).unwrap();
        let preview = |jid: &str| sessions.iter().find(|item| item.username == jid).unwrap().preview.as_deref();
        assert_eq!(preview("8613800000000@s.whatsapp.net"), Some("hello"));
        assert_eq!(preview("120363000000000001@g.us"), Some("[群事件]「项目群」"));
        assert_eq!(preview("other@g.us"), None);
        assert_eq!(preview("missing@g.us"), None);
        assert_eq!(preview("empty@g.us"), None);
        assert_eq!(preview("text@g.us"), Some("SGVsbG8="));

        conn.execute("UPDATE ZWACHATSESSION SET ZLASTMESSAGE = 2 WHERE Z_PK = 1", []).unwrap();
        let sessions = sessions_from(&conn).unwrap();
        assert_eq!(sessions.iter().find(|item| item.username.ends_with("@s.whatsapp.net")).unwrap().preview.as_deref(), Some("[图片] 报价.png"));
    }

    #[test]
    fn community_and_subgroups_keep_distinct_sessions() {
        let (_root, conn) = fixture();
        conn.execute_batch(r#"
            ALTER TABLE ZWACHATSESSION ADD COLUMN ZSESSIONTYPE INTEGER;
            INSERT INTO ZWACHATSESSION (Z_PK,ZCONTACTJID,ZPARTNERNAME,ZSESSIONTYPE) VALUES (4,'parent@g.us','社区',4);
            INSERT INTO ZWACHATSESSION (Z_PK,ZCONTACTJID,ZPARTNERNAME,ZSESSIONTYPE) VALUES (5,'general@g.us','General',1);
            INSERT INTO ZWACHATSESSION (Z_PK,ZCONTACTJID,ZPARTNERNAME,ZSESSIONTYPE) VALUES (6,'group@g.us','社区',1);
            INSERT INTO ZWAMESSAGE VALUES (5,4,0,6,816590670,NULL,'{"linked_groups":[{"groupJID":"general@g.us"},{"groupJID":"group@g.us"}]}',NULL);
            INSERT INTO ZWAMESSAGE VALUES (6,5,0,0,816590671,NULL,'General 的消息',NULL);
            INSERT INTO ZWAMESSAGE VALUES (7,6,0,0,816590672,NULL,'另一个群的消息',NULL);
        "#).unwrap();
        let sessions = sessions_from(&conn).unwrap();
        let parent = sessions.iter().find(|s| s.username == "parent@g.us").unwrap();
        let general = sessions.iter().find(|s| s.username == "general@g.us").unwrap();
        let group = sessions.iter().find(|s| s.username == "group@g.us").unwrap();
        assert_eq!(parent.community.as_ref().map(|c| (&*c.id, &*c.name, c.is_parent)), Some(("parent@g.us", "社区", true)));
        assert_eq!(general.community.as_ref().map(|c| (&*c.id, &*c.name, c.is_parent)), Some(("parent@g.us", "社区", false)));
        assert_eq!(group.community.as_ref().map(|c| (&*c.id, &*c.name, c.is_parent)), Some(("parent@g.us", "社区", false)));
        assert_eq!(messages_from(&conn, "general@g.us", 10, None, None, None).unwrap()[0].text, "General 的消息");
        assert_eq!(messages_from(&conn, "group@g.us", 10, None, None, None).unwrap()[0].text, "另一个群的消息");
    }

    #[test]
    fn missing_chat_is_distinct_from_an_unsupported_source_database() {
        let (_root,conn)=fixture();
        let missing=messages_from(&conn,"missing@g.us",10,None,None,None).unwrap_err();
        assert!(missing.starts_with("LOCAL_CHAT_MISSING:"));
        conn.execute_batch("ALTER TABLE ZWACHATSESSION RENAME COLUMN ZCONTACTJID TO unavailable;").unwrap();
        let broken=messages_from(&conn,"missing@g.us",10,None,None,None).unwrap_err();
        assert!(!broken.starts_with("LOCAL_CHAT_MISSING:"));
    }

    #[test]
    fn messages_are_rendered_and_paged_newest_first() {
        let (_root, conn) = fixture();
        let page = messages_from(&conn, "8613800000000@s.whatsapp.net", 10, None, None, None).unwrap();
        assert_eq!(page.len(), 2, "type 10 with empty text stays out of the transcript");
        assert_eq!(page[0].text, "[图片] 报价.png");
        assert!(page[0].is_self);
        assert_eq!(page[0].sender_name.as_deref(), Some("张杭"));
        let older = messages_from(
            &conn,
            "8613800000000@s.whatsapp.net",
            10,
            Some(MessageCursor { time: page[0].time, local_id: page[0].local_id, source_shard: String::new() }),
            None,
            None,
        )
        .unwrap();
        assert_eq!(older.len(), 1);
        assert_eq!(older[0].text, "hello");
    }

    #[test]
    fn message_range_uses_unix_seconds_and_excludes_end() {
        let (_root, conn) = fixture();
        conn.execute("UPDATE ZWAMESSAGE SET ZMESSAGEDATE = 816590671.75 WHERE Z_PK = 2", []).unwrap();
        let start = CORE_DATA_EPOCH + 816590670;
        let messages = messages_from(&conn, "8613800000000@s.whatsapp.net", 10, None, Some(start), Some(start + 2)).unwrap();
        assert_eq!(messages.iter().map(|message| message.local_id).collect::<Vec<_>>(), vec![2, 1]);
        let end = messages_from(&conn, "8613800000000@s.whatsapp.net", 10, None, Some(start), Some(start + 1)).unwrap();
        assert_eq!(end.iter().map(|message| message.local_id).collect::<Vec<_>>(), vec![1]);
    }


    #[test]
    fn fractional_seconds_never_skip_or_repeat_a_page() {
        let root = tempfile::tempdir().unwrap();
        let conn = Connection::open(root.path().join(DB_FILE)).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE ZWACHATSESSION (Z_PK INTEGER PRIMARY KEY, ZCONTACTJID VARCHAR, ZPARTNERNAME VARCHAR,
                ZLASTMESSAGEDATE TIMESTAMP, ZLASTMESSAGETEXT VARCHAR, ZUNREADCOUNT INTEGER);
            CREATE TABLE ZWAMESSAGE (Z_PK INTEGER PRIMARY KEY, ZCHATSESSION INTEGER, ZISFROMME INTEGER,
                ZMESSAGETYPE INTEGER, ZMESSAGEDATE TIMESTAMP, ZFROMJID VARCHAR, ZTEXT VARCHAR, ZMEDIAITEM INTEGER);
            INSERT INTO ZWACHATSESSION VALUES (1, '8613800000000@s.whatsapp.net', '张杭', 816590676.2, NULL, 0);
            INSERT INTO ZWAMESSAGE VALUES (1, 1, 0, 0, 816590675.1, '8613800000000@s.whatsapp.net', 'a', NULL);
            INSERT INTO ZWAMESSAGE VALUES (2, 1, 0, 0, 816590675.9, '8613800000000@s.whatsapp.net', 'b', NULL);
            INSERT INTO ZWAMESSAGE VALUES (3, 1, 0, 0, 816590675.5, '8613800000000@s.whatsapp.net', 'c', NULL);
            INSERT INTO ZWAMESSAGE VALUES (4, 1, 0, 0, 816590676.2, '8613800000000@s.whatsapp.net', 'd', NULL);
            "#,
        )
        .unwrap();
        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let page = messages_from(&conn, "8613800000000@s.whatsapp.net", 2, cursor, None, None).unwrap();
            if page.is_empty() {
                break;
            }
            cursor = page.last().map(|message| MessageCursor {
                time: message.time,
                local_id: message.local_id,
                source_shard: "whatsapp".into(),
            });
            seen.extend(page.into_iter().map(|message| message.text));
        }
        seen.sort();
        assert_eq!(seen, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn group_events_are_labelled_not_raw_json() {
        let (_root, conn) = fixture();
        let page = messages_from(&conn, "120363000000000001@g.us", 10, None, None, None).unwrap();
        assert_eq!(page[0].text, "[群事件]「项目群」");
    }

    #[test]
    fn dates_are_local_day_buckets() {
        let (_root, conn) = fixture();
        let days = message_dates_from(&conn, "8613800000000@s.whatsapp.net").unwrap();
        assert_eq!(days.iter().map(|(_, count)| count).sum::<usize>(), 3);
    }
}
