use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{Local, Timelike};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{message::header::ContentType, Message, SmtpTransport, Transport};
use serde::Serialize;
use sha2::Digest;
use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::Read;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::Emitter;
use tauri::{AppHandle, Manager, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

extern crate self as wechat_devify_bridge_lib;

pub mod access;
pub mod agent;
mod desktop_credentials;
pub mod history;
pub mod perm;
pub mod settings;
pub mod wechat;

pub mod mcp;

use wechat::capture;
use wechat::crypto;
use wechat::decrypt::{self, DecryptOutcome};
use wechat::discover::{self, ScanResult};
use wechat::keys::{self, KeyStore};
use wechat::query::{self, DigestReport, MessageItem, SearchReport, SessionItem};

#[derive(Default)]
struct SyncState {
    active_accounts: Mutex<HashSet<String>>,
}

#[derive(Default)]
struct UploadState {
    retrying: Mutex<HashSet<String>>,
}

struct RetryGuard<'a> {
    retrying: &'a Mutex<HashSet<String>>,
    id: String,
}

impl Drop for RetryGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut retrying) = self.retrying.lock() {
            retrying.remove(&self.id);
        }
    }
}

fn begin_retry<'a>(state: &'a UploadState, id: &str) -> Result<RetryGuard<'a>, String> {
    let mut retrying = state.retrying.lock().map_err(|_| "无法检查任务状态。")?;
    if !retrying.insert(id.to_string()) {
        return Err("该任务正在重试，请稍候。".into());
    }
    Ok(RetryGuard {
        retrying: &state.retrying,
        id: id.to_string(),
    })
}

#[derive(Default)]
struct DevifyAuthState {
    pending: Mutex<Option<(String, String)>>,
}

#[derive(Serialize)]
struct DevifyAuthResult {
    device_id: String,
    expires_in: u64,
}

#[derive(serde::Deserialize)]
struct DevifyAuthConfig {
    client_id: String,
    browser_authorize_path: String,
    token_path: String,
    redirect_uri: String,
}

async fn devify_auth_config(base_url: &str) -> Result<DevifyAuthConfig, String> {
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/auth/desktop/config",
            base_url.trim_end_matches('/')
        ))
        .send()
        .await
        .map_err(|e| format!("读取 Devify 登录配置失败：{e}"))?;
    if !response.status().is_success() {
        return Err(format!("读取 Devify 登录配置失败：{}", response.status()));
    }
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("读取 Devify 登录配置响应失败：{e}"))?;
    serde_json::from_value(payload.get("data").unwrap_or(&payload).clone())
        .map_err(|e| format!("Devify 登录配置格式无效：{e}"))
}

async fn devify_access_token_async(app_data: &Path, base_url: &str) -> Result<String, String> {
    let mut tokens = desktop_credentials::load(app_data, base_url)?;
    let access = tokens.access.as_str();
    let exp = access
        .split('.')
        .nth(1)
        .and_then(|part| URL_SAFE_NO_PAD.decode(part).ok())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|payload| payload.get("exp").and_then(|v| v.as_i64()));
    if !access.is_empty()
        && exp
            .map(|value| value > chrono::Utc::now().timestamp() + 60)
            .unwrap_or(false)
    {
        return Ok(access.to_string());
    }
    let refresh = if tokens.refresh.is_empty() {
        return Err("Devify refresh token 不存在，请重新登录。".into());
    } else {
        &tokens.refresh
    };
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/auth/token/refresh",
            base_url.trim_end_matches('/')
        ))
        .json(&serde_json::json!({"refresh": refresh}))
        .send()
        .await
        .map_err(|e| format!("刷新 Devify token 失败：{e}"))?;
    if !response.status().is_success() {
        return Err("Devify token 已失效，请重新登录。".into());
    }
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("读取刷新响应失败：{e}"))?;
    let data = payload.get("data").unwrap_or(&payload);
    let new_access = data
        .get("access")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "刷新响应缺少 access。".to_string())?;
    tokens.access = new_access.to_string();
    if let Some(new_refresh) = data.get("refresh").and_then(|v| v.as_str()) {
        tokens.refresh = new_refresh.to_string();
    }
    desktop_credentials::save(app_data, &tokens)?;
    Ok(new_access.to_string())
}

#[derive(Serialize)]
struct DevifyUploadResult {
    job_id: String,
    batch_id: i64,
    status: String,
    message_count: usize,
    conversations: usize,
}

#[derive(Serialize)]
struct DevifyUploadStatus {
    batch_id: i64,
    status: String,
    message_count: i64,
    duplicate_count: i64,
    error_message: String,
}

#[tauri::command]
async fn devify_upload_status(app: AppHandle, batch_id: i64) -> Result<DevifyUploadStatus, String> {
    let settings = settings::load(&app_data(&app))?;
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/chat/uploads/{batch_id}",
            settings.devify.base_url.trim_end_matches('/')
        ))
        .bearer_auth(devify_access_token_async(&app_data(&app), &settings.devify.base_url).await?)
        .send()
        .await
        .map_err(|e| format!("读取上传状态失败：{e}"))?;
    if !response.status().is_success() {
        return Err(format!("读取上传状态失败：{}", response.status()));
    }
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("读取上传状态响应失败：{e}"))?;
    let data = payload.get("data").unwrap_or(&payload);
    Ok(DevifyUploadStatus {
        batch_id,
        status: data
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        message_count: data
            .get("message_count")
            .and_then(|v| v.as_i64())
            .unwrap_or_default(),
        duplicate_count: data
            .get("duplicate_count")
            .and_then(|v| v.as_i64())
            .unwrap_or_default(),
        error_message: data
            .get("error_message")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

#[derive(serde::Deserialize, Serialize)]
struct DevifySpace {
    id: i64,
    name: String,
    kind: String,
    status: String,
    role: Option<String>,
    #[serde(default)]
    is_global_admin: bool,
    #[serde(default)]
    admin_takeover_active: bool,
}

#[tauri::command]
async fn devify_space_takeover(
    app: AppHandle,
    space_id: i64,
    reason: String,
) -> Result<(), String> {
    if reason.trim().is_empty() {
        return Err("请填写接管原因。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    let token = devify_access_token_async(&app_data(&app), &settings.devify.base_url).await?;
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/spaces/{space_id}/takeover",
            settings.devify.base_url.trim_end_matches('/')
        ))
        .bearer_auth(token)
        .json(&serde_json::json!({"reason": reason.trim()}))
        .send()
        .await
        .map_err(|e| format!("接管空间失败：{e}"))?;
    if !response.status().is_success() {
        return Err(format!("接管空间失败：{}", response.status()));
    }
    Ok(())
}

#[tauri::command]
async fn devify_spaces(app: AppHandle) -> Result<Vec<DevifySpace>, String> {
    let settings = settings::load(&app_data(&app))?;
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/spaces",
            settings.devify.base_url.trim_end_matches('/')
        ))
        .bearer_auth(devify_access_token_async(&app_data(&app), &settings.devify.base_url).await?)
        .send()
        .await
        .map_err(|e| format!("读取 Devify 空间失败：{e}"))?;
    if !response.status().is_success() {
        return Err(format!("读取 Devify 空间失败：{}", response.status()));
    }
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("读取空间响应失败：{e}"))?;
    let data = payload.get("data").unwrap_or(&payload);
    serde_json::from_value(data.clone()).map_err(|e| format!("空间响应格式无效：{e}"))
}

#[tauri::command]
async fn devify_analysis_list(
    app: AppHandle,
    kind: String,
    space_id: i64,
    page: Option<u32>,
) -> Result<serde_json::Value, String> {
    if !matches!(kind.as_str(), "events" | "reports") {
        return Err("不支持的服务端分析类型。".into());
    }
    if space_id <= 0 {
        return Err("请先选择 Devify 空间。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    if settings.devify.active_space_id != Some(space_id) {
        return Err("Devify 空间已改变，请重新选择后再同步。".into());
    }
    let resource = if kind == "events" {
        "events"
    } else {
        "reports"
    };
    let first_page = page.unwrap_or(1).max(1);
    let client = reqwest::Client::new();
    let token = devify_access_token_async(&app_data(&app), &settings.devify.base_url).await?;
    let mut all_items = Vec::new();
    let mut current_page = first_page;
    let mut total = None;
    for _ in 0..100 {
        let url = format!(
            "{}/api/v1/spaces/{space_id}/{resource}?page={current_page}&page_size=100",
            settings.devify.base_url.trim_end_matches('/')
        );
        let response = client
            .get(url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| format!("同步服务端{kind}失败：{e}"))?;
        let status = response.status();
        let payload: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("读取服务端{kind}响应失败：{e}"))?;
        if !status.is_success() {
            let detail = payload
                .get("detail")
                .and_then(|value| value.as_str())
                .unwrap_or("服务端拒绝了请求");
            return Err(format!("同步服务端{kind}失败：{detail}"));
        }
        let data = payload.get("data").cloned().unwrap_or(payload);
        if let Some(items) = data.get("list").and_then(|value| value.as_array()) {
            all_items.extend(items.iter().cloned());
        }
        total = data
            .get("pagination")
            .and_then(|value| value.get("total"))
            .and_then(serde_json::Value::as_u64);
        let next = data
            .get("pagination")
            .and_then(|value| value.get("next"))
            .and_then(|value| (!value.is_null()).then_some(value));
        if next.is_none() {
            break;
        }
        current_page += 1;
    }
    Ok(serde_json::json!({
        "list": all_items,
        "pagination": { "total": total.unwrap_or(all_items.len() as u64), "page": first_page, "page_size": all_items.len(), "next": null }
    }))
}

fn validate_devify_space(settings: &settings::AppSettings, space_id: i64) -> Result<(), String> {
    if space_id <= 0 || settings.devify.active_space_id != Some(space_id) {
        return Err("Devify 空间已改变，请重新选择后再同步。".into());
    }
    Ok(())
}

async fn devify_json_request(
    app: &AppHandle,
    method: reqwest::Method,
    path: String,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(app))?;
    let token = devify_access_token_async(&app_data(app), &settings.devify.base_url).await?;
    let client = reqwest::Client::new();
    let mut request = client
        .request(
            method,
            format!("{}{}", settings.devify.base_url.trim_end_matches('/'), path),
        )
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("访问 Devify 失败：{e}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::NO_CONTENT {
        return Ok(serde_json::Value::Null);
    }
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("读取 Devify 响应失败：{e}"))?;
    if !status.is_success() {
        let detail = payload
            .get("detail")
            .and_then(|value| value.as_str())
            .unwrap_or("服务端拒绝了请求");
        return Err(format!("Devify 请求失败：{detail}"));
    }
    Ok(payload.get("data").cloned().unwrap_or(payload))
}

#[tauri::command]
async fn devify_analysis_detail(
    app: AppHandle,
    kind: String,
    space_id: i64,
    item_id: i64,
) -> Result<serde_json::Value, String> {
    if !matches!(kind.as_str(), "events" | "reports") || item_id <= 0 {
        return Err("服务端分析对象无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
    let resource = if kind == "events" { "events" } else { "reports" };
    devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/{resource}/{item_id}"),
        None,
    )
    .await
}

#[tauri::command]
async fn devify_event_notes(
    app: AppHandle,
    space_id: i64,
    event_id: i64,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
    let event = devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/events/{event_id}"),
        None,
    )
    .await?;
    if event.get("space_id").and_then(serde_json::Value::as_i64) != Some(space_id) {
        return Err("事件不属于当前 Devify 空间。".into());
    }
    let ids = event
        .get("note_ids")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut notes = Vec::new();
    for id in ids {
        let Some(id) = id.as_i64() else { continue };
        if let Ok(note) = devify_json_request(&app, reqwest::Method::GET, format!("/api/v1/notes/{id}"), None).await {
            notes.push(note);
        }
    }
    Ok(serde_json::Value::Array(notes))
}

#[tauri::command]
async fn devify_topics(app: AppHandle, space_id: i64) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
    devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/spaces/{space_id}/topics"),
        None,
    )
    .await
}

#[tauri::command]
async fn devify_topic_create(
    app: AppHandle,
    space_id: i64,
    title: String,
    instruction: String,
    period: String,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
    let title = title.trim();
    let instruction = instruction.trim();
    if title.is_empty() || instruction.is_empty() {
        return Err("请填写主题名称和关注说明。".into());
    }
    if !matches!(period.as_str(), "day" | "week" | "month") {
        return Err("主题周期无效。".into());
    }
    devify_json_request(
        &app,
        reqwest::Method::POST,
        format!("/api/v1/spaces/{space_id}/topics"),
        Some(serde_json::json!({"title": title, "instruction": instruction, "period": period})),
    )
    .await
}

#[tauri::command]
async fn devify_topic_update(
    app: AppHandle,
    topic_id: i64,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    if topic_id <= 0 {
        return Err("主题标识无效。".into());
    }
    devify_json_request(
        &app,
        reqwest::Method::PATCH,
        format!("/api/v1/topics/{topic_id}"),
        Some(serde_json::json!({"enabled": enabled})),
    )
    .await
}

#[tauri::command]
async fn devify_topic_delete(app: AppHandle, topic_id: i64) -> Result<(), String> {
    if topic_id <= 0 {
        return Err("主题标识无效。".into());
    }
    devify_json_request(
        &app,
        reqwest::Method::DELETE,
        format!("/api/v1/topics/{topic_id}"),
        None,
    )
    .await
    .map(|_| ())
}

#[tauri::command]
async fn devify_questions(app: AppHandle, space_id: i64) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
    devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/spaces/{space_id}/questions"),
        None,
    )
    .await
}

#[tauri::command]
async fn devify_question_ask(
    app: AppHandle,
    space_id: i64,
    question: String,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
    let question = question.trim();
    if question.is_empty() {
        return Err("请输入问题。".into());
    }
    devify_json_request(
        &app,
        reqwest::Method::POST,
        format!("/api/v1/spaces/{space_id}/questions"),
        Some(serde_json::json!({"question": question})),
    )
    .await
}

#[derive(Serialize)]
struct UploadPreviewSession {
    username: String,
    display: String,
    message_count: usize,
}

