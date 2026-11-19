//! One durable transaction for settings, task intents and confirmed progress.
//! JSON is a legacy import / human-readable settings mirror, not a second task store.
use crate::settings::AppSettings;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::{collections::HashSet, fs, path::Path, time::Duration};

const FILE: &str = "client-tasks.sqlite";

fn open(data: &Path) -> Result<Connection, String> {
    fs::create_dir_all(data).map_err(|e| format!("创建任务目录失败：{e}"))?;
    let connection =
        Connection::open(data.join(FILE)).map_err(|e| format!("打开任务数据库失败：{e}"))?;
    connection
        .busy_timeout(Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    // A single, short writer and rollback journal keep this independent of WAL versions.
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(data.join(FILE), fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(connection)
}

pub fn load(data: &Path) -> Result<Option<AppSettings>, String> {
    if !data.join(FILE).exists() {
        return Ok(None);
    }
    let mut connection = open(data)?;
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if version == 0 {
        let tables: u32 = connection.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'", [], |row| row.get(0)).map_err(|e| e.to_string())?;
        if tables == 0 {
            return Ok(None);
        }
    }
    if version != 1 {
        return Err("任务数据库版本不受支持，请保留数据后更新客户端。".into());
    }
    let transaction = connection.transaction().map_err(|e| e.to_string())?;
    let raw: Option<String> = transaction
        .query_row("SELECT body FROM state WHERE id=1", [], |row| row.get(0))
        .optional()
        .map_err(|e| format!("读取任务数据库失败，请保留现场：{e}"))?;
    let Some(raw) = raw else {
        return Err("任务数据库缺少状态记录，请保留现场，不要清空后重传。".into());
    };
    let mut state: AppSettings =
        serde_json::from_str(&raw).map_err(|e| format!("任务状态格式无效：{e}"))?;
    let mut statement = transaction
        .prepare("SELECT body FROM uploads ORDER BY ordinal")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    for row in rows {
        let raw = row.map_err(|e| e.to_string())?;
        state
            .upload_queue
            .push(serde_json::from_str(&raw).map_err(|e| format!("任务记录格式无效：{e}"))?);
    }
    Ok(Some(state))
}

pub fn save(data: &Path, state: &AppSettings) -> Result<(), String> {
    let mut connection = open(data)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    transaction.execute_batch("CREATE TABLE IF NOT EXISTS state(id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL, body TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS uploads(id TEXT PRIMARY KEY, ordinal INTEGER NOT NULL, body TEXT NOT NULL);
        PRAGMA user_version=1;").map_err(|e| format!("初始化任务数据库失败：{e}"))?;
    let current: Option<u64> = transaction
        .query_row("SELECT revision FROM state WHERE id=1", [], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?;
    if current.is_some_and(|revision| revision.checked_add(1) != Some(state.settings_revision)) {
        return Err("应用策略已被其他操作修改，请重新加载后重试。".into());
    }
    let mut settings = state.clone();
    settings.upload_queue.clear();
    let body = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
    transaction.execute("INSERT INTO state(id,revision,body) VALUES(1,?1,?2) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,body=excluded.body", params![state.settings_revision, body]).map_err(|e| e.to_string())?;
    let mut ids = HashSet::new();
    for (ordinal, job) in state.upload_queue.iter().enumerate() {
        if job.id.is_empty() || !ids.insert(job.id.as_str()) {
            return Err("任务标识为空或重复，无法保存。".into());
        }
        let body = serde_json::to_string(job).map_err(|e| e.to_string())?;
        transaction.execute("INSERT INTO uploads(id,ordinal,body) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET ordinal=excluded.ordinal,body=excluded.body WHERE uploads.body<>excluded.body OR uploads.ordinal<>excluded.ordinal", params![job.id, ordinal, body]).map_err(|e| e.to_string())?;
    }
    let existing = {
        let mut query = transaction
            .prepare("SELECT id FROM uploads")
            .map_err(|e| e.to_string())?;
        let rows = query
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };
    for id in existing {
        if !ids.contains(id.as_str()) {
            transaction
                .execute("DELETE FROM uploads WHERE id=?1", [id])
                .map_err(|e| e.to_string())?;
        }
    }
    transaction
        .commit()
        .map_err(|e| format!("提交任务状态失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::UploadJob;

    #[test]
    fn tasks_and_progress_commit_together_and_survive_reopen() {
        let data = tempfile::tempdir().unwrap();
        let mut state = AppSettings::default();
        state.settings_revision = 1;
        state.upload_queue.push(UploadJob {
            id: "original".into(),
            status: "processing".into(),
            batch_id: Some(7),
            create_started: Some(true),
            ..Default::default()
        });
        save(data.path(), &state).unwrap();
        let mut next = load(data.path()).unwrap().unwrap();
        assert_eq!(next.upload_queue[0].batch_id, Some(7));
        next.settings_revision = 2;
        next.upload_queue[0].status = "completed".into();
        next.sync.last_message_count = 123;
        save(data.path(), &next).unwrap();
        let reopened = load(data.path()).unwrap().unwrap();
        assert_eq!(reopened.upload_queue[0].status, "completed");
        assert_eq!(reopened.sync.last_message_count, 123);
        assert!(save(data.path(), &state).is_err());
    }

    #[test]
    fn invalid_job_rolls_back_entire_transaction() {
        let data = tempfile::tempdir().unwrap();
        let mut state = AppSettings::default();
        state.settings_revision = 1;
        save(data.path(), &state).unwrap();
        state.settings_revision = 2;
        state.sync.last_message_count = 99;
        state.upload_queue.push(UploadJob::default());
        assert!(save(data.path(), &state).is_err());
        let restored = load(data.path()).unwrap().unwrap();
        assert_eq!(restored.settings_revision, 1);
        assert_eq!(restored.sync.last_message_count, 0);
    }
}

/// Called only between tasks. Prune confirmed history in a short transaction;
/// never delete uncertain requests or their immutable archive evidence.
pub fn idle_maintenance(data: &Path) -> Result<(), String> {
    let _archive = crate::upload_archive::lock()?;
    let state = crate::settings::load(data)?;
    if state
        .runtime
        .last_cleanup
        .as_deref()
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .is_some_and(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds() < 60)
    {
        return Ok(());
    }
    let mut terminal: Vec<_> = state
        .upload_queue
        .iter()
        .filter(|j| {
            j.status == "completed"
                && crate::upload_completion_is_verified(j)
                && !crate::upload_job_has_pending_sync(j)
        })
        .collect();
    terminal.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let cutoff = chrono::Utc::now() - chrono::Duration::days(30);
    let remove: HashSet<_> = terminal
        .iter()
        .enumerate()
        .filter(|(i, j)| {
            *i >= 500
                || chrono::DateTime::parse_from_rfc3339(&j.updated_at)
                    .ok()
                    .is_some_and(|t| t < cutoff)
        })
        .map(|(_, j)| j.id.clone())
        .collect();
    let saved = crate::settings::update(data, |s| {
        s.upload_queue.retain(|j| {
            !remove.contains(&j.id)
                || j.status != "completed"
                || !crate::upload_completion_is_verified(j)
                || crate::upload_job_has_pending_sync(j)
        });
        s.runtime.last_cleanup = Some(chrono::Utc::now().to_rfc3339());
        Ok(())
    })?;
    let referenced: HashSet<_> = saved
        .upload_queue
        .iter()
        .filter(|j| {
            j.status != "completed"
                || crate::upload_job_has_pending_sync(j)
                || !crate::upload_completion_is_verified(j)
        })
        .filter_map(|j| j.package_digest.as_deref())
        .collect();
    let completed: HashSet<_> = saved
        .upload_queue
        .iter()
        .filter(|j| {
            j.status == "completed"
                && !crate::upload_job_has_pending_sync(j)
                && crate::upload_completion_is_verified(j)
        })
        .filter_map(|j| j.package_digest.as_deref())
        .collect();
    let folder = data.join("upload-archives");
    if let Ok(entries) = fs::read_dir(&folder) {
        // Bounded cleanup. Fresh or referenced files are always retained.
        for entry in entries.take(100) {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let Some(digest) = path.file_stem().and_then(|v| v.to_str()) else {
                continue;
            };
            if path.extension().and_then(|v| v.to_str()) != Some("zip")
                || digest.len() != 64
                || !digest.bytes().all(|b| b.is_ascii_hexdigit())
                || referenced.contains(digest)
            {
                continue;
            }
            if completed.contains(digest)
                || entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age.as_secs() > 86400)
            {
                let _ = fs::remove_file(path);
            }
        }
    }
    let connection = open(data)?;
    let pages: u64 = connection
        .pragma_query_value(None, "page_count", |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let free: u64 = connection
        .pragma_query_value(None, "freelist_count", |r| r.get(0))
        .map_err(|e| e.to_string())?;
    // Compact only this small metadata database, never decrypted source databases.
    if pages > 4096 && free > pages / 4 {
        connection
            .execute_batch("VACUUM")
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn cache_connection(data: &Path) -> Result<Connection, String> {
    if !data.join(FILE).exists() {
        return Err("本地状态尚未初始化。".into());
    }
    let c = open(data)?;
    let version: u32 = c
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if version != 1 {
        return Err("缓存数据库版本不受支持。".into());
    }
    c.execute_batch("CREATE TABLE IF NOT EXISTS read_cache(key TEXT PRIMARY KEY, body TEXT NOT NULL, used_at INTEGER NOT NULL)").map_err(|e|e.to_string())?;
    Ok(c)
}
pub fn cache_get(data: &Path, key: &str) -> Option<serde_json::Value> {
    let c = cache_connection(data).ok()?;
    let raw: String = c
        .query_row("SELECT body FROM read_cache WHERE key=?1", [key], |r| {
            r.get(0)
        })
        .optional()
        .ok()??;
    if raw.len() > 2 * 1024 * 1024 {
        return None;
    }
    serde_json::from_str(&raw).ok()
}
pub fn cache_put(data: &Path, key: &str, value: &serde_json::Value) -> Result<(), String> {
    let raw = serde_json::to_string(value).map_err(|e| e.to_string())?;
    if raw.len() > 2 * 1024 * 1024 {
        return Ok(());
    }
    let mut c = cache_connection(data)?;
    let t = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    t.execute("INSERT INTO read_cache(key,body,used_at) VALUES(?1,?2,?3) ON CONFLICT(key) DO UPDATE SET body=excluded.body,used_at=excluded.used_at",params![key,raw,chrono::Utc::now().timestamp()]).map_err(|e|e.to_string())?;
    t.execute("DELETE FROM read_cache WHERE key IN (SELECT key FROM read_cache ORDER BY used_at DESC,key LIMIT -1 OFFSET 200)",[]).map_err(|e|e.to_string())?;
    // Count and byte caps apply together, independent of whether an entry was visited.
    let mut total: i64 = t
        .query_row(
            "SELECT COALESCE(SUM(length(CAST(body AS BLOB))),0) FROM read_cache",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    while total > 20 * 1024 * 1024 {
        let (old, size): (String, i64) = t
            .query_row(
                "SELECT key,length(CAST(body AS BLOB)) FROM read_cache ORDER BY used_at,key LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| e.to_string())?;
        t.execute("DELETE FROM read_cache WHERE key=?1", [old])
            .map_err(|e| e.to_string())?;
        total -= size;
    }
    t.commit().map_err(|e| e.to_string())
}

#[cfg(test)]
mod maintenance_tests {
    use super::*;
    #[test]
    fn metadata_cache_is_bounded_by_count_and_utf8_bytes() {
        let temp = tempfile::tempdir().unwrap();
        crate::settings::update(temp.path(), |_| Ok(())).unwrap();
        for i in 0..205 {
            cache_put(
                temp.path(),
                &format!("small-{i:03}"),
                &serde_json::json!({"i":i}),
            )
            .unwrap();
        }
        let c = cache_connection(temp.path()).unwrap();
        let count: i64 = c
            .query_row("SELECT count(*) FROM read_cache", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 200);
        let value = serde_json::json!({"text":"中".repeat(500_000)});
        for i in 0..16 {
            cache_put(temp.path(), &format!("large-{i:03}"), &value).unwrap();
        }
        let bytes: i64 = c
            .query_row(
                "SELECT sum(length(CAST(body AS BLOB))) FROM read_cache",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(bytes <= 20 * 1024 * 1024);
    }
    #[test]
    fn idle_cleanup_preserves_uncertain_archive_even_when_old() {
        use sha2::Digest;
        let temp = tempfile::tempdir().unwrap();
        let referenced = hex::encode(sha2::Sha256::digest(b"request evidence"));
        let orphan = hex::encode(sha2::Sha256::digest(b"orphan"));
        crate::upload_archive::save(temp.path(), &referenced, b"request evidence").unwrap();
        crate::upload_archive::save(temp.path(), &orphan, b"orphan").unwrap();
        for digest in [&referenced, &orphan] {
            fs::File::open(
                temp.path()
                    .join("upload-archives")
                    .join(format!("{digest}.zip")),
            )
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(std::time::SystemTime::now() - Duration::from_secs(2 * 86400)),
            )
            .unwrap();
        }
        crate::settings::update(temp.path(), |s| {
            s.upload_queue.push(crate::settings::UploadJob {
                id: "unknown-request".into(),
                status: "failed".into(),
                create_started: Some(true),
                package_digest: Some(referenced.clone()),
                ..Default::default()
            });
            Ok(())
        })
        .unwrap();
        idle_maintenance(temp.path()).unwrap();
        assert!(crate::upload_archive::exists(
            temp.path(),
            Some(&referenced)
        ));
        assert!(!crate::upload_archive::exists(temp.path(), Some(&orphan)));
        assert_eq!(
            crate::settings::load(temp.path())
                .unwrap()
                .upload_queue
                .len(),
            1
        );
    }
}
