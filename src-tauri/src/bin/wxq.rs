//! `wxq` — read-only command line client for the desktop app.
//!
//! The app exposes the same five whitelist-checked handlers as its MCP tools under
//! `/api/*` on the loopback listener. This binary speaks plain HTTP/1.1 to that
//! endpoint (no TLS, no extra crates) and formats the JSON for a terminal or an
//! agent. It never opens the databases, decrypts, or sends mail.
//!
//! Run `wxq help` for the command list.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use chrono::{Local, NaiveDate, NaiveTime, TimeZone};
use serde::Deserialize;
use serde_json::Value;

use wechat_devify_bridge_lib::wechat::query::{
    DigestReport, MessageItem, SearchReport, SessionItem,
};

const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:18765";

#[derive(Deserialize)]
struct Policy {
    account_id: String,
    whitelist_count: usize,
    sessions: Vec<SessionItem>,
    automation_enabled: bool,
}

#[derive(Deserialize)]
struct SessionList {
    account_id: String,
    sessions: Vec<SessionItem>,
}

#[derive(Deserialize)]
struct MessageList {
    account_id: String,
    username: String,
    messages: Vec<MessageItem>,
    has_more: bool,
}

#[derive(Default)]
struct Parsed {
    values: HashMap<String, Vec<String>>,
    flags: HashSet<String>,
}

impl Parsed {
    fn last(&self, key: &str) -> Option<&str> {
        self.values.get(key).and_then(|values| values.last()).map(String::as_str)
    }

    fn required(&self, key: &str) -> Result<&str, String> {
        self.last(key)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("缺少参数 --{key}。"))
    }

    fn number(&self, key: &str) -> Result<Option<usize>, String> {
        self.last(key)
            .map(|value| {
                value
                    .parse::<usize>()
                    .map_err(|_| format!("--{key} 需要正整数，收到：{value}"))
            })
            .transpose()
    }

    fn time(&self, key: &str, end_of_day: bool) -> Result<Option<i64>, String> {
        self.last(key).map(|value| parse_time(value, end_of_day)).transpose()
    }
}

struct Cli {
    endpoint: String,
    token: Option<String>,
    json: bool,
    command: String,
    args: Parsed,
}

fn main() {
    match run() {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
        }
        Err(error) => {
            eprintln!("wxq: {error}");
            std::process::exit(1);
        }
    }
}

/// First non-empty environment variable. `WECHAT_AGENT_*` is the current name; the
/// MCP-era variables remain as aliases for existing scripts.
fn env_first(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| std::env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn run() -> Result<String, String> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.is_empty() {
        return Ok(usage());
    }

    let mut endpoint = env_first(&["WECHAT_AGENT_ENDPOINT", "WECHAT_MCP_ENDPOINT"])
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
    let mut token = env_first(&["WECHAT_AGENT_TOKEN", "WECHAT_MCP_TOKEN"]);
    let mut json = false;
    let mut rest: Vec<String> = Vec::new();

    let mut index = 0;
    while index < raw.len() {
        let arg = raw[index].clone();
        let (name, inline) = split_inline(&arg);
        match name {
            "--endpoint" => endpoint = take_value(name, inline, &raw, &mut index)?,
            "--token" => token = Some(take_value(name, inline, &raw, &mut index)?),
            "--json" => json = true,
            "--help" | "-h" => rest.push("help".to_string()),
            _ => rest.push(arg),
        }
        index += 1;
    }

    let Some(command) = rest.first().cloned() else {
        return Ok(usage());
    };
    if command == "help" {
        return Ok(usage());
    }
    let args = parse_flags(&rest[1..])?;

    if args.flags.contains("help") {
        return Ok(usage());
    }

    let cli = Cli {
        endpoint,
        token,
        json,
        command,
        args,
    };
    dispatch(cli)
}

