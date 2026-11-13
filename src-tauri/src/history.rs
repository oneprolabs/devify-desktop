//! On-disk record of generated summaries.
//!
//! Both the rule-based digest and the agent output are derived from chat bodies, so they
//! stay local: a single JSON file under the app data directory, capped at the most recent
//! records. Nothing here is uploaded; the file is only read back by the drawer.
//!
//! Records are written by the backend, never by the UI, so ids and timestamps cannot be
//! forged and every write goes through the same size limits.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const HISTORY_FILE: &str = "summary-history.json";
pub const MAX_RECORDS: usize = 50;
const MAX_OUTPUT_CHARS: usize = 60_000;
const MAX_SESSIONS: usize = 200;
const MAX_FOCUS: usize = 200;
const MAX_TIMELINE: usize = 400;
const MAX_RELATED: usize = 100;
const MAX_TERMS: usize = 20;
const MAX_TEXT_CHARS: usize = 2_000;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct RecordSession {
    pub username: String,
    pub display: String,
    pub is_group: bool,
    pub messages: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct FocusItem {
    pub category: String,
    pub session: String,
    pub day: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct TimelineDay {
    pub day: String,
    pub messages: usize,
    pub sessions: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct RelatedSession {
    pub username: String,
    pub display: String,
    pub hits: usize,
    pub truncated: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct RecordInsights {
    pub focus: Vec<FocusItem>,
    pub timeline: Vec<TimelineDay>,
    pub related: Vec<RelatedSession>,
    pub terms: Vec<String>,
}

/// One saved summary. `kind` is `ai`, `local` or `daily`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct SummaryRecord {
    pub id: String,
    pub created_at: String,
    pub kind: String,
    pub account_id: String,
    pub range_label: String,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub sessions: Vec<RecordSession>,
    pub messages: usize,
    pub characters: usize,
    pub truncated: bool,
    pub agent_name: String,
    pub model: String,
    pub elapsed_ms: u64,
    /// Markdown for agent output (and the daily mail body).
    pub output: Option<String>,
    /// Rule-based digest, kept structured so it can be rendered without re-reading chat.
    pub insights: Option<RecordInsights>,
    /// Reason a record is marked as a fallback or partial result.
    pub note: Option<String>,
}

/// What the UI hands over; identity and time are assigned here.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct NewRecord {
    pub kind: String,
    pub account_id: String,
    pub range_label: String,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub sessions: Vec<RecordSession>,
    pub messages: usize,
    pub characters: usize,
    pub truncated: bool,
    pub agent_name: String,
    pub model: String,
    pub elapsed_ms: u64,
    pub output: Option<String>,
    pub insights: Option<RecordInsights>,
    pub note: Option<String>,
}

pub fn path(app_data: &Path) -> PathBuf {
    app_data.join(HISTORY_FILE)
}

fn capped(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    value.chars().take(max).collect::<String>() + "…"
}

fn normalize(mut record: SummaryRecord) -> SummaryRecord {
    record.kind = match record.kind.as_str() {
        "local" | "daily" => record.kind,
        _ => "ai".to_string(),
    };
    record.range_label = capped(record.range_label.trim(), 120);
    record.account_id = capped(record.account_id.trim(), 120);
    record.agent_name = capped(record.agent_name.trim(), 120);
    record.model = capped(record.model.trim(), 160);
    record.note = record.note.map(|note| capped(note.trim(), 500));
    record.output = record.output.map(|output| capped(&output, MAX_OUTPUT_CHARS));
    record.sessions.truncate(MAX_SESSIONS);
    for session in &mut record.sessions {
        session.username = capped(&session.username, 512);
        session.display = capped(&session.display, 200);
    }
    if let Some(insights) = record.insights.as_mut() {
        insights.focus.truncate(MAX_FOCUS);
        for item in &mut insights.focus {
            item.category = capped(&item.category, 16);
            item.session = capped(&item.session, 200);
            item.day = capped(&item.day, 40);
            item.text = capped(&item.text, MAX_TEXT_CHARS);
        }
        insights.timeline.truncate(MAX_TIMELINE);
        for day in &mut insights.timeline {
            day.day = capped(&day.day, 40);
            day.sessions.truncate(20);
            for session in &mut day.sessions {
                *session = capped(session, 200);
            }
        }
        insights.related.truncate(MAX_RELATED);
        for related in &mut insights.related {
            related.username = capped(&related.username, 512);
            related.display = capped(&related.display, 200);
        }
        insights.terms.truncate(MAX_TERMS);
        for term in &mut insights.terms {
            *term = capped(term, 80);
        }
    }
    record
}

/// Read the file. A corrupt file is moved aside instead of blocking every future save,
/// because losing history is preferable to losing the ability to write it.
pub fn load(app_data: &Path) -> Result<Vec<SummaryRecord>, String> {
    let file = path(app_data);
    if !file.exists() {
        return Ok(Vec::new());
    }
    // Files written before the 0600 rule, or restored from a backup, still hold chat
    // excerpts; tighten them whenever they are read.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&file, fs::Permissions::from_mode(0o600));
    }
    let content = fs::read_to_string(&file).map_err(|e| format!("读取总结记录失败：{e}"))?;
    match serde_json::from_str::<Vec<SummaryRecord>>(&content) {
        Ok(records) => Ok(records),
        Err(error) => {
            let broken = file.with_extension("json.corrupt");
            let _ = fs::rename(&file, broken);
            Err(format!("总结记录格式无效，已另存为备份：{error}"))
        }
    }
}

fn write(app_data: &Path, records: &[SummaryRecord]) -> Result<(), String> {
    fs::create_dir_all(app_data).map_err(|e| format!("创建应用数据目录失败：{e}"))?;
    let file = path(app_data);
    let temp = file.with_extension("json.tmp");
    let content = serde_json::to_vec_pretty(records).map_err(|e| format!("序列化总结记录失败：{e}"))?;
    fs::write(&temp, content).map_err(|e| format!("写入总结记录失败：{e}"))?;
    // Summaries contain chat excerpts, so keep the file owner-only rather than inheriting
    // the looser umask default.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&temp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&temp, &file).map_err(|e| format!("保存总结记录失败：{e}"))?;
    Ok(())
}

/// Insert at the front and keep the newest `MAX_RECORDS` entries.
pub fn add(app_data: &Path, new: NewRecord) -> Result<Vec<SummaryRecord>, String> {
    if new.kind != "local" && new.kind != "ai" && new.kind != "daily" {
        return Err("总结记录类型无效。".into());
    }
    if new.output.as_deref().unwrap_or("").trim().is_empty() && new.insights.is_none() {
        return Err("总结记录为空，未保存。".into());
    }
    let now = chrono::Local::now();
    let record = normalize(SummaryRecord {
        id: format!("{}-{}", now.format("%Y%m%d%H%M%S"), now.timestamp_subsec_micros()),
        created_at: now.format("%Y-%m-%d %H:%M").to_string(),
        kind: new.kind,
        account_id: new.account_id,
        range_label: new.range_label,
        since: new.since,
        until: new.until,
        sessions: new.sessions,
        messages: new.messages,
        characters: new.characters,
        truncated: new.truncated,
        agent_name: new.agent_name,
        model: new.model,
        elapsed_ms: new.elapsed_ms,
        output: new.output,
        insights: new.insights,
        note: new.note,
    });
    let mut records = load(app_data).unwrap_or_default();
    records.insert(0, record);
    records.truncate(MAX_RECORDS);
    write(app_data, &records)?;
    Ok(records)
}

pub fn remove(app_data: &Path, id: &str) -> Result<Vec<SummaryRecord>, String> {
    if id.trim().is_empty() {
        return Err("缺少要删除的记录标识。".into());
    }
    let mut records = load(app_data)?;
    let before = records.len();
    records.retain(|record| record.id != id);
    if records.len() == before {
        return Err("该记录已不存在。".into());
    }
    write(app_data, &records)?;
    Ok(records)
}

/// Read helper used when an account filter is wanted; the UI also filters, so this stays
/// a plain listing.
pub fn list(app_data: &Path) -> Result<Vec<SummaryRecord>, String> {
    let mut records = load(app_data)?;
    records.truncate(MAX_RECORDS);
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wcd-history-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn local_record() -> NewRecord {
        NewRecord {
            kind: "local".into(),
            account_id: "acct".into(),
            range_label: "近 7 天".into(),
            messages: 3,
            characters: 120,
            insights: Some(RecordInsights {
                focus: vec![FocusItem {
                    category: "待办".into(),
                    session: "项目群".into(),
                    day: "10月27日".into(),
                    text: "周五前给出方案".into(),
                }],
                timeline: vec![TimelineDay { day: "10-27".into(), messages: 3, sessions: vec!["项目群".into()] }],
                related: Vec::new(),
                terms: vec!["方案".into()],
            }),
            ..Default::default()
        }
    }

    #[test]
    fn newest_record_comes_first_and_is_persisted() {
        let dir = temp_dir("order");
        let mut first = local_record();
        first.range_label = "今天".into();
        add(&dir, first).expect("add first");
        let mut second = local_record();
        second.range_label = "近 30 天".into();
        let records = add(&dir, second).expect("add second");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].range_label, "近 30 天");
        assert_eq!(list(&dir).expect("list").len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn records_are_capped_at_the_limit() {
        let dir = temp_dir("cap");
        for _ in 0..(MAX_RECORDS + 3) {
            add(&dir, local_record()).expect("add");
        }
        let records = list(&dir).expect("list");
        assert_eq!(records.len(), MAX_RECORDS);
        assert!(records.iter().all(|record| !record.id.is_empty()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_records_and_unknown_kinds_are_rejected() {
        let dir = temp_dir("validate");
        assert!(add(&dir, NewRecord::default()).is_err());
        let mut record = local_record();
        record.insights = None;
        assert!(add(&dir, record).is_err());
        let mut record = local_record();
        record.kind = "email".into();
        assert!(add(&dir, record).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn removing_an_unknown_id_reports_the_problem() {
        let dir = temp_dir("remove");
        assert!(remove(&dir, "nope").is_err());
        let records = add(&dir, local_record()).expect("add");
        let removed = remove(&dir, &records[0].id).expect("remove");
        assert!(removed.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_files_are_moved_aside_instead_of_losing_writes() {
        let dir = temp_dir("corrupt");
        fs::write(path(&dir), b"{ not json").expect("write");
        assert!(load(&dir).is_err());
        assert!(path(&dir).with_extension("json.corrupt").exists());
        add(&dir, local_record()).expect("add after corruption");
        assert_eq!(list(&dir).expect("list").len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_text_is_truncated() {
        let dir = temp_dir("cap-text");
        let mut record = local_record();
        record.insights = Some(RecordInsights {
            focus: vec![FocusItem { category: "风险".into(), session: "群".into(), day: "x".into(), text: "字".repeat(MAX_TEXT_CHARS + 50) }],
            ..Default::default()
        });
        let records = add(&dir, record).expect("add");
        let text = &records[0].insights.as_ref().expect("insights").focus[0].text;
        assert_eq!(text.chars().count(), MAX_TEXT_CHARS + 1);
        assert!(text.ends_with('…'));
        let _ = fs::remove_dir_all(&dir);
    }
}