#[derive(Serialize)]
struct UploadPreview {
    fingerprint: String,
    sessions: Vec<UploadPreviewSession>,
    message_count: usize,
}

struct PreparedUpload {
    preview: UploadPreview,
    conversations: Vec<serde_json::Value>,
    messages: Vec<serde_json::Value>,
    parts: Vec<serde_json::Value>,
}

fn upload_fingerprint(value: &serde_json::Value) -> String {
    hex::encode(sha2::Sha256::digest(value.to_string().as_bytes()))
}

fn verify_upload_preview(expected: Option<&str>, actual: &str) -> Result<(), String> {
    if expected.is_some_and(|value| value != actual) {
        return Err("聊天记录或上传范围已变化，请重新预览后确认。".into());
    }
    Ok(())
}

fn filter_upload_messages(
    messages: Vec<MessageItem>,
    selected: Option<&[settings::MessageRef]>,
) -> Result<Vec<MessageItem>, String> {
    let Some(selected) = selected else {
        return Ok(messages);
    };
    if selected.is_empty() {
        return Err("逐条选择时请至少选择一条消息。".into());
    }
    let wanted: HashSet<_> = selected.iter().cloned().collect();
    if wanted.len() != selected.len() {
        return Err("消息选择包含重复项。".into());
    }
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for message in messages {
        let key = settings::MessageRef {
            local_id: message.local_id,
            time: message.time,
        };
        if wanted.contains(&key) {
            if !seen.insert(key) {
                return Err("消息标识不唯一，无法安全选择。".into());
            }
            result.push(message);
        }
    }
    if seen.len() != wanted.len() {
        return Err("所选消息已不在当前范围内，请重新选择。".into());
    }
    Ok(result)
}

fn collect_upload_messages(
    cache: &Path,
    username: &str,
    since: Option<i64>,
    until: Option<i64>,
    max_messages: Option<usize>,
) -> Result<Vec<MessageItem>, String> {
    let mut before = None;
    let mut session_messages = Vec::new();
    loop {
        let page = query::messages(cache, username, 500, before)?;
        if page.is_empty() {
            break;
        }
        let oldest = page.iter().map(|message| message.time).min();
        // A timestamp-only cursor cannot safely distinguish a full second at a page
        // boundary. Refuse rather than silently omit records from a confirmed upload.
        if page.len() == 500 {
            if let Some(boundary) = oldest {
                let seen = page.iter().filter(|m| m.time == boundary).count();
                if query::count_at_timestamp(cache, username, boundary)? > seen {
                    return Err(
                        "聊天记录分页遇到相同时间戳，无法完整读取；请先使用其他方式导出。".into(),
                    );
                }
            }
        }
        session_messages.extend(page.into_iter().filter(|message| {
            since.map(|v| message.time >= v).unwrap_or(true)
                && until.map(|v| message.time < v).unwrap_or(true)
        }));
        if max_messages.is_some_and(|max| session_messages.len() > max) {
            return Err("此范围的消息超过 5000 条，请缩小日期范围后逐条选择。".into());
        }
        if since
            .zip(oldest)
            .map(|(start, oldest)| oldest <= start)
            .unwrap_or(false)
            || oldest.is_none()
        {
            break;
        }
        before = oldest;
    }
    Ok(session_messages)
}

fn prepare_upload(
    cache: &Path,
    account_id: &str,
    space_id: i64,
    base_url: &str,
    usernames: &[String],
    since: Option<i64>,
    until: Option<i64>,
    message_selection: &BTreeMap<String, Vec<settings::MessageRef>>,
) -> Result<PreparedUpload, String> {
    if usernames.is_empty() {
        return Err("请至少选择一个会话。".into());
    }
    if since.zip(until).is_some_and(|(start, end)| start >= end) {
        return Err("上传日期范围无效。".into());
    }
    let unique: HashSet<&String> = usernames.iter().collect();
    if unique.len() != usernames.len() {
        return Err("会话列表包含重复项。".into());
    }
    if message_selection.keys().any(|name| !unique.contains(name)) {
        return Err("消息选择包含未选择的会话。".into());
    }
    let mut selected = query::sessions(cache)?
        .into_iter()
        .filter(|s| unique.contains(&s.username))
        .collect::<Vec<_>>();
    if selected.len() != usernames.len() {
        return Err("部分所选会话已不可用，请重新选择。".into());
    }
    selected.sort_by(|a, b| a.username.cmp(&b.username));
    let mut conversations = Vec::new();
    let mut messages = Vec::new();
    let mut parts = Vec::new();
    let mut summary = Vec::new();
    for session in &selected {
        conversations.push(serde_json::json!({"conversation_id":session.username,"title":session.display,"kind":if session.is_group {"group"} else {"direct"}}));
        let mut session_messages =
            collect_upload_messages(cache, &session.username, since, until, None)?;
        session_messages = filter_upload_messages(
            session_messages,
            message_selection.get(&session.username).map(Vec::as_slice),
        )?;
        let mut ids = HashSet::new();
        if session_messages
            .iter()
            .any(|message| !ids.insert(message.local_id))
        {
            return Err("会话中存在重复消息标识，无法安全上传。".into());
        }
        session_messages.sort_by(|a, b| a.time.cmp(&b.time).then(a.local_id.cmp(&b.local_id)));
        summary.push(UploadPreviewSession {
            username: session.username.clone(),
            display: session.display.clone(),
            message_count: session_messages.len(),
        });
        for message in session_messages {
            messages.push(serde_json::json!({"conversation_id":session.username,"message_key":format!("src:{}",message.local_id),"source_message_id":message.local_id.to_string(),"sender_key":message.sender,"sender_name":message.sender_name,"direction":if message.is_self {"outgoing"} else {"incoming"},"occurred_at":chrono::DateTime::from_timestamp(message.time,0).map(|v|v.to_rfc3339()),"source_fields":{"kind":message.kind}}));
            parts.push(serde_json::json!({"conversation_id":session.username,"message_key":format!("src:{}",message.local_id),"part_key":"text-0","kind":"text","text":message.text}));
        }
    }
    if messages.is_empty() {
        return Err("所选会话在时间范围内没有可上传的消息。".into());
    }
    let fingerprint = upload_fingerprint(
        &serde_json::json!({"account":account_id,"space":space_id,"base_url":base_url.trim_end_matches('/'),"since":since,"until":until,"conversations":conversations,"messages":messages,"parts":parts}),
    );
    let preview = UploadPreview {
        fingerprint,
        sessions: summary,
        message_count: messages.len(),
    };
    Ok(PreparedUpload {
        preview,
        conversations,
        messages,
        parts,
    })
}

fn validate_account_id(account_id: &str) -> Result<(), String> {
    let mut components = Path::new(account_id).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err("微信账号标识无效。".into());
    }
    Ok(())
}

fn validate_upload_scope(
    settings: &settings::AppSettings,
    account_id: &str,
    space_id: i64,
    usernames: &[String],
) -> Result<(), String> {
    validate_account_id(account_id)?;
    if space_id <= 0 || settings.devify.active_space_id != Some(space_id) {
        return Err("上传目标空间已改变，请重新选择。".into());
    }
    let allowed = settings.chat.allowed_usernames.get(account_id);
    if usernames.is_empty()
        || usernames
            .iter()
            .any(|name| !allowed.is_some_and(|list| list.contains(name)))
    {
        return Err("上传范围包含不在当前白名单中的会话。".into());
    }
    Ok(())
}

#[tauri::command]
async fn wechat_collection_messages(
    app: AppHandle,
    account_id: String,
    username: String,
    since: Option<i64>,
    until: Option<i64>,
) -> Result<Vec<MessageItem>, String> {
    if since.zip(until).is_some_and(|(start, end)| start >= end) {
        return Err("日期范围无效。".into());
    }
    let allowed = allowed_usernames(&app, &account_id)?;
    if !allowed.contains(&username) {
        return Err("该会话不在白名单中。".into());
    }
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    if !query::is_chat_session(&cache, &username) {
        return Err("该会话不在可采集范围中。".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        collect_upload_messages(&cache, &username, since, until, Some(5000))
    })
    .await
    .map_err(|e| format!("读取消息失败：{e}"))?
}

#[tauri::command]
async fn devify_upload_preview(
    app: AppHandle,
    account_id: String,
    space_id: i64,
    usernames: Vec<String>,
    since: Option<i64>,
    until: Option<i64>,
    message_selection: Option<BTreeMap<String, Vec<settings::MessageRef>>>,
) -> Result<UploadPreview, String> {
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    validate_upload_scope(&settings, &account_id, space_id, &usernames)?;
    let cache = decrypt::cache_dir(&data, &account_id);
    tauri::async_runtime::spawn_blocking(move || {
        prepare_upload(
            &cache,
            &account_id,
            space_id,
            &settings.devify.base_url,
            &usernames,
            since,
            until,
            &message_selection.unwrap_or_default(),
        )
        .map(|prepared| prepared.preview)
    })
    .await
    .map_err(|e| format!("读取上传预览失败：{e}"))?
}

#[tauri::command]
async fn devify_upload_selected(
    app: AppHandle,
    account_id: String,
    space_id: i64,
    usernames: Vec<String>,
    since: Option<i64>,
    until: Option<i64>,
    message_selection: Option<BTreeMap<String, Vec<settings::MessageRef>>>,
    fingerprint: String,
) -> Result<DevifyUploadResult, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_upload_scope(&settings, &account_id, space_id, &usernames)?;
    if fingerprint.len() != 64 || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("请先预览并确认上传范围。".into());
    }
    let message_selection = message_selection.unwrap_or_default();
    let scope = settings::UploadScope {
        usernames: usernames.clone(),
        since,
        until,
        base_url: settings.devify.base_url.clone(),
        message_selection: message_selection.clone(),
    };
    let job_id = format!("upload-{}", chrono::Utc::now().timestamp_millis());
    let now = chrono::Utc::now().to_rfc3339();
    let mut job = settings::UploadJob {
        id: job_id.clone(),
        space_id,
        account_id: account_id.clone(),
        batch_id: None,
        status: "running".into(),
        message: "正在生成并上传聊天包".into(),
        created_at: now.clone(),
        updated_at: now,
        scope: Some(scope),
    };
    let mut next_settings = settings.clone();
    next_settings.upload_queue.push(job.clone());
    settings::save(&app_data(&app), &next_settings)?;
    let result = devify_upload_wechat(
        app.clone(),
        space_id,
        account_id,
        usernames,
        since,
        until,
        Some(settings.devify.base_url.clone()),
        Some(fingerprint),
        message_selection,
    )
    .await;
    let mut next_settings = settings::load(&app_data(&app))?;
    match result {
        Ok(mut result) => {
            job.batch_id = Some(result.batch_id);
            job.status = "processing".into();
            job.message = "已提交服务端解析".into();
            job.updated_at = chrono::Utc::now().to_rfc3339();
            next_settings.upload_queue.retain(|item| item.id != job.id);
            next_settings.upload_queue.insert(0, job);
            settings::save(&app_data(&app), &next_settings)?;
            result.job_id = job_id;
            Ok(result)
        }
        Err(error) => {
            job.status = "failed".into();
            job.message = error.clone();
            job.updated_at = chrono::Utc::now().to_rfc3339();
            next_settings.upload_queue.retain(|item| item.id != job.id);
            next_settings.upload_queue.insert(0, job);
            settings::save(&app_data(&app), &next_settings)?;
            Err(error)
        }
    }
}

#[tauri::command]
async fn devify_upload_retry_preview(
    app: AppHandle,
    job_id: String,
) -> Result<UploadPreview, String> {
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    let job = settings
        .upload_queue
        .iter()
        .find(|item| item.id == job_id)
        .cloned()
        .ok_or_else(|| "找不到上传任务。".to_string())?;
    let scope = job.retry_scope(&settings)?.clone();
    let cache = decrypt::cache_dir(&data, &job.account_id);
    tauri::async_runtime::spawn_blocking(move || {
        prepare_upload(
            &cache,
            &job.account_id,
            job.space_id,
            &scope.base_url,
            &scope.usernames,
            scope.since,
            scope.until,
            &scope.message_selection,
        )
        .map(|prepared| prepared.preview)
    })
    .await
    .map_err(|e| format!("读取上传预览失败：{e}"))?
}

#[tauri::command]
async fn devify_upload_retry(
    app: AppHandle,
    state: State<'_, UploadState>,
    job_id: String,
    fingerprint: String,
) -> Result<DevifyUploadResult, String> {
    if fingerprint.len() != 64 || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("请先预览并确认重试范围。".into());
    }
    let _guard = begin_retry(&state, &job_id)?;
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    let job = settings
        .upload_queue
        .iter()
        .find(|item| item.id == job_id)
        .cloned()
        .ok_or_else(|| "找不到上传任务。".to_string())?;
    let scope = job.retry_scope(&settings)?.clone();
    let result = devify_upload_wechat(
        app.clone(),
        job.space_id,
        job.account_id.clone(),
        scope.usernames,
        scope.since,
        scope.until,
        Some(scope.base_url),
        Some(fingerprint),
        scope.message_selection,
    )
    .await;
    let mut next = settings::load(&data)?;
    if let Some(item) = next.upload_queue.iter_mut().find(|item| item.id == job_id) {
        item.updated_at = chrono::Utc::now().to_rfc3339();
        match &result {
            Ok(value) => {
                item.status = "processing".into();
                item.batch_id = Some(value.batch_id);
                item.message = "已重新提交服务端解析".into();
            }
            Err(error) => {
                item.status = "failed".into();
                item.message = error.clone();
            }
        }
    }
    settings::save(&data, &next)?;
    result.map(|mut value| {
        value.job_id = job_id;
        value
    })
}