fn dispatch(cli: Cli) -> Result<String, String> {
    let account = cli.args.last("account").map(str::to_string);
    match cli.command.as_str() {
        "health" => {
            let body = request_json(&cli, "GET", "/health", None)?;
            let service = body.get("service").and_then(Value::as_str).unwrap_or("unknown");
            if cli.json {
                return pretty(&body);
            }
            Ok(format!("ok · {service}\n服务地址：{}", cli.endpoint))
        }
        "policy" => {
            let path = with_account("/api/policy", account.as_deref());
            let body = request_json(&cli, "GET", &path, None)?;
            if cli.json {
                return pretty(&body);
            }
            let policy: Policy = decode(body)?;
            Ok(format_policy(&policy))
        }
        "sessions" => {
            let path = with_account("/api/sessions", account.as_deref());
            let body = request_json(&cli, "GET", &path, None)?;
            if cli.json {
                return pretty(&body);
            }
            let list: SessionList = decode(body)?;
            Ok(format_sessions(&list.account_id, &list.sessions))
        }
        "messages" => {
            let chat = cli.args.required("chat")?;
            let limit = cli.args.number("limit")?.unwrap_or(100).clamp(1, 500);
            let before = cli.args.time("before", false)?;
            let mut path = format!(
                "/api/messages?username={}&limit={limit}",
                encode(chat)
            );
            if let Some(before) = before {
                path.push_str(&format!("&before={before}"));
            }
            let path = with_account(&path, account.as_deref());
            let body = request_json(&cli, "GET", &path, None)?;
            if cli.json {
                return pretty(&body);
            }
            let list: MessageList = decode(body)?;
            Ok(format_messages(&list))
        }
        "search" => {
            let keyword = cli.args.required("keyword")?;
            let max_sessions = cli.args.number("max-sessions")?.unwrap_or(200).clamp(1, 200);
            let mut path = format!(
                "/api/search?keyword={}&max_sessions={max_sessions}",
                encode(keyword)
            );
            if cli.args.flags.contains("groups-only") {
                path.push_str("&groups_only=true");
            }
            let path = with_account(&path, account.as_deref());
            let body = request_json(&cli, "GET", &path, None)?;
            if cli.json {
                return pretty(&body);
            }
            let report: SearchReport = decode(body)?;
            Ok(format_search(&report))
        }
        "digest" => {
            let chats = cli
                .args
                .values
                .get("chat")
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|value| !value.trim().is_empty())
                .collect::<Vec<_>>();
            if chats.is_empty() {
                return Err("digest 至少需要一个 --chat（可重复）。".into());
            }
            let since = cli.args.time("since", false)?;
            let until = cli.args.time("until", true)?;
            let payload = serde_json::json!({
                "account_id": account,
                "usernames": chats,
                "since": since,
                "until": until
            });
            let body = request_json(&cli, "POST", "/api/digest", Some(&payload.to_string()))?;
            if cli.json {
                return pretty(&body);
            }
            let digest: DigestReport = decode(body)?;
            Ok(format_digest(&digest))
        }
        other => Err(format!("未知命令：{other}\n\n{}", usage())),
    }
}

/* ── formatting ─────────────────────────────────────────────────────────────── */

fn format_policy(policy: &Policy) -> String {
    let mut out = format!(
        "账号：{}　白名单：{} 个会话　自动同步：{}\n",
        policy.account_id,
        policy.whitelist_count,
        if policy.automation_enabled { "开" } else { "关" }
    );
    if policy.sessions.is_empty() {
        out.push_str("\n白名单为空：先在桌面应用「聊天白名单」里选择会话。");
    } else {
        out.push('\n');
        out.push_str(&session_rows(&policy.sessions));
    }
    out
}

fn format_sessions(account: &str, sessions: &[SessionItem]) -> String {
    if sessions.is_empty() {
        return format!("账号 {account} 的白名单为空：先在桌面应用「聊天白名单」里选择会话。");
    }
    format!("账号：{account}　可见会话：{}\n\n{}", sessions.len(), session_rows(sessions))
}

