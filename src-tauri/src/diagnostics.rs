//! Deliberately typed, greppable diagnostics. Every record is one text line:
//!
//! ```text
//! 2026-11-18T10:12:33.123+08:00	ERROR	upload.failure	method=POST url=https://host/api/v1/... status=503 duration_ms=1200 code=HTTP_ERROR request_id=... body={"device_id":"42","token":"***"}
//! ```
//!
//! Never scan application data files. Request bodies are captured by
//! [`capture_request`] with credential keys redacted and the whole value capped,
//! so a report stays useful without carrying passwords, tokens or chat text.
use chrono::{DateTime, Duration, Local, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

static LOG_LOCK: Mutex<()> = Mutex::new(());

/// One text line; the server accepts at most 4000 characters per line.
const LINE_MAX: usize = 3800;
/// Captured request body budget, after redaction.
const BODY_MAX: usize = 2000;
/// Captured URL budget.
const URL_MAX: usize = 512;
/// Longest string value kept inside a JSON body.
const VALUE_MAX: usize = 200;

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

impl Module {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Auth => "auth",
            Self::Upload => "upload",
            Self::Wechat => "wechat",
            Self::Feedback => "feedback",
            Self::Ui => "ui",
        }
    }
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

impl Operation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Open => "open",
            Self::Submit => "submit",
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Exception => "exception",
            Self::Panic => "panic",
        }
    }
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

impl Code {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CommandFailed => "COMMAND_FAILED",
            Self::RenderError => "RENDER_ERROR",
            Self::UnhandledRejection => "UNHANDLED_REJECTION",
            Self::RustPanic => "RUST_PANIC",
            Self::HttpError => "HTTP_ERROR",
            Self::NetworkTimeout => "NETWORK_TIMEOUT",
            Self::NetworkConnect => "NETWORK_CONNECT",
            Self::NetworkRequest => "NETWORK_REQUEST",
        }
    }
}