async fn devify_upload_wechat(
    app: AppHandle,
    space_id: i64,
    account_id: String,
    usernames: Vec<String>,
    since: Option<i64>,
    until: Option<i64>,
    expected_base_url: Option<String>,
    expected_fingerprint: Option<String>,
    message_selection: BTreeMap<String, Vec<settings::MessageRef>>,
) -> Result<DevifyUploadResult, String> {
    if usernames.is_empty() {
        return Err("请至少选择一个会话。".into());
    }
    let allowed = allowed_usernames(&app, &account_id)?;
    if usernames.iter().any(|name| !allowed.contains(name)) {
        return Err("上传范围包含不在当前白名单中的会话。".into());
    }
    let data = app_data(&app);
    let cache = decrypt::cache_dir(&data, &account_id);
    let base_url = expected_base_url
        .clone()
        .unwrap_or(settings::load(&data)?.devify.base_url);
    let account_for_prepare = account_id.clone();
    let names_for_prepare = usernames.clone();
    let prepared = tauri::async_runtime::spawn_blocking(move || {
        prepare_upload(
            &cache,
            &account_for_prepare,
            space_id,
            &base_url,
            &names_for_prepare,
            since,
            until,
            &message_selection,
        )
    })
    .await
    .map_err(|e| format!("准备上传失败：{e}"))??;
    verify_upload_preview(
        expected_fingerprint.as_deref(),
        &prepared.preview.fingerprint,
    )?;
    let message_count = prepared.preview.message_count;
    let session_count = prepared.preview.sessions.len();
    let PreparedUpload {
        conversations,
        messages,
        parts,
        ..
    } = prepared;
    let manifest = serde_json::json!({
        "schema_version": "wechat.v1",
        "platform": "wechat",
        "source_namespace": account_id,
        "counts": { "conversations": conversations.len(), "messages": messages.len() },
        "scope": { "conversation_ids": usernames },
    });
    let mut archive = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut archive);
        let options = SimpleFileOptions::default();
        for (name, records) in [
            ("conversations.jsonl", conversations),
            ("messages.jsonl", messages),
            ("message_parts.jsonl", parts),
        ] {
            zip.start_file(name, options).map_err(|e| e.to_string())?;
            for record in records {
                writeln!(
                    zip,
                    "{}",
                    serde_json::to_string(&record).map_err(|e| e.to_string())?
                )
                .map_err(|e| e.to_string())?;
            }
        }
        zip.start_file("manifest.json", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(
            serde_json::to_string(&manifest)
                .map_err(|e| e.to_string())?
                .as_bytes(),
        )
        .map_err(|e| e.to_string())?;
        zip.finish().map_err(|e| e.to_string())?;
    }
    let body = archive.into_inner();
    let settings = settings::load(&data)?;
    if expected_base_url.as_deref().is_some_and(|url| {
        url.trim_end_matches('/') != settings.devify.base_url.trim_end_matches('/')
    }) {
        return Err("服务地址已更改，上传已取消，请重新创建采集任务。".into());
    }
    let allowed_now = settings.chat.allowed_usernames.get(&account_id);
    if usernames
        .iter()
        .any(|name| !allowed_now.is_some_and(|list| list.contains(name)))
    {
        return Err("上传范围中的会话已移出白名单，上传已取消。".into());
    }
    let token = devify_access_token_async(&data, &settings.devify.base_url).await?;
    let client = reqwest::Client::new();
    let key = format!(
        "wechat-{}-{}",
        account_id,
        chrono::Utc::now().timestamp_millis()
    );
    let created = client
        .post(format!(
            "{}/api/v1/spaces/{space_id}/chat/uploads",
            settings.devify.base_url.trim_end_matches('/')
        ))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "source_type": "wechat",
            "source_account_id": account_id,
            "idempotency_key": key,
            "archive_size": body.len(),
        }))
        .send()
        .await
        .map_err(|e| format!("创建上传批次失败：{e}"))?;
    if !created.status().is_success() {
        return Err(format!("创建上传批次失败：{}", created.status()));
    }
    let payload: serde_json::Value = created
        .json()
        .await
        .map_err(|e| format!("读取上传响应失败：{e}"))?;
    let batch = payload.get("data").unwrap_or(&payload);
    let batch_id = batch
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "上传响应缺少批次 ID。".to_string())?;
    let form = reqwest::multipart::Form::new().part(
        "archive",
        reqwest::multipart::Part::bytes(body)
            .file_name("wechat.v1.zip")
            .mime_str("application/zip")
            .map_err(|e| e.to_string())?,
    );
    let uploaded = client
        .put(format!(
            "{}/api/v1/chat/uploads/{batch_id}/archive",
            settings.devify.base_url.trim_end_matches('/')
        ))
        .bearer_auth(&token)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("上传归档失败：{e}"))?;
    if !uploaded.status().is_success() {
        return Err(format!("上传归档失败：{}", uploaded.status()));
    }
    let completed = client
        .post(format!(
            "{}/api/v1/chat/uploads/{batch_id}/complete",
            settings.devify.base_url.trim_end_matches('/')
        ))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| format!("提交解析失败：{e}"))?;
    if !completed.status().is_success() {
        return Err(format!("提交解析失败：{}", completed.status()));
    }
    Ok(DevifyUploadResult {
        job_id: String::new(),
        batch_id,
        status: "processing".into(),
        message_count,
        conversations: session_count,
    })
}

#[tauri::command]
async fn devify_auth_start(
    app: AppHandle,
    state: State<'_, DevifyAuthState>,
    device_label: Option<String>,
) -> Result<String, String> {
    let settings = settings::load(&app_data(&app))?;
    let auth_config = devify_auth_config(&settings.devify.base_url).await?;
    let mut state_bytes = [0u8; 24];
    let mut verifier_bytes = [0u8; 48];
    getrandom::getrandom(&mut state_bytes).map_err(|e| e.to_string())?;
    getrandom::getrandom(&mut verifier_bytes).map_err(|e| e.to_string())?;
    let oauth_state = URL_SAFE_NO_PAD.encode(state_bytes);
    let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
    let digest = sha2::Sha256::digest(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(digest);
    state
        .pending
        .lock()
        .map_err(|_| "读取登录状态失败。".to_string())?
        .replace((oauth_state.clone(), verifier));
    let listener = TcpListener::bind("127.0.0.1:47832")
        .await
        .map_err(|e| format!("无法启动本地登录回调服务（47832）：{e}"))?;
    let callback_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buffer = [0u8; 8192];
            if let Ok(size) = stream.read(&mut buffer).await {
                let request = String::from_utf8_lossy(&buffer[..size]);
                if let Some(target) = request
                    .lines()
                    .next()
                    .and_then(|line| line.strip_prefix("GET ")?.split_whitespace().next())
                {
                    let callback = format!("http://127.0.0.1:47832{target}");
                    let _ = callback_handle.emit("devify-auth-loopback", callback);
                }
            }
            let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\nLogin returned to AImyChats. You can close this page.".as_bytes();
            let _ = stream.write_all(response).await;
        }
    });
    let mut url = Url::parse(&format!(
        "{}{}",
        settings.devify.base_url.trim_end_matches('/'),
        auth_config.browser_authorize_path
    ))
    .map_err(|e| format!("Devify 地址无效：{e}"))?;
    url.query_pairs_mut()
        .append_pair("client_id", &auth_config.client_id)
        .append_pair("redirect_uri", &auth_config.redirect_uri)
        .append_pair("mode", "browser")
        .append_pair("response_type", "code")
        .append_pair("state", &oauth_state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair(
            "device_label",
            device_label.as_deref().unwrap_or("AImyChats"),
        );
    Ok(url.to_string())
}

#[tauri::command]
fn devify_auth_open(url: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = Command::new("cmd");
    #[cfg(target_os = "windows")]
    command.args(["/C", "start", "", &url]);
    #[cfg(target_os = "macos")]
    command.arg(&url);
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(&url);
        command
    };
    command
        .status()
        .map_err(|e| format!("打开浏览器失败：{e}"))?
        .success()
        .then_some(())
        .ok_or_else(|| "打开浏览器失败。".to_string())
}

#[tauri::command]
async fn devify_auth_callback(
    app: AppHandle,
    state: State<'_, DevifyAuthState>,
    callback_url: String,
) -> Result<DevifyAuthResult, String> {
    let url = Url::parse(&callback_url).map_err(|e| format!("回调地址无效：{e}"))?;
    let code = url
        .query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.to_string())
        .ok_or_else(|| "回调缺少授权码。".to_string())?;
    let returned_state = url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.to_string())
        .unwrap_or_default();
    let (expected_state, verifier) = state
        .pending
        .lock()
        .map_err(|_| "读取登录状态失败。".to_string())?
        .take()
        .ok_or_else(|| "没有待处理的登录请求。".to_string())?;
    if returned_state != expected_state {
        return Err("登录 state 校验失败。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    let auth_config = devify_auth_config(&settings.devify.base_url).await?;
    let response = reqwest::Client::new().post(format!("{}{}", settings.devify.base_url.trim_end_matches('/'), auth_config.token_path)).json(&serde_json::json!({"grant_type":"authorization_code","client_id":auth_config.client_id,"redirect_uri":auth_config.redirect_uri,"code":code,"code_verifier":verifier,"device_label":settings.devify.device_label})).send().await.map_err(|e| format!("换取 Devify token 失败：{e}"))?;
    if !response.status().is_success() {
        return Err(format!("Devify 登录失败：{}", response.status()));
    }
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("读取 token 响应失败：{e}"))?;
    let data = payload
        .get("data")
        .filter(|v| v.is_object())
        .unwrap_or(&payload);
    let access = data
        .get("access")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| "token 响应缺少 access。".to_string())?;
    let refresh = data
        .get("refresh")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| "token 响应缺少 refresh。".to_string())?;
    desktop_credentials::save(
        &app_data(&app),
        &desktop_credentials::Credentials {
            base_url: settings.devify.base_url.trim_end_matches('/').to_string(),
            access: access.to_string(),
            refresh: refresh.to_string(),
        },
    )?;
    Ok(DevifyAuthResult {
        device_id: data
            .get("device_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        expires_in: data
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or_default(),
    })
}

#[derive(Clone, Serialize)]
struct McpStatus {
    running: bool,
    address: String,
    endpoint: String,
    local_only: bool,
    error: Option<String>,
}

#[derive(Clone)]
struct McpState {
    status: Arc<Mutex<McpStatus>>,
}

impl Default for McpState {
    fn default() -> Self {
        let bind = mcp::configured_bind();
        let local_only = !mcp::requires_token_for_ui(&bind);
        Self {
            status: Arc::new(Mutex::new(McpStatus {
                running: false,
                endpoint: format!("http://{bind}/sse"),
                address: bind,
                local_only,
                error: None,
            })),
        }
    }
}

#[tauri::command]
fn mcp_status(state: State<McpState>) -> Result<McpStatus, String> {
    state
        .status
        .lock()
        .map(|status| status.clone())
        .map_err(|_| "读取 MCP 服务状态失败。".to_string())
}

fn access_state(state: &State<McpState>) -> Result<McpStatus, String> {
    state
        .status
        .lock()
        .map(|status| status.clone())
        .map_err(|_| "读取本机服务状态失败。".to_string())
}

#[tauri::command]
fn agent_access_status(
    app: AppHandle,
    state: State<McpState>,
) -> Result<access::AccessStatus, String> {
    let service = access_state(&state)?;
    let data = app_data(&app);
    let configured = settings::load(&data)
        .map(|settings| settings.agent.provider_id)
        .unwrap_or_default();
    Ok(access::status(
        &data,
        &configured,
        &service.address,
        service.running,
        !service.local_only,
    ))
}

#[tauri::command]
fn agent_access_prompt(
    app: AppHandle,
    state: State<McpState>,
    tool: String,
    bin_dir: String,
    skills_dir: String,
) -> Result<access::PromptOut, String> {
    let service = access_state(&state)?;
    let targets = access::targets();
    let target = access::target_by_id(&targets, &tool).unwrap_or_else(|| {
        access::target_by_id(&targets, "custom")
            .unwrap_or_else(|| targets[targets.len() - 1].clone())
    });
    let bin = access::expand_path(&bin_dir)
        .ok_or_else(|| "CLI 目录必须是绝对路径或以 ~/ 开头。".to_string())?;
    let skills = access::expand_path(&skills_dir)
        .ok_or_else(|| "Skills 目录必须是绝对路径或以 ~/ 开头。".to_string())?;
    let wxq = access::resolve_wxq(&app_data(&app));
    let base_url = access::client_base(&service.address);
    let prompt = access::build_prompt(
        &target,
        &bin,
        &skills,
        &base_url,
        !service.local_only,
        service.running,
        wxq.is_some(),
    );
    Ok(access::PromptOut {
        prompt,
        bin_dir: bin.to_string_lossy().to_string(),
        skills_dir: skills.to_string_lossy().to_string(),
    })
}