fn session_rows(sessions: &[SessionItem]) -> String {
    let mut out = String::new();
    for session in sessions {
        out.push_str(&format!(
            "{} | {} | {}{} | {}\n",
            stamp(session.last_time),
            if session.is_group { "群聊" } else { "单聊" },
            session.display,
            session
                .member_count
                .map(|count| format!("（{count} 人）"))
                .unwrap_or_default(),
            session.username
        ));
    }
    out
}

fn format_messages(list: &MessageList) -> String {
    let mut out = format!(
        "账号：{}　会话：{}　本次 {} 条{}\n\n",
        list.account_id,
        list.username,
        list.messages.len(),
        if list.has_more { "（还有更早消息）" } else { "" }
    );
    if list.messages.is_empty() {
        out.push_str("这个会话没有可读取的文字消息。");
        return out;
    }
    // The API returns newest first; a transcript reads better oldest first.
    for message in list.messages.iter().rev() {
        out.push_str(&format!(
            "[{}] {}: {}\n",
            stamp(Some(message.time)),
            sender_name(message),
            one_line(&message.text)
        ));
    }
    if list.has_more {
        if let Some(oldest) = list.messages.last() {
            out.push_str(&format!(
                "\n继续读取更早消息：wxq messages --chat {} --before {}\n",
                list.username, oldest.time
            ));
        }
    }
    out
}

fn format_search(report: &SearchReport) -> String {
    let mut out = format!(
        "关键词「{}」：扫描 {} 个会话，跳过 {} 个，命中 {} 个会话\n",
        report.keyword,
        report.scanned,
        report.skipped,
        report.hits.len()
    );
    if report.hits.is_empty() {
        out.push_str("\n没有命中。搜索只覆盖白名单会话，最多 200 个。");
        return out;
    }
    for hit in &report.hits {
        out.push_str(&format!(
            "\n- [{}] {}（{}）命中 {} 条{} | {}\n",
            if hit.is_group { "群聊" } else { "单聊" },
            hit.display,
            stamp(hit.last_time),
            hit.hits,
            if hit.truncated { "，已截断" } else { "" },
            hit.username
        ));
        let mut snippets = hit.snippets.clone();
        snippets.sort_by_key(|snippet| snippet.time);
        for snippet in snippets {
            let who = snippet
                .sender_name
                .clone()
                .or_else(|| snippet.sender.clone())
                .unwrap_or_else(|| "未知".to_string());
            out.push_str(&format!(
                "    [{}] {}: {}\n",
                stamp(Some(snippet.time)),
                who,
                one_line(&snippet.text)
            ));
        }
    }
    out
}

fn format_digest(digest: &DigestReport) -> String {
    if digest.markdown.trim().is_empty() {
        return "所选会话在该时间范围内没有可整理的消息。".to_string();
    }
    format!(
        "{}\n---\n规则抽取：{} 个会话 / {} 条消息；待办 {}、决定 {}、问题 {}、风险 {}、数据 {}。\n这是确定性规则抽取，不是 AI 总结；负责人、截止时间和因果关系需回原文确认。",
        digest.markdown.trim_end(),
        digest.sessions,
        digest.messages,
        digest.todo,
        digest.decisions,
        digest.issues,
        digest.questions,
        digest.data
    )
}

fn sender_name(message: &MessageItem) -> String {
    if message.is_self {
        return "我".to_string();
    }
    message
        .sender_name
        .clone()
        .or_else(|| message.sender.clone())
        .unwrap_or_else(|| "未知".to_string())
}

fn one_line(text: &str) -> String {
    text.replace("\r\n", " ⏎ ").replace('\n', " ⏎ ").trim().to_string()
}

fn stamp(time: Option<i64>) -> String {
    match time {
        Some(seconds) => Local
            .timestamp_opt(seconds, 0)
            .single()
            .map(|value| value.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| seconds.to_string()),
        None => "—".to_string(),
    }
}

/* ── HTTP ───────────────────────────────────────────────────────────────────── */

