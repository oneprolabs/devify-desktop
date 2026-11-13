//! Deliberately typed, content-free diagnostics. Never scan application data files.
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

static LOG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Module {
    App,
    Auth,
    Upload,
    Wechat,
    Feedback,
    Ui,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Start,
    Open,
    Submit,
    Success,
    Failure,
    Exception,
    Panic,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Code {
    CommandFailed,
    RenderError,
    UnhandledRejection,
    RustPanic,
    HttpError,
    NetworkTimeout,
    NetworkConnect,
    NetworkRequest,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EventInput {
    pub module: Module,
    pub operation: Operation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<Code>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<crate::devify_http::RequestId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_request_id: Option<crate::devify_http::RequestId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Event {
    pub at: String,
    #[serde(flatten)]
    pub input: EventInput,
}

pub fn record(root: &Path, event: EventInput) -> Result<(), String> {
    let _guard = LOG_LOCK.lock().map_err(|_| "无法写入诊断日志。")?;
    let dir = root.join("diagnostics");
    fs::create_dir_all(&dir).map_err(|_| "无法创建诊断目录。")?;
    // Fixed names only, bounded to seven daily files, 512 KiB each.
    for entry in fs::read_dir(&dir)
        .map_err(|_| "无法读取诊断目录。")?
        .flatten()
    {
        if let Some(name) = entry.file_name().to_str() {
            if name.starts_with("events-") && name.ends_with(".jsonl") {
                let cutoff = format!(
                    "events-{}.jsonl",
                    (Utc::now() - Duration::days(6)).format("%Y-%m-%d")
                );
                if name < cutoff.as_str() {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
    let path = dir.join(format!("events-{}.jsonl", Utc::now().format("%Y-%m-%d")));
    if fs::metadata(&path)
        .map(|m| m.len() > 512 * 1024)
        .unwrap_or(false)
    {
        fs::remove_file(&path).map_err(|_| "无法轮换诊断日志。")?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|_| "无法写入诊断日志。")?;
    let row = Event {
        at: Utc::now().to_rfc3339(),
        input: event,
    };
    writeln!(
        file,
        "{}",
        serde_json::to_string(&row).map_err(|_| "无法编码诊断日志。")?
    )
    .map_err(|_| "无法保存诊断日志。".to_string())
}

pub fn snapshot(root: &Path, hours: u32) -> serde_json::Value {
    let cutoff = Utc::now() - Duration::minutes(if hours == 24 { 1440 } else { 30 });
    let mut events = Vec::new();
    for day in 0..=1 {
        let path = root.join("diagnostics").join(format!(
            "events-{}.jsonl",
            (Utc::now() - Duration::days(day)).format("%Y-%m-%d")
        ));
        if let Ok(raw) = fs::read_to_string(path) {
            for line in raw.lines() {
                if let Ok(event) = serde_json::from_str::<Event>(line) {
                    if chrono::DateTime::parse_from_rfc3339(&event.at).is_ok_and(|at| at > cutoff) {
                        events.push(event);
                    }
                }
            }
        }
    }
    events.sort_by(|a, b| a.at.cmp(&b.at));
    let events = events.split_off(events.len().saturating_sub(1000));
    serde_json::json!({ "schema_version": 1, "generated_at": Utc::now().to_rfc3339(), "events": events })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_reject_content_and_do_not_read_sensitive_files() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("devify-credentials.json"),
            "secret credential",
        )
        .unwrap();
        assert!(serde_json::from_value::<EventInput>(
            serde_json::json!({"module":"ui","operation":"failure","message":"private chat"})
        )
        .is_err());
        record(
            root.path(),
            EventInput {
                module: Module::Upload,
                operation: Operation::Failure,
                code: Some(Code::CommandFailed),
                batch_id: Some(42),
                duration_ms: Some(10),
                request_id: None,
                server_request_id: None,
                http_status: None,
            },
        )
        .unwrap();
        let value = snapshot(root.path(), 0);
        assert_eq!(value["events"][0]["batch_id"], 42);
        assert!(!value.to_string().contains("secret credential"));
    }
}