/// Rebuild `wxq` from the source checkout so the download endpoint has something to serve.
/// Packaged installs without source get a clear error instead of a silent failure.
#[tauri::command]
async fn agent_build_wxq() -> Result<String, String> {
    let Some(dir) = access::source_dir() else {
        return Err("当前安装没有附带源码，无法在应用内构建 wxq。".into());
    };
    let build_dir = dir.clone();
    let output = tauri::async_runtime::spawn_blocking(move || {
        std::process::Command::new("cargo")
            .args(["build", "--release", "--bin", access::CLI_NAME])
            .current_dir(&build_dir)
            .output()
    })
    .await
    .map_err(|error| format!("构建任务失败：{error}"))?
    .map_err(|error| format!("启动 cargo 失败：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .trim()
            .chars()
            .rev()
            .take(900)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        return Err(format!("构建 wxq 失败：\n{tail}"));
    }
    let path = dir.join("target/release").join(access::CLI_NAME);
    if !path.is_file() {
        return Err("cargo 返回成功，但没有找到 target/release/wxq。".into());
    }
    Ok(path.to_string_lossy().to_string())
}

fn begin_sync(state: &SyncState, account_id: &str) -> bool {
    state
        .active_accounts
        .lock()
        .map(|mut active| active.insert(account_id.to_string()))
        .unwrap_or(false)
}

fn end_sync(state: &SyncState, account_id: &str) {
    if let Ok(mut active) = state.active_accounts.lock() {
        active.remove(account_id);
    }
}

#[derive(Serialize)]
struct MailConfigStatus {
    configured: bool,
    host: Option<String>,
    port: Option<u16>,
    from: Option<String>,
    missing: Vec<String>,
}

fn env_nonempty(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[tauri::command]
fn mail_config_status() -> MailConfigStatus {
    let mut missing = Vec::new();
    for key in [
        "DEVIFY_SMTP_HOST",
        "DEVIFY_SMTP_USERNAME",
        "DEVIFY_SMTP_PASSWORD",
        "DEVIFY_MAIL_FROM",
    ] {
        if env_nonempty(key).is_none() {
            missing.push(key.to_string());
        }
    }
    let port = env_nonempty("DEVIFY_SMTP_PORT")
        .and_then(|v| v.parse::<u16>().ok())
        .or(Some(587));
    MailConfigStatus {
        configured: missing.is_empty(),
        host: env_nonempty("DEVIFY_SMTP_HOST"),
        port,
        from: env_nonempty("DEVIFY_MAIL_FROM"),
        missing,
    }
}

fn send_mail(recipient: &str, subject: &str, body: String) -> Result<String, String> {
    let recipient = recipient.trim();
    if recipient.is_empty()
        || !recipient.contains('@')
        || recipient.contains('\n')
        || recipient.contains('\r')
    {
        return Err("收件地址无效，请填写 Devify 为你的账号分配的接收邮箱。".into());
    }
    if subject.trim().is_empty() {
        return Err("邮件主题不能为空。".into());
    }
    if body.trim().is_empty() {
        return Err("邮件正文为空，没有发送任何内容。".into());
    }
    if body.len() > 1_000_000 {
        return Err("邮件正文超过 1 MB 限制。".into());
    }

    let host = env_nonempty("DEVIFY_SMTP_HOST").ok_or("缺少环境变量 DEVIFY_SMTP_HOST")?;
    let username =
        env_nonempty("DEVIFY_SMTP_USERNAME").ok_or("缺少环境变量 DEVIFY_SMTP_USERNAME")?;
    let password =
        env_nonempty("DEVIFY_SMTP_PASSWORD").ok_or("缺少环境变量 DEVIFY_SMTP_PASSWORD")?;
    let from = env_nonempty("DEVIFY_MAIL_FROM").ok_or("缺少环境变量 DEVIFY_MAIL_FROM")?;
    let port = env_nonempty("DEVIFY_SMTP_PORT")
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(587);

    let from_mailbox = from
        .parse()
        .map_err(|_| "DEVIFY_MAIL_FROM 不是有效邮箱地址")?;
    let to_mailbox = recipient.parse().map_err(|_| "收件地址格式无效")?;
    let email = Message::builder()
        .from(from_mailbox)
        .to(to_mailbox)
        .subject(subject.trim())
        .header(ContentType::TEXT_PLAIN)
        .body(body)
        .map_err(|e| format!("构建邮件失败：{e}"))?;

    let creds = Credentials::new(username, password);
    // Port 587 uses STARTTLS; lettre's relay() selects the standard submission transport.
    let mailer = SmtpTransport::relay(&host)
        .map_err(|e| format!("SMTP 配置错误：{e}"))?
        .port(port)
        .credentials(creds)
        .build();
    mailer
        .send(&email)
        .map_err(|e| format!("SMTP 发送失败：{e}"))?;
    Ok("SMTP 已接受邮件发送请求。是否进入 Devify 的处理流程，还取决于 Devify 的邮箱域名、收件规则和后台工作进程配置。".into())
}

fn app_data(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Identity shown on the About page. The version comes from `Cargo.toml`, which is kept
/// in sync with `package.json` and `tauri.conf.json`.
#[derive(Clone, Serialize)]
struct AppInfo {
    name: String,
    version: String,
    stage: String,
    platform: String,
    arch: String,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        name: "AImyChats".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        stage: "Alpha".to_string(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    }
}

/// macOS does not let an app ask for Full Disk Access; it can only report whether the grant
/// is present and point the user at the switch that turns it on.
#[tauri::command]
fn perm_status() -> perm::DiskAccess {
    perm::status()
}

#[tauri::command]
fn perm_open_settings() -> Result<String, String> {
    perm::open_settings()
}

#[tauri::command]
fn perm_reveal_app() -> Result<String, String> {
    perm::reveal_app()
}

fn allowed_usernames(app: &AppHandle, account_id: &str) -> Result<HashSet<String>, String> {
    validate_account_id(account_id)?;
    let settings = settings::load(&app_data(app))?;
    Ok(settings
        .chat
        .allowed_usernames
        .get(account_id)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect())
}

fn automation_result(
    app: &AppHandle,
    fallback: settings::AppSettings,
    day: &str,
    status: &str,
    error: Option<String>,
) {
    let mut value = settings::load(&app_data(app)).unwrap_or(fallback);
    if status == "sent" || status == "no_messages" {
        value.automation.last_daily_run = Some(day.to_string());
    }
    value.automation.last_daily_status = Some(status.to_string());
    value.automation.last_daily_error = error;
    let _ = settings::save(&app_data(app), &value);
}

async fn automation_tick(app: &AppHandle) {
    let data = app_data(app);
    let Ok(value) = settings::load(&data) else {
        return;
    };
    let now = Local::now();
    let day = now.format("%Y-%m-%d").to_string();
    let Some((hour, minute)) = value
        .automation
        .daily_time
        .split_once(':')
        .and_then(|(hour, minute)| Some((hour.parse::<u32>().ok()?, minute.parse::<u32>().ok()?)))
    else {
        return;
    };
    let scheduled = now.hour() > hour || (now.hour() == hour && now.minute() >= minute);
    if !value.automation.enabled
        || !value.automation.daily_summary_enabled
        || !scheduled
        || value.automation.last_daily_run.as_deref() == Some(day.as_str())
    {
        return;
    }

    let account_id = value.active_account_id.clone();
    if account_id.is_empty() {
        automation_result(
            app,
            value,
            &day,
            "error",
            Some("没有配置活动微信账号。".into()),
        );
        return;
    }
    let Some(allowed) = value.chat.allowed_usernames.get(&account_id).cloned() else {
        automation_result(
            app,
            value,
            &day,
            "error",
            Some("没有配置该账号的聊天白名单。".into()),
        );
        return;
    };
    if allowed.is_empty() {
        automation_result(
            app,
            value,
            &day,
            "error",
            Some("自动日报必须配置非空聊天白名单。".into()),
        );
        return;
    }
    if value.devify.recipient.trim().is_empty() {
        automation_result(
            app,
            value,
            &day,
            "error",
            Some("没有配置 Devify 接收邮箱。".into()),
        );
        return;
    }

    let scan = discover::scan();
    let Some(account) = scan
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .cloned()
    else {
        automation_result(
            app,
            value,
            &day,
            "error",
            Some("找不到策略配置中的微信账号。".into()),
        );
        return;
    };
    let store = keys::load(&keys::store_path(&data));
    let sync_state = app.state::<SyncState>();
    if !begin_sync(&sync_state, &account_id) {
        automation_result(
            app,
            value,
            &day,
            "busy",
            Some("该账号正在执行另一项同步任务。".into()),
        );
        return;
    }
    let data_for_sync = data.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        decrypt::decrypt_account(&data_for_sync, &account, &store, true, true)
    })
    .await;
    end_sync(&sync_state, &account_id);
    let outcomes = match joined {
        Ok(outcomes) => outcomes,
        Err(error) => {
            automation_result(
                app,
                value,
                &day,
                "error",
                Some(format!("同步任务失败：{error}")),
            );
            return;
        }
    };
    if !outcomes
        .iter()
        .any(|outcome| outcome.status == "ok" || outcome.status == "cached")
    {
        automation_result(
            app,
            value,
            &day,
            "error",
            Some("没有可用的解密缓存，日报未生成。".into()),
        );
        return;
    }

    let cache = decrypt::cache_dir(&data, &account_id);
    let allowed_set: HashSet<String> = allowed.into_iter().collect();
    let allowed_list: Vec<String> = allowed_set.iter().cloned().collect();
    let start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|local| local.and_local_timezone(Local).single())
        .map(|local| local.timestamp());
    let digest = match query::digest_filtered(
        &cache,
        &allowed_list,
        start,
        Some(now.timestamp()),
        Some(&allowed_set),
    ) {
        Ok(digest) => digest,
        Err(error) => {
            automation_result(
                app,
                value,
                &day,
                "error",
                Some(format!("日报整理失败：{error}")),
            );
            return;
        }
    };
    if digest.messages == 0 {
        automation_result(app, value, &day, "no_messages", None);
        return;
    }

    let mut body = digest.markdown.clone();
    let mut summary_error = None;
    if value.agent.use_for_daily && !value.agent.provider_id.trim().is_empty() {
        let agent_settings = value.agent.clone();
        let agent_data = data.clone();
        let material = query::summary_input(
            &cache,
            &allowed_list,
            start,
            Some(now.timestamp()),
            Some(&allowed_set),
        );
        match material {
            Ok(input) => {
                let sessions = input.sessions;
                let messages = input.messages;
                let truncated = input.truncated;
                let joined = tauri::async_runtime::spawn_blocking(move || {
                    agent::summarize(&agent_settings, &input, &agent_data)
                })
                .await;
                match joined {
                    Ok(Ok(run)) => {
                        body = format!(
                            "{}\n\n---\n由 {} 生成 · {} 个会话 / {} 条消息{}",
                            run.output,
                            agent::agent_name(&value.agent.provider_id),
                            sessions,
                            messages,
                            if truncated {
                                "（已按上限截断）"
                            } else {
                                ""
                            }
                        );
                    }
                    Ok(Err(error)) => {
                        summary_error = Some(format!("Agent 总结失败，已回退规则整理：{error}"));
                    }
                    Err(error) => {
                        summary_error =
                            Some(format!("Agent 总结任务失败，已回退规则整理：{error}"));
                    }
                }
            }
            Err(error) => {
                summary_error = Some(format!("Agent 总结输入准备失败，已回退规则整理：{error}"))
            }
        }
    }

    // Keep a copy of what was actually mailed. The record is written only after the send
    // succeeded, so history never claims a report that did not go out.
    let record_body = body.clone();
    let record_messages = digest.messages;
    if let Err(error) = send_mail(&value.devify.recipient, &value.devify.subject, body) {
        automation_result(app, value, &day, "error", Some(error));
        return;
    }
    let record = history::NewRecord {
        kind: "daily".into(),
        account_id: account_id.clone(),
        range_label: format!("自动日报 · {day}"),
        messages: record_messages,
        characters: record_body.chars().count(),
        output: Some(record_body),
        note: summary_error.clone(),
        ..Default::default()
    };
    if let Err(error) = history::add(&data, record) {
        eprintln!("保存自动日报记录失败：{error}");
    }
    automation_result(app, value, &day, "sent", summary_error);
}

async fn sync_automation_tick(app: &AppHandle) {
    let data = app_data(app);
    let Ok(value) = settings::load(&data) else {
        return;
    };
    if !value.automation.enabled || value.active_account_id.is_empty() {
        return;
    }
    let account_id = value.active_account_id;
    let scan = discover::scan();
    let Some(account) = scan
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .cloned()
    else {
        return;
    };
    let sync_state = app.state::<SyncState>();
    if !begin_sync(&sync_state, &account_id) {
        return;
    }
    let store = keys::load(&keys::store_path(&data));
    let data_for_sync = data.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        decrypt::decrypt_account(&data_for_sync, &account, &store, true, true)
    })
    .await;
    end_sync(&sync_state, &account_id);
}

#[tauri::command]
fn app_settings_get(app: AppHandle) -> Result<settings::AppSettings, String> {
    settings::load(&app_data(&app))
}

#[tauri::command]
fn app_settings_save(
    app: AppHandle,
    mut value: settings::AppSettings,
) -> Result<settings::AppSettings, String> {
    // The queue is managed by upload commands, not by potentially stale settings forms.
    value.upload_queue = settings::load(&app_data(&app))?.upload_queue;
    settings::save(&app_data(&app), &value)?;
    Ok(value)
}

#[derive(Serialize)]
struct KeyStatus {
    path: String,
    stored: usize,
    keys: Vec<StoredKeySummary>,
}

#[derive(Serialize)]
struct StoredKeySummary {
    account: String,
    rel: String,
    salt: String,
    source: String,
    verified_at: String,
    verified_secs: Option<u64>,
}

#[tauri::command]
async fn wechat_key_status(app: AppHandle) -> KeyStatus {
    let path = keys::store_path(&app_data(&app));
    let store = keys::load(&path);
    let mut keys: Vec<StoredKeySummary> = store
        .keys
        .iter()
        .map(|(id, entry)| {
            let (account, rel) = id.split_once("::").unwrap_or(("", id.as_str()));
            StoredKeySummary {
                account: account.to_string(),
                rel: rel.to_string(),
                salt: entry.salt.clone(),
                source: entry.source.clone(),
                verified_at: entry.verified_at.clone(),
                verified_secs: keys::verified_secs(entry),
            }
        })
        .collect();
    keys.sort_by(|a, b| a.account.cmp(&b.account).then(a.rel.cmp(&b.rel)));
    KeyStatus {
        path: path.to_string_lossy().to_string(),
        stored: keys.len(),
        keys,
    }
}

#[derive(Serialize)]
struct AccountKeyMatch {
    account: String,
    matched: usize,
    already: usize,
    total: usize,
    selected: bool,
}

#[derive(Serialize)]
struct KeyImportResult {
    matched: usize,
    already: usize,
    unmatched: Vec<String>,
    accounts: Vec<AccountKeyMatch>,
    message: String,
}

#[derive(Serialize)]
struct SavedKeySource {
    path: String,
    exists: bool,
    modified_at: Option<u64>,
    age_seconds: Option<u64>,
    mode: Option<String>,
    secure: bool,
    extractor: Option<String>,
    /// How the extractor was located, so the UI can explain the discovery instead of
    /// telling the user to set an environment variable.
    extractor_source: Option<String>,
    extractor_installed: bool,
    install_target: String,
    /// A bundle that exists somewhere else and can be copied into `/Applications`.
    install_source: Option<String>,
}