struct Response {
    status: u16,
    body: String,
}

fn request_json(cli: &Cli, method: &str, path: &str, body: Option<&str>) -> Result<Value, String> {
    let response = http(cli, method, path, body)?;
    let value: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
    if !(200..300).contains(&response.status) {
        if response.status == 404 {
            return Err(format!(
                "端点不存在（HTTP 404）：{path}\n桌面应用可能是改造前的版本，请重新构建并启动应用，或先运行 wxq health 确认服务。"
            ));
        }
        let message = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("请求失败。")
            .to_string();
        return Err(format!("{message}（HTTP {}）", response.status));
    }
    Ok(value)
}

fn http(cli: &Cli, method: &str, path: &str, body: Option<&str>) -> Result<Response, String> {
    let base = cli.endpoint.trim_end_matches('/');
    let rest = base
        .strip_prefix("http://")
        .ok_or_else(|| "wxq 只支持 http:// 的本地服务地址。".to_string())?;
    let (authority, base_path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, ""),
    };
    let (host, port) = split_authority(authority)?;
    let path = format!("{base_path}{path}");

    let mut stream = TcpStream::connect((host.as_str(), port)).map_err(|error| {
        format!("无法连接 {host}:{port}：{error}\n请确认桌面应用已启动（「MCP 服务」页应显示运行中）。")
    })?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nAccept: application/json\r\nConnection: close\r\n"
    );
    if let Some(token) = &cli.token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if let Some(body) = body {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    if let Some(body) = body {
        request.push_str(body);
    }

    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("发送请求失败：{error}"))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|error| format!("读取响应失败：{error}"))?;
    parse_response(&raw)
}

fn split_authority(authority: &str) -> Result<(String, u16), String> {
    if let Some((host, port)) = authority.rsplit_once(':') {
        let port = port
            .parse::<u16>()
            .map_err(|_| format!("服务地址端口无效：{authority}"))?;
        Ok((host.trim_matches(['[', ']']).to_string(), port))
    } else {
        Ok((authority.to_string(), 80))
    }
}

fn parse_response(raw: &[u8]) -> Result<Response, String> {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "响应格式无效：缺少头部结束标记。".to_string())?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let body = &raw[split + 4..];
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| "响应格式无效：缺少状态码。".to_string())?;
    let chunked = lines.any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("transfer-encoding:") && lower.contains("chunked")
    });
    let body = if chunked { decode_chunked(body)? } else { body.to_vec() };
    Ok(Response {
        status,
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

fn decode_chunked(mut input: &[u8]) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    loop {
        let line_end = input
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| "分块响应缺少行尾。".to_string())?;
        let size_line = String::from_utf8_lossy(&input[..line_end]);
        let size_text = size_line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| format!("分块长度无效：{size_text}"))?;
        input = &input[line_end + 2..];
        if size == 0 {
            break;
        }
        if input.len() < size + 2 {
            return Err("分块响应被截断。".into());
        }
        output.extend_from_slice(&input[..size]);
        input = &input[size + 2..];
    }
    Ok(output)
}

/* ── arguments ──────────────────────────────────────────────────────────────── */

fn parse_flags(raw: &[String]) -> Result<Parsed, String> {
    let mut parsed = Parsed::default();
    let mut index = 0;
    while index < raw.len() {
        let arg = raw[index].clone();
        let (name, inline) = split_inline(&arg);
        match name {
            "--json" => {
                parsed.flags.insert("json".to_string());
            }
            "--groups-only" => {
                parsed.flags.insert("groups-only".to_string());
            }
            "--help" | "-h" => {
                parsed.flags.insert("help".to_string());
            }
            "--account" | "--chat" | "--keyword" | "--limit" | "--before" | "--since"
            | "--until" | "--max-sessions" => {
                let value = take_value(name, inline, raw, &mut index)?;
                let key = name.trim_start_matches("--").to_string();
                parsed.values.entry(key).or_default().push(value);
            }
            other => return Err(format!("未知参数：{other}\n\n{}", usage())),
        }
        index += 1;
    }
    Ok(parsed)
}