/// What was sent, already safe to persist. Built by [`capture_request`].
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RequestInfo {
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

impl RequestInfo {
    /// The same request without its body, for the `start` record.
    pub fn without_body(&self) -> Self {
        Self {
            method: self.method.clone(),
            url: self.url.clone(),
            body: None,
        }
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<RequestInfo>,
}

/// Legacy on-disk shape (schema version 1), still readable so drafts and log
/// files written before the text format keep working.
#[derive(Clone, Deserialize, Serialize)]
pub struct Event {
    pub at: String,
    #[serde(flatten)]
    pub input: EventInput,
}

#[derive(Deserialize)]
struct LegacyEventRow {
    at: String,
    module: Module,
    operation: Operation,
    #[serde(default)]
    code: Option<Code>,
    #[serde(default)]
    batch_id: Option<u64>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    request_id: Option<crate::devify_http::RequestId>,
    #[serde(default)]
    server_request_id: Option<crate::devify_http::RequestId>,
    #[serde(default)]
    http_status: Option<u16>,
}

impl From<LegacyEventRow> for Event {
    fn from(row: LegacyEventRow) -> Self {
        Self {
            at: row.at,
            input: EventInput {
                module: row.module,
                operation: row.operation,
                code: row.code,
                batch_id: row.batch_id,
                duration_ms: row.duration_ms,
                request_id: row.request_id,
                server_request_id: row.server_request_id,
                http_status: row.http_status,
                request: None,
            },
        }
    }
}

/// Severity of one record. A 4xx is a warning (the request reached the server
/// and was understood); timeouts, connect failures, 5xx and panics are errors.
fn level(event: &EventInput) -> &'static str {
    match event.operation {
        Operation::Start | Operation::Open | Operation::Submit | Operation::Success => "INFO",
        Operation::Failure => match (event.http_status, &event.code) {
            (Some(status), _) if (400..500).contains(&status) => "WARN",
            _ => "ERROR",
        },
        Operation::Exception | Operation::Panic => "ERROR",
    }
}

fn content(event: &EventInput) -> String {
    let mut fields: Vec<String> = Vec::new();
    if let Some(request) = &event.request {
        fields.push(format!("method={}", request.method));
        fields.push(format!("url={}", request.url));
    }
    if let Some(status) = event.http_status {
        fields.push(format!("status={status}"));
    }
    if let Some(duration) = event.duration_ms {
        fields.push(format!("duration_ms={duration}"));
    }
    if let Some(code) = &event.code {
        fields.push(format!("code={}", code.as_str()));
    }
    if let Some(id) = &event.request_id {
        fields.push(format!("request_id={}", String::from(id.clone())));
    }
    if let Some(id) = &event.server_request_id {
        fields.push(format!("server_request_id={}", String::from(id.clone())));
    }
    if let Some(batch_id) = event.batch_id {
        fields.push(format!("batch_id={batch_id}"));
    }
    if let Some(body) = &event
        .request
        .as_ref()
        .and_then(|request| request.body.as_ref())
    {
        fields.push(format!("body={body}"));
    }
    let content = if fields.is_empty() {
        "-".to_string()
    } else {
        fields.join(" ")
    };
    one_line(&content)
}

fn one_line(value: &str) -> String {
    truncate(
        &value.replace(['\t', '\n', '\r'], " ").trim().to_string(),
        LINE_MAX,
    )
}

/// Format one record as `time<TAB>level<TAB>module.operation<TAB>content`.
pub fn text_line(at: &str, event: &EventInput) -> String {
    format!(
        "{at}\t{}\t{}.{}\t{}",
        level(event),
        event.module.as_str(),
        event.operation.as_str(),
        content(event)
    )
}

pub fn record(root: &Path, event: EventInput) -> Result<(), String> {
    let _guard = LOG_LOCK.lock().map_err(|_| "无法写入诊断日志。")?;
    let dir = root.join("diagnostics");
    fs::create_dir_all(&dir).map_err(|_| "无法创建诊断目录。")?;
    remove_expired(&dir);
    let path = dir.join(format!("events-{}.log", Local::now().format("%Y-%m-%d")));
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
    let at = Local::now().to_rfc3339_opts(SecondsFormat::Millis, false);
    writeln!(file, "{}", text_line(&at, &event)).map_err(|_| "无法保存诊断日志。".to_string())
}

/// Drop both the current `.log` files and the legacy `.jsonl` files after seven days.
fn remove_expired(dir: &Path) {
    // Fixed names only, bounded to seven daily files, 512 KiB each.
    let cutoff = (Utc::now() - Duration::days(6))
        .format("%Y-%m-%d")
        .to_string();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        if let Some(name) = entry.file_name().to_str() {
            let Some(date) = name.strip_prefix("events-").and_then(|rest| {
                rest.strip_suffix(".log")
                    .or_else(|| rest.strip_suffix(".jsonl"))
            }) else {
                continue;
            };
            if date < cutoff.as_str() {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// Snapshot for the feedback form. Schema version 2 carries plain text lines;
/// legacy `.jsonl` rows are converted with the same formatter so an upgrade does
/// not hide the last few hours of history.
pub fn snapshot(root: &Path, hours: u32) -> serde_json::Value {
    let cutoff = Utc::now() - Duration::minutes(if hours == 24 { 1440 } else { 30 });
    let mut lines: Vec<(DateTime<Utc>, String)> = Vec::new();
    for day in 0..=1 {
        let date = (Local::now() - Duration::days(day))
            .format("%Y-%m-%d")
            .to_string();
        let dir = root.join("diagnostics");
        if let Ok(raw) = fs::read_to_string(dir.join(format!("events-{date}.log"))) {
            for line in raw.lines().filter(|line| !line.trim().is_empty()) {
                let at = line
                    .split_once('\t')
                    .and_then(|(at, _)| DateTime::parse_from_rfc3339(at).ok())
                    .map(|at| at.with_timezone(&Utc));
                if at.map_or(true, |at| at > cutoff) {
                    lines.push((at.unwrap_or_else(Utc::now), truncate(line, LINE_MAX)));
                }
            }
        }
        if let Ok(raw) = fs::read_to_string(dir.join(format!("events-{date}.jsonl"))) {
            for row in raw
                .lines()
                .filter_map(|line| serde_json::from_str::<LegacyEventRow>(line).ok())
            {
                let event = Event::from(row);
                let at = DateTime::parse_from_rfc3339(&event.at)
                    .map(|at| at.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                if at > cutoff {
                    let line = text_line(
                        &at.with_timezone(&Local)
                            .to_rfc3339_opts(SecondsFormat::Millis, false),
                        &event.input,
                    );
                    lines.push((at, line));
                }
            }
        }
    }
    lines.sort_by(|a, b| a.0.cmp(&b.0));
    let lines = lines
        .split_off(lines.len().saturating_sub(1000))
        .into_iter()
        .map(|(_, line)| line)
        .collect::<Vec<_>>();
    serde_json::json!({
        "schema_version": 2,
        "generated_at": Utc::now().to_rfc3339(),
        "lines": lines,
    })
}

/// Capture method, URL and body of a builder without consuming it.
///
/// `None` when the body cannot be cloned (multipart/streaming); those call sites
/// pass an explicit hint with [`request_hint`].
pub fn capture_request(builder: &reqwest::RequestBuilder) -> Option<RequestInfo> {
    let request = builder.try_clone()?.build().ok()?;
    Some(RequestInfo {
        method: request.method().as_str().to_string(),
        url: sanitize_url(request.url()),
        body: request
            .body()
            .and_then(|body| body.as_bytes())
            .map(sanitize_body),
    })
}

/// Explicit request summary for a request whose body cannot be inspected.
pub fn request_hint(method: &str, url: &str, body: Option<String>) -> RequestInfo {
    RequestInfo {
        method: method.to_string(),
        url: reqwest::Url::parse(url)
            .map(|url| sanitize_url(&url))
            .unwrap_or_else(|_| truncate(url, URL_MAX)),
        body: body.map(|body| truncate(&one_line(&body), BODY_MAX)),
    }
}

fn sanitize_url(url: &reqwest::Url) -> String {
    let mut clean = url.clone();
    let _ = clean.set_username("");
    let _ = clean.set_password(None);
    clean.set_fragment(None);
    if clean.query().is_some() {
        let pairs = clean
            .query_pairs()
            .map(|(key, value)| {
                let value = if sensitive(&key) {
                    "***".to_string()
                } else {
                    value.into_owned()
                };
                (key.into_owned(), value)
            })
            .collect::<Vec<_>>();
        clean.set_query(None);
        if !pairs.is_empty() {
            let mut query = clean.query_pairs_mut();
            for (key, value) in pairs {
                query.append_pair(&key, &value);
            }
        }
    }
    truncate(clean.as_str(), URL_MAX)
}

fn sanitize_body(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return format!("<{} bytes>", bytes.len());
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(text.trim()) else {
        // Only JSON bodies are captured; anything else could be chat content.
        return format!("<{} bytes>", bytes.len());
    };
    redact(&mut value, 0);
    truncate(&value.to_string(), BODY_MAX)
}

fn redact(value: &mut serde_json::Value, depth: usize) {
    if depth > 6 {
        *value = serde_json::Value::String("<deep>".into());
        return;
    }
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                if sensitive(key) {
                    *value = serde_json::Value::String("***".into());
                } else {
                    redact(value, depth + 1);
                }
            }
        }
        serde_json::Value::Array(items) => {
            if items.len() > 50 {
                items.truncate(50);
                items.push(serde_json::Value::String("<truncated>".into()));
            }
            for item in items {
                redact(item, depth + 1);
            }
        }
        serde_json::Value::String(text) if text.chars().count() > VALUE_MAX => {
            let total = text.chars().count();
            let prefix: String = text.chars().take(VALUE_MAX).collect();
            *text = format!("{prefix}… ({total} chars)");
        }
        _ => {}
    }
}

fn sensitive(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    // Credentials and account identifiers.
    matches!(
        key.as_str(),
        "password"
            | "passwd"
            | "secret"
            | "token"
            | "code"
            | "refresh"
            | "session"
            | "authorization"
            | "cookie"
            | "credential"
            | "api_key"
            | "apikey"
            | "private_key"
            | "code_verifier"
            | "verifier"
            | "username"
            | "email"
            | "phone"
            // Search terms are user content even when they ride in the URL.
            | "search"
            | "query"
            | "q"
            | "keyword"
    ) || key.ends_with("_token")
        || key.ends_with("_secret")
        || key.ends_with("_password")
        || key.ends_with("_credential")
        || key.ends_with("_key")
        // Free-form user content: a note, event, question or report body must not
        // travel in a diagnostic just because its request failed.
        || matches!(
            key.as_str(),
            "content"
                | "text"
                | "message"
                | "description"
                | "title"
                | "question"
                | "answer"
                | "prompt"
                | "input"
                | "output"
                | "summary"
                | "note"
                | "body"
                | "reply"
        )
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max).collect();
    out.push('…');
    out
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
                request: None,
            },
        )
        .unwrap();
        let value = snapshot(root.path(), 0);
        assert_eq!(value["schema_version"], 2);
        let lines = value["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 1);
        let line = lines[0].as_str().unwrap();
        assert!(line.contains("\tERROR\tupload.failure\t"));
        assert!(line.contains("batch_id=42"));
        assert!(!value.to_string().contains("secret credential"));
    }

    #[test]
    fn request_summaries_redact_credentials_and_stay_bounded() {
        let info = capture_request(
            &reqwest::Client::new()
                .post("https://user:pass@example.test/api/v1/login?token=abc&page=2&search=private+chat")
                .json(&serde_json::json!({
                    "username": "alice",
                    "password": "hunter2",
                    "refresh": "r-1",
                    "title": "private note",
                    "content": "chat text that must not travel",
                    "nested": {"access_token": "a-1", "note": "x".repeat(400)},
                    "items": (0..80).collect::<Vec<_>>(),
                })),
        )
        .unwrap();
        assert_eq!(info.method, "POST");
        assert!(info.url.contains("token=***"));
        assert!(info.url.contains("page=2"));
        assert!(info.url.contains("search=***"));
        assert!(!info.url.contains("private"));
        assert!(!info.url.contains("pass"));
        let body = info.body.unwrap();
        assert!(body.contains(r#""password":"***""#));
        assert!(body.contains(r#""refresh":"***""#));
        assert!(body.contains(r#""access_token":"***""#));
        assert!(!body.contains("hunter2"));
        assert!(!body.contains("r-1"));
        assert!(body.contains(r#""username":"***""#));
        assert!(body.contains(r#""title":"***""#));
        assert!(body.contains(r#""content":"***""#));
        assert!(!body.contains("private note"));
        assert!(!body.contains("chat text that must not travel"));
        assert!(body.chars().count() <= BODY_MAX + 1);
    }

    #[test]
    fn non_json_bodies_are_not_persisted() {
        let info = capture_request(
            &reqwest::Client::new()
                .post("https://example.test/upload")
                .body("private chat text"),
        )
        .unwrap();
        assert_eq!(info.body.unwrap(), "<17 bytes>");
    }

    #[test]
    fn legacy_jsonl_rows_are_still_readable() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("diagnostics");
        fs::create_dir_all(&dir).unwrap();
        let at = Local::now().to_rfc3339();
        fs::write(
            dir.join(format!("events-{}.jsonl", Local::now().format("%Y-%m-%d"))),
            format!(r#"{{"at":"{at}","module":"upload","operation":"failure","code":"COMMAND_FAILED","batch_id":7}}"#),
        )
        .unwrap();
        let value = snapshot(root.path(), 0);
        let lines = value["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].as_str().unwrap().contains("batch_id=7"));
    }
}