fn saved_key_path() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".wcdb-key-tool")
        .join("wechat-passphrase.json")
}

const EXTRACTOR_BUNDLE: &str = "WeDataKeyExtractor.app";

fn is_app_bundle(path: &Path) -> bool {
    path.extension()
        .map(|ext| ext.eq_ignore_ascii_case("app"))
        .unwrap_or(false)
        && path.is_dir()
        && path.join("Contents").join("Info.plist").is_file()
}

/// Accepts either a `.app` bundle or a directory that contains one (the reference project's
/// `dist` output), so both "pick the app" and "pick the build folder" work.
fn resolve_extractor(path: &Path) -> Option<PathBuf> {
    if is_app_bundle(path) {
        return Some(path.to_path_buf());
    }
    if !path.is_dir() {
        return None;
    }
    let mut bundles: Vec<PathBuf> = fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|candidate| is_app_bundle(candidate))
        .collect();
    if let Some(named) = bundles.iter().find(|candidate| {
        candidate
            .file_name()
            .map(|name| name == EXTRACTOR_BUNDLE)
            .unwrap_or(false)
    }) {
        return Some(named.clone());
    }
    bundles.sort();
    bundles.pop()
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

fn reference_dist_dir() -> Option<PathBuf> {
    home_dir().map(|home| {
        home.join("Workspace")
            .join("AI")
            .join("WeChatDataAnalysis")
            .join("macos-key-extractor")
            .join("dist")
    })
}

/// Ordered search: the bundle the user picked in the app, then `WECHAT_KEY_EXTRACTOR` (kept
/// for development), then the per-user Applications folder, the reference build output, and
/// finally the system Applications folder.
fn extractor_candidates(preferred: Option<&str>) -> Vec<(PathBuf, &'static str)> {
    let mut candidates: Vec<(PathBuf, &'static str)> = Vec::new();
    if let Some(value) = preferred.map(str::trim).filter(|value| !value.is_empty()) {
        candidates.push((PathBuf::from(value), "已选择"));
    }
    if let Some(value) = env::var_os("WECHAT_KEY_EXTRACTOR") {
        candidates.push((PathBuf::from(value), "环境变量"));
    }
    if let Some(home) = home_dir() {
        candidates.push((
            home.join("Applications").join(EXTRACTOR_BUNDLE),
            "用户应用程序",
        ));
    }
    if let Some(dist) = reference_dist_dir() {
        candidates.push((dist, "参考项目"));
    }
    candidates.push((
        PathBuf::from("/Applications").join(EXTRACTOR_BUNDLE),
        "应用程序",
    ));

    candidates
}

fn find_key_extractor(preferred: Option<&str>) -> Option<(PathBuf, &'static str)> {
    extractor_candidates(preferred)
        .into_iter()
        .find_map(|(candidate, source)| {
            resolve_extractor(&candidate).map(|bundle| (bundle, source))
        })
}

fn extractor_is_installed(bundle: &Path) -> bool {
    if bundle.starts_with(Path::new("/Applications")) {
        return true;
    }
    home_dir()
        .map(|home| bundle.starts_with(home.join("Applications")))
        .unwrap_or(false)
}

/// A bundle that is not installed yet but could be copied. Only the user-picked path and the
/// reference build output are considered: the app never copies bundles from arbitrary places
/// it discovered by itself without the user pointing at them first.
fn extractor_install_source(preferred: Option<&str>) -> Option<PathBuf> {
    let mut sources: Vec<PathBuf> = Vec::new();
    if let Some(value) = preferred.map(str::trim).filter(|value| !value.is_empty()) {
        sources.push(PathBuf::from(value));
    }
    if let Some(dist) = reference_dist_dir() {
        sources.push(dist);
    }
    sources
        .into_iter()
        .filter_map(|candidate| resolve_extractor(&candidate))
        .find(|bundle| !extractor_is_installed(bundle))
}

fn saved_key_source(app: &AppHandle) -> SavedKeySource {
    let path = saved_key_path();
    let metadata = fs::metadata(&path).ok();
    let exists = metadata.is_some();
    let modified_at = metadata
        .as_ref()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_secs());
    let age_seconds = modified_at.and_then(|time| now_secs().map(|now| now.saturating_sub(time)));
    let mode = metadata.as_ref().and_then(file_mode);
    let secure = metadata.as_ref().map(file_is_private).unwrap_or(false);
    let preferred = settings::load(&app_data(app))
        .ok()
        .map(|value| value.keys.extractor_path)
        .unwrap_or_default();
    let found = find_key_extractor(Some(&preferred));
    SavedKeySource {
        path: path.to_string_lossy().to_string(),
        exists,
        modified_at,
        age_seconds,
        mode,
        secure,
        extractor_source: found.as_ref().map(|(_, source)| source.to_string()),
        extractor_installed: found
            .as_ref()
            .map(|(bundle, _)| extractor_is_installed(bundle))
            .unwrap_or(false),
        install_target: PathBuf::from("/Applications")
            .join(EXTRACTOR_BUNDLE)
            .to_string_lossy()
            .to_string(),
        install_source: extractor_install_source(Some(&preferred))
            .map(|bundle| bundle.to_string_lossy().to_string()),
        extractor: found.map(|(bundle, _)| bundle.to_string_lossy().to_string()),
    }
}

fn now_secs() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_secs())
}

#[tauri::command]
fn wechat_key_source_status(app: AppHandle) -> SavedKeySource {
    saved_key_source(&app)
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    Some(format!("{:o}", metadata.permissions().mode() & 0o777))
}

#[cfg(not(unix))]
fn file_mode(_metadata: &fs::Metadata) -> Option<String> {
    None
}

#[cfg(unix)]
fn file_is_private(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o077 == 0
}

#[cfg(not(unix))]
fn file_is_private(_metadata: &fs::Metadata) -> bool {
    true
}

/// Launch the reference extractor bundle. The app never re-signs or attaches to WeChat
/// itself; it only opens the external tool that documents and performs that flow.
#[tauri::command]
async fn wechat_open_key_extractor(app: AppHandle) -> Result<String, String> {
    let preferred = settings::load(&app_data(&app))
        .map(|value| value.keys.extractor_path)
        .unwrap_or_default();
    let (bundle, _) = find_key_extractor(Some(&preferred)).ok_or_else(|| {
        "还没有找到 WeDataKeyExtractor.app。先点「安装到「应用程序」」或「选择提取器」把它接入，再打开。"
            .to_string()
    })?;
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("open")
            .arg(&bundle)
            .status()
            .map_err(|error| format!("无法启动提取器：{error}"))?;
        if !status.success() {
            return Err("密钥提取器没有成功启动，请在「应用程序」中手动打开。".into());
        }
        Ok(bundle.to_string_lossy().to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = bundle;
        Err("密钥提取器目前仅支持 macOS。其他平台请导入可信工具导出的密钥 JSON。".into())
    }
}

/// Let the user point at an existing bundle (or the reference project's `dist` folder) with
/// the native macOS chooser, then remember it. Returns `None` when the chooser was cancelled.
#[tauri::command]
async fn wechat_pick_key_extractor(app: AppHandle) -> Result<Option<SavedKeySource>, String> {
    #[cfg(target_os = "macos")]
    {
        let script = r#"POSIX path of (choose application with prompt "选择 WeDataKeyExtractor.app（可用「浏览…」定位到它）")"#;
        let output = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|error| format!("无法打开选择窗口：{error}"))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            if detail.contains("-128") {
                return Ok(None);
            }
            return Err(format!("没有选择提取器：{}", detail.trim()));
        }
        let chosen = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let bundle = resolve_extractor(Path::new(&chosen)).ok_or_else(|| {
            "选择的不是可用的 .app 应用包，请选择 WeDataKeyExtractor.app。".to_string()
        })?;
        let mut current = settings::load(&app_data(&app))?;
        current.keys.extractor_path = bundle.to_string_lossy().to_string();
        settings::save(&app_data(&app), &current)?;
        Ok(Some(saved_key_source(&app)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("密钥提取器目前仅支持 macOS。".into())
    }
}

#[derive(Serialize)]
struct ExtractorInstall {
    path: String,
    /// True when macOS had to ask for an administrator password (the password is typed into
    /// the system dialog and never reaches this process).
    used_admin: bool,
    message: String,
}