fn split_inline(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((name, value)) if name.starts_with("--") => (name, Some(value)),
        _ => (arg, None),
    }
}

fn take_value(
    name: &str,
    inline: Option<&str>,
    raw: &[String],
    index: &mut usize,
) -> Result<String, String> {
    if let Some(value) = inline {
        return Ok(value.to_string());
    }
    let next = raw.get(*index + 1).cloned();
    match next {
        Some(value) if !value.starts_with("--") => {
            *index += 1;
            Ok(value)
        }
        _ => Err(format!("{name} 需要一个值。")),
    }
}

fn parse_time(raw: &str, end_of_day: bool) -> Result<i64, String> {
    if let Ok(seconds) = raw.trim().parse::<i64>() {
        return Ok(seconds);
    }
    let (date_part, time_part) = match raw.split_once(' ') {
        Some((date, time)) => (date, Some(time)),
        None => (raw, None),
    };
    let date = NaiveDate::parse_from_str(date_part.trim(), "%Y-%m-%d")
        .map_err(|_| format!("时间无效：{raw}（支持 Unix 秒、2026-10-20 或 \"2026-10-20 18:00\"）"))?;
    let time = match time_part {
        Some(value) => NaiveTime::parse_from_str(value.trim(), "%H:%M")
            .or_else(|_| NaiveTime::parse_from_str(value.trim(), "%H:%M:%S"))
            .map_err(|_| format!("时间无效：{raw}"))?,
        None if end_of_day => NaiveTime::from_hms_opt(23, 59, 59).unwrap_or_default(),
        None => NaiveTime::from_hms_opt(0, 0, 0).unwrap_or_default(),
    };
    Local
        .from_local_datetime(&date.and_time(time))
        .single()
        .map(|value| value.timestamp())
        .ok_or_else(|| format!("时间无法转换：{raw}"))
}

fn with_account(path: &str, account: Option<&str>) -> String {
    match account.filter(|value| !value.trim().is_empty()) {
        Some(account) => {
            let separator = if path.contains('?') { '&' } else { '?' };
            format!("{path}{separator}account_id={}", encode(account))
        }
        None => path.to_string(),
    }
}

fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| format!("响应字段不符合预期：{error}"))
}

fn pretty(value: &Value) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|error| error.to_string())
}

fn usage() -> String {
    format!(
        "wxq — 只读读取 AIMyChats 白名单聊天记录

用法:
  wxq [--endpoint URL] [--token TOKEN] [--json] <命令> [参数]

命令:
  health                                    检查本地服务是否在运行
  policy   [--account ID]                   账号、白名单数量、可见会话
  sessions [--account ID]                   只列白名单会话
  messages --chat USERNAME [--limit N] [--before TS] [--account ID]
  search   --keyword KEY [--groups-only] [--max-sessions N] [--account ID]
  digest   --chat USERNAME [--chat ...] [--since TIME] [--until TIME] [--account ID]

时间:
  Unix 秒，或 2026-10-20 / \"2026-10-20 18:00\"；--until 只给日期时按当天结束。

环境变量:
  WECHAT_AGENT_ENDPOINT  默认 {DEFAULT_ENDPOINT}（旧名 WECHAT_MCP_ENDPOINT 仍兼容）
  WECHAT_AGENT_TOKEN     服务绑定非本机地址时必填（旧名 WECHAT_MCP_TOKEN 仍兼容）

示例:
  wxq policy
  wxq messages --chat 56209716836@chatroom --limit 50
  wxq search --keyword 报价 --groups-only
  wxq digest --chat 56209716836@chatroom --since 2026-10-20 --until 2026-10-27

服务由桌面应用启动，数据只读且受聊天白名单约束；wxq 不读取数据库、不解密、不发邮件。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn splits_inline_flags() {
        assert_eq!(split_inline("--limit=50"), ("--limit", Some("50")));
        assert_eq!(split_inline("--limit"), ("--limit", None));
        assert_eq!(split_inline("-h"), ("-h", None));
    }

    #[test]
    fn parses_repeated_chat_and_numbers() {
        let raw = vec![
            "--chat".to_string(),
            "a@chatroom".to_string(),
            "--chat=b@chatroom".to_string(),
            "--limit".to_string(),
            "25".to_string(),
            "--groups-only".to_string(),
        ];
        let parsed = parse_flags(&raw).expect("flags must parse");
        assert_eq!(parsed.values["chat"], vec!["a@chatroom", "b@chatroom"]);
        assert_eq!(parsed.number("limit").expect("limit"), Some(25));
        assert!(parsed.flags.contains("groups-only"));
    }

    #[test]
    fn rejects_missing_value() {
        let raw = vec!["--chat".to_string()];
        assert!(parse_flags(&raw).is_err());
    }

    #[test]
    fn parses_dates_and_epoch() {
        assert_eq!(parse_time("1730000000", false).expect("epoch"), 1730000000);
        let day = parse_time("2026-10-20", false).expect("date");
        let end = parse_time("2026-10-20", true).expect("date end");
        assert!(end - day >= 86_000);
    }

    #[test]
    fn encodes_query_values() {
        assert_eq!(encode("56209716836@chatroom"), "56209716836%40chatroom");
        assert_eq!(encode("报价 表"), "%E6%8A%A5%E4%BB%B7%20%E8%A1%A8");
    }

    #[test]
    fn decodes_chunked_body() {
        let raw = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        assert_eq!(decode_chunked(raw).expect("chunks"), b"hello world");
    }

    #[test]
    fn parses_http_response_with_content_length() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}";
        let response = parse_response(raw).expect("response");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "{\"ok\":true}");
    }

    #[test]
    fn formats_newest_first_messages_as_transcript() {
        let list = MessageList {
            account_id: "acc".into(),
            username: "chat@chatroom".into(),
            messages: vec![
                MessageItem {
                    local_id: 2,
                    time: 1_700_000_120,
                    sender: Some("wxid_b".into()),
                    sender_name: Some("李四".into()),
                    kind: 1,
                    text: "第二条".into(),
                    is_self: false,
                },
                MessageItem {
                    local_id: 1,
                    time: 1_700_000_000,
                    sender: Some("me".into()),
                    sender_name: None,
                    kind: 1,
                    text: "第一条".into(),
                    is_self: true,
                },
            ],
            has_more: true,
        };
        let text = format_messages(&list);
        let first = text.find("第一条").expect("first");
        let second = text.find("第二条").expect("second");
        assert!(first < second, "oldest message must be printed first");
        assert!(text.contains("我: 第一条"));
        assert!(text.contains("--before 1700000000"));
    }

    #[test]
    fn policy_output_mentions_empty_whitelist() {
        let policy = Policy {
            account_id: "acc".into(),
            whitelist_count: 0,
            sessions: vec![],
            automation_enabled: false,
        };
        assert!(format_policy(&policy).contains("白名单为空"));
    }

    #[test]
    fn digest_output_keeps_rule_disclaimer() {
        let digest = DigestReport {
            markdown: "## 摘要\n- 完成 A".into(),
            sessions: 1,
            messages: 2,
            todo: 1,
            ..Default::default()
        };
        let text = format_digest(&digest);
        assert!(text.contains("不是 AI 总结"));
        assert!(text.contains("待办 1"));
    }

    #[test]
    fn json_number_flag_is_available_to_dispatch() {
        let raw = vec!["--json".to_string(), "--limit".to_string(), "5".to_string()];
        let parsed = parse_flags(&raw).expect("parse");
        assert!(parsed.flags.contains("json"));
        assert_eq!(parsed.number("limit").expect("limit"), Some(5));
        assert_eq!(json!({"ok": true}).get("ok"), Some(&Value::Bool(true)));
    }
}