fn shell_single_quote(value: &Path) -> String {
    let text = value.to_string_lossy();
    format!("'{}'", text.replace('\'', r#"'\''"#))
}

fn copy_bundle(source: &Path, target: &Path) -> Result<(), String> {
    if target.exists() {
        fs::remove_dir_all(target).map_err(|error| format!("无法替换旧版本：{error}"))?;
    }
    let status = Command::new("ditto")
        .arg(source)
        .arg(target)
        .status()
        .map_err(|error| format!("无法复制提取器：{error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("复制被系统拒绝（没有写入「应用程序」的权限）。".into())
    }
}

/// Install the bundle into `/Applications`. The reference tool documents being placed there;
/// when the copy needs administrator rights, macOS shows its own password dialog.
#[tauri::command]
async fn wechat_install_key_extractor(app: AppHandle) -> Result<ExtractorInstall, String> {
    let preferred = settings::load(&app_data(&app))
        .map(|value| value.keys.extractor_path)
        .unwrap_or_default();
    let source = extractor_install_source(Some(&preferred)).ok_or_else(|| {
        "没有可安装的提取器。先把 WeDataKeyExtractor.app 构建到参考项目的 dist 目录，或用「选择提取器」指定它。"
            .to_string()
    })?;
    let target = PathBuf::from("/Applications").join(EXTRACTOR_BUNDLE);
    #[cfg(target_os = "macos")]
    {
        if let Ok(()) = copy_bundle(&source, &target) {
            let mut current = settings::load(&app_data(&app))?;
            current.keys.extractor_path = target.to_string_lossy().to_string();
            settings::save(&app_data(&app), &current)?;
            return Ok(ExtractorInstall {
                path: target.to_string_lossy().to_string(),
                used_admin: false,
                message: format!(
                    "已安装到 {}。接下来需要在完全磁盘访问权限里启用它。",
                    target.to_string_lossy()
                ),
            });
        }

        // The bundle lives in a user-writable place but `/Applications` belongs to admin.
        // `do shell script … with administrator privileges` is the same mechanism the
        // reference extractor uses: the password goes to macOS, not to this app.
        let command = format!(
            "/bin/rm -rf {target} && /usr/bin/ditto {source} {target}",
            target = shell_single_quote(&target),
            source = shell_single_quote(&source)
        );
        let script = format!(
            "do shell script \"{}\" with administrator privileges",
            command.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let output = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .map_err(|error| format!("无法请求管理员授权：{error}"))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            if detail.contains("-128") {
                return Err("已取消管理员授权，提取器没有安装。".into());
            }
            return Err(format!("安装失败：{}", detail.trim()));
        }
        if !is_app_bundle(&target) {
            return Err("安装命令已执行，但没有在「应用程序」中找到可用的提取器。".into());
        }
        let mut current = settings::load(&app_data(&app))?;
        current.keys.extractor_path = target.to_string_lossy().to_string();
        settings::save(&app_data(&app), &current)?;
        Ok(ExtractorInstall {
            path: target.to_string_lossy().to_string(),
            used_admin: true,
            message: format!(
                "已通过管理员授权安装到 {}。接下来需要在完全磁盘访问权限里启用它。",
                target.to_string_lossy()
            ),
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (source, target);
        Err("密钥提取器目前仅支持 macOS。".into())
    }
}

/// Shrink the extracted passphrase file to owner-only, which the guide asks for instead of
/// telling the user to run a `chmod` command.
#[tauri::command]
fn wechat_fix_key_permissions(app: AppHandle) -> Result<SavedKeySource, String> {
    let path = saved_key_path();
    if !path.is_file() {
        return Err("还没有提取器写出的口令文件，先完成一次提取。".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("无法收紧口令文件权限：{error}"))?;
        Ok(saved_key_source(&app))
    }
    #[cfg(not(unix))]
    {
        let _ = (app, path);
        Err("该平台不支持修改文件权限。".into())
    }
}

/// The extractor's own README asks for Full Disk Access; open the exact settings pane so the
/// user only has to flip the switch (the app never grants it on their behalf).
#[tauri::command]
fn wechat_open_full_disk_access() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
            .status()
            .map_err(|error| format!("无法打开系统设置：{error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err("系统设置没有打开，请手动进入「隐私与安全性 → 完全磁盘访问权限」。".into())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("完全磁盘访问权限是 macOS 的概念。".into())
    }
}

/* ── in-app passphrase capture (ported from the reference project) ─────────── */

fn capture_root(app: &AppHandle) -> PathBuf {
    app_data(app).join("key-capture")
}

fn capture_backup_root(app: &AppHandle) -> PathBuf {
    capture_root(app).join("wechat-app-backups")
}

fn read_db_page1(path: &str) -> Option<Vec<u8>> {
    let mut buffer = vec![0u8; crypto::PAGE_SIZE];
    let mut file = fs::File::open(path).ok()?;
    file.read_exact(&mut buffer).ok()?;
    Some(buffer)
}

/// Pick the message and session databases of one account, matching the reference's role
/// resolution. Both must be encrypted; a candidate has to unlock both.
fn account_capture_probes(account_id: &str) -> Result<Vec<capture::Probe>, String> {
    let scan = discover::scan();
    let account = scan
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .ok_or_else(|| format!("找不到账号 {account_id}，请先重新扫描账号。"))?;
    let encrypted = |rel: &str| account.dbs.iter().find(|db| db.rel == rel && db.encrypted);
    let combined = account
        .dbs
        .iter()
        .find(|db| db.encrypted && matches!(db.rel.as_str(), "MicroMsg.db" | "micromsg.db"));
    let session = account
        .dbs
        .iter()
        .find(|db| {
            db.encrypted
                && matches!(
                    db.rel.as_str(),
                    "session/session.db" | "session.db" | "Session.db"
                )
        })
        .or(combined);
    let message = account
        .dbs
        .iter()
        .find(|db| db.encrypted && db.rel.starts_with("message/") && db.rel.ends_with(".db"))
        .or_else(|| {
            account.dbs.iter().find(|db| {
                db.encrypted && db.rel.starts_with("message_") && db.rel.ends_with(".db")
            })
        })
        .or(combined);
    let _ = encrypted;

    let build = |role: &str, db: Option<&discover::DbFile>| -> Result<capture::Probe, String> {
        let db = db.ok_or_else(|| {
            format!("账号 {account_id} 缺少可用的 {role} 数据库，无法完成双库校验，已停止捕获。")
        })?;
        let page1 = read_db_page1(&db.path).ok_or_else(|| {
            format!(
                "无法读取账号 {account_id} 的 {role} 数据库首页（{db_path}）。",
                db_path = db.rel
            )
        })?;
        Ok(capture::Probe {
            role: role.to_string(),
            path: db.path.clone(),
            page1_hex: hex::encode(page1),
        })
    };
    Ok(vec![build("message", message)?, build("session", session)?])
}

/// `codesign -dvvv` is fast but not free, and the UI polls this, so keep it off the main thread.
#[tauri::command]
async fn wechat_capture_status(app: AppHandle) -> capture::CaptureStatus {
    let root = capture_root(&app);
    let backup_root = capture_backup_root(&app);
    tauri::async_runtime::spawn_blocking(move || {
        capture::status(Path::new(capture::WECHAT_APP), &root, &backup_root)
    })
    .await
    .unwrap_or_else(|_| {
        capture::status(
            Path::new(capture::WECHAT_APP),
            &capture_root(&app),
            &capture_backup_root(&app),
        )
    })
}

/// Verify, back up, clone, temporarily re-sign and swap in the debug build. The user then
/// opens WeChat themselves; nothing is launched or authorized on their behalf here.
#[tauri::command]
async fn wechat_capture_prepare(
    app: AppHandle,
) -> Result<capture::PrepareResult, capture::CaptureError> {
    let root = capture_root(&app);
    let backup_root = capture_backup_root(&app);
    tauri::async_runtime::spawn_blocking(move || {
        capture::prepare(Path::new(capture::WECHAT_APP), &root, &backup_root, true)
    })
    .await
    .map_err(|error| {
        capture::CaptureError::new("command_failed", format!("准备任务异常结束：{error}"))
    })?
}

/// Open the already-signed temporary WeChat. The user decides whether to accept any macOS
/// security prompt; the app never bypasses one. Returns a diagnosis when nothing starts, because
/// macOS 15.1 can reject the launch without showing a dialog.
#[tauri::command]
fn wechat_capture_open() -> Result<capture::LaunchAttempt, String> {
    #[cfg(target_os = "macos")]
    {
        Ok(capture::open_wechat_for_user(Path::new(
            capture::WECHAT_APP,
        )))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("应用内捕获目前仅支持 macOS。".into())
    }
}

/// Gatekeeper's "Open Anyway" lives in the Privacy & Security pane; the app only navigates there.
#[tauri::command]
fn wechat_open_privacy_security_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("/usr/bin/open")
            .arg("x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension")
            .status()
            .map_err(|error| format!("无法打开系统设置：{error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err("系统设置没有打开，请手动进入「隐私与安全性 → 安全性」。".into())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("该入口仅适用于 macOS。".into())
    }
}

#[tauri::command]
fn wechat_capture_confirm_launch(
    app: AppHandle,
    transaction_id: String,
) -> Result<capture::LaunchCheck, capture::CaptureError> {
    capture::confirm_launch(
        Path::new(capture::WECHAT_APP),
        &capture_root(&app),
        &transaction_id,
    )
}

/// Attach briefly (administrator prompt) and prove the PBKDF breakpoint resolves *before* the
/// user logs out.
#[tauri::command]
async fn wechat_capture_preflight(
    app: AppHandle,
) -> Result<capture::PreflightResult, capture::CaptureError> {
    let root = capture_root(&app);
    tauri::async_runtime::spawn_blocking(move || {
        capture::preflight(Path::new(capture::WECHAT_APP), &root)
    })
    .await
    .map_err(|error| {
        capture::CaptureError::new("command_failed", format!("预检任务异常结束：{error}"))
    })?
}

#[derive(Serialize)]
struct CaptureImport {
    matched: usize,
    already: usize,
    accounts: Vec<AccountKeyMatch>,
    message: String,
    key_mode: String,
    saved_path: String,
    restored: bool,
    validated_roles: Vec<String>,
}

/// Attach LLDB as administrator, capture on the next login, validate the candidate against the
/// message **and** session databases, restore the official bundle, then import the saved
/// passphrase through the existing per-database verification.
#[tauri::command]
async fn wechat_capture_start(
    app: AppHandle,
    account_id: String,
    timeout_secs: Option<u64>,
) -> Result<CaptureImport, capture::CaptureError> {
    let probes = account_capture_probes(&account_id)
        .map_err(|message| capture::CaptureError::new("account_probe_invalid", message))?;
    let root = capture_root(&app);
    let timeout = timeout_secs.unwrap_or(240).clamp(60, 900);
    let outcome = {
        let root = root.clone();
        tauri::async_runtime::spawn_blocking(move || {
            capture::capture(Path::new(capture::WECHAT_APP), &root, &probes, timeout)
        })
        .await
        .map_err(|error| {
            capture::CaptureError::new("command_failed", format!("捕获任务异常结束：{error}"))
        })?
    }?;

    let imported = import_key_text(&app, &outcome.passphrase_hex, Some(&account_id))
        .map_err(|message| capture::CaptureError::new("key_import_failed", message))?;
    Ok(CaptureImport {
        matched: imported.matched,
        already: imported.already,
        accounts: imported.accounts,
        message: imported.message,
        key_mode: outcome.key_mode,
        saved_path: outcome.saved_path,
        restored: outcome.restored,
        validated_roles: outcome.validated_roles,
    })
}

/// Cancel an in-flight capture and restore the Tencent-signed bundle.
#[tauri::command]
async fn wechat_capture_restore(
    app: AppHandle,
) -> Result<capture::RestoreResult, capture::CaptureError> {
    let root = capture_root(&app);
    tauri::async_runtime::spawn_blocking(move || {
        capture::cancel(Path::new(capture::WECHAT_APP), &root)
    })
    .await
    .map_err(|error| {
        capture::CaptureError::new("command_failed", format!("恢复任务异常结束：{error}"))
    })?
}

/// Delete one account's decrypted snapshot. Returns how many databases it contained.
fn clear_account_cache(data: &Path, account: &str) -> Result<usize, String> {
    let dir = decrypt::cache_dir(data, account);
    if !dir.exists() {
        return Ok(0);
    }
    let count = count_cached_dbs(&dir);
    std::fs::remove_dir_all(&dir).map_err(|error| format!("清除解密缓存失败：{error}"))?;
    Ok(count)
}

fn clear_all_caches(data: &Path) -> Result<usize, String> {
    let root = data.join("wechat-cache");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Ok(0);
    };
    let mut count = 0;
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let cached = count_cached_dbs(&entry.path());
        std::fs::remove_dir_all(entry.path())
            .map_err(|error| format!("清除解密缓存失败：{error}"))?;
        count += cached;
    }
    Ok(count)
}

/// Number of decrypted databases in a cache directory (the mirror is nested, so walk it) minus
/// the sync metadata bookkeeping file.
fn count_cached_dbs(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                count_cached_dbs(&path)
            } else if entry.file_name() == "_sync.json" {
                0
            } else {
                1
            }
        })
        .sum()
}

#[tauri::command]
async fn wechat_clear_keys(app: AppHandle, account_id: Option<String>) -> Result<String, String> {
    let data = app_data(&app);
    let path = keys::store_path(&data);
    let mut store = keys::load(&path);
    let target = account_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let removed = match target {
        Some(id) => keys::remove_account(&mut store, id),
        None => keys::remove_all(&mut store),
    };
    if removed > 0 {
        keys::save(&path, &store)?;
    }
    // Keys alone do not lock the data back up: the decrypted snapshot sits in the cache and would
    // keep being served. Drop it together with the keys so the next attempt has to prove a key.
    let cached = match target {
        Some(id) => clear_account_cache(&data, id)?,
        None => clear_all_caches(&data)?,
    };
    let mut parts: Vec<String> = Vec::new();
    if removed > 0 {
        parts.push(format!("已清除 {removed} 个已保存密钥"));
    }
    if cached > 0 {
        parts.push(format!("已删除 {cached} 份解密缓存"));
    }
    Ok(if parts.is_empty() {
        "没有可清除的已保存密钥或解密缓存。".to_string()
    } else {
        format!(
            "{}。微信原始数据库不受影响；重新查看需要重新捕获或导入口令。",
            parts.join("，")
        )
    })
}

fn read_page1(path: &Path) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; crypto::PAGE_SIZE];
    File::open(path).ok()?.read_exact(&mut buf).ok()?;
    Some(buf)
}

struct ImportStats {
    matched: usize,
    already: usize,
    accounts: Vec<AccountKeyMatch>,
    unmatched: Vec<String>,
}

fn apply_candidates(
    store: &mut KeyStore,
    scan: &ScanResult,
    candidates: &[keys::Candidate],
    selected: Option<&str>,
) -> ImportStats {
    let mut matched = 0usize;
    let mut already = 0usize;
    let mut accounts: Vec<AccountKeyMatch> = Vec::new();
    let mut unmatched: Vec<String> = Vec::new();

    for account in &scan.accounts {
        let encrypted: Vec<&discover::DbFile> =
            account.dbs.iter().filter(|db| db.encrypted).collect();
        if encrypted.is_empty() {
            continue;
        }
        let total = encrypted.len();
        let mut account_matched = 0usize;
        let mut account_already = 0usize;
        let mut account_failed: Vec<String> = Vec::new();

        for db in encrypted {
            let Some(page1) = read_page1(Path::new(&db.path)) else {
                account_failed.push(format!("{}::{}（无法读取）", account.id, db.rel));
                continue;
            };
            // A key stored under the current salt is cheap to re-verify (page-1 HMAC only).
            // Re-checking means a rotated key is replaced instead of being trusted forever.
            if let Some(existing) = keys::lookup(store, &account.id, &db.rel) {
                if Some(&existing.salt) == db.salt.as_ref() {
                    let still_valid = hex::decode(&existing.enc_key)
                        .map(|key| crypto::verify_key(&page1, &key))
                        .unwrap_or(false);
                    if still_valid {
                        account_already += 1;
                        continue;
                    }
                }
            }
            // Raw database keys are tried before passphrases so a passphrase never pays a
            // 256000-round PBKDF2 derivation when a direct key already matches.
            match keys::resolve_db_key(&page1, candidates) {
                Some((key, label)) => {
                    let salt = crypto::read_salt(&page1).unwrap_or_default();
                    let source = match keys::lookup(store, &account.id, &db.rel) {
                        Some(existing) => format!("{label}（替换 {}）", existing.source),
                        None => label,
                    };
                    keys::store_key(store, &account.id, &db.rel, &key, &salt, &source);
                    account_matched += 1;
                }
                None => account_failed.push(format!("{}::{}", account.id, db.rel)),
            }
        }

        matched += account_matched;
        already += account_already;
        if account_matched + account_already > 0 {
            unmatched.extend(account_failed);
            accounts.push(AccountKeyMatch {
                account: account.id.clone(),
                matched: account_matched,
                already: account_already,
                total,
                selected: selected == Some(account.id.as_str()),
            });
        }
    }

    ImportStats {
        matched,
        already,
        accounts,
        unmatched,
    }
}

/// Import key material (database keys or an account passphrase), verify it against every
/// discovered database with a page-1 HMAC check, and keep only what actually matches.
///
/// `selected` is the account currently open in the UI. It is only used to explain the
/// outcome: an import always verifies every account so a passphrase issued for another
/// WeChat account is stored where it belongs instead of silently doing nothing.
fn import_key_text(
    app: &AppHandle,
    text: &str,
    selected: Option<&str>,
) -> Result<KeyImportResult, String> {
    let candidates = keys::parse_candidates(text)?;
    let path = keys::store_path(&app_data(app));
    let mut store: KeyStore = keys::load(&path);
    let scan = discover::scan();
    let stats = apply_candidates(&mut store, &scan, &candidates, selected);

    if stats.matched > 0 {
        keys::save(&path, &store)?;
    }
    let message = import_summary(selected, stats.matched, &stats.accounts);
    Ok(KeyImportResult {
        matched: stats.matched,
        already: stats.already,
        unmatched: stats.unmatched,
        accounts: stats.accounts,
        message,
    })
}

fn import_summary(selected: Option<&str>, matched: usize, accounts: &[AccountKeyMatch]) -> String {
    let target = selected.unwrap_or("").trim();
    if accounts.is_empty() {
        return if target.is_empty() {
            "没有任何候选通过第 1 页 HMAC 校验。请确认：① 密钥或口令属于本机当前登录的微信号；② 口令是 64 位十六进制；③ 微信更新或重新登录后口令会变化，需要重新提取。".to_string()
        } else {
            format!(
                "导入内容没有通过账号 {target} 任何数据库的第 1 页校验。通常是口令属于另一个微信号、微信更新/重新登录后已轮换，或数据库正在写入。请重新运行提取器后再导入。"
            )
        };
    }

    let target_label = if target.is_empty() {
        "未选择"
    } else {
        target
    };
    // Report newly stored keys first: a passphrase for another account may be imported while
    // the selected account already has valid keys, and that must not read as a no-op.
    if let Some(entry) = accounts
        .iter()
        .find(|entry| entry.selected && entry.matched > 0)
    {
        let extra = if entry.already > 0 {
            format!("，另外 {} 个数据库的现有密钥仍然有效", entry.already)
        } else {
            String::new()
        };
        return format!(
            "账号 {} 的 {} 个数据库密钥已校验并保存{extra}。",
            entry.account, entry.matched
        );
    }

    let newly: Vec<&AccountKeyMatch> = accounts.iter().filter(|entry| entry.matched > 0).collect();
    if !newly.is_empty() {
        let owner = newly
            .iter()
            .map(|entry| entry.account.as_str())
            .collect::<Vec<_>>()
            .join("、");
        return format!(
            "该口令属于账号 {owner}（当前选择的是 {target_label}），已保存 {matched} 个数据库密钥。请切换到该账号继续。"
        );
    }

    if let Some(entry) = accounts.iter().find(|entry| entry.selected) {
        // Nothing new was stored. Say that explicitly instead of only praising the existing
        // keys: a wrong paste otherwise reads as "import succeeded".
        return format!(
            "导入内容没有为账号 {} 新增密钥，已保存的 {} 个密钥仍然有效，无需重复导入。如果这是刚重新提取的口令，说明它与本机数据库不匹配。",
            entry.account, entry.already
        );
    }

    let owner = accounts
        .iter()
        .map(|entry| entry.account.as_str())
        .collect::<Vec<_>>()
        .join("、");
    format!("现有密钥（账号 {owner}）仍然有效，无需重复导入。当前选择的是 {target_label}。")
}

#[tauri::command]
async fn wechat_import_keys(
    app: AppHandle,
    text: String,
    account_id: Option<String>,
) -> Result<KeyImportResult, String> {
    import_key_text(&app, &text, account_id.as_deref())
}

/// The reference key extractor writes this verified passphrase file. Read only this
/// well-known local path; the passphrase is never returned to the renderer or logs.
#[tauri::command]
async fn wechat_import_saved_key(
    app: AppHandle,
    account_id: Option<String>,
) -> Result<KeyImportResult, String> {
    let path = saved_key_path();
    let text = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "没有找到参考密钥提取器生成的文件：{}",
            path.to_string_lossy()
        )
    })?;
    import_key_text(&app, &text, account_id.as_deref())
}

#[tauri::command]
async fn wechat_scan() -> ScanResult {
    discover::scan()
}

#[tauri::command]
async fn wechat_decrypt(
    app: AppHandle,
    account_id: String,
    apply_wal: bool,
    only_chat: bool,
) -> Result<Vec<DecryptOutcome>, String> {
    let scan = discover::scan();
    let account = scan
        .accounts
        .iter()
        .find(|a| a.id == account_id)
        .cloned()
        .ok_or_else(|| format!("找不到账号 {account_id}"))?;
    let store = keys::load(&keys::store_path(&app_data(&app)));
    Ok(decrypt::decrypt_account(
        &app_data(&app),
        &account,
        &store,
        apply_wal,
        only_chat,
    ))
}

#[tauri::command]
async fn wechat_sync(
    app: AppHandle,
    state: State<'_, SyncState>,
    account_id: String,
) -> Result<Vec<DecryptOutcome>, String> {
    let scan = discover::scan();
    let account = scan
        .accounts
        .iter()
        .find(|a| a.id == account_id)
        .cloned()
        .ok_or_else(|| format!("找不到账号 {account_id}"))?;
    if !begin_sync(&state, &account_id) {
        return Err("该账号正在同步，请稍后重试。".into());
    }
    let store = keys::load(&keys::store_path(&app_data(&app)));
    let outcomes = decrypt::decrypt_account(&app_data(&app), &account, &store, true, true);
    end_sync(&state, &account_id);
    Ok(outcomes)
}

#[tauri::command]
async fn wechat_sessions(app: AppHandle, account_id: String) -> Result<Vec<SessionItem>, String> {
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    let allowed = allowed_usernames(&app, &account_id)?;
    query::sessions_filtered(&cache, Some(&allowed))
}

#[derive(Serialize)]
struct AvatarItem {
    username: String,
    data_url: String,
}

/// Avatars for the given usernames, as `data:` URLs. Missing names are simply absent, so the
/// caller keeps its letter avatars; nothing here is an error worth interrupting a chat for.
#[tauri::command]
async fn wechat_avatars(
    app: AppHandle,
    account_id: String,
    usernames: Vec<String>,
) -> Vec<AvatarItem> {
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    wechat::avatar::avatars(&cache, &usernames)
        .into_iter()
        .map(|avatar| AvatarItem {
            username: avatar.username,
            data_url: avatar.data_url,
        })
        .collect()
}

#[tauri::command]
async fn wechat_session_catalog(
    app: AppHandle,
    account_id: String,
) -> Result<Vec<SessionItem>, String> {
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    query::sessions(&cache)
}

#[tauri::command]
async fn wechat_messages(
    app: AppHandle,
    account_id: String,
    username: String,
    limit: Option<usize>,
    before: Option<i64>,
) -> Result<Vec<MessageItem>, String> {
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    if !query::is_chat_session(&cache, &username) {
        return Err("公众号、服务号和系统会话不在查看范围内。".into());
    }
    let allowed = allowed_usernames(&app, &account_id)?;
    if !allowed.contains(&username) {
        return Err("该会话不在当前策略白名单中。".into());
    }
    query::messages(&cache, &username, limit.unwrap_or(100), before)
}

/// Bounded keyword search over decrypted message content.
///
/// `max_sessions` caps how many conversations are scanned; the report tells the UI how
/// many were actually scanned and how many were skipped, so results are never presented
/// as exhaustive when they are not.
#[tauri::command]
async fn wechat_search(
    app: AppHandle,
    account_id: String,
    keyword: String,
    groups_only: Option<bool>,
    max_sessions: Option<usize>,
) -> Result<SearchReport, String> {
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    let allowed = allowed_usernames(&app, &account_id)?;
    query::search_filtered(
        &cache,
        &keyword,
        groups_only.unwrap_or(false),
        max_sessions.unwrap_or(200),
        Some(&allowed),
    )
}

#[tauri::command]
async fn wechat_digest(
    app: AppHandle,
    account_id: String,
    usernames: Vec<String>,
    since: Option<i64>,
    until: Option<i64>,
) -> Result<DigestReport, String> {
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    let allowed = allowed_usernames(&app, &account_id)?;
    if usernames.iter().any(|username| !allowed.contains(username)) {
        return Err("整理内容包含不在当前策略白名单中的会话。".into());
    }
    query::digest_filtered(&cache, &usernames, since, until, Some(&allowed))
}

#[tauri::command]
async fn wechat_clear_cache(app: AppHandle, account_id: String) -> Result<String, String> {
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    if cache.exists() {
        std::fs::remove_dir_all(&cache).map_err(|e| format!("清理缓存失败：{e}"))?;
    }
    Ok(format!("已清理 {}", cache.to_string_lossy()))
}

#[derive(Serialize)]
struct WechatEnv {
    app_data_dir: String,
    cache_root: String,
    keys_path: String,
    scanner_path: Option<String>,
}

#[tauri::command]
fn wechat_env(app: AppHandle) -> WechatEnv {
    let data = app_data(&app);
    let scanner = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("wechat-key-scan")))
        .filter(|path| path.exists())
        .map(|path| path.to_string_lossy().to_string());
    WechatEnv {
        cache_root: data.join("wechat-cache").to_string_lossy().to_string(),
        keys_path: keys::store_path(&data).to_string_lossy().to_string(),
        app_data_dir: data.to_string_lossy().to_string(),
        scanner_path: scanner,
    }
}

#[derive(Serialize)]
struct AgentCurrent {
    configured: bool,
    agent_id: String,
    agent_name: String,
    command: Option<String>,
    available: bool,
}

#[tauri::command]
async fn agent_current(app: AppHandle) -> Result<AgentCurrent, String> {
    let agent = settings::load(&app_data(&app))?.agent;
    let configured = !agent.provider_id.trim().is_empty();
    let path = if configured {
        agent::resolve(&agent)
    } else {
        None
    };
    Ok(AgentCurrent {
        configured,
        agent_id: agent.provider_id.clone(),
        agent_name: if configured {
            agent::agent_name(&agent.provider_id)
        } else {
            String::new()
        },
        command: path.as_ref().map(|path| path.to_string_lossy().to_string()),
        available: path.is_some(),
    })
}

#[tauri::command]
async fn agent_scan(app: AppHandle) -> Result<Vec<agent::AgentInfo>, String> {
    let agent = settings::load(&app_data(&app))?.agent;
    tauri::async_runtime::spawn_blocking(move || agent::scan(&agent.provider_id, &agent.command))
        .await
        .map_err(|error| format!("扫描本地 Agent 失败：{error}"))
}

/// Models the selected CLI reports as usable, so the UI can offer a list instead of
/// asking people to memorise provider/model strings.
#[tauri::command]
async fn agent_models(
    app: AppHandle,
    value: settings::AgentSettings,
) -> Result<Vec<String>, String> {
    let data = app_data(&app);
    tauri::async_runtime::spawn_blocking(move || agent::list_models(&value, &data))
        .await
        .map_err(|error| format!("读取模型列表失败：{error}"))?
}

#[tauri::command]
async fn summary_history_list(app: AppHandle) -> Result<Vec<history::SummaryRecord>, String> {
    history::list(&app_data(&app))
}

/// Persist a summary after it has been shown to the user. The backend owns the id and the
/// timestamp, and enforces the record cap, so the drawer cannot grow the file unbounded.
#[tauri::command]
async fn summary_history_add(
    app: AppHandle,
    record: history::NewRecord,
) -> Result<Vec<history::SummaryRecord>, String> {
    history::add(&app_data(&app), record)
}

#[tauri::command]
async fn summary_history_delete(
    app: AppHandle,
    id: String,
) -> Result<Vec<history::SummaryRecord>, String> {
    history::remove(&app_data(&app), &id)
}

#[tauri::command]
async fn agent_test(
    app: AppHandle,
    value: settings::AgentSettings,
) -> Result<agent::AgentRun, String> {
    let data = app_data(&app);
    tauri::async_runtime::spawn_blocking(move || agent::test(&value, &data))
        .await
        .map_err(|error| format!("测试 Agent 失败：{error}"))?
}

#[derive(Serialize)]
struct AgentSummary {
    agent_id: String,
    agent_name: String,
    model: String,
    output: String,
    sessions: usize,
    messages: usize,
    characters: usize,
    truncated: bool,
    elapsed_ms: u64,
}

/// Summarise the selected conversations with the configured local agent.
///
/// Chat content leaves the app only through that agent's own CLI. The whitelist is
/// re-checked here, not only in the UI, because the transcript contains message bodies.
/// `messages` carries exactly the preview the user reviewed; when it is absent the
/// transcript is rebuilt from the decrypted cache for the given range.
#[tauri::command]
async fn agent_summarize(
    app: AppHandle,
    account_id: String,
    usernames: Vec<String>,
    messages: Option<Vec<query::SummaryMessage>>,
    since: Option<i64>,
    until: Option<i64>,
) -> Result<AgentSummary, String> {
    let data = app_data(&app);
    let value = settings::load(&data)?;
    if value.agent.provider_id.trim().is_empty() {
        return Err("尚未在「Agent 总结」页面选择本地 Agent。".into());
    }
    let allowed = allowed_usernames(&app, &account_id)?;
    if usernames.iter().any(|username| !allowed.contains(username)) {
        return Err("总结内容包含不在当前策略白名单中的会话。".into());
    }
    let input = match messages {
        Some(list) if !list.is_empty() => {
            if list
                .iter()
                .any(|message| !allowed.contains(&message.username))
            {
                return Err("总结内容包含不在当前策略白名单中的会话。".into());
            }
            let input = query::summarize_messages(list);
            if input.messages == 0 {
                return Err("没有可总结的文字消息。".into());
            }
            input
        }
        _ => {
            let cache = decrypt::cache_dir(&data, &account_id);
            query::summary_input(&cache, &usernames, since, until, Some(&allowed))?
        }
    };
    let stats = (
        input.sessions,
        input.messages,
        input.characters,
        input.truncated,
    );
    let agent_settings = value.agent.clone();
    let run = tauri::async_runtime::spawn_blocking(move || {
        agent::summarize(&agent_settings, &input, &data)
    })
    .await
    .map_err(|error| format!("Agent 总结任务失败：{error}"))??;
    Ok(AgentSummary {
        agent_id: value.agent.provider_id.clone(),
        agent_name: agent::agent_name(&value.agent.provider_id),
        model: value.agent.model.clone(),
        output: run.output,
        sessions: stats.0,
        messages: stats.1,
        characters: stats.2,
        truncated: stats.3,
        elapsed_ms: run.elapsed_ms,
    })
}

pub fn run() {
    tauri::Builder::default()
        .manage(SyncState::default())
        .manage(UploadState::default())
        .manage(DevifyAuthState::default())
        .manage(McpState::default())
        .setup(|app| {
            #[cfg(desktop)]
            {
                app.handle().plugin(tauri_plugin_deep_link::init())?;
            }
            let mcp_handle = app.handle().clone();
            let mcp_status = app.state::<McpState>().status.clone();
            let mcp_bind = mcp::configured_bind();
            let mcp_token = mcp::configured_token();
            let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
            let mcp_data = app_data(&mcp_handle);
            let server_status = mcp_status.clone();
            tauri::async_runtime::spawn(async move {
                let result = mcp::start(mcp_data, mcp_bind, mcp_token, Some(ready_tx)).await;
                if let Ok(mut status) = server_status.lock() {
                    status.running = false;
                    if let Err(error) = result {
                        status.error = Some(error);
                    }
                }
            });
            tauri::async_runtime::spawn(async move {
                let result = ready_rx
                    .await
                    .unwrap_or_else(|_| Err("本机只读服务未能启动。".to_string()));
                if let Ok(mut status) = mcp_status.lock() {
                    match result {
                        Ok(()) => {
                            status.running = true;
                            status.error = None;
                        }
                        Err(error) => {
                            status.running = false;
                            status.error = Some(error);
                        }
                    }
                }
            });
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    automation_tick(&handle).await;
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
            });
            let sync_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    sync_automation_tick(&sync_handle).await;
                    let seconds = settings::load(&app_data(&sync_handle))
                        .map(|value| value.chat.sync_interval_seconds)
                        .unwrap_or(30)
                        .clamp(1, 3600);
                    tokio::time::sleep(Duration::from_secs(seconds)).await;
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            devify_auth_start,
            devify_auth_open,
            devify_auth_callback,
            devify_spaces,
            devify_analysis_list,
            devify_analysis_detail,
            devify_event_notes,
            devify_topics,
            devify_topic_create,
            devify_topic_update,
            devify_topic_delete,
            devify_questions,
            devify_question_ask,
            devify_space_takeover,
            wechat_collection_messages,
            devify_upload_preview,
            devify_upload_selected,
            devify_upload_retry_preview,
            devify_upload_retry,
            devify_upload_status,
            mail_config_status,
            app_settings_get,
            app_settings_save,
            wechat_scan,
            wechat_key_status,
            wechat_key_source_status,
            wechat_open_key_extractor,
            wechat_pick_key_extractor,
            wechat_install_key_extractor,
            wechat_fix_key_permissions,
            wechat_open_full_disk_access,
            wechat_capture_status,
            wechat_capture_prepare,
            wechat_capture_open,
            wechat_open_privacy_security_settings,
            wechat_capture_confirm_launch,
            wechat_capture_preflight,
            wechat_capture_start,
            wechat_capture_restore,
            wechat_clear_keys,
            wechat_import_keys,
            wechat_import_saved_key,
            wechat_decrypt,
            wechat_sync,
            wechat_sessions,
            wechat_avatars,
            wechat_session_catalog,
            wechat_messages,
            wechat_search,
            wechat_digest,
            wechat_clear_cache,
            wechat_env,
            app_info,
            perm_status,
            perm_open_settings,
            perm_reveal_app,
            mcp_status,
            agent_access_status,
            agent_access_prompt,
            agent_build_wxq,
            agent_current,
            agent_scan,
            agent_test,
            agent_models,
            agent_summarize,
            summary_history_list,
            summary_history_add,
            summary_history_delete
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}

#[cfg(test)]
mod key_flow_tests {
    use super::*;

    #[test]
    fn upload_preview_reflects_real_data_and_detects_changes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("session")).unwrap();
        std::fs::create_dir_all(dir.path().join("message")).unwrap();
        let session = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
        session.execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','测试会话');").unwrap();
        let message = rusqlite::Connection::open(dir.path().join("message/message_0.db")).unwrap();
        let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
        message.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); INSERT INTO {table} VALUES (1,100,'one',1),(2,200,'two',1),(3,300,'three',1);")).unwrap();
        let names = vec!["person".to_string()];
        let first = prepare_upload(
            dir.path(),
            "account",
            7,
            "https://example.test",
            &names,
            Some(100),
            Some(300),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(first.preview.message_count, 2);
        assert_eq!(first.preview.sessions[0].message_count, 2);
        let mut picks = BTreeMap::new();
        picks.insert(
            "person".into(),
            vec![settings::MessageRef {
                local_id: 2,
                time: 200,
            }],
        );
        let one = prepare_upload(
            dir.path(),
            "account",
            7,
            "https://example.test",
            &names,
            Some(100),
            Some(300),
            &picks,
        )
        .unwrap();
        assert_eq!(one.preview.message_count, 1);
        assert_eq!(one.parts[0]["text"], "two");
        assert_ne!(one.preview.fingerprint, first.preview.fingerprint);
        picks.insert(
            "person".into(),
            vec![settings::MessageRef {
                local_id: 3,
                time: 300,
            }],
        );
        assert!(prepare_upload(
            dir.path(),
            "account",
            7,
            "https://example.test",
            &names,
            Some(100),
            Some(300),
            &picks
        )
        .is_err());
        let unchanged = prepare_upload(
            dir.path(),
            "account",
            7,
            "https://example.test",
            &names,
            Some(100),
            Some(300),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(first.preview.fingerprint, unchanged.preview.fingerprint);
        message
            .execute(
                &format!("UPDATE {table} SET message_content='edited' WHERE local_id=1"),
                [],
            )
            .unwrap();
        let edited = prepare_upload(
            dir.path(),
            "account",
            7,
            "https://example.test",
            &names,
            Some(100),
            Some(300),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_ne!(first.preview.fingerprint, edited.preview.fingerprint);
        assert!(verify_upload_preview(
            Some(&first.preview.fingerprint),
            &unchanged.preview.fingerprint
        )
        .is_ok());
        assert!(verify_upload_preview(
            Some(&first.preview.fingerprint),
            &edited.preview.fingerprint
        )
        .is_err());
    }

    #[test]
    fn upload_preview_rejects_truncated_same_second_boundary() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("session")).unwrap();
        std::fs::create_dir_all(dir.path().join("message")).unwrap();
        let session = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
        session
            .execute_batch(
                "CREATE TABLE Session (username TEXT); INSERT INTO Session VALUES ('person');",
            )
            .unwrap();
        let message = rusqlite::Connection::open(dir.path().join("message/message_0.db")).unwrap();
        let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
        message.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<501) INSERT INTO {table} SELECT x,100,'text',1 FROM n;")).unwrap();
        let result = prepare_upload(
            dir.path(),
            "account",
            7,
            "https://example.test",
            &["person".into()],
            None,
            None,
            &BTreeMap::new(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn upload_scope_uses_selected_accounts_own_allowlist() {
        let mut settings = settings::AppSettings::default();
        settings.active_account_id = "account-a".into();
        settings.devify.active_space_id = Some(7);
        settings
            .chat
            .allowed_usernames
            .insert("account-a".into(), vec!["chat-a".into()]);
        settings
            .chat
            .allowed_usernames
            .insert("account-b".into(), vec!["chat-b".into()]);
        assert!(validate_upload_scope(&settings, "account-b", 7, &["chat-b".into()]).is_ok());
        assert!(validate_upload_scope(&settings, "account-b", 7, &["chat-a".into()]).is_err());
        assert!(validate_upload_scope(&settings, "", 7, &["chat-b".into()]).is_err());
        assert!(validate_upload_scope(&settings, "account-b", 8, &["chat-b".into()]).is_err());
        settings
            .chat
            .allowed_usernames
            .insert("../other".into(), vec!["chat-b".into()]);
        assert!(validate_upload_scope(&settings, "../other", 7, &["chat-b".into()]).is_err());
    }

    #[test]
    fn individual_selection_is_exact_and_rejects_missing_or_ambiguous_messages() {
        let message = |local_id, time, text: &str| MessageItem {
            local_id,
            time,
            text: text.into(),
            sender: None,
            sender_name: None,
            kind: 1,
            is_self: false,
        };
        let all = vec![message(1, 100, "a"), message(2, 200, "b")];
        let refs = vec![settings::MessageRef {
            local_id: 2,
            time: 200,
        }];
        let selected = filter_upload_messages(all.clone(), Some(&refs)).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].text, "b");
        assert!(filter_upload_messages(
            all.clone(),
            Some(&[settings::MessageRef {
                local_id: 3,
                time: 300
            }])
        )
        .is_err());
        assert!(filter_upload_messages(
            vec![all[0].clone(), all[0].clone()],
            Some(&[settings::MessageRef {
                local_id: 1,
                time: 100
            }])
        )
        .is_err());
        assert!(filter_upload_messages(all, Some(&[])).is_err());
    }

    #[test]
    fn upload_fingerprint_changes_with_content_and_scope() {
        let a = serde_json::json!({"account":"a", "messages":[{"id":1,"text":"hello"}]});
        let b = serde_json::json!({"account":"a", "messages":[{"id":1,"text":"changed"}]});
        let c = serde_json::json!({"account":"b", "messages":[{"id":1,"text":"hello"}]});
        assert_eq!(upload_fingerprint(&a), upload_fingerprint(&a));
        assert_ne!(upload_fingerprint(&a), upload_fingerprint(&b));
        assert_ne!(upload_fingerprint(&a), upload_fingerprint(&c));
    }

    #[test]
    fn same_upload_cannot_be_retried_concurrently() {
        let state = UploadState::default();
        let first = begin_retry(&state, "job-1").unwrap();
        assert!(begin_retry(&state, "job-1").is_err());
        assert!(begin_retry(&state, "job-2").is_ok());
        drop(first);
        assert!(begin_retry(&state, "job-1").is_ok());
    }

    fn matched_account(
        account: &str,
        matched: usize,
        already: usize,
        selected: bool,
    ) -> AccountKeyMatch {
        AccountKeyMatch {
            account: account.to_string(),
            matched,
            already,
            total: matched + already,
            selected,
        }
    }

    #[test]
    fn summary_names_the_selected_account() {
        let message = import_summary(
            Some("acct-a"),
            20,
            &[matched_account("acct-a", 18, 2, true)],
        );
        assert!(message.contains("acct-a"));
        assert!(message.contains("18 个数据库密钥已校验并保存"));
        assert!(message.contains("2 个数据库的现有密钥仍然有效"));
    }

    #[test]
    fn summary_explains_an_account_mismatch() {
        let message = import_summary(
            Some("acct-a"),
            20,
            &[matched_account("acct-b", 20, 0, false)],
        );
        assert!(message.contains("属于账号 acct-b"));
        assert!(message.contains("当前选择的是 acct-a"));
    }

    #[test]
    fn summary_reports_new_keys_even_when_the_selected_account_is_already_valid() {
        let message = import_summary(
            Some("acct-a"),
            20,
            &[
                matched_account("acct-a", 0, 20, true),
                matched_account("acct-b", 20, 0, false),
            ],
        );
        assert!(message.contains("属于账号 acct-b"));
        assert!(!message.contains("无需重复导入"));
    }

    #[test]
    fn summary_reports_already_stored_keys_without_an_error_tone() {
        let message = import_summary(Some("acct-a"), 0, &[matched_account("acct-a", 0, 20, true)]);
        assert!(message.contains("无需重复导入"));
        assert!(!message.contains("没有通过"));
        // A wrong paste must not read as a silent success.
        assert!(message.contains("没有为账号 acct-a 新增密钥"));
        assert!(message.contains("与本机数据库不匹配"));
    }

    #[test]
    fn summary_rejects_when_nothing_matches() {
        let message = import_summary(Some("acct-a"), 0, &[]);
        assert!(message.contains("没有通过账号 acct-a"));
        assert!(message.contains("重新运行提取器"));
    }

    fn fake_bundle(root: &Path, name: &str) -> PathBuf {
        let bundle = root.join(name);
        fs::create_dir_all(bundle.join("Contents").join("MacOS")).expect("create bundle");
        fs::write(bundle.join("Contents").join("Info.plist"), b"<plist/>").expect("write plist");
        bundle
    }

    #[test]
    fn extractor_resolves_a_bundle_or_the_folder_that_holds_it() {
        let root = env::temp_dir().join(format!("wxkeys-extractor-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dist = root.join("dist");
        fs::create_dir_all(&dist).expect("create dist");
        let bundle = fake_bundle(&dist, EXTRACTOR_BUNDLE);

        assert_eq!(resolve_extractor(&bundle), Some(bundle.clone()));
        // The reference project hands out a `dist` folder, so the folder must work too.
        assert_eq!(resolve_extractor(&dist), Some(bundle.clone()));
        // A folder that is not a bundle and holds no bundle is rejected.
        assert_eq!(resolve_extractor(&root), None);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn only_system_or_user_applications_count_as_installed() {
        assert!(extractor_is_installed(Path::new(
            "/Applications/WeDataKeyExtractor.app"
        )));
        assert!(!extractor_is_installed(Path::new(
            "/tmp/build/dist/WeDataKeyExtractor.app"
        )));
    }

    #[test]
    fn paths_are_quoted_for_the_administrator_copy() {
        assert_eq!(
            shell_single_quote(Path::new("/Applications/WeDataKeyExtractor.app")),
            "'/Applications/WeDataKeyExtractor.app'"
        );
        assert_eq!(
            shell_single_quote(Path::new("/tmp/it's here/WeData.app")),
            r#"'/tmp/it'\''s here/WeData.app'"#
        );
    }

    /// Manual check against the real machine: run with
    /// `cargo test --lib real_extracted_passphrase -- --ignored --nocapture`.
    #[test]
    #[ignore = "requires local WeChat databases and an extracted passphrase file"]
    fn real_extracted_passphrase_imports_and_is_idempotent() {
        let path = saved_key_path();
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("missing {}: {error}", path.to_string_lossy()));
        let candidates = keys::parse_candidates(&text).expect("passphrase file must parse");
        let scan = discover::scan();
        if scan.accounts.is_empty() {
            eprintln!("no local WeChat accounts; nothing to verify");
            return;
        }
        let selected = scan.accounts[0].id.clone();
        let mut store = KeyStore::default();

        let first = apply_candidates(&mut store, &scan, &candidates, Some(&selected));
        assert!(
            first.matched > 0,
            "passphrase must unlock at least one database"
        );
        assert_eq!(first.already, 0);

        let second = apply_candidates(&mut store, &scan, &candidates, Some(&selected));
        assert_eq!(second.matched, 0, "second import must not rewrite keys");
        assert!(
            second.already > 0,
            "stored keys must re-verify against page 1"
        );
        eprintln!(
            "first: matched={} accounts={:?}; second: already={}",
            first.matched,
            first
                .accounts
                .iter()
                .map(|entry| (&entry.account, entry.matched))
                .collect::<Vec<_>>(),
            second.already
        );
    }

    #[test]
    fn clearing_keys_also_removes_the_decrypted_snapshot() {
        let root = env::temp_dir().join(format!("wxclear-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cache = decrypt::cache_dir(&root, "acct");
        std::fs::create_dir_all(cache.join("message")).unwrap();
        std::fs::create_dir_all(cache.join("session")).unwrap();
        std::fs::write(cache.join("_sync.json"), "{}").unwrap();
        std::fs::write(cache.join("message/message_0.db"), b"x").unwrap();
        std::fs::write(cache.join("session/session.db"), b"x").unwrap();

        // Plausible copy of another account must survive a scoped clear.
        let other = decrypt::cache_dir(&root, "other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("session.db"), b"x").unwrap();

        assert_eq!(clear_account_cache(&root, "acct").unwrap(), 2);
        assert!(!cache.exists());
        assert!(other.exists());

        let _ = std::fs::remove_dir_all(&root);
    }
}
