use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{Local, Utc};
use devify_http::{RequestTrace, RetryPolicy};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::{BTreeMap, HashMap, HashSet};
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
pub mod backup;
pub mod capabilities;
pub mod connectors;
mod commands;
mod desktop_credentials;
mod diagnostics;
mod devify_http;
mod feedback;
pub mod entitlements;
pub mod history;
pub(crate) mod http_client;
pub mod perm;
pub mod settings;
mod task_store;
mod config_runtime;
mod local_scope;
mod upload_archive;
mod instance_lock;
mod task_runtime;
mod runtime_commands;
mod report_plan;
mod cloud_summary;
mod summary_jobs;
use runtime_commands::{flush_config_outbox,emit_task_runtime};
#[cfg(test)]
use task_runtime::plan_automatic_batch;
pub mod wechat;
pub mod whatsapp;

pub mod mcp;
mod network;
mod update_gate;
#[cfg(test)]
#[path = "tests/runtime.rs"]
mod runtime_tests;
#[cfg(test)]
#[path = "tests/reconcile_owner.rs"]
mod reconcile_owner_tests;
#[cfg(test)]
#[path = "tests/connector_gate.rs"]
mod connector_gate_tests;
#[cfg(test)]
#[path = "tests/manual_upload.rs"]
mod manual_upload_tests;
#[cfg(test)]
#[path = "tests/security_identity.rs"]
mod security_identity_tests;

/// Network calls to Devify must never leave the UI spinning forever. Without a
/// timeout, a wrong LAN address or an unreachable host keeps "checking login"
/// pending until the kernel gives up, which can be minutes. The clients that carry
/// these deadlines live in [`http_client`] so one connection pool is reused.
/// A timeout or a refused connection means "server unavailable", not "logged
/// out". Keep reqwest's URL-heavy wording out of the UI.
fn devify_unreachable(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect()
}

fn devify_network_message(context: &str, error: &reqwest::Error) -> String {
    if devify_unreachable(error) {
        format!("{context}：服务端暂不可用，请稍后重试。")
    } else if error.is_body() || error.is_decode() {
        format!("{context}：服务端响应无效，请稍后重试。")
    } else {
        format!("{context}：{error}")
    }
}

/// Errors raised while resolving a desktop token. `Unavailable` must not be
/// treated as "the session expired": validation or credential persistence may
/// be temporarily blocked, including by an upload's fixed-identity lease.
#[derive(Debug)]
enum DevifyTokenError {
    Message(String),
    Unavailable(String),
}

impl std::fmt::Display for DevifyTokenError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(message) | Self::Unavailable(message) => formatter.write_str(message),
        }
    }
}

impl From<DevifyTokenError> for String {
    fn from(error: DevifyTokenError) -> Self {
        error.to_string()
    }
}

impl From<String> for DevifyTokenError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for DevifyTokenError {
    fn from(message: &str) -> Self {
        Self::Message(message.to_string())
    }
}

/// Single-file page shown in the browser after the loopback callback fires.
/// It is inlined because the listener answers exactly one request.
const AUTH_COMPLETE_HTML: &str = include_str!("../assets/auth-complete.html");

use wechat::capture;
use wechat::crypto;
use wechat::decrypt::{self, DecryptOutcome};
use wechat::discover::{self, ScanResult};
use wechat::keys::{self, KeyStore};
use wechat::media;
use wechat::query::{self, DigestReport, MessageItem, SearchReport, SessionItem};
use wechat::voice;
#[cfg(target_os = "windows")]
use wechat::windows_keys;

#[derive(Default)]
struct SyncState {
    active_accounts: Mutex<HashSet<String>>,
}

#[derive(Default)]
struct UploadState {
    retrying: Mutex<HashSet<String>>,
    syncing: update_gate::UpdateGate,
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

fn begin_account_upload<'a>(state: &'a UploadState, account: &str) -> Result<update_gate::UploadGuard<'a>, String> {
    state.syncing.begin_upload(account)
}

#[tauri::command]
fn update_begin_install(state: State<'_, UploadState>, force: Option<bool>) -> Result<(), String> { state.syncing.begin_install(force.unwrap_or(false)) }

#[tauri::command]
fn update_upload_accounts(state: State<'_, UploadState>) -> Result<Vec<String>, String> { state.syncing.uploading_accounts() }

#[tauri::command]
fn update_cancel_install(state: State<'_, UploadState>) { state.syncing.cancel_install(); }

struct PendingDevifyAuth {
    state: String,
    verifier: String,
    base_url: String,
    generation: u64,
    config: DevifyAuthConfig,
}

#[derive(Default)]
struct DevifyAuthState {
    pending: Mutex<Option<PendingDevifyAuth>>,
    /// Keeps the loopback listener task so a new attempt can abort a stale one
    /// instead of failing with `Address already in use` on port 47832.
    listener: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

#[derive(Serialize)]
struct DevifyAuthResult {
    device_id: String,
    expires_in: u64,
}

#[derive(Clone, serde::Deserialize)]
struct DevifyAuthConfig {
    client_id: String,
    browser_authorize_path: String,
    token_path: String,
    redirect_uri: String,
}

async fn devify_auth_config(
    data: &Path,
    settings: &settings::DevifySettings,
) -> Result<DevifyAuthConfig, String> {
    let base_url = settings.resolved_base_url()?;
    let response = http_client::strict()
        .get(format!("{}/api/v1/auth/desktop/config", base_url))
        .send_traced_retrying(data, diagnostics::Module::Auth, RetryPolicy::READ)
        .await
        .map_err(|e| devify_network_message("读取服务端登录配置失败", &e))?;
    if !response.status().is_success() {
        return Err(format!("读取服务端登录配置失败：{}", response.status()));
    }
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|_| "服务端登录配置响应无效，请检查服务地址并稍后重试。".to_string())?;
    serde_json::from_value(payload.get("data").unwrap_or(&payload).clone())
        .map_err(|e| format!("服务端登录配置格式无效：{e}"))
}

/// Read a string claim from a JWT without verifying it. The server remains the
/// authority; this only lets the client recover the device id it was issued.
fn access_token_claim(access: &str, claim: &str) -> Option<String> {
    access
        .split('.')
        .nth(1)
        .and_then(|part| URL_SAFE_NO_PAD.decode(part).ok())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|payload| {
            payload
                .get(claim)
                .and_then(|value|value.as_str().map(str::to_string).or_else(||value.as_i64().map(|v|v.to_string())))
        })
}

fn device_session_from_response(data: &serde_json::Value, device_id: &str) -> Result<(Option<String>, Option<String>), String> {
    match data.get("session") {
        None | Some(serde_json::Value::Null) => Ok((None, None)),
        Some(serde_json::Value::String(session)) if !session.is_empty() && !device_id.is_empty() =>
            Ok((Some(session.clone()), Some(device_id.to_string()))),
        _ => Err("设备会话响应无效，请稍后重试。".into()),
    }
}

/// Only short identity commits hold this lock. Uploads validate their frozen owner
/// before each request and before committing its result.
pub(crate) static UPLOAD_IDENTITY_LOCK: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());
/// Signing out must not wait for a background batch to finish, so it bumps this counter and
/// every in-flight upload stops at its next checkpoint.
/// Uploads capture the value when they start; a bump also stops the sync loop from opening
/// another batch, which keeps the drain short even for long uploads.
static UPLOAD_ABORT_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static CREDENTIAL_REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static AUTH_START_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Shown when a sign-out or logout-driven cancellation stopped an upload on purpose.
const UPLOAD_ABORTED_MESSAGE: &str = "已退出登录，上传已取消。";

fn upload_epoch() -> u64 {
    UPLOAD_ABORT_EPOCH.load(std::sync::atomic::Ordering::SeqCst)
}

/// An upload may only commit while the login it was verified against is still current.
fn upload_aborted_since(epoch: u64, current: u64) -> bool {
    epoch != current
}

fn upload_aborted(epoch: u64) -> bool {
    upload_aborted_since(epoch, upload_epoch())
}

/// Ask every in-flight upload to stop at its next checkpoint.
fn abort_in_flight_uploads() {
    UPLOAD_ABORT_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

async fn devify_access_token_async(app_data: &Path, base_url: &str) -> Result<String, DevifyTokenError> {
    devify_access_token_with_cleanup(app_data, base_url, &mut None).await
}

async fn devify_access_token_with_cleanup(app_data: &Path, base_url: &str, cleared_generation: &mut Option<u64>) -> Result<String, DevifyTokenError> {
    devify_access_token_resolve(app_data, base_url, cleared_generation, false).await
}

async fn devify_access_token_resolve(app_data: &Path, base_url: &str, cleared_generation: &mut Option<u64>, force_refresh: bool) -> Result<String, DevifyTokenError> {
    recover_local_logout(app_data).map_err(DevifyTokenError::Unavailable)?;
    let base_url = settings::canonical_api_base(base_url)?;
    // Serialize refresh requests while allowing login/logout to invalidate their CAS.
    let _refresh_guard = CREDENTIAL_REFRESH_LOCK.lock().await;
    let (mut tokens, token_generation) = desktop_credentials::load_with_generation(app_data, &base_url)?;
    let original_tokens = tokens.clone();
    let access = tokens.access.as_str();
    let exp = access
        .split('.')
        .nth(1)
        .and_then(|part| URL_SAFE_NO_PAD.decode(part).ok())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|payload| payload.get("exp").and_then(|v| v.as_i64()));
    if !force_refresh && !access.is_empty()
        && exp
            .map(|value| value > chrono::Utc::now().timestamp() + 60)
            .unwrap_or(false)
    {
        return Ok(access.to_string());
    }
    let refresh = if tokens.refresh.is_empty() && tokens.device_session.is_none() {
        return Err("服务端刷新令牌不存在，请重新登录。".into());
    } else {
        &tokens.refresh
    };
    let (path, body) = match (&tokens.device_session, &tokens.session_device_id) {
        (Some(session), Some(device_id)) if !session.is_empty() && !device_id.is_empty() =>
            ("/api/v1/auth/desktop/session/refresh", serde_json::json!({"session": session, "device_id": device_id})),
        _ => ("/api/v1/auth/token/refresh", serde_json::json!({"refresh": refresh})),
    };
    // Do not rotate a remote refresh token when an active upload already makes
    // local credential persistence impossible. The response still uses CAS below
    // in case the identity changes after this availability check.
    drop(UPLOAD_IDENTITY_LOCK.try_write().map_err(|_| DevifyTokenError::Unavailable(
        "备份正在使用登录状态，请在当前上传结束后重试。".into(),
    ))?);
    let response = http_client::strict()
        .post(format!("{}{path}", base_url.trim_end_matches('/')))
        .json(&body)
        .send_traced_retrying(app_data, diagnostics::Module::Auth, RetryPolicy::WRITE)
        .await
        .map_err(|e| {
            DevifyTokenError::Unavailable(devify_network_message("刷新服务端登录令牌失败", &e))
        })?;
    let status = response.status();
    if !status.is_success() {
        if status == reqwest::StatusCode::UNAUTHORIZED {
            // The refresh token was revoked or its account no longer exists.
            // Keep trying it forever would only hide the real problem.
            match desktop_credentials::clear_if_current(app_data, token_generation, &original_tokens) {
                Ok(true) => *cleared_generation = Some(token_generation.wrapping_add(1)),
                _ => return Err(DevifyTokenError::Unavailable("登录状态正在变更，请稍后重试。".into())),
            }
            return Err("服务端登录令牌已失效，请重新登录。".into());
        }
        return Err(DevifyTokenError::Unavailable(format!("刷新登录状态失败（HTTP {}），请稍后重试。", status.as_u16())));
    }
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| DevifyTokenError::Unavailable(format!("读取刷新响应失败：{e}")))?;
    let data = payload.get("data").unwrap_or(&payload);
    let new_access = data
        .get("access")
        .and_then(|v| v.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| DevifyTokenError::Unavailable("刷新响应缺少有效的 access。".into()))?;
    tokens.access = new_access.to_string();
    if let Some(new_refresh) = data.get("refresh").and_then(|v| v.as_str()) {
        tokens.refresh = new_refresh.to_string();
    }
    if !desktop_credentials::save_if_current(app_data, token_generation, &original_tokens, &tokens)
        .map_err(DevifyTokenError::Unavailable)? {
        return Err(DevifyTokenError::Unavailable("登录状态已改变，请重新检查。".into()));
    }
    config_runtime::activate_profile(app_data).map_err(DevifyTokenError::Unavailable)?;
    Ok(new_access.to_string())
}

#[derive(Serialize)]
struct DevifyUploadResult {
    job_id: String,
    batch_id: i64,
    status: String,
    message_count: usize,
    conversations: usize,
    /// Newest local message included in the package, used to advance the sync cursor.
    latest_position: Option<(i64, i64)>,
    /// Per-conversation newest message included in the package. Stored on the job so the
    /// chat list can tell "already uploaded" from "new local records" after a manual upload.
    covered: BTreeMap<String, settings::SyncCursor>,
    /// Voice messages in this package that still have no successful transcript.
    voice_pending: BTreeMap<String, Vec<settings::VoiceRef>>,
    /// Voice messages considered by this package, used to replace retry state safely.
    voice_processed: BTreeMap<String, Vec<settings::VoiceRef>>,
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
    let api_base = settings.devify.resolved_base_url()?;
    let token = devify_access_token_async(&app_data(&app), &api_base).await?;
    let generation = request_generation(&app_data(&app), &api_base, &token)?;
    let response = http_client::api()
        .get(format!(
            "{}/api/v1/chat/uploads/{batch_id}",
            api_base
        ))
        .bearer_auth(&token)
        .send_traced_retrying(&app_data(&app), diagnostics::Module::Upload, RetryPolicy::READ)
        .await
        .map_err(|e| devify_network_message("读取上传状态失败", &e))?;
    let status = response.status();
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("读取上传状态响应失败：{e}"))?;
    if !status.is_success() {
        let fallback = format!("HTTP {status}");
        return Err(format!(
            "读取上传状态失败：{}",
            devify_response_error(&app, status, &payload, &fallback, &settings.devify.base_url, &token, generation)
        ));
    }
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

const UPLOAD_LIST_MAX_PAGE: u32 = 1_000_000;
const UPLOAD_LIST_MAX_PAGE_SIZE: u32 = 100;
const UPLOAD_LIST_MAX_RESPONSE_BYTES: usize = 5 * 1024 * 1024;

fn upload_list_pagination(page: Option<u32>, page_size: Option<u32>) -> Result<(u32, u32), String> {
    let page = page.unwrap_or(1);
    let page_size = page_size.unwrap_or(UPLOAD_LIST_MAX_PAGE_SIZE);
    if !(1..=UPLOAD_LIST_MAX_PAGE).contains(&page)
        || !(1..=UPLOAD_LIST_MAX_PAGE_SIZE).contains(&page_size)
    {
        return Err("上传列表分页参数无效。".into());
    }
    Ok((page, page_size))
}

fn upload_list_base_url(settings: &settings::AppSettings, space_id: &str) -> Result<String, String> {
    validate_devify_space(settings, &space_id)?;
    let base_url = settings.devify.resolved_base_url()?;
    let parsed = Url::parse(&base_url).map_err(|_| "服务端地址无效。")?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("服务端地址无效。".into());
    }
    Ok(base_url)
}

fn parse_upload_list(payload: serde_json::Value) -> Result<serde_json::Value, String> {
    let data = payload.get("data").unwrap_or(&payload);
    let pagination = data.get("pagination");
    let link_valid = |name: &str| pagination.and_then(|value| value.get(name))
        .is_some_and(|value| value.is_null() || value.is_string());
    if !data.get("list").and_then(serde_json::Value::as_array)
        .is_some_and(|items| items.iter().all(serde_json::Value::is_object))
        || !pagination.and_then(|value| value.get("total")).is_some_and(|value| value.as_u64().is_some())
        || !pagination.and_then(|value| value.get("page")).is_some_and(|value| value.as_u64().is_some_and(|page| page > 0))
        || !pagination.and_then(|value| value.get("page_size")).is_some_and(|value| value.as_u64().is_some_and(|size| size > 0 && size <= u64::from(UPLOAD_LIST_MAX_PAGE_SIZE)))
        || !link_valid("next")
        || !link_valid("previous")
    {
        return Err("服务端上传列表响应格式无效。".into());
    }
    Ok(data.clone())
}

async fn fetch_upload_list(
    data: &Path,
    base_url: &str,
    space_id: &str,
    page: u32,
    page_size: u32,
    token: &str,
) -> Result<(reqwest::StatusCode, Vec<u8>), String> {
    // Never forward a bearer token via an HTTP redirect or return request URLs in errors.
    let client = http_client::strict();
    let mut response = client
        .get(format!("{base_url}/api/v1/spaces/{space_id}/chat/uploads"))
        .query(&[("page", page), ("page_size", page_size)])
        .bearer_auth(token)
        .send_traced_retrying(data, diagnostics::Module::Upload, RetryPolicy::READ)
        .await
        .map_err(|_| "读取服务端上传列表失败，请检查网络连接。")?;
    let status = response.status();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "读取上传列表响应失败。")? {
        if body.len().saturating_add(chunk.len()) > UPLOAD_LIST_MAX_RESPONSE_BYTES {
            return Err("服务端上传列表响应过大。".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok((status, body))
}

#[tauri::command]
async fn devify_upload_list(
    app: AppHandle,
    space_id: String,
    page: Option<u32>,
    page_size: Option<u32>,
) -> Result<serde_json::Value, String> {
    let (page, page_size) = upload_list_pagination(page, page_size)?;
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    let base_url = upload_list_base_url(&settings, &space_id)?;
    let token = match devify_access_token_async(&data, &base_url).await {
        Ok(token) => token,
        // A temporary failure can also be a credential-write conflict with an upload.
        // Preserve its actionable cause rather than mislabelling it as a network error.
        Err(DevifyTokenError::Unavailable(message)) => {
            return Err(message);
        }
        Err(error) => return Err(error.to_string()),
    };
    if upload_list_base_url(&settings::load(&data)?, &space_id)? != base_url {
        return Err("服务端地址已改变，请重新读取上传列表。".into());
    }
    if desktop_credentials::load(&data, &base_url)?.access != token {
        return Err("服务端登录状态已改变，请重新读取上传列表。".into());
    }
    let generation = request_generation(&data, &base_url, &token)?;
    let (status, body) = fetch_upload_list(&data, &base_url, &space_id, page, page_size, &token).await?;
    // A response for an old selection must not overwrite the new space or service.
    let current = settings::load(&data)?;
    if upload_list_base_url(&current, &space_id)? != base_url {
        return Err("服务端地址已改变，请重新读取上传列表。".into());
    }
    if desktop_credentials::load(&data, &base_url)?.access != token {
        return Err("服务端登录状态已改变，请重新读取上传列表。".into());
    }
    if !status.is_success() {
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(forget_devify_session(&app, &serde_json::Value::Null, &base_url, &token, generation));
        }
        return Err(format!("读取服务端上传列表失败：HTTP {}。", status.as_u16()));
    }
    let payload = serde_json::from_slice(&body).map_err(|_| "服务端上传列表响应格式无效。")?;
    parse_upload_list(payload)
}

#[derive(serde::Deserialize, Serialize)]
struct DevifySpace {
    id: String,
    name: String,
    kind: String,
    status: String,
    role: Option<String>,
}

/// Prefer the personal space the server creates for every account; fall back to the first
/// space the account can see. Used after login and by `devify_space_ensure`.
fn pick_personal_space(spaces: &[DevifySpace]) -> Option<String> {
    spaces
        .iter()
        .find(|space| space.kind.eq_ignore_ascii_case("personal"))
        .or_else(|| spaces.iter().find(|space| space.name.contains("个人")))
        .or_else(|| {
            spaces
                .iter()
                .find(|space| space.kind.eq_ignore_ascii_case("private"))
        })
        .or_else(|| spaces.first())
        .map(|space| space.id.clone())
}

/// Login should not leave the app without an upload target: select the personal space when
/// nothing is chosen yet (or when the stored space no longer exists).
async fn ensure_active_space(app: &AppHandle) -> Option<String> {
    let mut ignored_generation = None;
    ensure_active_space_for(app, None, &mut ignored_generation).await
}

async fn ensure_active_space_for(app: &AppHandle, expected: Option<(&desktop_credentials::Credentials, u64)>, completed_generation: &mut Option<u64>) -> Option<String> {
    let data = app_data(app);
    let settings = settings::load(&data).ok()?;
    let base = settings.devify.resolved_base_url().ok()?;
    let token = if let Some((credentials, _)) = expected {
        credentials.access.clone()
    } else {
        devify_access_token_async(&data, &base).await.ok()?
    };
    let (credentials, generation) = desktop_credentials::load_with_generation(&data, &base).ok()?;
    let current = settings::load(&data).ok()?;
    if credentials.access != token || current.devify.resolved_base_url().ok()? != base
        || expected.is_some_and(|(expected, expected_generation)| credentials != *expected || generation != expected_generation)
        || current.devify.active_space_id != settings.devify.active_space_id
        || current.devify.device_id != settings.devify.device_id
        || current.active_account_id != settings.active_account_id { return None; }
    let (spaces_result,_,_)=tokio::join!(
        fixed_identity_get(&data,&base,&token,"/api/v1/spaces"),
        devify_current_user(app.clone()),
        entitlements::refresh(&data,&base,&token),
    );
    let payload = spaces_result.ok()?;
    let spaces: Vec<DevifySpace> = serde_json::from_value(payload).ok()?;
    let known = |id: &str| spaces.iter().any(|space| space.id == id);
    let chosen = settings.devify.active_space_id.clone().filter(|id| known(id))
        .or_else(|| pick_personal_space(&spaces));
    let mut space_changed = false;
    settings::update_identity(&data, |current| {
        let (stored, current_generation) = desktop_credentials::load_with_generation(&data, &base)?;
        if stored != credentials || current_generation != generation
            || current.devify.resolved_base_url()? != base
            || current.devify.active_space_id != settings.devify.active_space_id
            || current.devify.device_id != settings.devify.device_id
            || current.active_account_id != settings.active_account_id {
            return Err("选择空间时登录状态已改变。".into());
        }
        space_changed = current.devify.active_space_id.as_deref() != chosen.as_deref();
        current.devify.active_space_id = chosen.clone();
        Ok(())
    }).ok()?;
    *completed_generation = Some(generation.wrapping_add(u64::from(space_changed)));
    chosen
}

#[tauri::command]
async fn devify_space_ensure(app: AppHandle) -> Result<Option<String>, String> {
    match ensure_active_space(&app).await {
        Some(id) => Ok(Some(id)),
        None => Ok(None),
    }
}

#[tauri::command]
async fn devify_spaces(app: AppHandle) -> Result<Vec<DevifySpace>, String> {
    let settings = settings::load(&app_data(&app))?;
    let api_base = settings.devify.resolved_base_url()?;
    let token = devify_access_token_async(&app_data(&app), &api_base).await?;
    let generation = request_generation(&app_data(&app), &api_base, &token)?;
    let response = http_client::api()
        .get(format!(
            "{}/api/v1/spaces",
            api_base
        ))
        .bearer_auth(&token)
        .send_traced_retrying(&app_data(&app), diagnostics::Module::App, RetryPolicy::READ)
        .await
        .map_err(|e| devify_network_message("读取 空间失败", &e))?;
    let status = response.status();
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("读取空间响应失败：{e}"))?;
    if !status.is_success() {
        let fallback = format!("HTTP {status}");
        return Err(format!(
            "读取 空间失败：{}",
            devify_response_error(&app, status, &payload, &fallback, &settings.devify.base_url, &token, generation)
        ));
    }
    let data = payload.get("data").unwrap_or(&payload);
    serde_json::from_value(data.clone()).map_err(|e| format!("空间响应格式无效：{e}"))
}



fn validate_devify_space(settings: &settings::AppSettings, space_id: &str) -> Result<(), String> {
    if space_id.trim().is_empty() || settings.devify.active_space_id.as_deref() != Some(space_id) {
        return Err("空间已改变，请重新选择后再同步。".into());
    }
    Ok(())
}

/// Turn a Devify error payload into the message the user should see.
///
/// The server wraps every response as `{"code", "message", "data"}`, so the
/// useful reason lives under `data.detail` (permission, not found, auth) or as
/// field errors under `data` (validation). Reading only the top-level `detail`
/// hid all of that and turned every failure into "服务端拒绝了请求".
fn devify_error_message(payload: &serde_json::Value) -> Option<String> {
    fn flatten(value: &serde_json::Value) -> Option<String> {
        match value {
            serde_json::Value::String(text) => {
                let text = text.trim();
                (!text.is_empty()).then(|| text.to_string())
            }
            serde_json::Value::Array(items) => {
                let parts: Vec<String> = items.iter().filter_map(flatten).collect();
                (!parts.is_empty()).then(|| parts.join("；"))
            }
            serde_json::Value::Object(map) => {
                let parts: Vec<String> = map
                    .iter()
                    .filter(|(key, _)| key.as_str() != "code" && key.as_str() != "message")
                    .filter_map(|(key, value)| flatten(value).map(|text| format!("{key}：{text}")))
                    .collect();
                (!parts.is_empty()).then(|| parts.join("；"))
            }
            _ => None,
        }
    }
    // The envelope's own `message` is always "failed"/"success", never a reason.
    fn meaningful(value: Option<String>) -> Option<String> {
        value.filter(|text| !matches!(text.as_str(), "failed" | "success"))
    }

    let data = payload.get("data").unwrap_or(payload);
    meaningful(data.get("detail").and_then(flatten))
        .or_else(|| meaningful(data.get("message").and_then(flatten)))
        .or_else(|| flatten(data))
        .or_else(|| meaningful(payload.get("detail").and_then(flatten)))
        .or_else(|| meaningful(payload.get("message").and_then(flatten)))
        .or_else(|| flatten(payload))
}

fn devify_error_text(payload: &serde_json::Value, fallback: &str) -> String {
    devify_error_message(payload).unwrap_or_else(|| fallback.to_string())
}

/// The server rejected the stored credentials (expired, revoked or the account
/// no longer exists). Drop them and tell the UI to reopen the login gate
/// instead of retrying the same request until every page looks broken.
fn request_generation(data: &Path, base_url: &str, token: &str) -> Result<u64, String> {
    let (credentials, generation) = desktop_credentials::load_with_generation(data, base_url)?;
    if credentials.access != token { return Err("服务端登录状态已改变，请重新操作。".into()); }
    Ok(generation)
}

fn forget_devify_session(app: &AppHandle, payload: &serde_json::Value, base_url: &str, token: &str, generation: u64) -> String {
    if request_generation(&app_data(app), base_url, token).ok() != Some(generation) {
        return "服务端登录状态已改变，请重新操作。".into();
    }
    let message = devify_error_message(payload).unwrap_or_else(|| "正在检查登录状态。".to_string());
    let _ = app.emit("devify-auth-expired", serde_json::json!({ "message": message }));
    "服务端需要重新验证登录，请稍后重试。".to_string()
}

fn devify_response_error(
    app: &AppHandle,
    status: reqwest::StatusCode,
    payload: &serde_json::Value,
    fallback: &str,
    base_url: &str,
    token: &str,
    generation: u64,
) -> String {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return forget_devify_session(app, payload, base_url, token, generation);
    }
    devify_friendly_error(status, &devify_error_text(payload, fallback))
}

/// Translate server wording that means "pick another space" into something the
/// user can act on. The API hides a space when the caller is not a member of it,
/// so "not a member" and "gone" share the same 404.
fn devify_friendly_error(status: reqwest::StatusCode, detail: &str) -> String {
    if status == reqwest::StatusCode::NOT_FOUND && detail.contains("Space not found") {
        return "当前 空间不存在或无权访问，请在「设置 / 账户」重新选择空间。".into();
    }
    detail.to_string()
}

async fn devify_json_request(
    app: &AppHandle,
    method: reqwest::Method,
    path: String,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(app))?;
    let token = devify_access_token_async(&app_data(app), &settings.devify.base_url).await?;
    let generation = request_generation(&app_data(app), &settings.devify.base_url, &token)?;
    let client = http_client::api();
    let policy = RetryPolicy::for_method(&method);
    let mut request = client
        .request(
            method,
            format!("{}{}", settings.devify.resolved_base_url()?, path),
        )
        .bearer_auth(&token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let mut response = request
        .send_traced_retrying(&app_data(app), diagnostics::Module::App, policy)
        .await
        .map_err(|e| devify_network_message("访问服务端失败", &e))?;
    let status = response.status();
    let mut bytes=Vec::new();
    while let Some(chunk)=response.chunk().await.map_err(|e|format!("读取服务端响应失败：{e}"))? {
        if bytes.len().saturating_add(chunk.len())>2*1024*1024 {return Err("服务端响应过大，请缩小查询范围。".into());}
        bytes.extend_from_slice(&chunk);
    }
    if request_generation(&app_data(app),&settings.devify.base_url,&token).ok()!=Some(generation) {return Err("登录状态已改变，请重新操作。".into());}
    if status==reqwest::StatusCode::NO_CONTENT {return Ok(serde_json::Value::Null);}
    let payload: serde_json::Value=serde_json::from_slice(&bytes).map_err(|e|format!("读取服务端响应失败：{e}"))?;
    if !status.is_success() {
        let detail = devify_response_error(app, status, &payload, "服务端拒绝了请求", &settings.devify.base_url, &token, generation);
        return Err(format!("服务端请求失败：{detail}"));
    }
    Ok(payload.get("data").cloned().unwrap_or(payload))
}

#[tauri::command]
async fn devify_current_user(app: AppHandle) -> Result<serde_json::Value, String> {
    let data=app_data(&app);let s=settings::load(&data)?;let base=s.devify.resolved_base_url()?;
    let token=devify_access_token_async(&data,&base).await?;
    let user_id=access_token_claim(&token,"user_id").unwrap_or_default();
    let key=format!("user-v1-{}",hex::encode(sha2::Sha256::digest(format!("{base}\0{user_id}").as_bytes())));
    if let Some(cache)=(!user_id.is_empty()).then(||task_store::cache_get(&data,&key)).flatten().filter(|v|v["at"].as_i64().is_some_and(|at|Utc::now().timestamp()-at<120)) {return Ok(cache["user"].clone());}
    let user = devify_json_request(&app, reqwest::Method::GET, "/api/v1/auth/user".into(), None).await?;
    let result=serde_json::json!({ "id": user.get("id"), "display_name": user.get("display_name"), "username": user.get("username") });
    if !user_id.is_empty() {let _=task_store::cache_put(&data,&key,&serde_json::json!({"at":Utc::now().timestamp(),"user":result}));}
    Ok(result)
}














#[tauri::command]
async fn devify_conversations(
    app: AppHandle,
    space_id: String,
    search: Option<String>,
    page: Option<u32>,
    page_size: Option<u32>,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
    let mut path = format!(
        "/api/v1/spaces/{space_id}/conversations?page={}&page_size={}",
        page.unwrap_or(1).max(1),
        page_size.unwrap_or(25).clamp(1, 100)
    );
    let keyword = search.unwrap_or_default().trim().to_string();
    if !keyword.is_empty() {
        let encoded: String = url::form_urlencoded::byte_serialize(keyword.as_bytes()).collect();
        path.push_str(&format!("&search={encoded}"));
    }
    devify_json_request(&app, reqwest::Method::GET, path, None).await
}

#[tauri::command]
async fn devify_conversation_detail(
    app: AppHandle,
    space_id: String,
    conversation_id: i64,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
    if conversation_id <= 0 {
        return Err("会话标识无效。".into());
    }
    devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/spaces/{space_id}/conversations/{conversation_id}"),
        None,
    )
    .await
}

#[tauri::command]
async fn devify_conversation_messages(
    app: AppHandle,
    space_id: String,
    conversation_id: i64,
    page: Option<u32>,
    page_size: Option<u32>,
    focus_message_id: Option<i64>,
    latest: Option<bool>,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
    if conversation_id <= 0 {
        return Err("会话标识无效。".into());
    }
    let mut path = format!(
        "/api/v1/spaces/{space_id}/conversations/{conversation_id}/messages?page={}&page_size={}",
        if latest.unwrap_or(false) {"last".to_string()} else {page.unwrap_or(1).max(1).to_string()},
        page_size.unwrap_or(50).clamp(1, 100)
    );
    if let Some(message_id) = focus_message_id.filter(|id| *id > 0) {
        path.push_str(&format!("&focus_message_id={message_id}"));
    }
    devify_json_request(&app, reqwest::Method::GET, path, None).await
}

#[tauri::command]
async fn devify_message_detail(
    app: AppHandle,
    space_id: String,
    conversation_id: i64,
    message_id: i64,
) -> Result<serde_json::Value, String> {
    if conversation_id <= 0 || message_id <= 0 {
        return Err("消息标识无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
    devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/spaces/{space_id}/conversations/{conversation_id}/messages/{message_id}"),
        None,
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
    /// One record per avatar, pointing at a file under `media/avatars/`.
    avatars: Vec<serde_json::Value>,
    /// Avatar image files, addressed by their content digest.
    avatar_files: Vec<(String, Vec<u8>)>,
    /// Decoded message images, addressed by their content digest.
    media_files: Vec<(String, Vec<u8>)>,
    /// Voice messages that still need a transcript before the next automatic sync.
    voice_pending: BTreeMap<String, Vec<settings::VoiceRef>>,
    /// Voice messages examined while preparing this package.
    voice_processed: BTreeMap<String, Vec<settings::VoiceRef>>,
    /// Newest local message (time, local id) included in this package.
    latest_position: Option<(i64, i64)>,
    /// Newest included message per conversation, keyed by username.
    covered: BTreeMap<String, settings::SyncCursor>,
}

#[derive(Default)]
struct ExistingUploadKeys {
    legacy: HashMap<i64, Option<i64>>,
    stable: HashSet<String>,
}
type LegacyUploadKeys = HashMap<String, ExistingUploadKeys>;

async fn fixed_identity_get(data: &Path, base: &str, token: &str, path: &str) -> Result<serde_json::Value, String> {
    fixed_identity_json(data, base, token, reqwest::Method::GET, path, None).await
}

async fn fixed_identity_json(data: &Path, base: &str, token: &str, method: reqwest::Method, path: &str,
    body: Option<&serde_json::Value>) -> Result<serde_json::Value, String> {
    let client = http_client::strict();
    let policy = RetryPolicy::for_method(&method);
    let mut request = client.request(method, format!("{base}{path}")).bearer_auth(token);
    if let Some(body) = body { request = request.json(body); }
    let mut response = request.send_traced_retrying(data, diagnostics::Module::Upload, policy)
        .await.map_err(|_| "无法访问云端，请检查网络连接。")?;
    if !response.status().is_success() { return Err(format!("无法读取云端数据：HTTP {}。", response.status().as_u16())); }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "无法读取云端响应。")? {
        append_retry_identity_body(&mut body, &chunk)?;
    }
    let payload: serde_json::Value = serde_json::from_slice(&body).map_err(|_| "云端响应格式无效。")?;
    Ok(payload.get("data").cloned().unwrap_or(payload))
}

async fn load_batch_legacy_keys(app:&AppHandle,space:&str,account:&str,selection:&BTreeMap<String,Vec<settings::MessageRef>>)->Result<LegacyUploadKeys,String>{
    let cache=decrypt::cache_dir(&app_data(app),account);let copy=selection.clone();
    let queries=tauri::async_runtime::spawn_blocking(move||->Result<Vec<serde_json::Value>,String>{
        let mut queries=Vec::new();
        for (name,refs) in copy {
            let messages=query::messages_selected(&cache,&name,&refs)?;let mut keys=Vec::new();
            for m in messages {keys.push(format!("src:{}",m.local_id));keys.push(if m.server_id!=0 {format!("srv:{}",m.server_id)} else {format!("local:{}:{}:{}",m.source_shard,m.local_id,m.time)});}
            keys.sort();keys.dedup();queries.push(serde_json::json!({"username":name,"keys":keys}));
        }
        Ok(queries)
    }).await.map_err(|e|e.to_string())??;
    let data=devify_json_request(app,reqwest::Method::POST,format!("/api/v1/spaces/{space}/desktop/message-keys"),Some(serde_json::json!({"source_account_id":account,"queries":queries}))).await?;
    let list=data["list"].as_array().ok_or("服务端消息标识响应无效。")?;
    if list.len()!=selection.len() {return Err("服务端消息标识范围不完整。".into());}
    let mut result=LegacyUploadKeys::new();
    for item in list {
        let name=item["username"].as_str().filter(|n|selection.contains_key(*n)).ok_or("消息标识来源不匹配。")?;
        let mut keys=ExistingUploadKeys::default();collect_existing_keys(item["keys"].as_array().ok_or("缺少消息标识。")?,&mut keys)?;
        if result.insert(name.into(),keys).is_some() {return Err("重复的消息标识范围。".into());}
    }
    Ok(result)
}

/// Read existing legacy message keys before assigning new identities. Never assume that a
/// historic `src:<local_id>` belongs to the earliest or latest local shard. `epoch` is the
/// sign-out generation captured by the caller, so a scan never outlives its login.
async fn load_legacy_upload_keys(app: &AppHandle, space_id: &str, account_id: &str, usernames: &[String], fixed: Option<(&str, &str)>, epoch: u64) -> Result<LegacyUploadKeys, String> {
    let mut result = LegacyUploadKeys::new();
    for username in usernames {
        let encoded: String = url::form_urlencoded::byte_serialize(username.as_bytes()).collect();
        let mut conversation = None;
        for page in 1..=100 {
            if upload_aborted(epoch) { return Err(UPLOAD_ABORTED_MESSAGE.into()); }
            let path = format!("/api/v1/spaces/{space_id}/conversations?search={encoded}&page={page}&page_size=100");
            let data = if let Some((base, token)) = fixed { fixed_identity_get(&app_data(app), base, token, &path).await? } else { devify_json_request(app, reqwest::Method::GET, path, None).await? };
            let items = data.get("list").and_then(|v| v.as_array()).ok_or("无法读取云端会话列表。")?;
            for item in items {
                if item.get("platform").and_then(|v| v.as_str()) == Some("wechat")
                    && item.get("source_namespace").and_then(|v| v.as_str()) == Some(account_id)
                    && item.get("source_conversation_id").and_then(|v| v.as_str()) == Some(username.as_str()) {
                    let id = item.get("id").and_then(|v| v.as_i64()).ok_or("云端会话标识无效。")?;
                    if conversation.replace(id).is_some() { return Err("云端存在多个相同来源会话，无法安全上传。".into()); }
                }
            }
            let next = data.pointer("/pagination/next").ok_or("云端会话分页信息不完整，无法安全校验旧消息。")?;
            if next.is_null() { break; }
            if page == 100 { return Err("云端会话列表过长，无法完整校验旧消息。".into()); }
        }
        if let Some(id) = conversation {
            let scan = LegacyPageScan { app: app.clone(), fixed: fixed.map(|(base, token)| (base.to_string(), token.to_string())), space_id: space_id.to_string(), conversation_id: id };
            let keys = scan.existing_keys(epoch).await?;
            result.insert(username.clone(), keys);
        } else {
            result.insert(username.clone(), ExistingUploadKeys::default());
        }
    }
    Ok(result)
}

/// Pages of one cloud conversation, fetched in bounded parallel chunks. Every page is an
/// independent read and the key set is order-independent, so the scan keeps the same result
/// while finishing in a fraction of the sequential time.
const LEGACY_KEY_SCAN_CONCURRENCY: u32 = 6;
const LEGACY_KEY_SCAN_MAX_PAGES: u32 = 10000;

#[derive(Clone)]
struct LegacyPageScan {
    app: AppHandle,
    fixed: Option<(String, String)>,
    space_id: String,
    conversation_id: i64,
}

impl LegacyPageScan {
    async fn page(&self, page: u32) -> Result<serde_json::Value, String> {
        let path = format!("/api/v1/spaces/{}/conversations/{}/messages?page={page}&page_size=100", self.space_id, self.conversation_id);
        match &self.fixed {
            Some((base, token)) => fixed_identity_get(&app_data(&self.app), base, token, &path).await,
            None => devify_json_request(&self.app, reqwest::Method::GET, path, None).await,
        }
    }

    async fn existing_keys(&self, epoch: u64) -> Result<ExistingUploadKeys, String> {
        let mut keys = ExistingUploadKeys::default();
        let mut page: u32 = 1;
        loop {
            if upload_aborted(epoch) { return Err(UPLOAD_ABORTED_MESSAGE.into()); }
            if page > LEGACY_KEY_SCAN_MAX_PAGES { return Err("云端消息过多，无法完整校验旧消息。".into()); }
            let last = (page + LEGACY_KEY_SCAN_CONCURRENCY - 1).min(LEGACY_KEY_SCAN_MAX_PAGES);
            let handles: Vec<_> = (page..=last)
                .map(|page| {
                    let scan = self.clone();
                    (page, tauri::async_runtime::spawn(async move { scan.page(page).await }))
                })
                .collect();
            let mut pages = Vec::with_capacity(handles.len());
            for (_, handle) in handles {
                pages.push(match handle.await {
                    Ok(result) => result,
                    Err(error) => Err(format!("读取云端消息失败：{error}")),
                });
            }
            if merge_legacy_pages(pages, &mut keys)? { break; }
            page = last + 1;
        }
        Ok(keys)
    }
}

/// Merge one chunk of fetched pages in page order. Pages prefetched past the end cannot
/// change the key set, so only the first page that reports no successor matters.
fn merge_legacy_pages(pages: Vec<Result<serde_json::Value, String>>, keys: &mut ExistingUploadKeys) -> Result<bool, String> {
    let mut finished = false;
    for result in pages {
        if finished { continue; }
        let data = result?;
        let items = data.get("list").and_then(|v| v.as_array()).ok_or("无法读取云端消息列表。")?;
        collect_existing_keys(items, keys)?;
        let next = data.pointer("/pagination/next").ok_or("云端消息分页信息不完整，无法安全校验旧消息。")?;
        if next.is_null() { finished = true; }
    }
    Ok(finished)
}

fn collect_existing_keys(items: &[serde_json::Value], keys: &mut ExistingUploadKeys) -> Result<(), String> {
    for item in items {
        let key = item.get("message_key").and_then(|v| v.as_str()).unwrap_or_default();
        if key.starts_with("srv:") || key.starts_with("local:") { keys.stable.insert(key.to_string()); }
        if let Some(id) = key.strip_prefix("src:").and_then(|s| s.parse::<i64>().ok()) {
            let time = item.get("occurred_at").and_then(|v| v.as_str()).and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok()).map(|v| v.timestamp());
            if keys.legacy.insert(id, time).is_some() { return Err("云端存在冲突的旧消息标识，无法安全上传。".into()); }
        }
    }
    Ok(())
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

fn validate_upload_batch_size(count: usize) -> Result<(), String> {
    if count > SYNC_MESSAGES_PER_BATCH {
        return Err(format!("单批最多上传 {SYNC_MESSAGES_PER_BATCH} 条消息，请缩小日期范围或等待自动分批。"));
    }
    Ok(())
}

#[derive(Debug)]
struct PlannedSyncBatch {
    messages: Vec<settings::MessageRef>,
    has_more: bool,
}

fn manual_batch_plan(selection: &BTreeMap<String, Vec<settings::MessageRef>>, limit: usize) -> Result<Vec<BTreeMap<String, Vec<settings::MessageRef>>>, String> {
    if limit == 0 { return Err("每批消息数量必须大于零。".into()); }
    let mut batches = Vec::new();
    let mut current: BTreeMap<String, Vec<settings::MessageRef>> = BTreeMap::new();
    let mut count = 0;
    for (name, messages) in selection {
        let mut unique = HashSet::new();
        for message in messages {
            if !unique.insert(message) { return Err("消息选择包含重复项。".into()); }
            if count == limit { batches.push(std::mem::take(&mut current)); count = 0; }
            current.entry(name.clone()).or_default().push(message.clone());
            count += 1;
        }
    }
    if count != 0 { batches.push(current); }
    Ok(batches)
}

fn next_manual_batch(value: &settings::AppSettings, owner: &settings::UploadOwner) -> Option<String> {
    value.upload_queue.iter().filter(|job| {
        let Some(group) = job.manual_group.as_ref() else { return false; };
        if job.owner.as_ref() != Some(owner) || job.account_id != owner.account_id
            || job.account_id != value.active_account_id || job.space_id != owner.space_id
            || value.devify.active_space_id.as_deref() != Some(job.space_id.as_str()) { return false; }
        if !matches!(job.status.as_str(), "queued" | "running")
            || job.batch_id.is_some() || job.create_started != Some(false) { return false; }
        let previous: Vec<_> = value.upload_queue.iter().filter(|other| other.manual_group.as_ref() == Some(group) && other.manual_index < job.manual_index).collect();
        previous.len() == job.manual_index && previous.iter().all(|other| other.owner.as_ref() == Some(owner) && other.status == "completed")
    }).min_by_key(|job| (&job.created_at, job.manual_index)).map(|job| job.id.clone())
}

fn plan_sync_batch(messages: &[settings::MessageRef], cursor: Option<settings::SyncCursor>, limit: usize) -> Result<PlannedSyncBatch, String> {
    if limit == 0 { return Err("每批消息数量必须大于零。".into()); }
    let mut ordered = messages.to_vec();
    ordered.sort_by(|a, b| (a.time, a.local_id, &a.source_shard).cmp(&(b.time, b.local_id, &b.source_shard)));
    if ordered.windows(2).any(|p| p[0] == p[1]) { return Err("消息来源标识重复，无法安全分批。".into()); }
    let mut selected = Vec::new();
    let mut offset = 0;
    while offset < ordered.len() {
        let end = ordered[offset..].iter().position(|m| m.time != ordered[offset].time).map_or(ordered.len(), |n| offset + n);
        let group = &ordered[offset..end];
        let newest_id = group.iter().map(|m| m.local_id).max().unwrap_or_default();
        if cursor.is_some_and(|c| c.covers_boundary((group[0].time, newest_id), group.len())) { offset = end; continue; }
        if group.len() > limit { return Err("同一秒的消息数量超过单批上限，无法安全分批。".into()); }
        if selected.len() + group.len() > limit { return Ok(PlannedSyncBatch { messages: selected, has_more: true }); }
        selected.extend_from_slice(group);
        offset = end;
    }
    Ok(PlannedSyncBatch { messages: selected, has_more: false })
}

fn sync_message_selection(messages: &[settings::MessageRef], cursor: Option<settings::SyncCursor>, pending: &[settings::VoiceRef]) -> Vec<settings::MessageRef> {
    messages.iter().filter(|m| cursor.map_or(true, |c| m.time >= c.time) || pending.iter().any(|v| v.local_id == m.local_id && v.time == m.time && (v.source_shard.is_empty() || v.source_shard == m.source_shard))).cloned().collect()
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
            source_shard: message.source_shard.clone(),
        };
        let legacy = settings::MessageRef { source_shard: String::new(), ..key.clone() };
        let selected_key = if wanted.contains(&key) { Some(key) } else if wanted.contains(&legacy) { Some(legacy) } else { None };
        if let Some(selected_key) = selected_key {
            if !seen.insert(selected_key) {
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
        // Keyset cursor on (time, local id). Many messages can share one second, and a
        // timestamp-only cursor would skip whatever is left of that second at a page
        // boundary; this resumes exactly after the last row read.
        let oldest = page.last().map(|message| message.time);
        before = page.last().map(query::MessageCursor::from_message);
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
        {
            break;
        }
    }
    Ok(session_messages)
}

fn raw_messages_for_upload(
    cache: &Path,
    username: &str,
    selected: &[MessageItem],
) -> Result<Vec<query::RawMessage>, String> {
    query::raw_messages_selected(cache, username, selected)
}

fn image_resource_hash(cache: &Path, item: &media::MediaItem) -> Option<String> {
    if item.server_id == 0 { return None; }
    let db = rusqlite::Connection::open_with_flags(
        cache.join("message/message_resource.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ).ok()?;
    let mut statement = db.prepare("SELECT packed_info FROM MessageResourceInfo WHERE message_svr_id = ?1 AND message_local_id = ?2 AND message_create_time = ?3 LIMIT 2").ok()?;
    let mut rows = statement.query(rusqlite::params![item.server_id, item.local_id, item.time]).ok()?;
    let bytes: Vec<u8> = rows.next().ok()??.get(0).ok()?;
    if rows.next().ok()?.is_some() { return None; }
    let matches: Vec<_> = bytes.split(|b| !b.is_ascii_hexdigit()).filter(|s| s.len() == 32).collect();
    if matches.len() != 1 { return None; }
    Some(String::from_utf8(matches[0].to_ascii_lowercase()).ok()?)
}

#[cfg(test)]
fn prepare_upload(
    cache: &Path,
    account_id: &str,
    space_id: &str,
    base_url: &str,
    usernames: &[String],
    since: Option<i64>,
    until: Option<i64>,
    message_selection: &BTreeMap<String, Vec<settings::MessageRef>>,
) -> Result<PreparedUpload, String> {
    let defaults = settings::VoiceUploadSettings::default();
    prepare_upload_with_voice(
        cache,
        cache,
        cache,
        account_id,
        space_id,
        base_url,
        usernames,
        since,
        until,
        message_selection,
        &defaults,
        "images",
        &LegacyUploadKeys::new(),
    )
}

/// Total attachment payload the ``all`` scope is allowed to put in one archive. The server
/// accepts far more; this only keeps a month of big group files from exhausting memory.
const ATTACHMENT_TOTAL_LIMIT_BYTES: u64 = 32 * 1024 * 1024;

/// What one local attachment contributes to the ``all`` scope.
enum Attachment {
    Bytes(Vec<u8>),
    /// Bigger than [`settings::ATTACHMENT_LIMIT_BYTES`]; recorded as ``omitted``.
    TooLarge,
    /// The file is gone from this machine; recorded as ``missing``.
    Missing,
}

fn read_attachment(cache: &Path, item: &media::MediaItem) -> Attachment {
    let Some(rel) = item.rel_path.as_deref() else {
        return Attachment::Missing;
    };
    let path = cache.join(rel);
    let Ok(meta) = std::fs::metadata(&path) else {
        return Attachment::Missing;
    };
    if meta.len() > settings::ATTACHMENT_LIMIT_BYTES {
        return Attachment::TooLarge;
    }
    match std::fs::read(&path) {
        Ok(bytes) if !bytes.is_empty() => Attachment::Bytes(bytes),
        _ => Attachment::Missing,
    }
}

/// Extension for a media member, restricted to characters that are safe in a file name.
fn attachment_extension(name: Option<&str>) -> String {
    let extension = name
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, extension)| extension)
        .filter(|extension| {
            !extension.is_empty()
                && extension.len() <= 8
                && extension.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .unwrap_or("bin");
    extension.to_lowercase()
}

#[cfg(test)]
fn prepare_upload_with_voice(
    app_data: &Path,
    cache: &Path,
    media_root: &Path,
    account_id: &str,
    space_id: &str,
    base_url: &str,
    usernames: &[String],
    since: Option<i64>,
    until: Option<i64>,
    message_selection: &BTreeMap<String, Vec<settings::MessageRef>>,
    voice_settings: &settings::VoiceUploadSettings,
    attachments: &str,
    legacy_keys: &LegacyUploadKeys,
) -> Result<PreparedUpload, String> {
    prepare_upload_scoped(app_data,cache,media_root,account_id,space_id,base_url,usernames,since,until,message_selection,voice_settings,attachments,legacy_keys,app_data)
}

fn prepare_upload_scoped(
    app_data: &Path,
    cache: &Path,
    media_root: &Path,
    account_id: &str,
    space_id: &str,
    base_url: &str,
    usernames: &[String],
    since: Option<i64>,
    until: Option<i64>,
    message_selection: &BTreeMap<String, Vec<settings::MessageRef>>,
    voice_settings: &settings::VoiceUploadSettings,
    attachments: &str,
    legacy_keys: &LegacyUploadKeys,
    private_data: &Path,
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
    let mut selected = query::sessions_filtered(cache,Some(&usernames.iter().cloned().collect()))?
        .into_iter()
        .filter(|s| unique.contains(&s.username))
        .collect::<Vec<_>>();
    if selected.len() != usernames.len() {
        return Err("部分所选会话已不可用，请重新选择。".into());
    }
    selected.sort_by(|a, b| a.username.cmp(&b.username));
    // The server uses this key to tell the user's own messages apart.
    let self_key = query::self_username(cache).unwrap_or_default();
    let mut conversations = Vec::new();
    let mut messages = Vec::new();
    let mut parts = Vec::new();
    let mut media_files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut voice_pending: BTreeMap<String, Vec<settings::VoiceRef>> = BTreeMap::new();
    let mut voice_processed: BTreeMap<String, Vec<settings::VoiceRef>> = BTreeMap::new();
    let mut media_paths = HashSet::new();
    let mut media_budget = 0u64;
    // "仅文本" keeps message text and voice transcripts but no attachment bytes; "全部"
    // adds video, files and voice audio next to the pictures.
    let include_images = attachments != "text";
    let include_files = attachments == "all";
    let mut summary = Vec::new();
    let mut latest_position: Option<(i64, i64)> = None;
    let mut covered: BTreeMap<String, settings::SyncCursor> = BTreeMap::new();
    let voice_language = voice::load_settings(app_data).language;
    for session in &selected {
        conversations.push(serde_json::json!({"conversation_id":session.username,"title":session.display,"kind":if session.is_group {"group"} else {"direct"},"self_sender_key":self_key}));
        let mut session_messages = if let Some(refs)=message_selection.get(&session.username) {query::messages_selected(cache,&session.username,refs)?}
            else {collect_upload_messages(cache,&session.username,since,until,Some(5000))?};
        session_messages.retain(|m|since.is_none_or(|s|m.time>=s) && until.is_none_or(|u|m.time<u));
        session_messages = filter_upload_messages(
            session_messages,
            message_selection.get(&session.username).map(Vec::as_slice),
        )?;
        let inventory = query::local_id_inventory_for(cache, &session.username, Some(&session_messages.iter().map(|m|m.local_id).collect::<Vec<_>>()))?;
        let server_counts = query::server_id_counts_for(cache, &session.username, Some(&session_messages.iter().map(|m|m.server_id).collect::<Vec<_>>()))?;
        let ambiguous_local_ids: HashSet<i64> = inventory.iter().filter_map(|(id, times)| (times.values().sum::<usize>() > 1).then_some(*id)).collect();
        let legacy = legacy_keys.get(&session.username);
        let mut message_keys = HashMap::new();
        let mut unique_keys = HashSet::new();
        for message in &session_messages {
            let occurrences = inventory.get(&message.local_id).ok_or("无法核对消息编号。")?;
            let total: usize = occurrences.values().sum();
            let previous = legacy.and_then(|items| items.legacy.get(&message.local_id));
            let stable_key = if message.server_id != 0 {
                format!("srv:{}", message.server_id)
            } else {
                format!("local:{}:{}:{}", message.source_shard, message.local_id, message.time)
            };
            let use_legacy = match previous {
                Some(Some(time)) if *time == message.time => {
                    if occurrences.get(time).copied().unwrap_or_default() != 1 {
                        return Err("旧消息与多个本地记录匹配，无法安全上传。".into());
                    }
                    true
                }
                Some(None) => return Err("云端旧消息缺少时间，无法安全匹配。".into()),
                Some(Some(_)) => false,
                None => total == 1 && !legacy.is_some_and(|items| items.stable.contains(&stable_key)),
            };
            let key = if use_legacy {
                format!("src:{}", message.local_id)
            } else {
                if message.server_id != 0 && server_counts.get(&message.server_id).copied().unwrap_or_default() != 1 {
                    return Err("微信服务端消息编号存在重复，无法安全上传。".into());
                }
                stable_key
            };
            if !unique_keys.insert(key.clone()) {
                return Err("消息的新标识仍有冲突，已停止上传以保护数据。".into());
            }
            message_keys.insert((message.source_shard.clone(), message.local_id, message.time), key);
        }
        session_messages.sort_by(|a, b| a.time.cmp(&b.time).then(a.local_id.cmp(&b.local_id)));
        if let Some(last) = session_messages.last() {
            covered.insert(
                session.username.clone(),
                settings::SyncCursor {
                    time: last.time,
                    local_id: last.local_id,
                    boundary_count: Some(session_messages.iter().filter(|message| message.time == last.time).count()),
                },
            );
        }
        summary.push(UploadPreviewSession {
            username: session.username.clone(),
            display: session.display.clone(),
            message_count: session_messages.len(),
        });
        let selected_messages = session_messages.clone();
        for (index, message) in session_messages.into_iter().enumerate() {
            latest_position = Some(
                latest_position.map_or((message.time, message.local_id), |current| {
                        current.max((message.time, message.local_id))
                    }),
            );
            let message_key = message_keys.get(&(message.source_shard.clone(), message.local_id, message.time)).ok_or("无法匹配消息标识。")?.clone();
            messages.push(serde_json::json!({"conversation_id":session.username,"message_key":message_key,"source_message_id":message.local_id.to_string(),"sender_key":message.sender,"sender_name":message.sender_name,"direction":if message.is_self {"outgoing"} else {"incoming"},"occurred_at":chrono::DateTime::from_timestamp(message.time,0).map(|v|v.to_rfc3339()),"source_order":index as i64 + 1,"record_kind":"message","source_fields":{"kind":message.kind}}));
            if message.kind == 34 {
                parts.push(serde_json::json!({
                    "conversation_id": session.username,
                    "message_key": message_key,
                    "part_key": format!("voice-{}", message.local_id),
                    "ordinal": 0,
                    "kind": "audio",
                    "binding_state": "missing",
                    "source_locator": {"source": "wechat_voice", "local_id": message.local_id},
                }));
            } else {
            parts.push(serde_json::json!({"conversation_id":session.username,"message_key":message_key,"part_key":"text-0","ordinal":0,"kind":"text","text":message.text}));
        }
        }

        // Keep the original image bytes when WeChat has them locally. The
        // resolver picks the best available source (original, bubble, thumb)
        // and decodes encrypted `.dat` files before they enter the archive.
        let raw = raw_messages_for_upload(cache, &session.username, &selected_messages)?;
        let keys = media::resolve_image_keys(media_root, &query::md5_hex(&session.username));
        let report = media::resolve(media_root, &session.username, &raw, keys.as_ref());
        if include_images {
        for item in report.items.iter().filter(|item| item.kind == "image") {
            let resource_hash = ambiguous_local_ids.contains(&item.local_id).then(|| image_resource_hash(cache, item)).flatten();
            let sources = item
                .sources
                .iter()
                .filter(|path| {
                    if !ambiguous_local_ids.contains(&item.local_id) { return true; }
                    let Some(raw) = raw.iter().find(|m| m.local_id == item.local_id && m.time == item.time && m.source_shard == item.source_shard) else { return false; };
                    let name = Path::new(path).file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
                    let exact_prefix = format!("{}_{}_", item.local_id, item.time);
                    if name.starts_with(&exact_prefix) { return true; }
                    let hash = name.split('_').next().unwrap_or_default().split('.').next().unwrap_or_default();
                    if resource_hash.as_deref() == Some(hash) { return true; }
                    hash.len() == 32 && hash.chars().all(|c| c.is_ascii_hexdigit()) && raw.content.to_lowercase().contains(hash)
                })
                .map(|path| media_root.join(path))
                .collect::<Vec<_>>();
            let message_key = message_keys.get(&(item.source_shard.clone(), item.local_id, item.time)).ok_or("无法匹配图片所属消息。")?.clone();
            let decoded = media::decode_for_upload(media_root, &sources).ok();
            if let Some((bytes, kind)) = decoded.filter(|(bytes,_)|media_budget.saturating_add(bytes.len() as u64)<=ATTACHMENT_TOTAL_LIMIT_BYTES) {
                media_budget+=bytes.len() as u64;
                let digest = hex::encode(sha2::Sha256::digest(&bytes));
                let extension = if kind == "png" { "png" } else { "jpg" };
                let path = format!("media/images/{digest}.{extension}");
                if media_paths.insert(path.clone()) {
                    media_files.push((path.clone(), bytes));
                }
                parts.push(serde_json::json!({
                    "conversation_id": session.username,
                    "message_key": message_key,
                    "part_key": format!("image-{}", item.local_id),
                    "ordinal": 1,
                    "kind": "image",
                    "upload_item_path": path,
                }));
            } else {
                parts.push(serde_json::json!({
                    "conversation_id": session.username,
                    "message_key": message_key,
                    "part_key": format!("image-{}", item.local_id),
                    "ordinal": 1,
                    "kind": "image",
                    "binding_state": "missing",
                }));
            }
        }
        }

        // "全部（≤ 25 MB）" also carries video and file attachments. Anything that is gone
        // from this machine, or bigger than the ceiling, stays in the manifest as a
        // missing/omitted part so the server still knows the message had an attachment.
        if include_files {
            for item in report
                .items
                .iter()
                .filter(|item| item.kind == "video" || item.kind == "file")
            {
                let mut part = serde_json::json!({
                    "conversation_id": session.username,
                    "message_key": message_keys.get(&(item.source_shard.clone(), item.local_id, item.time)).ok_or("无法匹配附件所属消息。")?,
                    "part_key": format!("{}-{}", item.kind, item.local_id),
                    "ordinal": 1,
                    "kind": item.kind,
                });
                let attachment = if ambiguous_local_ids.contains(&item.local_id) { Attachment::Missing } else { read_attachment(media_root, item) };
                match attachment {
                    Attachment::Bytes(bytes)
                        if media_budget + bytes.len() as u64
                            <= ATTACHMENT_TOTAL_LIMIT_BYTES =>
                    {
                        media_budget += bytes.len() as u64;
                        let digest = hex::encode(sha2::Sha256::digest(&bytes));
                        let extension = attachment_extension(item.name.as_deref());
                        let path = format!("media/files/{digest}.{extension}");
                        if media_paths.insert(path.clone()) {
                            media_files.push((path.clone(), bytes));
                        }
                        part["upload_item_path"] = serde_json::json!(path);
                    }
                    Attachment::Bytes(_) | Attachment::TooLarge => {
                        part["binding_state"] = serde_json::json!("omitted");
                    }
                    Attachment::Missing => {
                        part["binding_state"] = serde_json::json!("missing");
                    }
                }
                parts.push(part);
            }
        }

        let voice_items: Vec<_> = report
            .items
            .iter()
            .filter(|item| item.kind == "voice")
            .collect();
        if !voice_items.is_empty() {
            let ids: Vec<i64> = voice_items.iter().map(|item| item.server_id).collect();
            let blobs = voice::read_voice_blobs(cache, &ids).unwrap_or_default();
            let mut transcripts = voice::load_transcripts(private_data);
            let mut transcripts_changed = false;
            let engine_ready =
                voice_settings.auto_transcribe && voice::view(app_data).selected.is_some();
            for item in voice_items {
                let voice_ref = settings::VoiceRef {
                    local_id: item.local_id,
                    source_shard: item.source_shard.clone(),
                    time: selected_messages
                        .iter()
                        .find(|message| message.local_id == item.local_id && message.time == item.time && message.source_shard == item.source_shard)
                        .map(|message| message.time)
                        .unwrap_or_default(),
                    server_id: item.server_id,
                };
                voice_processed
                    .entry(session.username.clone())
                    .or_default()
                    .push(voice_ref.clone());
                if item.server_id == 0 {
                    voice_pending.entry(session.username.clone()).or_default().push(voice_ref);
                    continue;
                }
                let key = voice::transcript_key(account_id, item.server_id);
                let mut transcript = transcripts.get(&key).cloned();
                if transcript.is_none() && engine_ready {
                    if let Some(blob) = blobs.get(&item.server_id) {
                        if let Ok((wav, _)) =
                            voice::ensure_wav(private_data, account_id, item.server_id, blob)
                        {
                            if let Ok((text, engine, model)) =
                                voice::transcribe_file(app_data, &wav, 300)
                            {
                                let entry = voice::TranscriptEntry {
                                    text,
                                    engine,
                                    model,
                                    created_at: voice::now_secs(),
                                };
                                transcripts.insert(key.clone(), entry.clone());
                                transcript = Some(entry);
                                transcripts_changed = true;
                            }
                        }
                    }
                }
                let has_transcript = transcript
                    .as_ref()
                    .is_some_and(|entry| !entry.text.trim().is_empty());
                if voice_settings.upload_transcripts && has_transcript {
                    if let Some(entry) = transcript {
                        parts.push(serde_json::json!({
                        "conversation_id": session.username,
                        "message_key": message_keys.get(&(item.source_shard.clone(), item.local_id, item.time)).ok_or("无法匹配语音所属消息。")?,
                        "part_key": format!("voice-transcript-{}", item.server_id),
                        "ordinal": 1,
                        "kind": "text",
                        "text": entry.text,
                        "source_locator": {"source": "wechat_voice_transcript", "server_id": item.server_id, "local_id": item.local_id},
                        "source_header": {"engine": entry.engine, "model": entry.model, "language": voice_language},
                    }));
                    }
                }
                if !has_transcript {
                    voice_pending
                        .entry(session.username.clone())
                        .or_default()
                        .push(voice_ref);
                }
                // "全部" sends the decoded voice as an audio member; the part record above
                // already carries the same key, and a later record for a key wins.
                if include_files {
                    if let Some(blob) = blobs.get(&item.server_id) {
                        let wav = voice::ensure_wav(private_data, account_id, item.server_id, blob);
                        if let Ok((wav, _)) = wav {
                            if let Ok(bytes) = std::fs::read(&wav) {
                                if !bytes.is_empty()
                                    && bytes.len() as u64 <= settings::ATTACHMENT_LIMIT_BYTES
                                    && media_budget + bytes.len() as u64
                                        <= ATTACHMENT_TOTAL_LIMIT_BYTES
                                {
                                    media_budget += bytes.len() as u64;
                                    let digest = hex::encode(sha2::Sha256::digest(&bytes));
                                    let path = format!("media/audio/{digest}.wav");
                                    if media_paths.insert(path.clone()) {
                                        media_files.push((path.clone(), bytes));
                                    }
                                    parts.push(serde_json::json!({
                                        "conversation_id": session.username,
                                        "message_key": message_keys.get(&(item.source_shard.clone(), item.local_id, item.time)).ok_or("无法匹配语音所属消息。")?,
                                        "part_key": format!("voice-{}", item.local_id),
                                        "ordinal": 0,
                                        "kind": "audio",
                                        "upload_item_path": path,
                                        "source_locator": {"source": "wechat_voice", "local_id": item.local_id},
                                    }));
                                }
                            }
                        }
                    }
                }
            }
            if transcripts_changed {
                let _ = voice::save_transcripts(private_data, &transcripts);
            }
        }
    }
    if messages.is_empty() {
        return Err("所选会话在时间范围内没有可上传的消息。".into());
    }
    let mut avatar_names: Vec<String> = selected
        .iter()
        .map(|session| session.username.clone())
        .collect();
    for message in &messages {
        if let Some(key) = message.get("sender_key").and_then(|value| value.as_str()) {
            if !key.is_empty() {
                avatar_names.push(key.to_string());
            }
        }
    }
    avatar_names.sort();
    avatar_names.dedup();
    let mut avatars = Vec::new();
    let mut avatar_files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut written_paths = HashSet::new();
    for image in wechat::avatar::images(cache, &avatar_names) {
        let extension = match image.content_type {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            _ => "webp",
        };
        let digest = hex::encode(sha2::Sha256::digest(&image.bytes));
        let path = format!("media/avatars/{digest}.{extension}");
        if written_paths.insert(path.clone()) {
            avatar_files.push((path.clone(), image.bytes));
        }
        if selected
            .iter()
            .any(|session| session.username == image.username)
        {
            avatars.push(serde_json::json!({
                "scope": "conversation",
                "conversation_id": image.username,
                "upload_item_path": path.clone(),
            }));
        }
        if messages.iter().any(|message| {
            message.get("sender_key").and_then(|value| value.as_str())
                == Some(image.username.as_str())
        }) {
            avatars.push(serde_json::json!({
                "scope": "sender",
                "sender_key": image.username,
                "upload_item_path": path,
            }));
        }
    }
    let fingerprint = upload_fingerprint(
        &serde_json::json!({"account":account_id,"space":space_id,"base_url":base_url.trim_end_matches('/'),"since":since,"until":until,"conversations":conversations,"messages":messages,"parts":parts,"avatars":avatars,"media_files":media_files}),
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
        avatars,
        avatar_files,
        media_files,
        voice_pending,
        voice_processed,
        latest_position,
        covered,
    })
}


/// WhatsApp counterpart of [`prepare_upload_with_voice`]'s data-collection half.
///
/// Only the collection step differs: the archive envelope is the same JSONL set, so batch
/// creation, chunking, retry, coverage and the preview UI stay shared. WhatsApp media and
/// voice are not packaged yet; a text label ("[图片] 报价.png") is uploaded instead, which
/// keeps the transcript readable until attachment mapping lands.
fn prepare_whatsapp_upload(
    app_data: &Path,
    account_id: &str,
    space_id: &str,
    base_url: &str,
    usernames: &[String],
    since: Option<i64>,
    until: Option<i64>,
    message_selection: &BTreeMap<String, Vec<settings::MessageRef>>,
) -> Result<PreparedUpload, String> {
    let unique: HashSet<&String> = usernames.iter().collect();
    if unique.len() != usernames.len() {
        return Err("会话列表包含重复项。".into());
    }
    if message_selection.keys().any(|name| !unique.contains(name)) {
        return Err("消息选择包含未选择的会话。".into());
    }
    let selected = whatsapp::upload_conversations(app_data, usernames, since, until, message_selection)?;
    let mut conversations = Vec::new();
    let mut messages = Vec::new();
    let mut parts = Vec::new();
    let mut summary = Vec::new();
    let mut latest_position: Option<(i64, i64)> = None;
    let mut covered: BTreeMap<String, settings::SyncCursor> = BTreeMap::new();
    for conversation in &selected {
        let session = &conversation.session;
        conversations.push(serde_json::json!({
            "conversation_id": session.username,
            "title": session.display,
            "kind": if session.is_group { "group" } else { "direct" },
            // The server uses this key to tell the user's own messages apart.
            "self_sender_key": account_id,
        }));
        summary.push(UploadPreviewSession {
            username: session.username.clone(),
            display: session.display.clone(),
            message_count: conversation.entries.len(),
        });
        if let Some((_, last)) = conversation.entries.last() {
            covered.insert(
                session.username.clone(),
                settings::SyncCursor {
                    time: last.time,
                    local_id: last.local_id,
                    boundary_count: Some(conversation.entries.iter().filter(|(_, item)| item.time == last.time).count()),
                },
            );
        }
        for (index, (message_key, message)) in conversation.entries.iter().enumerate() {
            latest_position = Some(
                latest_position.map_or((message.time, message.local_id), |current| {
                    current.max((message.time, message.local_id))
                }),
            );
            let sender_key = message
                .sender
                .clone()
                .or_else(|| message.is_self.then(|| account_id.to_string()));
            messages.push(serde_json::json!({
                "conversation_id": session.username,
                "message_key": message_key,
                "source_message_id": message.local_id.to_string(),
                "sender_key": sender_key,
                "sender_name": message.sender_name,
                "direction": if message.is_self { "outgoing" } else { "incoming" },
                "occurred_at": chrono::DateTime::from_timestamp(message.time, 0).map(|value| value.to_rfc3339()),
                "source_order": index as i64 + 1,
                "record_kind": "message",
                "source_fields": {"kind": message.kind},
            }));
            parts.push(serde_json::json!({
                "conversation_id": session.username,
                "message_key": message_key,
                "part_key": "text-0",
                "ordinal": 0,
                "kind": "text",
                "text": message.text,
            }));
        }
    }
    if messages.is_empty() {
        return Err("所选会话在时间范围内没有可上传的消息。".into());
    }
    let fingerprint = upload_fingerprint(
        &serde_json::json!({"account":account_id,"space":space_id,"base_url":base_url.trim_end_matches('/'),"since":since,"until":until,"conversations":conversations,"messages":messages,"parts":parts,"avatars":Vec::<serde_json::Value>::new(),"media_files":Vec::<serde_json::Value>::new()}),
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
        avatars: Vec::new(),
        avatar_files: Vec::new(),
        media_files: Vec::new(),
        voice_pending: BTreeMap::new(),
        voice_processed: BTreeMap::new(),
        latest_position,
        covered,
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
    space_id: &str,
    usernames: &[String],
) -> Result<(), String> {
    validate_account_id(account_id)?;
    if space_id.trim().is_empty() || settings.devify.active_space_id.as_deref() != Some(space_id) {
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
async fn wechat_collection_dates(app: AppHandle, account_id: String, usernames: Vec<String>) -> Result<BTreeMap<String, Vec<(String, usize)>>, String> {
    if whatsapp::is_account(&account_id) {
        let allowed = whatsapp_allowed_usernames(&app, &account_id)?;
        if usernames.iter().any(|name| !allowed.contains(name)) { return Err("会话不在已选范围中。".into()); }
        let data = app_data(&app);
        return tauri::async_runtime::spawn_blocking(move || {
            usernames.into_iter().map(|name| whatsapp::message_dates(&data, &name).map(|dates| (name, dates))).collect()
        }).await.map_err(|e| format!("读取日期失败：{e}"))?;
    }
    let allowed = allowed_usernames(&app, &account_id)?;
    if usernames.iter().any(|name| !allowed.contains(name)) { return Err("会话不在已选范围中。".into()); }
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    tauri::async_runtime::spawn_blocking(move || usernames.into_iter().map(|name| query::message_dates(&cache, &name).map(|dates| (name, dates))).collect()).await.map_err(|e| format!("读取日期失败：{e}"))?
}

#[tauri::command]
async fn wechat_catalog_dates(app: AppHandle, account_id: String, username: String) -> Result<Vec<(String, usize)>, String> {
    validate_account_id(&account_id)?;
    if whatsapp::is_account(&account_id) {
        let data = app_data(&app);
        return tauri::async_runtime::spawn_blocking(move || whatsapp::message_dates(&data, &username))
            .await
            .map_err(|error| format!("读取日期失败：{error}"))?;
    }
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    tauri::async_runtime::spawn_blocking(move || {
        if !query::is_chat_session(&cache, &username) { return Err("该会话不可用。".into()); }
        query::message_dates(&cache, &username)
    }).await.map_err(|e| format!("读取日期失败：{e}"))?
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
    if whatsapp::is_account(&account_id) {
        let allowed = whatsapp_allowed_usernames(&app, &account_id)?;
        if !allowed.contains(&username) {
            return Err("该会话不在白名单中。".into());
        }
        let data = app_data(&app);
        return tauri::async_runtime::spawn_blocking(move || {
            whatsapp::messages_between(&data, &username, since, until, 5000)
        })
        .await
        .map_err(|error| format!("读取消息失败：{error}"))?;
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
    space_id: String,
    usernames: Vec<String>,
    since: Option<i64>,
    until: Option<i64>,
    message_selection: Option<BTreeMap<String, Vec<settings::MessageRef>>>,
) -> Result<UploadPreview, String> {
    let data = app_data(&app);
    let upload_epoch_at_start = upload_epoch();
    let settings = settings::load(&data)?;
    validate_upload_scope(&settings, &account_id, &space_id, &usernames)?;
    entitlements::require(&data, &settings.devify.base_url, capabilities::INGEST_UPLOAD)?;
    let (owner, token) = verified_upload_identity(&data, &account_id, &space_id).await?;
    let latest = settings::load(&data)?;
    validate_owner_state(&latest, &owner, &token)?;
    validate_upload_scope(&latest, &account_id, &space_id, &usernames)?;
    if desktop_credentials::load(&data, &owner.base_url)?.access != token {
        return Err("登录状态已改变，请重新确认采集任务。".into());
    }
    let base_url = owner.base_url.clone();
    let selection = message_selection.unwrap_or_default();
    let data_for_prepare = data.clone();
    let whatsapp_source = whatsapp::is_account(&account_id);
    let preview = if whatsapp_source {
        tauri::async_runtime::spawn_blocking(move || {
            prepare_whatsapp_upload(
                &data_for_prepare,
                &account_id,
                &space_id,
                &base_url,
                &usernames,
                since,
                until,
                &selection,
            )
            .map(|prepared| prepared.preview)
        })
        .await
        .map_err(|e| format!("读取上传预览失败：{e}"))??
    } else {
        let cache = decrypt::cache_dir(&data, &account_id);
        let media_root = account_root(&account_id).unwrap_or_else(|_| cache.clone());
        let voice_settings = latest.voice.clone();
        let attachments = latest.backup.attachments.clone();
        let private_data=local_scope::root(&data);
        let legacy_keys = load_legacy_upload_keys(&app, &space_id, &account_id, &usernames, Some((&owner.base_url, &token)), upload_epoch_at_start).await?;
        tauri::async_runtime::spawn_blocking(move || {
            prepare_upload_scoped(
                &data_for_prepare,
                &cache,
                &media_root,
                &account_id,
                &space_id,
                &base_url,
                &usernames,
                since,
                until,
                &selection,
                &voice_settings,
                &attachments,
                &legacy_keys,
                &private_data,
            )
            .map(|prepared| prepared.preview)
        })
        .await
        .map_err(|e| format!("读取上传预览失败：{e}"))??
    };
    validate_owner_state(&settings::load(&data)?, &owner, &token)?;
    if desktop_credentials::load(&data, &owner.base_url)?.access != token {
        return Err("登录状态已改变，请重新确认采集任务。".into());
    }
    Ok(preview)
}

/// Merge an upload outcome into the persisted task without restoring the pre-POST snapshot.
fn apply_manual_upload_outcome(
    item: &mut settings::UploadJob,
    expected: &settings::UploadJob,
    result: &Result<DevifyUploadResult, String>,
    success_message: &str,
) -> Result<(), String> {
    if item.id != expected.id || item.owner.is_none() || item.owner != expected.owner || item.scope != expected.scope {
        return Err("任务身份或上传范围已改变，无法保存结果。".into());
    }
    if let Ok(value) = result {
        if item.batch_id != Some(value.batch_id) || item.create_started != Some(true) {
            return Err("本机批次状态已改变，无法保存结果。".into());
        }
    }
    // Reconciliation can report a terminal state before this request returns. Do not
    // overwrite that later observation with an older request's success or failure.
    let later_terminal = matches!(item.status.as_str(), "completed" | "cancelled")
        || (item.status == "failed" && (expected.status != "failed" || item.updated_at != expected.updated_at));
    if later_terminal {
        if matches!(item.status.as_str(), "completed" | "cancelled")
            && (item.batch_id.is_none() || item.create_started != Some(true)) {
            return Err("已结束任务缺少可核验的批次状态。".into());
        }
        return Ok(());
    }
    item.updated_at = chrono::Utc::now().to_rfc3339();
    match result {
        Ok(value) => {
            item.status = "processing".into();
            item.covered = value.covered.clone();
            item.voice_processed = value.voice_processed.clone();
            item.voice_pending = value.voice_pending.clone();
            item.message = success_message.into();
        }
        Err(error) if item.batch_id.is_some() => {
            item.status = "processing".into();
            item.message = format!("上传中断，等待核对服务端批次：{error}");
        }
        Err(error) => {
            item.status = "failed".into();
            item.message = error.clone();
        }
    }
    Ok(())
}

fn mark_manual_batch_failed(app: &AppHandle, job_id: &str, owner: &settings::UploadOwner, token: &str, error: &str) -> Result<(), String> {
    let data = app_data(app);
    settings::update(&data, |value| {
        validate_owner_state(value, owner, token)?;
        if desktop_credentials::load(&data, &owner.base_url)?.access != token {
            return Err("登录状态已改变，不会修改原账号的任务。".into());
        }
        let job = value.upload_queue.iter_mut().find(|job| job.id == job_id)
            .ok_or("找不到上传任务，无法保存失败状态。")?;
        if job.owner.as_ref() != Some(owner) { return Err("任务归属已改变。".into()); }
        if matches!(job.status.as_str(), "completed" | "failed" | "cancelled") { return Ok(()); }
        if job.batch_id.is_some() {
            job.status = "processing".into();
            job.message = format!("上传中断，等待核对服务端批次：{error}");
        } else {
            job.status = "failed".into();
            job.message = error.into();
        }
        job.updated_at = chrono::Utc::now().to_rfc3339();
        Ok(())
    }).map(|_| ())
}

async fn submit_manual_batch(app: &AppHandle, job_id: &str) -> Result<DevifyUploadResult, String> {
    let data = app_data(app);
    let value = settings::load(&data)?;
    let job = value.upload_queue.iter().find(|job| job.id == job_id).cloned().ok_or("找不到上传任务。")?;
    let scope = job.scope.clone().ok_or("找不到上传范围。")?;
    validate_upload_scope(&value, &job.account_id, &job.space_id, &scope.usernames)?;
    if !settings::same_service_url(&scope.base_url, &value.devify.base_url) {
        return Err("服务地址已更改，无法继续原上传任务。".into());
    }
    let (owner, token) = verified_upload_identity(&data, &job.account_id, &job.space_id).await?;
    validate_owned_job(&value, &job, &owner, &token, &scope)?;
    if !matches!(job.status.as_str(), "queued" | "running") || job.batch_id.is_some() || job.create_started != Some(false) {
        return Err("无法确认本机批次尚未提交，请检查服务端记录。".into());
    }
    let preview = devify_upload_preview(app.clone(), job.account_id.clone(), job.space_id.clone(), scope.usernames.clone(), scope.since, scope.until, Some(scope.message_selection.clone())).await?;
    if job.manual_fingerprint.as_deref().is_some_and(|old| old != preview.fingerprint) {
        return Err("本批消息或附件已变化，无法安全重试；请重新预览并创建任务。".into());
    }
    settings::update(&data, |current| {
        let item = current.upload_queue.iter().find(|item| item.id == job_id).ok_or("找不到上传任务。")?;
        validate_owned_job(current, item, &owner, &token, &scope)?;
        if desktop_credentials::load(&data, &owner.base_url)?.access != token
            || !matches!(item.status.as_str(), "queued" | "running")
            || item.batch_id.is_some() || item.create_started != Some(false) {
            return Err("上传任务或登录状态已改变，请重新确认。".into());
        }
        let item = current.upload_queue.iter_mut().find(|item| item.id == job_id).ok_or("找不到上传任务。")?;
        item.status = "running".into();
        item.manual_fingerprint = Some(preview.fingerprint.clone());
        item.message = format!("正在上传第 {}/{} 批", job.manual_index + 1, job.manual_total);
        item.updated_at = chrono::Utc::now().to_rfc3339();
        Ok(())
    })?;
    let result = devify_upload_wechat(app.clone(), job_id.to_string(), job.space_id.clone(), job.account_id.clone(), scope.usernames.clone(), scope.since, scope.until, Some(scope.base_url.clone()), Some(preview.fingerprint), scope.message_selection.clone(), UploadTrigger::Manual, None).await;
    settings::update(&data, |current| {
        let item = current.upload_queue.iter_mut().find(|item| item.id == job_id).ok_or("找不到上传任务。")?;
        apply_manual_upload_outcome(
            item, &job, &result,
            &format!("第 {}/{} 批已提交服务端处理", job.manual_index + 1, job.manual_total),
        )
    })?;
    result.map(|mut result| { result.job_id = job_id.into(); result })
}

#[tauri::command]
async fn devify_upload_selected(
    app: AppHandle,
    account_id: String,
    space_id: String,
    usernames: Vec<String>,
    since: Option<i64>,
    until: Option<i64>,
    message_selection: Option<BTreeMap<String, Vec<settings::MessageRef>>>,
    fingerprint: String,
    initial_history: Option<bool>,
) -> Result<DevifyUploadResult, String> {
    let upload_state = app.state::<UploadState>();
    let _upload_guard = begin_account_upload(&upload_state, &account_id)?;
    let settings = settings::load(&app_data(&app))?;
    validate_upload_scope(&settings, &account_id, &space_id, &usernames)?;
    entitlements::require(&app_data(&app), &settings.devify.base_url, capabilities::INGEST_UPLOAD)?;
    if fingerprint.len() != 64 || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("请先预览并确认上传范围。".into());
    }
    let message_selection = message_selection.unwrap_or_default();
    let (owner, owner_token) = verified_upload_identity(&app_data(&app), &account_id, &space_id).await?;
    let whatsapp_source = whatsapp::is_account(&account_id);
    let data = app_data(&app);
    let mut initial_baselines = BTreeMap::new();
    if initial_history.unwrap_or(false) {
        if !message_selection.is_empty() { return Err("首次历史上传请选择整段记录。".into()); }
        for username in &usernames {
            if settings.sync.cursors_for_owner(&owner).is_some_and(|items| items.contains_key(username)) {
                return Err("所选会话已有自动同步记录，不能重新设置首次范围。".into());
            }
            if settings.upload_queue.iter().any(|job| upload_job_in_sync_scope(job, &owner) && job.initial_baselines.contains_key(username)) {
                return Err("所选会话已有尚未完成的首次历史上传，请先处理该任务。".into());
            }
            let (time, local_id, boundary_count) = if whatsapp_source {
                let (time, local_id) = whatsapp::latest_position(&data, username)?.ok_or("所选会话没有本地消息。")?;
                (time, local_id, whatsapp::count_at_time(&data, username, time)?)
            } else {
                let cache = decrypt::cache_dir(&data, &account_id);
                let (time, local_id) = query::latest_position(&cache, username)?.ok_or("所选会话没有本地消息。")?;
                (time, local_id, query::count_at_time(&cache, username, time)?)
            };
            initial_baselines.insert(username.clone(), settings::SyncCursor { time, local_id, boundary_count: Some(boundary_count) });
        }
    }
    let mut selected_refs = BTreeMap::new();
    if whatsapp_source {
        for username in &usernames {
            let scope = message_selection
                .get(username)
                .map(|refs| BTreeMap::from([(username.clone(), refs.clone())]))
                .unwrap_or_default();
            let conversations = whatsapp::upload_conversations(&data, std::slice::from_ref(username), since, until, &scope)?;
            let messages = conversations.into_iter().next().map(|item| item.entries).unwrap_or_default();
            selected_refs.insert(username.clone(), messages.into_iter().map(|(_, message)| settings::MessageRef {
                local_id: message.local_id,
                time: message.time,
                source_shard: message.source_shard,
            }).collect());
        }
    } else {
        let cache = decrypt::cache_dir(&data, &account_id);
        for username in &usernames {
            let messages = collect_upload_messages(&cache, username, since, until, None)?;
            let messages = filter_upload_messages(messages, message_selection.get(username).map(Vec::as_slice))?;
            selected_refs.insert(username.clone(), messages.into_iter().map(|m| settings::MessageRef { local_id: m.local_id, time: m.time, source_shard: m.source_shard }).collect());
        }
    }
    let batches = manual_batch_plan(&selected_refs, SYNC_MESSAGES_PER_BATCH)?;
    if batches.len() > 1 {
        let preview = devify_upload_preview(app.clone(), account_id.clone(), space_id.clone(), usernames.clone(), since, until, Some(message_selection.clone())).await?;
        verify_upload_preview(Some(&fingerprint), &preview.fingerprint)?;
        let group = format!("manual-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default());
        let total = batches.len();
        let now = chrono::Utc::now().to_rfc3339();
        let data = app_data(&app);
        let mut pending = Vec::with_capacity(total);
        for (index, selection) in batches.into_iter().enumerate() {
            let names = selection.keys().cloned().collect();
            pending.push(settings::UploadJob {
                id: format!("{group}-{index}"), space_id: space_id.clone(), account_id: account_id.clone(),
                owner: Some(owner.clone()), create_started: Some(false),
                status: "queued".into(), message: format!("等待上传第 {}/{} 批", index + 1, total),
                created_at: now.clone(), updated_at: now.clone(),
                scope: Some(settings::UploadScope { usernames: names, since, until, base_url: settings.devify.base_url.clone(), message_selection: selection }),
                initial_baselines: if index + 1 == total { initial_baselines.clone() } else { BTreeMap::new() },
                manual_group: Some(group.clone()), manual_index: index, manual_total: total,
                ..Default::default()
            });
        }
        settings::update(&data, |current| {
            validate_owner_state(current, &owner, &owner_token)?;
            if desktop_credentials::load(&data, &owner.base_url)?.access != owner_token {
                return Err("登录状态已改变，请重新确认采集任务。".into());
            }
            validate_upload_scope(current, &account_id, &space_id, &usernames)?;
            if pending.iter().any(|job| current.upload_queue.iter().any(|existing| existing.id == job.id)) {
                return Err("上传任务标识重复，请重新创建采集任务。".into());
            }
            current.upload_queue.extend(pending);
            Ok(())
        })?;
        let first_id = format!("{group}-0");
        let result = submit_manual_batch(&app, &first_id).await;
        if let Err(error) = &result {
            mark_manual_batch_failed(&app, &first_id, &owner, &owner_token, error)
                .map_err(|save_error| format!("{error}；保存本机任务失败：{save_error}"))?;
        }
        return result;
    }
    let scope = settings::UploadScope {
        usernames: usernames.clone(),
        since,
        until,
        base_url: settings.devify.base_url.clone(),
        message_selection: message_selection.clone(),
    };
    let job_id = format!("upload-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default());
    let now = chrono::Utc::now().to_rfc3339();
    let job = settings::UploadJob {
        id: job_id.clone(),
        space_id: space_id.clone(),
        account_id: account_id.clone(),
        batch_id: None,
        owner: Some(owner.clone()),
        create_started: Some(false),
        create_key: None,
        package_digest: None,
        package_client_version: None,
        verified_completion_digest: None,
        legacy_failure_confirmed: false,
        status: "running".into(),
        message: "正在生成并上传聊天包".into(),
        created_at: now.clone(),
        updated_at: now,
        scope: Some(scope),
        covered: BTreeMap::new(),
        initial_baselines,
        initial_chunks: BTreeMap::new(),
        sync_advance: false,
        continue_sync: false,
        continuation_manual: None,
        manual_group: None,
        manual_index: 0,
        manual_total: 0,
        manual_fingerprint: None,
        voice_processed: BTreeMap::new(),
        voice_pending: BTreeMap::new(),
    };
    let data = app_data(&app);
    settings::update(&data, |current| {
        validate_owner_state(current, &owner, &owner_token)?;
        if desktop_credentials::load(&data, &owner.base_url)?.access != owner_token {
            return Err("登录状态已改变，请重新确认采集任务。".into());
        }
        validate_upload_scope(current, &account_id, &space_id, &usernames)?;
        if current.upload_queue.iter().any(|existing| existing.id == job.id) {
            return Err("上传任务标识重复，请重新创建采集任务。".into());
        }
        current.upload_queue.push(job.clone());
        Ok(())
    })?;
    let upload_account_id = account_id.clone();
    let upload_usernames = usernames.clone();
    let result = devify_upload_wechat(
        app.clone(),
        job_id.clone(),
        space_id,
        upload_account_id,
        upload_usernames,
        since,
        until,
        Some(settings.devify.base_url.clone()),
        Some(fingerprint),
        message_selection,
        UploadTrigger::Manual,
        None,
    )
    .await;
    settings::update(&data, |current| {
        let item = current.upload_queue.iter_mut().find(|item| item.id == job_id)
            .ok_or("找不到上传任务，无法保存结果。")?;
        apply_manual_upload_outcome(item, &job, &result, "已提交服务端解析")
    }).map_err(|save_error| match &result {
        Err(error) => format!("{error}；保存本机任务失败：{save_error}"),
        Ok(_) => save_error,
    })?;
    result.map(|mut value| { value.job_id = job_id; value })
}

#[tauri::command]
async fn devify_upload_retry_preview(
    app: AppHandle,
    job_id: String,
) -> Result<UploadPreview, String> {
    let data = app_data(&app);
    let upload_epoch_at_start = upload_epoch();
    let settings = settings::load(&data)?;
    let job = settings
        .upload_queue
        .iter()
        .find(|item| item.id == job_id)
        .cloned()
        .ok_or_else(|| "找不到上传任务。".to_string())?;
    let scope = job.retry_scope(&settings)?.clone();
    if job.owner.is_none() || (job.batch_id.is_none() && job.create_started != Some(false) && !job.can_recover_create()) {
        return Err("无法确认原任务的账号或服务端批次，请核对后重新创建采集任务。".into());
    }
    if job.batch_id.is_some() && job.package_digest.is_none() {
        return Err("原任务没有保存归档摘要，无法安全重试，请重新创建采集任务。".into());
    }
    let (owner, token) = verified_upload_identity(&data, &job.account_id, &job.space_id).await?;
    validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
    let latest = settings::load(&data)?;
    let account_id = job.account_id.clone();
    let data_for_prepare = data.clone();
    let scope_for_prepare = scope.clone();
    let space_id = job.space_id.clone();
    let preview = if whatsapp::is_account(&account_id) {
        tauri::async_runtime::spawn_blocking(move || {
            prepare_whatsapp_upload(
                &data_for_prepare,
                &account_id,
                &space_id,
                &scope_for_prepare.base_url,
                &scope_for_prepare.usernames,
                scope_for_prepare.since,
                scope_for_prepare.until,
                &scope_for_prepare.message_selection,
            )
            .map(|prepared| prepared.preview)
        })
        .await
        .map_err(|e| format!("读取上传预览失败：{e}"))??
    } else {
        let cache = decrypt::cache_dir(&data, &job.account_id);
        let voice_settings = latest.voice.clone();
        let attachments = latest.backup.attachments.clone();
        let media_root = account_root(&account_id).unwrap_or_else(|_| cache.clone());
        let private_data=local_scope::root(&data);
        let legacy_keys = load_legacy_upload_keys(&app, &job.space_id, &account_id, &scope.usernames, Some((&owner.base_url, &token)), upload_epoch_at_start).await?;
        tauri::async_runtime::spawn_blocking(move || {
            prepare_upload_scoped(
                &data_for_prepare,
                &cache,
                &media_root,
                &account_id,
                &space_id,
                &scope_for_prepare.base_url,
                &scope_for_prepare.usernames,
                scope_for_prepare.since,
                scope_for_prepare.until,
                &scope_for_prepare.message_selection,
                &voice_settings,
                &attachments,
                &legacy_keys,
                &private_data,
            )
            .map(|prepared| prepared.preview)
        })
        .await
        .map_err(|e| format!("读取上传预览失败：{e}"))??
    };
    validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
    Ok(preview)
}

// The token is deliberately never serialized, formatted, or logged.
struct CloudRetryAuth {
    token: String,
    base_url: String,
    job_id: String,
    space_id: String,
    account_id: String,
    batch_id: i64,
    device_id: String,
    user_id: i64,
    idempotency_key: String,
    scope: settings::UploadScope,
}

fn cloud_retry_device_id(configured: &str, token: &str) -> Result<String, String> {
    let claim = access_token_claim(token, "device_id");
    let device_id = if configured.is_empty() { claim.as_deref().unwrap_or("") } else { configured };
    if device_id.is_empty() || device_id.trim() != device_id
        || claim.as_deref().is_some_and(|value| value != device_id)
    {
        return Err("无法确认当前设备，不能重试云端批次。".into());
    }
    Ok(device_id.to_string())
}

async fn verified_upload_identity(data: &Path, account_id: &str, space_id: &str) -> Result<(settings::UploadOwner, String), String> {
    // Cache only verified principal/device registration, keyed by token and local store.
    // The shared gate coalesces simultaneous startup and task requests.
    static VERIFY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    static CACHE: std::sync::Mutex<Option<(PathBuf,String,String,i64,String,std::time::Instant)>> = std::sync::Mutex::new(None);
    let _single=VERIFY.lock().await;
    let value=settings::load(data)?;
    let base_url=upload_list_base_url(&value,space_id)?;
    let token=devify_access_token_async(data,&base_url).await?;
    let device_id=cloud_retry_device_id(&value.devify.device_id,&token)?;
    let cached=CACHE.lock().ok().and_then(|c|if cfg!(test) {None} else {c.clone()}).filter(|(path,base,access,_,device,at)|
        path==data && settings::same_service_url(base,&base_url) && access==&token && device==&device_id && at.elapsed()<Duration::from_secs(120));
    let user_id=if let Some((_,_,_,id,_,_))=cached {id} else {
        let (user,devices)=tokio::try_join!(fixed_identity_get(data,&base_url,&token,"/api/v1/auth/user"),fixed_identity_get(data,&base_url,&token,"/api/v1/auth/desktop/devices"))?;
        let user_id=user.get("id").and_then(serde_json::Value::as_i64).filter(|id|*id>0).ok_or("无法确认云端用户。")?;
        let items=devices.as_array().or_else(||devices.get("list").and_then(serde_json::Value::as_array)).ok_or("无法确认当前设备登记。")?;
        if items.iter().filter(|d|d.get("device_id").and_then(serde_json::Value::as_str)==Some(&device_id) && d.get("status").and_then(serde_json::Value::as_str)==Some("active")).count()!=1 {return Err("当前设备已停用，请重新登录。".into());}
        if let Ok(mut cache)=CACHE.lock() {*cache=Some((data.to_path_buf(),base_url.clone(),token.clone(),user_id,device_id.clone(),std::time::Instant::now()));}
        user_id
    };
    let owner=settings::UploadOwner {base_url,space_id:space_id.into(),account_id:account_id.into(),user_id,device_id};
    validate_owner_state(&settings::load(data)?,&owner,&token)?;
    if desktop_credentials::load(data,&owner.base_url)?.access!=token {return Err("登录状态已改变。".into());}
    Ok((owner,token))
}

fn validate_owner_state(settings: &settings::AppSettings, owner: &settings::UploadOwner, token: &str) -> Result<(), String> {
    if access_token_claim(token,"user_id").is_some_and(|v|v.parse::<i64>().ok()!=Some(owner.user_id)) || owner.user_id <= 0 || owner.device_id.is_empty() || owner.device_id.trim() != owner.device_id
        || owner.account_id.is_empty()
        || upload_list_base_url(settings, &owner.space_id).ok().as_ref()
            .is_none_or(|base| !settings::same_service_url(base, &owner.base_url))
        || cloud_retry_device_id(&settings.devify.device_id, token).ok().as_deref() != Some(owner.device_id.as_str()) {
        return Err("任务的登录状态、服务、空间、微信账号或设备已改变。".into());
    }
    Ok(())
}

#[tauri::command]
async fn devify_upload_identity(app: AppHandle) -> Result<serde_json::Value, String> {
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    let space = settings.devify.active_space_id.ok_or("请先选择空间。")?;
    let (owner, _) = verified_upload_identity(&data, &settings.active_account_id, &space).await?;
    Ok(serde_json::json!({"user_id": owner.user_id, "device_id": owner.device_id}))
}

fn validate_owned_job(settings: &settings::AppSettings, job: &settings::UploadJob,
    owner: &settings::UploadOwner, token: &str, scope: &settings::UploadScope) -> Result<(), String> {
    validate_owner_state(settings, owner, token)?;
    if job.owner.as_ref() != Some(owner) || job.account_id != owner.account_id
        || job.space_id != owner.space_id || job.scope.as_ref() != Some(scope)
        || !settings::same_service_url(&scope.base_url, &owner.base_url) {
        return Err("任务身份或上传范围已改变，请重新创建采集任务。".into());
    }
    validate_upload_scope(settings, &owner.account_id, &owner.space_id, &scope.usernames)
}

fn validate_live_owned_job(data: &Path, job_id: &str, owner: &settings::UploadOwner,
    token: &str, scope: &settings::UploadScope) -> Result<(), String> {
    let settings = settings::load(data)?;
    let job = settings.upload_queue.iter().find(|job| job.id == job_id)
        .ok_or("找不到原始上传任务。")?;
    if desktop_credentials::load(data, &owner.base_url)?.access != token {
        return Err("服务端登录状态已改变，请重新操作。".into());
    }
    validate_owned_job(&settings, job, owner, token, scope)
}

fn validate_cloud_retry_context(
    settings: &settings::AppSettings,
    job: &settings::UploadJob,
    auth: &CloudRetryAuth,
    current_token: &str,
) -> Result<(), String> {
    let invalid = || "重试任务的登录状态、空间、账号、设备或范围已改变，请重新确认。".to_string();
    if auth.token.is_empty() || current_token != auth.token || auth.batch_id <= 0
        || job.id != auth.job_id || job.batch_id != Some(auth.batch_id)
        || job.space_id != auth.space_id || job.account_id != auth.account_id
        || settings.active_account_id != auth.account_id
        || upload_list_base_url(settings, &auth.space_id).ok().as_ref()
            .is_none_or(|base| !settings::same_service_url(base, &auth.base_url))
        || cloud_retry_device_id(&settings.devify.device_id, current_token).ok().as_deref() != Some(auth.device_id.as_str())
    {
        return Err(invalid());
    }
    validate_upload_scope(settings, &auth.account_id, &auth.space_id, &auth.scope.usernames)
        .map_err(|_| invalid())?;
    let scope = job.retry_scope(settings).map_err(|_| invalid())?;
    if scope.usernames != auth.scope.usernames || scope.since != auth.scope.since
        || scope.until != auth.scope.until || scope.base_url != auth.scope.base_url
        || scope.message_selection != auth.scope.message_selection
    {
        return Err(invalid());
    }
    Ok(())
}

/// The server stores the chat source on the batch; a WhatsApp upload must not be checked
/// against a hard-coded `wechat`.
fn expected_source_type(account_id: &str) -> &'static str {
    if whatsapp::is_account(account_id) { "whatsapp" } else { "wechat" }
}

fn validate_remote_owner(owner: &settings::UploadOwner, batch_id: i64, batch: &serde_json::Value, key: Option<&str>) -> Result<(), String> {
    if batch_id <= 0 || batch.get("id").and_then(serde_json::Value::as_i64) != Some(batch_id)
        || batch.get("space_id").and_then(serde_json::Value::as_str) != Some(owner.space_id.as_str())
        || batch.get("uploaded_by_id").and_then(serde_json::Value::as_i64) != Some(owner.user_id)
        || batch.get("source_type").and_then(serde_json::Value::as_str) != Some(expected_source_type(&owner.account_id))
        || batch.get("source_account_id").and_then(serde_json::Value::as_str) != Some(owner.account_id.as_str())
        || batch.get("device_id").and_then(serde_json::Value::as_str) != Some(owner.device_id.as_str())
        || key.is_some_and(|expected| batch.get("idempotency_key").and_then(serde_json::Value::as_str) != Some(expected)) {
        return Err("无法确认云端批次的用户、空间、设备和来源。".into());
    }
    Ok(())
}

fn validate_cloud_retry_remote(
    auth: &CloudRetryAuth,
    user: &serde_json::Value,
    batch: &serde_json::Value,
) -> Result<(i64, String), String> {
    let user_id = user.get("id").and_then(serde_json::Value::as_i64).filter(|id| *id > 0);
    if user_id.is_none()
        || batch.get("id").and_then(serde_json::Value::as_i64) != Some(auth.batch_id)
        || batch.get("space_id").and_then(serde_json::Value::as_str) != Some(auth.space_id.as_str())
        || batch.get("uploaded_by_id").and_then(serde_json::Value::as_i64) != user_id
        || batch.get("source_type").and_then(serde_json::Value::as_str) != Some(expected_source_type(&auth.account_id))
        || batch.get("source_account_id").and_then(serde_json::Value::as_str) != Some(auth.account_id.as_str())
        || batch.get("device_id").and_then(serde_json::Value::as_str) != Some(auth.device_id.as_str())
        || batch.get("status").and_then(serde_json::Value::as_str) != Some("failed")
    {
        return Err("无法确认云端失败批次的用户、设备和来源，不能重试。".into());
    }
    let key = batch.get("idempotency_key").and_then(serde_json::Value::as_str)
        .filter(|key| !key.is_empty() && key.len() <= 1024)
        .ok_or("云端批次缺少有效幂等键，不能安全重试。")?;
    Ok((user_id.ok_or("无法确认云端用户。")?, key.to_string()))
}

fn validate_cloud_retry_package(auth: &CloudRetryAuth, space_id: &str, account_id: &str, key: &str) -> Result<(), String> {
    if auth.space_id != space_id || auth.account_id != account_id || auth.idempotency_key != key {
        return Err("上传内容或范围已变化，请重新创建采集任务。".into());
    }
    Ok(())
}

fn validate_live_cloud_retry(data: &Path, auth: &CloudRetryAuth) -> Result<(), String> {
    let settings = settings::load(data)?;
    let job = settings.upload_queue.iter().find(|job| job.id == auth.job_id)
        .ok_or("找不到原始上传任务，不能重试。")?;
    let credentials = desktop_credentials::load(data, &auth.base_url)?;
    validate_cloud_retry_context(&settings, job, auth, &credentials.access)
}

fn append_retry_identity_body(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), String> {
    if body.len().saturating_add(chunk.len()) > UPLOAD_LIST_MAX_RESPONSE_BYTES {
        return Err("云端重试身份响应过大。".into());
    }
    body.extend_from_slice(chunk);
    Ok(())
}

async fn retry_identity_get(data: &Path, client: &reqwest::Client, auth: &CloudRetryAuth, path: &str) -> Result<serde_json::Value, String> {
    let mut response = client.get(format!("{}{path}", auth.base_url))
        .bearer_auth(&auth.token).send_traced_retrying(data, diagnostics::Module::Upload, RetryPolicy::READ).await
        .map_err(|_| "无法读取云端重试身份，请检查网络连接。")?;
    if !response.status().is_success() {
        return Err(format!("无法确认云端重试身份：HTTP {}。", response.status().as_u16()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "无法读取云端重试身份响应。")? {
        append_retry_identity_body(&mut body, &chunk)?;
    }
    let payload: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| "云端重试身份响应格式无效。")?;
    Ok(payload.get("data").cloned().unwrap_or(payload))
}

async fn verify_cloud_retry_remote(data: &Path, auth: &CloudRetryAuth) -> Result<(i64, String), String> {
    let client = http_client::strict();
    let user = retry_identity_get(data, &client, auth, "/api/v1/auth/user").await?;
    let batch = retry_identity_get(data, &client, auth, &format!("/api/v1/chat/uploads/{}", auth.batch_id)).await?;
    validate_cloud_retry_remote(auth, &user, &batch)
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
    let _upload_guard = begin_account_upload(&state, &job.account_id)?;
    let scope = job.retry_scope(&settings)?.clone();
    if job.manual_fingerprint.as_deref().is_some_and(|old| old != fingerprint) {
        return Err("本批消息或附件已变化，无法安全重试；请重新创建采集任务。".into());
    }
    if job.owner.is_none() || (job.batch_id.is_none() && job.create_started != Some(false) && !job.can_recover_create()) {
        return Err("无法确认原任务的账号或服务端批次，请核对后重新创建采集任务。".into());
    }
    if job.batch_id.is_some() && job.package_digest.is_none() {
        return Err("原任务没有保存归档摘要，无法安全重试。".into());
    }
    let (owner, token) = verified_upload_identity(&data, &job.account_id, &job.space_id).await?;
    validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
    let cloud_retry = if let Some(batch_id) = job.batch_id {
        if batch_id <= 0 { return Err("原上传批次无效，不能重试。".into()); }
        let mut auth = CloudRetryAuth {
            token: token.clone(), base_url: owner.base_url.clone(), job_id: job.id.clone(),
            space_id: job.space_id.clone(), account_id: job.account_id.clone(), batch_id,
            device_id: owner.device_id.clone(), user_id: 0, idempotency_key: String::new(), scope: scope.clone(),
        };
        validate_live_cloud_retry(&data, &auth)?;
        let (user_id, key) = verify_cloud_retry_remote(&data, &auth).await?;
        if user_id != owner.user_id { return Err("云端上传者与原任务账号不一致，不能重试。".into()); }
        auth.user_id = user_id;
        auth.idempotency_key = key;
        validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
        validate_live_cloud_retry(&data, &auth)?;
        let confirmed_key = format!("wechat-{}-{}", job.account_id, &fingerprint[..32]);
        validate_cloud_retry_package(&auth, &job.space_id, &job.account_id, &confirmed_key)?;
        Some(auth)
    } else {
        None
    };
    if job.manual_group.is_some() && job.manual_fingerprint.is_none() {
        settings::update(&data, |current| {
            let item = current.upload_queue.iter().find(|item| item.id == job_id).ok_or("找不到上传任务。")?;
            validate_owned_job(current, item, &owner, &token, &scope)?;
            if desktop_credentials::load(&data, &owner.base_url)?.access != token {
                return Err("登录状态已改变，请重新确认重试任务。".into());
            }
            let item = current.upload_queue.iter_mut().find(|item| item.id == job_id).ok_or("找不到上传任务。")?;
            if item.manual_fingerprint.as_deref().is_some_and(|old| old != fingerprint) {
                return Err("原任务内容已变化，请重新创建采集任务。".into());
            }
            item.manual_fingerprint = Some(fingerprint.clone());
            Ok(())
        })?;
    }
    let retry_usernames = scope.usernames.clone();
    let retry_since = scope.since;
    let retry_until = scope.until;
    let retry_base_url = scope.base_url.clone();
    let retry_selection = scope.message_selection.clone();
    let result = devify_upload_wechat(
        app.clone(),
        job_id.clone(),
        job.space_id.clone(),
        job.account_id.clone(),
        retry_usernames,
        retry_since,
        retry_until,
        Some(retry_base_url),
        Some(fingerprint),
        retry_selection,
        if job.id.starts_with("sync-") { UploadTrigger::Unknown } else { UploadTrigger::Manual },
        cloud_retry,
    )
    .await;
    settings::update(&data, |current| {
        let item = current.upload_queue.iter_mut().find(|item| item.id == job_id)
            .ok_or("找不到上传任务，无法保存重试结果。")?;
        apply_manual_upload_outcome(item, &job, &result, "已重新提交服务端解析")
    })?;
    result.map(|mut value| {
        value.job_id = job_id;
        value
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum UploadTrigger {
    Manual,
    Scheduled,
    Unknown,
}

fn sync_upload_trigger(manual: bool, continuation_only: bool, original_manual: Option<bool>) -> UploadTrigger {
    match if continuation_only { original_manual } else { Some(manual) } {
        Some(true) => UploadTrigger::Manual,
        Some(false) => UploadTrigger::Scheduled,
        None => UploadTrigger::Unknown,
    }
}

fn upload_create_body(
    account_id: &str,
    key: &str,
    device_id: &str,
    device_label: &str,
    platform: &str,
    source_type: &str,
    trigger: UploadTrigger,
) -> serde_json::Value {
    serde_json::json!({
        // `source_type` is the chat source and must match the archive's `platform`;
        // the batch's own `platform` stays the device platform (macOS/Windows).
        "source_type": source_type,
        "trigger": trigger,
        "source_account_id": account_id,
        "idempotency_key": key,
        "device_id": device_id,
        "device_label": device_label,
        "platform": platform,
    })
}

fn build_upload_archive<'a>(
    manifest: &serde_json::Value,
    records: [(&str, &[serde_json::Value]); 4],
    files: impl Iterator<Item = &'a (String, Vec<u8>)>,
) -> Result<Vec<u8>, String> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();
    for (name, entries) in records {
        if name == "avatars.jsonl" && entries.is_empty() {
            continue;
        }
        zip.start_file(name, options).map_err(|e| e.to_string())?;
        for record in entries {
            writeln!(zip, "{}", serde_json::to_string(record).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        }
    }
    for (name, bytes) in files {
        zip.start_file(name.as_str(), options).map_err(|e| e.to_string())?;
        zip.write_all(bytes).map_err(|e| e.to_string())?;
    }
    zip.start_file("manifest.json", options).map_err(|e| e.to_string())?;
    zip.write_all(serde_json::to_string(manifest).map_err(|e| e.to_string())?.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(zip.finish().map_err(|e| e.to_string())?.into_inner())
}

fn select_upload_archive(
    expected_digest: Option<&str>,
    saved_version: Option<&str>,
    mut build: impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Result<(Vec<u8>, String, String), String> {
    let current = env!("CARGO_PKG_VERSION");
    // Older jobs stored only the digest. Try the affected release versions,
    // and accept only a byte-for-byte match with the recorded digest.
    let versions: &[&str] = match (expected_digest, saved_version) {
        (None, _) => &[current],
        (Some(_), Some(_)) => saved_version.as_slice(),
        (Some(_), None) => &[current, "0.1.7", "0.1.6"],
    };
    for version in versions {
        let body = build(version)?;
        let digest = hex::encode(sha2::Sha256::digest(&body));
        if expected_digest.is_none_or(|expected| digest == expected) {
            return Ok((body, digest, (*version).to_string()));
        }
    }
    Err("原始归档已变化，无法安全恢复。".into())
}

async fn devify_upload_wechat(
    app: AppHandle,
    job_id: String,
    space_id: String,
    account_id: String,
    usernames: Vec<String>,
    since: Option<i64>,
    until: Option<i64>,
    expected_base_url: Option<String>,
    expected_fingerprint: Option<String>,
    message_selection: BTreeMap<String, Vec<settings::MessageRef>>,
    trigger: UploadTrigger,
    cloud_retry: Option<CloudRetryAuth>,
) -> Result<DevifyUploadResult, String> {
    if usernames.is_empty() {
        return Err("请至少选择一个会话。".into());
    }
    let upload_epoch_at_start = upload_epoch();
    if settings::load(&app_data(&app))?.runtime.paused {return Err("后台任务已暂停。".into());}
    let whatsapp_source = whatsapp::is_account(&account_id);
    let allowed = if whatsapp_source {
        whatsapp_allowed_usernames(&app, &account_id)?
    } else {
        allowed_usernames(&app, &account_id)?
    };
    if usernames.iter().any(|name| !allowed.contains(name)) {
        return Err("上传范围包含不在当前白名单中的会话。".into());
    }
    let data = app_data(&app);
    let (owner, token) = verified_upload_identity(&data, &account_id, &space_id).await?;
    let current_settings = settings::load(&data)?;
    let job_snapshot = current_settings.upload_queue.iter().find(|job| job.id == job_id)
        .ok_or("找不到原始上传任务。")?;
    let scope = job_snapshot.scope.clone().ok_or("找不到原始上传范围。")?;
    if scope.usernames != usernames || scope.since != since || scope.until != until
        || scope.message_selection != message_selection {
        return Err("上传范围已改变，请重新创建采集任务。".into());
    }
    validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
    let base_url = expected_base_url
        .clone()
        .unwrap_or_else(|| current_settings.devify.base_url.clone());
    if !settings::same_service_url(&base_url, &current_settings.devify.base_url) {
        return Err("服务地址已更改，无法安全匹配旧消息。".into());
    }
    let account_for_prepare = account_id.clone();
    let names_for_prepare = usernames.clone();
    let selection_for_prepare = message_selection.clone();
    let data_for_prepare = data.clone();
    let base_url_for_prepare = base_url.clone();
    let space_id_for_prepare = space_id.clone();
    let prepared = if whatsapp_source {
        let account_wa = account_for_prepare.clone();
        tauri::async_runtime::spawn_blocking(move || {
            prepare_whatsapp_upload(
                &data_for_prepare,
                &account_wa,
                &space_id_for_prepare,
                &base_url_for_prepare,
                &names_for_prepare,
                since,
                until,
                &selection_for_prepare,
            )
        })
        .await
        .map_err(|e| format!("准备上传失败：{e}"))??
    } else {
        let cache = decrypt::cache_dir(&data, &account_id);
        let private_data=local_scope::root(&data);
        let legacy_keys = if matches!(trigger,UploadTrigger::Scheduled) && !message_selection.is_empty() {
            load_batch_legacy_keys(&app,&space_id,&account_id,&message_selection).await?
        } else {load_legacy_upload_keys(&app,&space_id,&account_id,&usernames,Some((&owner.base_url,&token)),upload_epoch_at_start).await?};
        let voice_settings = current_settings.voice.clone();
        let attachments = current_settings.backup.attachments.clone();
        let names = names_for_prepare.clone();
        let base = base_url_for_prepare.clone();
        let space = space_id_for_prepare.clone();
        let selection = selection_for_prepare.clone();
        let data_copy = data_for_prepare.clone();
        let account_wechat = account_id.clone();
        let media_root_for_prepare = account_root(&account_id).unwrap_or_else(|_| cache.clone());
        tauri::async_runtime::spawn_blocking(move || {
            prepare_upload_scoped(
                &data_copy,
                &cache,
                &media_root_for_prepare,
                &account_wechat,
                &space,
                &base,
                &names,
                since,
                until,
                &selection,
                &voice_settings,
                &attachments,
                &legacy_keys,
                &private_data,
            )
        })
        .await
        .map_err(|e| format!("准备上传失败：{e}"))??
    };
    validate_upload_batch_size(prepared.preview.message_count)?;
    verify_upload_preview(
        expected_fingerprint.as_deref(),
        &prepared.preview.fingerprint,
    )?;
    let message_count = prepared.preview.message_count;
    let session_count = prepared.preview.sessions.len();
    let latest_position = prepared.latest_position;
    let covered = prepared.covered;
    let package_key = prepared.preview.fingerprint.clone();
    let mut whitelist: Vec<String> = allowed.iter().cloned().collect();
    whitelist.sort();
    let PreparedUpload {
        conversations,
        messages,
        parts,
        avatars,
        avatar_files,
        media_files,
        voice_pending,
        voice_processed,
        ..
    } = prepared;
    // The server rejects an archive whose `schema_version` does not match `platform`, so the
    // source decides both. The JSONL layout itself is shared (conversations / messages /
    // message_parts), which is why only the envelope label changes.
    let source_platform = if whatsapp_source { "whatsapp" } else { "wechat" };
    let mut manifest = serde_json::json!({
        "schema_version": format!("{source_platform}.v1"),
        "platform": source_platform,
        "source_namespace": account_id,
        "counts": { "conversations": conversations.len(), "messages": messages.len() },
        "scope": { "conversation_ids": usernames },
        "whitelist_snapshot": whitelist,
        "client": { "name": "AIMyChats" },
    });
    let recovering = cloud_retry.is_some() || (job_snapshot.status == "failed" && job_snapshot.can_recover_create());
    let expected_digest = if recovering {
        Some(job_snapshot.package_digest.as_deref().ok_or("找不到原始归档摘要。")?)
    } else {
        None
    };
    let (body, digest, archive_version) = select_upload_archive(
        expected_digest,
        job_snapshot.package_client_version.as_deref(),
        |version| {
            manifest["client"]["version"] = serde_json::json!(version);
            build_upload_archive(
                &manifest,
                [
                    ("conversations.jsonl", conversations.as_slice()),
                    ("messages.jsonl", messages.as_slice()),
                    ("message_parts.jsonl", parts.as_slice()),
                    ("avatars.jsonl", avatars.as_slice()),
                ],
                avatar_files.iter().chain(media_files.iter()),
            )
        },
    )?;
    let settings = settings::load(&data)?;
    if expected_base_url.as_deref().is_some_and(|url| {
        !settings::same_service_url(url, &settings.devify.base_url)
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
    // The key must still identify the batch whose ownership was verified, before POST can
    // create anything. Otherwise a changed archive would silently create a new batch.
    // The schema version is part of the key: an archive whose envelope was corrected must
    // create a fresh batch instead of replaying the rejected one under the same key.
    let key = if matches!(trigger, UploadTrigger::Scheduled) {
        // A new durable job may contain the same messages with a different
        // whitelist/version. Give it its own identity; retries keep the stored
        // key and exact ZIP, while message-level dedup handles overlapping jobs.
        job_snapshot.create_key.clone().unwrap_or_else(|| format!("desktop-{source_platform}-{}", job_snapshot.id))
    } else if whatsapp_source {
        format!("whatsapp.v1-{}-{}", account_id, &package_key[..32])
    } else {
        format!("wechat-{}-{}", account_id, &package_key[..32])
    };
    if let Some(auth) = &cloud_retry {
        if auth.token != token || auth.user_id != owner.user_id || auth.device_id != owner.device_id {
            return Err("重试任务的登录用户或设备已改变。".into());
        }
        validate_cloud_retry_package(auth, &space_id, &account_id, &key)?;
        validate_live_cloud_retry(&data, auth)?;
        let (user_id, remote_key) = verify_cloud_retry_remote(&data, auth).await?;
        if user_id != auth.user_id || remote_key != auth.idempotency_key {
            return Err("云端批次或用户已变化，请重新确认重试任务。".into());
        }
        validate_live_cloud_retry(&data, auth)?;
    }
    {
    let _archive_guard=upload_archive::lock()?;
    upload_archive::save(&data,&digest,&body)?;
    if matches!(trigger,UploadTrigger::Scheduled) && !config_runtime::ready(&settings::load(&data)?,&owner) {return Err("备份配置正在更新，稍后会自动继续。".into());}
    // Record the exact ZIP and the possibility of a sent request *before* POST. If the
    // response is lost, the unknown batch cannot be blindly created a second time.
    if upload_aborted(upload_epoch_at_start) { return Err(UPLOAD_ABORTED_MESSAGE.into()); }
    settings::update(&data, |current| {
        let job = current.upload_queue.iter().find(|item| item.id == job_id).ok_or("找不到上传任务。")?;
        validate_owned_job(current, job, &owner, &token, &scope)?;
        if desktop_credentials::load(&data, &owner.base_url)?.access != token { return Err("登录状态已改变。".into()); }
        if cloud_retry.is_some() || (job.status == "failed" && job.can_recover_create()) {
            if !job.matches_create_package(&key, &digest) {
                return Err("原始归档或批次标识已变化，无法安全恢复。".into());
            }
        } else if job.batch_id.is_some() || job.create_started != Some(false) {
            return Err("无法确认服务端是否已有批次，请核对后重新创建采集任务。".into());
        }
        let job = current.upload_queue.iter_mut().find(|item| item.id == job_id).ok_or("找不到上传任务。")?;
        job.create_started = Some(true);
        job.create_key = Some(key.clone());
        job.package_digest = Some(digest.clone());
        job.package_client_version = Some(archive_version.clone());
        job.covered=covered.clone();job.voice_pending=voice_pending.clone();job.voice_processed=voice_processed.clone();
        Ok(())
    })?;
    }
    // Only batch creation is safe to repeat: the server resolves an existing
    // batch atomically using the same user, space and idempotency key.
    let client = http_client::upload();
    let upload_platform = if settings.devify.platform.is_empty() {
        settings::default_platform()
    } else {
        settings.devify.platform.clone()
    };
    let upload_device_id = owner.device_id.clone();
    let upload_base_url = owner.base_url.as_str();
    let mut create_body=upload_create_body(&account_id,&key,&upload_device_id,&settings::resolved_device_label(&settings.devify),&upload_platform,source_platform,trigger);
    create_body["archive_sha256"]=serde_json::json!(digest);create_body["archive_size"]=serde_json::json!(body.len());
    let request = client
        .post(format!("{upload_base_url}/api/v1/spaces/{space_id}/chat/uploads"))
        .bearer_auth(&token)
        .json(&create_body);
    validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
    if let Some(auth) = &cloud_retry {
        validate_live_cloud_retry(&data, auth)?;
    }
    let retry_request = request.try_clone().ok_or("无法准备创建批次请求。")?;
    let first = request.send_traced(&data, diagnostics::Module::Upload).await;
    let retry_create = match &first {
        Err(error) => error.is_timeout() || error.is_connect(),
        Ok(response) => matches!(response.status().as_u16(), 502 | 503 | 504),
    };
    let created = if retry_create {
        tokio::time::sleep(Duration::from_millis(300)).await;
        validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
        if let Some(auth) = &cloud_retry {
            validate_live_cloud_retry(&data, auth)?;
        }
        retry_request.send_traced(&data, diagnostics::Module::Upload).await
    } else {
        first
    }.map_err(|error| devify_network_message("创建上传批次失败", &error))?;
    if !created.status().is_success() {
        return Err(format!("创建上传批次失败：{}", created.status()));
    }
    let payload: serde_json::Value = created
        .json()
        .await
        .map_err(|e| format!("读取上传响应失败：{e}"))?;
    let batch = payload.get("data").unwrap_or(&payload);
    let batch_id = batch.get("id").and_then(|v| v.as_i64()).filter(|id| *id > 0)
        .ok_or_else(|| "上传响应缺少批次 ID；请核对服务端记录。".to_string())?;
    validate_remote_owner(&owner, batch_id, batch, Some(&key))?;
    if cloud_retry.as_ref().is_some_and(|auth| auth.batch_id != batch_id) {
        return Err("服务端返回了不同的上传批次，已取消上传。".into());
    }
    // Save the server ID and package coverage before any subsequent network request.
    // A failed save leaves the original digest and key available for recovery.
    settings::update(&data, |current| {
        let job = current.upload_queue.iter().find(|item| item.id == job_id).ok_or("找不到上传任务。")?;
        validate_owned_job(current, job, &owner, &token, &scope)?;
        if job.batch_id.is_some_and(|saved| saved != batch_id) { return Err("本机批次标识已改变。".into()); }
        let job = current.upload_queue.iter_mut().find(|item| item.id == job_id).ok_or("找不到上传任务。")?;
        job.batch_id = Some(batch_id);
        job.covered = covered.clone();
        job.voice_pending = voice_pending.clone();
        job.voice_processed = voice_processed.clone();
        Ok(())
    })?;
    validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
    let remote = fixed_identity_get(&data, upload_base_url, &token, &format!("/api/v1/chat/uploads/{batch_id}")).await?;
    validate_remote_owner(&owner, batch_id, &remote, Some(&key))?;
    let batch_status = remote.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if matches!(batch_status, "processing" | "completed") {
        // This package was already accepted; never upload or submit it a second time.
        return Ok(DevifyUploadResult {
            job_id: String::new(),
            batch_id,
            status: batch_status.to_string(),
            message_count,
            conversations: session_count,
            latest_position,
            covered,
            voice_pending,
            voice_processed,
        });
    }
    let archive_size = body.len();
    let form = reqwest::multipart::Form::new().part(
        "archive",
        reqwest::multipart::Part::bytes(body)
            .file_name("wechat.v1.zip")
            .mime_str("application/zip")
            .map_err(|e| e.to_string())?,
    );
    if upload_aborted(upload_epoch_at_start) { return Err(UPLOAD_ABORTED_MESSAGE.into()); }
    validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
    if let Some(auth) = &cloud_retry {
        validate_live_cloud_retry(&data, auth)?;
    }
    let started = std::time::Instant::now();
    let archive_path = format!("{upload_base_url}/api/v1/chat/uploads/{batch_id}/archive");
    let uploaded = client
        .put(&archive_path)
        .bearer_auth(&token)
        .multipart(form)
        // Multipart bodies are streamed, so the trace uses an explicit summary.
        .send_traced_hinted(
            &data,
            diagnostics::Module::Upload,
            diagnostics::request_hint(
                "PUT",
                &archive_path,
                Some(format!("multipart: archive {archive_size} bytes, body not captured")),
            ),
        )
        .await
        .map_err(|e| {
            let reason = if e.is_timeout() {
                "传输或等待服务端响应超时，上传结果需要核对"
            } else if e.is_connect() {
                "无法连接服务端，上传结果需要核对"
            } else {
                "上传请求中断，上传结果需要核对"
            };
            format!("{reason}（归档 {archive_size} 字节，耗时 {:.1} 秒）", started.elapsed().as_secs_f64())
        })?;
    if !uploaded.status().is_success() {
        return Err(format!("上传归档失败：HTTP {}（归档 {archive_size} 字节，耗时 {:.1} 秒）", uploaded.status().as_u16(), started.elapsed().as_secs_f64()));
    }
    validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
    if let Some(auth) = &cloud_retry {
        validate_live_cloud_retry(&data, auth)?;
    }
    let completed = client
        .post(format!("{upload_base_url}/api/v1/chat/uploads/{batch_id}/complete"))
        .bearer_auth(&token)
        .send_traced(&data, diagnostics::Module::Upload)
        .await
        .map_err(|e| devify_network_message("提交解析失败", &e))?;
    if !completed.status().is_success() {
        return Err(format!("提交解析失败：{}", completed.status()));
    }
    Ok(DevifyUploadResult {
        job_id: String::new(),
        batch_id,
        status: "processing".into(),
        message_count,
        conversations: session_count,
        latest_position,
        covered,
        voice_pending,
        voice_processed,
    })
}

#[tauri::command]
async fn devify_auth_start(
    app: AppHandle,
    state: State<'_, DevifyAuthState>,
    device_label: Option<String>,
) -> Result<String, String> {
    let _start = AUTH_START_LOCK.lock().await;
    recover_local_logout(&app_data(&app))?;
    let auth_generation = desktop_credentials::begin_auth_attempt()?;
    let settings = settings::load(&app_data(&app))?;
    let base_url = settings.devify.resolved_base_url()?;
    let device_label = device_label
        .map(|value| value.trim().to_string())
        .filter(|value| {
            !value.is_empty()
                && value != "AIMyChats"
                && value != "AIMyChats Desktop"
        })
        .unwrap_or_else(settings::device_default_label);
    let config_started = std::time::Instant::now();
    let config_result = devify_auth_config(&app_data(&app), &settings.devify).await;
    eprintln!("桌面授权：读取登录配置耗时 {} 毫秒", config_started.elapsed().as_millis());
    let auth_config = config_result?;
    if desktop_credentials::generation()? != auth_generation {
        return Err("登录状态已改变，请重新登录。".into());
    }
    let mut state_bytes = [0u8; 24];
    let mut verifier_bytes = [0u8; 48];
    getrandom::getrandom(&mut state_bytes).map_err(|e| e.to_string())?;
    getrandom::getrandom(&mut verifier_bytes).map_err(|e| e.to_string())?;
    let oauth_state = URL_SAFE_NO_PAD.encode(state_bytes);
    let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
    let digest = sha2::Sha256::digest(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(digest);
    {
        let mut previous = state
            .listener
            .lock()
            .map_err(|_| "读取登录回调状态失败。".to_string())?;
        if let Some(handle) = previous.take() {
            handle.abort();
        }
    }
    let mut bound = None;
    for attempt in 0..12 {
        match TcpListener::bind("127.0.0.1:47832").await {
            Ok(listener) => {
                bound = Some(listener);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse && attempt < 11 => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => {
                return Err(format!(
                    "无法启动本地登录回调服务（47832）：{error}。请关闭其他 AIMyChats 实例后重试。"
                ))
            }
        }
    }
    let listener = bound.ok_or_else(|| {
        "无法启动本地登录回调服务（47832）：端口已被占用。请关闭其他 AIMyChats 实例后重试，或改用 AI 邮箱用户名和密码登录。".to_string()
    })?;
    let callback_handle = app.clone();
    let handle = tauri::async_runtime::spawn(async move {
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
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}",
                AUTH_COMPLETE_HTML.len(),
                AUTH_COMPLETE_HTML
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    if let Ok(mut slot) = state.listener.lock() {
        *slot = Some(handle);
    }
    {
        let mut pending = state.pending.lock().map_err(|_| "读取登录状态失败。".to_string())?;
        if desktop_credentials::generation()? != auth_generation
            || settings::load(&app_data(&app))?.devify.resolved_base_url()? != base_url {
            return Err("登录状态已改变，请重新登录。".into());
        }
        pending.replace(PendingDevifyAuth { state: oauth_state.clone(), verifier, base_url: base_url.clone(), generation: auth_generation, config: auth_config.clone() });
    }
    let mut url = Url::parse(&format!(
        "{}{}",
        base_url,
        auth_config.browser_authorize_path
    ))
    .map_err(|e| format!("服务端地址无效：{e}"))?;
    url.query_pairs_mut()
        .append_pair("client_id", &auth_config.client_id)
        .append_pair("redirect_uri", &auth_config.redirect_uri)
        .append_pair("mode", "browser")
        .append_pair("response_type", "code")
        .append_pair("state", &oauth_state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("device_label", &device_label);
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
    let pending = {
        let mut slot = state.pending.lock().map_err(|_| "读取登录状态失败。".to_string())?;
        let expected = slot.as_ref().ok_or_else(|| "没有待处理的登录请求。".to_string())?;
        if returned_state != expected.state { return Err("登录 state 校验失败。".into()); }
        slot.take().ok_or_else(|| "没有待处理的登录请求。".to_string())?
    };
    let settings = settings::load(&app_data(&app))?;
    if settings.devify.resolved_base_url()? != pending.base_url
        || desktop_credentials::generation()? != pending.generation {
        return Err("登录状态已改变，请重新登录。".into());
    }
    let auth_config = &pending.config;
    let platform = if settings.devify.platform.is_empty() {
        settings::default_platform()
    } else {
        settings.devify.platform.clone()
    };
    let device_label = settings::resolved_device_label(&settings.devify);
    let exchange_started = std::time::Instant::now();
    let exchange_result = http_client::strict()
        .post(format!(
            "{}{}",
            pending.base_url,
            auth_config.token_path
        ))
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "client_id": auth_config.client_id,
            "redirect_uri": auth_config.redirect_uri,
            "code": code,
            "code_verifier": pending.verifier,
            "device_label": device_label.clone(),
            "device_id": settings.devify.device_id.clone(),
            "platform": platform.clone(),
        }))
        .send_traced_retrying(&app_data(&app), diagnostics::Module::Auth, RetryPolicy::WRITE)
        .await;
    eprintln!("桌面授权：换取登录令牌耗时 {} 毫秒", exchange_started.elapsed().as_millis());
    let response = exchange_result.map_err(|e| devify_network_message("换取服务端登录令牌失败", &e))?;
    if !response.status().is_success() {
        return Err(format!("服务端登录失败：{}", response.status()));
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
    let device_id = data.get("device_id").and_then(|v| v.as_str())
        .map(str::to_string).or_else(|| access_token_claim(access, "device_id")).unwrap_or_default();
    // New servers return the durable device-session secret in the PKCE exchange.
    // Save it atomically with the JWT pair so the first refresh does not need to
    // create a second session (which could invalidate the just-issued token pair).
    let (device_session, session_device_id) = device_session_from_response(data, &device_id)?;
    let app_data = app_data(&app);
    let credentials = desktop_credentials::Credentials {
        base_url: pending.base_url.clone(), access: access.to_string(), refresh: refresh.to_string(), device_session, session_device_id,
    };
    settings::with_identity(&app_data, |current| {
        if current.devify.resolved_base_url()? != pending.base_url
            || !desktop_credentials::save_if_generation_locked(&app_data, pending.generation, &credentials)? {
            return Err("服务端登录状态已改变，请重新登录。".into());
        }
        Ok(())
    })?;
    let mut device_changed = false;
    settings::update_identity(&app_data, |current| {
        let (stored, generation) = desktop_credentials::load_with_generation(&app_data, &pending.base_url)?;
        if current.devify.resolved_base_url()? != pending.base_url
            || stored != credentials || generation != pending.generation.wrapping_add(1) {
            return Err("服务端登录状态已改变，请重新登录。".into());
        }
        device_changed = current.devify.device_id != device_id;
        current.devify.device_id = device_id.clone();
        current.devify.platform = platform;
        current.devify.device_label = device_label;
        Ok(())
    })?;
    let committed_generation = pending.generation.wrapping_add(1).wrapping_add(u64::from(device_changed));
    let mut completed_generation = None;
    let _ = ensure_active_space_for(&app, Some((&credentials, committed_generation)), &mut completed_generation).await;
    if !desktop_credentials::load_with_generation(&app_data, &pending.base_url).ok().as_ref()
        .is_some_and(|(stored, generation)| stored == &credentials && *generation == completed_generation.unwrap_or(committed_generation)) {
        return Err("服务端登录状态已改变，请重新登录。".into());
    }
    config_runtime::activate_profile(&app_data)?;
    let service = app.clone();
    tauri::async_runtime::spawn(async move {
        refresh_entitlements_then_reconcile(&service).await;
    });
    Ok(DevifyAuthResult {
        device_id,
        expires_in: data
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or_default(),
    })
}

#[tauri::command]
fn devify_auth_cancel(state: State<'_, DevifyAuthState>) -> Result<(), String> {
    // A callback may already have consumed pending and be awaiting the token endpoint.
    desktop_credentials::begin_auth_attempt()?;
    if let Some(handle) = state
        .listener
        .lock()
        .map_err(|_| "读取登录回调状态失败。".to_string())?
        .take()
    {
        handle.abort();
    }
    state
        .pending
        .lock()
        .map_err(|_| "读取登录状态失败。".to_string())?
        .take();
    Ok(())
}

#[derive(Serialize)]
struct DevifyAuthStatus {
    authenticated: bool,
    /// The server could not be reached (timeout, refused connection or 5xx).
    /// The UI must say "service unavailable, retry later" instead of showing the
    /// login gate, and must not drop the stored session.
    unavailable: bool,
    error: String,
}

fn auth_status_after_token_failure(data: &Path, base: &str, before: u64, cleared_generation: Option<u64>) -> Result<DevifyAuthStatus, String> {
    if let Some(cleared_generation) = cleared_generation {
        return settings::with_identity(data, |current| {
            if current.devify.resolved_base_url()? != settings::canonical_api_base(base)?
                || !desktop_credentials::absent_at_generation(data, cleared_generation)? {
                return Err("登录状态已改变，请重新检查。".into());
            }
            Ok(DevifyAuthStatus { authenticated: false, unavailable: false, error: String::new() })
        });
    }
    if desktop_credentials::generation()? != before {
        return Err("登录状态已改变，请重新检查。".into());
    }
    Ok(DevifyAuthStatus { authenticated: false, unavailable: false, error: String::new() })
}

/// Report whether usable desktop credentials exist. An expired access token is
/// refreshed here so the UI can decide between the login gate and the app.
///
/// A refresh can succeed for an account that the server has since removed (the
/// refresh endpoint only checks the token signature), so ask the server once for
/// an authenticated resource. Only an explicit 401 invalidates the session;
/// network errors keep it so an offline user is not sent back to the login gate.
/// Every capability the client knows how to gate, with its Chinese label and whether it is
/// part of the free baseline. Used by settings to explain a plan and by the UI to render
/// "not included" states without hard-coding keys.
#[tauri::command]
fn capabilities_list() -> Vec<capabilities::Capability> {
    capabilities::CAPABILITIES.to_vec()
}

/// The cached entitlement view for the configured server. Never contacts the network, so it
/// is safe to call while rendering settings.
#[tauri::command]
fn entitlements_get(app: AppHandle) -> Result<entitlements::View, String> {
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    Ok(entitlements::view(&data, &settings.devify.base_url))
}

/// Ask the server for the current plan. A missing endpoint resolves to `source = "unsupported"`
/// and leaves every feature enabled, matching the behaviour before entitlements existed.
#[tauri::command]
async fn entitlements_refresh(app: AppHandle) -> Result<entitlements::View, String> {
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    let token = devify_access_token_async(&data, &settings.devify.base_url).await?;
    let view = entitlements::refresh(&data, &settings.devify.base_url, &token).await?;
    reconcile_local_service(&app).await;
    Ok(view)
}

#[tauri::command]
async fn devify_auth_status(app: AppHandle) -> Result<DevifyAuthStatus, String> {
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    recover_local_logout(&data)?;
    config_runtime::activate_profile(&data)?;
    let before = desktop_credentials::generation()?;
    let mut cleared_generation = None;
    let token = match devify_access_token_with_cleanup(&data, &settings.devify.base_url, &mut cleared_generation).await {
        Ok(token) => token,
        // A network problem says nothing about the stored session: keep it, and let the UI
        // offer "retry later" instead of dropping the user back to the login gate.
        Err(DevifyTokenError::Unavailable(message)) => {
            if desktop_credentials::generation()? != before {
                return Err("登录状态已改变，请重新检查。".into());
            }
            return Ok(DevifyAuthStatus { authenticated: false, unavailable: true, error: message });
        }
        Err(DevifyTokenError::Message(_)) => {
            return auth_status_after_token_failure(&data, &settings.devify.base_url, before, cleared_generation);
        }
    };
    let (credentials, generation) = desktop_credentials::load_with_generation(&data, &settings.devify.base_url)?;
    if credentials.access != token || settings::load(&data)?.devify.resolved_base_url()? != settings.devify.resolved_base_url()? {
        return Err("登录状态已改变，请重新检查。".into());
    }
    // These three authenticated reads have no data dependency. Each retains its own
    // bounded response and identity checks; the UI consumes the resulting local caches.
    let user_read=devify_current_user(app.clone());
    let rights_read=entitlements::refresh(&data,&settings.devify.base_url,&token);
    let (connection,_,_)=tokio::join!(http_client::api()
        .get(format!(
            "{}/api/v1/spaces",
            settings.devify.resolved_base_url()?
        ))
        .bearer_auth(&token)
        .send_traced_retrying(&data, diagnostics::Module::Auth, RetryPolicy::READ)
        ,user_read,rights_read);
    match connection
    {
        Ok(response) if response.status() == reqwest::StatusCode::UNAUTHORIZED => {
            if request_generation(&data, &settings.devify.base_url, &token)? != generation {
                return Err("登录状态已改变，请重新检查。".into());
            }
            let mut cleared = None;
            match devify_access_token_resolve(&data, &settings.devify.base_url, &mut cleared, true).await {
                Ok(refreshed) => {
                    let refreshed_generation = request_generation(&data, &settings.devify.base_url, &refreshed)?;
                    let response = http_client::api().get(format!("{}/api/v1/spaces", settings.devify.resolved_base_url()?))
                        .bearer_auth(&refreshed).send().await;
                    if request_generation(&data, &settings.devify.base_url, &refreshed)? != refreshed_generation {
                        return Err("登录状态已改变，请重新检查。".into());
                    }
                    match response {
                        Ok(value) if value.status().is_success() => Ok(DevifyAuthStatus { authenticated: true, unavailable: false, error: String::new() }),
                        Ok(value) => Ok(DevifyAuthStatus { authenticated: false, unavailable: true, error: format!("登录验证失败（HTTP {}），请稍后重试。", value.status().as_u16()) }),
                        Err(error) => Ok(DevifyAuthStatus { authenticated: false, unavailable: true, error: devify_network_message("检查登录状态失败", &error) }),
                    }
                }
                Err(DevifyTokenError::Unavailable(error)) => Ok(DevifyAuthStatus { authenticated: false, unavailable: true, error }),
                Err(DevifyTokenError::Message(_)) => auth_status_after_token_failure(&data, &settings.devify.base_url, generation, cleared),
            }
        }
        Ok(response) if !response.status().is_success() => {
            if request_generation(&data, &settings.devify.base_url, &token)? != generation {
                return Err("登录状态已改变，请重新检查。".into());
            }
            Ok(DevifyAuthStatus {
                authenticated: false,
                unavailable: true,
                error: format!(
                    "服务端异常（HTTP {}），请稍后重试。",
                    response.status().as_u16()
                ),
            })
        }
        Ok(_) => {
            if request_generation(&data, &settings.devify.base_url, &token)? != generation {
                return Err("登录状态已改变，请重新检查。".into());
            }
            Ok(DevifyAuthStatus { authenticated: true, unavailable: false, error: String::new() })
        },
        Err(error) if devify_unreachable(&error) => {
            if request_generation(&data, &settings.devify.base_url, &token)? != generation {
                return Err("登录状态已改变，请重新检查。".into());
            }
            Ok(DevifyAuthStatus {
                authenticated: false,
                unavailable: true,
                error: devify_network_message("检查登录状态失败", &error),
            })
        }
        // Unknown failures (TLS, proxy, body) keep the stored session: a local
        // problem must not look like a logout.
        Err(error) => {
            if request_generation(&data, &settings.devify.base_url, &token)? != generation {
                return Err("登录状态已改变，请重新检查。".into());
            }
            Ok(DevifyAuthStatus { authenticated: false, unavailable: true, error: devify_network_message("检查登录状态失败", &error) })
        }
    }
}

#[derive(Serialize)]
struct DevifyDeviceResult {
    device_id: String,
    label: String,
    platform: String,
    status: String,
    #[serde(skip)]
    generation: u64,
}

/// Bind or refresh this machine on Devify without forcing a new browser login.
///
/// Tokens issued before device binding existed still authenticate, so the
/// client calls this on startup and after login to make the device appear in
/// the web console's 设备与设置 page.
#[tauri::command]
async fn devify_device_register(app: AppHandle) -> Result<DevifyDeviceResult, String> {
    let mut result = devify_device_register_for(app.clone(), None).await?;
    let data = app_data(&app);
    let base = settings::load(&data)?.devify.resolved_base_url()?;
    let (_, generation) = establish_device_session(&data, &base, &result).await?;
    result.generation = generation;
    Ok(result)
}

async fn establish_device_session(data: &Path, base: &str, result: &DevifyDeviceResult) -> Result<(desktop_credentials::Credentials, u64), String> {
    let _refresh_guard = CREDENTIAL_REFRESH_LOCK.lock().await;
    let config = settings::load(data)?;
    let (mut credentials, generation) = desktop_credentials::load_with_generation(data, base)?;
    if generation != result.generation || config.devify.resolved_base_url()? != base
        || config.devify.device_id != result.device_id {
        return Err("设备登记的登录状态已改变。".into());
    }
    let mut committed_generation = generation;
    if credentials.device_session.is_none() {
        let original = credentials.clone();
        let response = http_client::strict().post(format!("{base}/api/v1/auth/desktop/session"))
            .bearer_auth(&credentials.access)
            .json(&serde_json::json!({"device_id": result.device_id}))
            .send_traced_retrying(&data, diagnostics::Module::Auth, RetryPolicy::WRITE).await
            .map_err(|e| devify_network_message("建立设备登录会话失败", &e))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND { return Ok((credentials, generation)); }
        if !response.status().is_success() { return Err(format!("建立设备登录会话失败：{}", response.status())); }
        let value: serde_json::Value = response.json().await.map_err(|e| format!("读取设备会话失败：{e}"))?;
        let value = value.get("data").unwrap_or(&value);
        let secret = value.get("session").and_then(|v| v.as_str()).filter(|v| !v.is_empty()).ok_or("设备会话响应无效。")?;
        credentials.device_session = Some(secret.to_string());
        credentials.session_device_id = Some(result.device_id.clone());
        credentials.access = value.get("access").and_then(|v| v.as_str()).filter(|v| !v.is_empty()).ok_or("设备会话缺少访问令牌。")?.to_string();
        if let Some(refresh) = value.get("refresh").and_then(|v| v.as_str()).filter(|v| !v.is_empty()) {
            credentials.refresh = refresh.to_string();
        }
        if !desktop_credentials::save_if_current(&data, generation, &original, &credentials)? {
            return Err("登录状态已改变，请重试。".into());
        }
        committed_generation = generation.wrapping_add(1);
    }
    Ok((credentials, committed_generation))
}

async fn devify_device_register_for(app: AppHandle, expected: Option<(&desktop_credentials::Credentials, u64)>) -> Result<DevifyDeviceResult, String> {
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    let base = settings.devify.resolved_base_url()?;
    let token = if let Some((credentials, _)) = expected {
        credentials.access.clone()
    } else {
        devify_access_token_async(&data, &base).await?
    };
    let (credentials, generation) = desktop_credentials::load_with_generation(&data, &base)?;
    let current = settings::load(&data)?;
    if credentials.access != token || current.devify.resolved_base_url()? != base
        || expected.is_some_and(|(expected, expected_generation)| credentials != *expected || generation != expected_generation)
        || current.devify.device_id != settings.devify.device_id
        || current.devify.active_space_id != settings.devify.active_space_id
        || current.active_account_id != settings.active_account_id {
        return Err("设备登记的登录状态已改变。".into());
    }
    let mut device_id = settings.devify.device_id.clone();
    if device_id.is_empty() {
        device_id = access_token_claim(&token, "device_id").unwrap_or_default();
    }
    let platform = if settings.devify.platform.is_empty() {
        settings::default_platform()
    } else {
        settings.devify.platform.clone()
    };
    let device_label = settings::resolved_device_label(&settings.devify);
    let response = http_client::strict()
        .post(format!("{base}/api/v1/auth/desktop/devices"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "device_id": device_id.clone(),
            "label": device_label.clone(),
            "platform": platform.clone(),
        }))
        .send_traced_retrying(&data, diagnostics::Module::Auth, RetryPolicy::WRITE)
        .await
        .map_err(|e| devify_network_message("登记设备失败", &e))?;
    if !response.status().is_success() {
        return Err(format!("登记设备失败：{}", response.status()));
    }
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("读取设备响应失败：{e}"))?;
    let device = payload.get("data").unwrap_or(&payload);
    let device_id = device
        .get("device_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "设备响应缺少 device_id。".to_string())?
        .to_string();
    if access_token_claim(&token, "device_id").is_some_and(|claim| claim != device_id) {
        return Err("登记设备与登录设备不匹配。".into());
    }
    let devices = fixed_identity_get(&data, &base, &token, "/api/v1/auth/desktop/devices").await?;
    let items = devices.as_array().or_else(|| devices.get("list").and_then(serde_json::Value::as_array))
        .ok_or("无法确认当前设备登记。")?;
    if items.iter().filter(|item| item.get("device_id").and_then(serde_json::Value::as_str) == Some(device_id.as_str())
        && item.get("status").and_then(serde_json::Value::as_str) == Some("active")).count() != 1 {
        return Err("当前设备不属于该用户或已停用。".into());
    }
    // A heartbeat for an already registered device must not take the exclusive
    // identity lock: an upload can legitimately hold its read lease for minutes.
    if device_id == settings.devify.device_id
        && platform == settings.devify.platform
        && device_label == settings.devify.device_label
    {
        let (stored, current_generation) = desktop_credentials::load_with_generation(&data, &base)?;
        let current = settings::load(&data)?;
        if stored != credentials || current_generation != generation
            || current.devify.resolved_base_url()? != base
            || current.devify.device_id != device_id
            || current.devify.active_space_id != settings.devify.active_space_id
            || current.active_account_id != settings.active_account_id {
            return Err("设备登记的登录状态已改变。".into());
        }
        return Ok(DevifyDeviceResult {
            device_id,
            label: device.get("label").and_then(|value| value.as_str())
                .filter(|value| !value.is_empty()).unwrap_or(device_label.as_str()).to_string(),
            platform,
            status: device.get("status").and_then(|value| value.as_str()).unwrap_or("active").to_string(),
            generation,
        });
    }
    let device_changed = device_id != settings.devify.device_id;
    settings::update_identity(&data, |current| {
        let (stored, current_generation) = desktop_credentials::load_with_generation(&data, &base)?;
        if stored != credentials || current_generation != generation
            || current.devify.resolved_base_url()? != base
            || current.devify.device_id != settings.devify.device_id
            || current.devify.active_space_id != settings.devify.active_space_id
            || current.active_account_id != settings.active_account_id {
            return Err("设备登记的登录状态已改变。".into());
        }
        current.devify.device_id = device_id.clone();
        current.devify.platform = platform.clone();
        current.devify.device_label = device_label.clone();
        Ok(())
    })?;
    Ok(DevifyDeviceResult {
        device_id,
        label: device
            .get("label")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .unwrap_or(device_label.as_str())
            .to_string(),
        platform,
        status: device
            .get("status")
            .and_then(|value| value.as_str())
            .unwrap_or("active")
            .to_string(),
        generation: generation.wrapping_add(u64::from(device_changed)),
    })
}

/// Sign in with an AI mailbox username and password, then store the same
/// desktop credentials the browser callback produces.
#[tauri::command]
async fn devify_login_password(
    app: AppHandle,
    username: String,
    password: String,
) -> Result<DevifyAuthResult, String> {
    let username = username.trim();
    if username.is_empty() || password.is_empty() {
        return Err("请输入用户名和密码。".to_string());
    }
    let data = app_data(&app);
    recover_local_logout(&data)?;
    let auth_generation = desktop_credentials::begin_auth_attempt()?;
    let settings = settings::load(&data)?;
    let base_url = settings.devify.resolved_base_url()?;
    let response = http_client::strict()
        .post(format!("{base_url}/api/v1/auth/login"))
        .json(&serde_json::json!({ "username": username, "password": password }))
        .send_traced_retrying(&data, diagnostics::Module::Auth, RetryPolicy::WRITE)
        .await
        .map_err(|e| devify_network_message("连接服务端登录服务失败", &e))?;
    let status = response.status();
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("读取服务端登录响应失败：{e}"))?;
    if !status.is_success() {
        let detail = payload
            .get("data")
            .and_then(|value| {
                value
                    .get("non_field_errors")
                    .and_then(|errors| errors.as_array())
                    .and_then(|errors| errors.first())
                    .and_then(|error| error.as_str())
                    .or_else(|| value.get("detail").and_then(|error| error.as_str()))
            })
            .or_else(|| payload.get("message").and_then(|message| message.as_str()))
            .unwrap_or("用户名或密码不正确。");
        return Err(if detail.contains("Unable to log in") {
            "用户名或密码不正确。".to_string()
        } else {
            detail.to_string()
        });
    }
    let tokens = payload
        .get("data")
        .filter(|value| value.is_object())
        .unwrap_or(&payload);
    let access = tokens
        .get("access")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "登录响应缺少 access token。".to_string())?;
    let refresh = tokens
        .get("refresh")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "登录响应缺少 refresh token。".to_string())?;
    let credentials = desktop_credentials::Credentials {
        base_url: base_url.clone(), access: access.to_string(), refresh: refresh.to_string(), device_session: None, session_device_id: None,
    };
    settings::with_identity(&data, |current| {
        if current.devify.resolved_base_url()? != base_url
            || !desktop_credentials::save_if_generation_locked(&data, auth_generation, &credentials)? {
            return Err("服务端登录状态已改变，请重新登录。".into());
        }
        Ok(())
    })?;
    let committed_generation = auth_generation.wrapping_add(1);
    let device = devify_device_register_for(app.clone(), Some((&credentials, committed_generation))).await.ok();
    let mut device_generation = device.as_ref().map(|value| value.generation).unwrap_or(committed_generation);
    // Complete device-session migration before reporting a password login as
    // finished, preserving the same generation/identity checks as PKCE login.
    let mut credentials = credentials;
    if let Some(device) = &device {
        if let Ok((stored, generation)) = establish_device_session(&data, &base_url, device).await {
            credentials = stored;
            device_generation = generation;
        }
    }
    let mut completed_generation = None;
    let _ = ensure_active_space_for(&app, Some((&credentials, device_generation)), &mut completed_generation).await;
    if !desktop_credentials::load_with_generation(&data, &base_url).ok().as_ref()
        .is_some_and(|(stored, generation)| stored == &credentials && *generation == completed_generation.unwrap_or(device_generation)) {
        return Err("服务端登录状态已改变，请重新登录。".into());
    }
    config_runtime::activate_profile(&data)?;
    let service = app.clone();
    tauri::async_runtime::spawn(async move {
        refresh_entitlements_then_reconcile(&service).await;
    });
    Ok(DevifyAuthResult {
        device_id: device.map(|value| value.device_id).unwrap_or_default(),
        expires_in: 3600,
    })
}

/// Only short credential transactions use the identity lock. Network and packaging never hold it.
const LOGOUT_UPLOAD_DRAIN_BUDGET: Duration = Duration::from_millis(250);

#[tauri::command]
async fn devify_logout(app: AppHandle) -> Result<(), String> {
    let data = app_data(&app);
    let cleared = sign_out_local_session(&data).await?;
    tauri::async_runtime::spawn(async move {
        reconcile_local_service(&app).await;
        if let Some((base, credentials))=cleared {
            if let (Some(secret),Some(device_id))=(credentials.device_session,credentials.session_device_id) {
                let _=tokio::time::timeout(Duration::from_secs(8),http_client::strict().delete(format!("{base}/api/v1/auth/desktop/session/refresh"))
                    .json(&serde_json::json!({"device_id":device_id,"session":secret})).send()).await;
            }
        }
    });
    Ok(())
}

/// Clear the local session for a sign-out. An in-flight upload is abandoned instead of making
/// the user wait for the next batch: the next login resumes from the saved cursor, and an
/// archive that never reached the server is rebuilt from local messages. Returns the cleared
/// credentials so the caller can revoke the device session.
async fn sign_out_local_session(data: &Path) -> Result<Option<(String, desktop_credentials::Credentials)>, String> {
    abort_in_flight_uploads();
    let identity = tokio::time::timeout(LOGOUT_UPLOAD_DRAIN_BUDGET, UPLOAD_IDENTITY_LOCK.write()).await.map_err(|_|"登录状态正在更新，请稍后重试退出。")?;
    let settings = settings::load(data)?;
    let base = settings.devify.resolved_base_url().ok();
    let stored = base.as_deref().and_then(|base| desktop_credentials::load(data, base).ok());
    // Anything still active is marked interrupted, so the next login neither waits on it nor
    // reports it as a running upload.
    settings::update(data, |current| {
        current.runtime.logout_pending=true;
        for job in current.upload_queue.iter_mut() {
            abandon_upload_for_logout(job);
        }
        Ok(())
    })?;
    // Clearing bumps the credential generation, so a late write from the abandoned upload
    // fails its compare-and-set even when the lease could not be drained in time.
    desktop_credentials::clear_with_identity_lock(data)?;
    drop(identity);
    settings::update(data,|s|{s.runtime.logout_pending=false;Ok(())})?;
    Ok(base.and_then(|base| stored.map(|credentials| (base, credentials))))
}

/// A crash during logout must never silently restore the old login.
fn recover_local_logout(data:&Path)->Result<(),String>{
    if !settings::load(data)?.runtime.logout_pending {return Ok(());}
    settings::update_identity(data,|s|{
        if s.runtime.logout_pending {
            desktop_credentials::clear_with_identity_lock(data)?;
            for job in &mut s.upload_queue {abandon_upload_for_logout(job);}
            s.runtime.logout_pending=false;
        }
        Ok(())
    })?;Ok(())
}

/// Signing out ends uploads that are still in flight. A batch that never reached the server
/// is released for the next login; one that may exist keeps the existing remote lookup path
/// instead of a blind rebuild.
fn abandon_upload_for_logout(job: &mut settings::UploadJob) -> bool {
    if !matches!(job.status.as_str(), "queued" | "running" | "uploading" | "processing") {
        return false;
    }
    job.status = "failed".into();
    if job.create_started == Some(true) {
        job.message = "退出登录时上传已中断，重新登录后会自动核对云端批次。".into();
    } else {
        job.legacy_failure_confirmed = true;
        job.message = "退出登录时上传已中断，重新登录后会自动继续。".into();
    }
    job.updated_at = chrono::Utc::now().to_rfc3339();
    true
}

#[cfg(test)]
#[path = "tests/logout_queue.rs"]
mod logout_queue_tests;

#[cfg(test)]
#[path = "tests/legacy_key_scan.rs"]
mod legacy_key_scan_tests;

#[derive(Clone, Serialize)]
struct McpStatus {
    running: bool,
    address: String,
    endpoint: String,
    local_only: bool,
    error: Option<String>,
    /// Set when the listener is intentionally down (not signed in, no bridge capability,
    /// connector off). Kept apart from `error` so the UI can explain instead of alarm.
    reason: Option<String>,
}

struct RunningService {
    shutdown: tokio::sync::oneshot::Sender<()>,
    handle: tauri::async_runtime::JoinHandle<()>,
}

#[derive(Clone)]
struct McpState {
    status: Arc<Mutex<McpStatus>>,
    token: Option<String>,
    task: Arc<tokio::sync::Mutex<Option<RunningService>>>,
}

impl McpState {
    fn new(bind: &str, token: Option<String>) -> Self {
        let local_only = !mcp::requires_token_for_ui(bind);
        Self {
            status: Arc::new(Mutex::new(McpStatus {
                running: false,
                endpoint: format!("http://{bind}/sse"),
                address: bind.to_string(),
                local_only,
                error: None,
                reason: None,
            })),
            token,
            task: Arc::new(tokio::sync::Mutex::new(None)),
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

/// Why the listener is down when it is not an error.
fn local_service_idle_reason(data: &Path) -> String {
    let Ok(settings) = settings::load(data) else {
        return "无法读取本机设置，本机 Agent 服务不会启动。".to_string();
    };
    if !settings.connectors.wechat_enabled {
        return "微信连接已断开，本机 Agent 服务不会启动。".to_string();
    }
    let Ok(base_url) = settings.devify.resolved_base_url() else {
        return "服务端地址无效，本机 Agent 服务不会启动。".to_string();
    };
    if desktop_credentials::load(data, &base_url).is_err() {
        return "未登录，本机 Agent 服务不会启动。".to_string();
    }
    "当前套餐未包含本地 Agent 集成，本机服务不会启动。".to_string()
}

/// Start or stop the loopback listener so it exists only while the account is signed in and
/// its plan carries a bridge capability. Called after login, logout, entitlement refresh,
/// settings changes and on the periodic tick, so the port tracks state instead of the app's
/// lifetime.
async fn reconcile_local_service(app: &AppHandle) {
    let state = app.state::<McpState>();
    let data = app_data(app);
    let allowed = mcp::bridge_listen_allowed(&data);
    let mut slot = state.task.lock().await;

    if allowed {
        if slot.is_some() {
            if let Ok(mut status) = state.status.lock() {
                status.reason = None;
            }
            return;
        }
        let Some(token) = state.token.clone() else {
            if let Ok(mut status) = state.status.lock() {
                status.running = false;
                status.reason = Some("无法创建本机访问 Token，本机服务不会启动。".to_string());
            }
            return;
        };
        let bind = mcp::configured_bind();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let status = state.status.clone();
        let start_data = data.clone();
        let handle = tauri::async_runtime::spawn(async move {
            let result = mcp::start(start_data, bind, Some(token), Some(ready_tx), Some(shutdown_rx)).await;
            if let Err(error) = result {
                if let Ok(mut status) = status.lock() {
                    status.running = false;
                    status.error = Some(error);
                }
            }
        });
        match tokio::time::timeout(Duration::from_secs(5), ready_rx).await {
            Ok(Ok(Ok(()))) => {
                *slot = Some(RunningService { shutdown: shutdown_tx, handle });
                if let Ok(mut status) = state.status.lock() {
                    status.running = true;
                    status.error = None;
                    status.reason = None;
                }
            }
            Ok(Ok(Err(_))) => {
                // `start` already reported the bind error through the ready channel and the
                // spawned task; nothing else to record here.
            }
            Ok(Err(_)) | Err(_) => {
                if let Ok(mut status) = state.status.lock() {
                    status.running = false;
                    status.error = Some("本机服务启动超时。".to_string());
                }
            }
        }
    } else {
        if let Some(running) = slot.take() {
            let _ = running.shutdown.send(());
            let _ = tokio::time::timeout(Duration::from_secs(5), running.handle).await;
        }
        if let Ok(mut status) = state.status.lock() {
            status.running = false;
            status.error = None;
            status.reason = Some(local_service_idle_reason(&data));
        }
    }
}

/// Refresh the snapshot after a login and then reconcile, so a restored session picks up a
/// plan change without waiting for the frontend's first refresh.
async fn refresh_entitlements_then_reconcile(app: &AppHandle) {
    let data = app_data(app);
    if let Ok(settings) = settings::load(&data) {
        if let Ok(base_url) = settings.devify.resolved_base_url() {
            if let Ok(token) = devify_access_token_async(&data, &base_url).await {
                let _ = entitlements::refresh(&data, &base_url, &token).await;
            }
        }
    }
    reconcile_local_service(app).await;
}

/// The agent access kit installs a read-only CLI into other agents; it needs both an
/// internal account and the bridge capability. A missing snapshot means "no".
fn require_internal_agent(data: &std::path::Path, base_url: &str) -> Result<(), String> {
    entitlements::require_agent_access(data, base_url)
}

#[tauri::command]
fn agent_access_status(
    app: AppHandle,
    state: State<McpState>,
) -> Result<access::AccessStatus, String> {
    let service = access_state(&state)?;
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    require_internal_agent(&data, &settings.devify.base_url)?;
    let configured = settings.agent.provider_id;
    Ok(access::status(
        &data,
        &configured,
        &service.address,
        service.running,
        service.reason,
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
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    require_internal_agent(&data, &settings.devify.base_url)?;
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
    let wxq = access::resolve_wxq(&data);
    let base_url = access::client_base(&service.address);
    let prompt = access::build_prompt(
        &target,
        &bin,
        &skills,
        &base_url,
        &mcp::token_path(&data),
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
async fn agent_build_wxq(app: AppHandle) -> Result<String, String> {
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    require_internal_agent(&data, &settings.devify.base_url)?;
    let Some(dir) = access::source_dir() else {
        return Err("当前安装没有附带源码，无法在应用内构建 wxq。".into());
    };
    let build_dir = dir.clone();
    let output = tauri::async_runtime::spawn_blocking(move || {
        std::process::Command::new("cargo")
            .args([
                "build",
                "--release",
                "--bin",
                access::CLI_NAME,
                "--features",
                "aux-binaries",
            ])
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
    platform: String,
    arch: String,
    os_version: String,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        name: "AIMyChats".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        os_version: feedback::os_version(),
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

/// Shown whenever a read is blocked because the user disconnected WeChat.
const WECHAT_DISCONNECTED: &str = "微信连接已断开，重新连接后才能读取聊天。已上传的记录不受影响。";

fn allowed_usernames(app: &AppHandle, account_id: &str) -> Result<HashSet<String>, String> {
    validate_account_id(account_id)?;
    let settings = settings::load(&app_data(app))?;
    if !settings.connectors.wechat_enabled {
        return Err(WECHAT_DISCONNECTED.into());
    }
    Ok(config_runtime::browse_names(&settings,account_id).into_iter().collect())
}

/// Whitelist for a WhatsApp account. Same policy shape as WeChat, separate connect switch.
fn whatsapp_allowed_usernames(app: &AppHandle, account_id: &str) -> Result<HashSet<String>, String> {
    validate_account_id(account_id)?;
    let settings = settings::load(&app_data(app))?;
    if !settings.connectors.whatsapp_enabled {
        return Err("WhatsApp 已断开，已停止读取本机消息。".into());
    }
    if whatsapp::account().is_none_or(|a|a.id!=account_id) {return Err("WhatsApp 登录账号已变化，请重新完成连接向导。".into());}
    Ok(config_runtime::browse_names(&settings,account_id).into_iter().collect())
}

const DAILY_TOPIC_TITLE: &str = "微信工作日报";
const DAILY_TOPIC_INSTRUCTION: &str =
    "根据已上传的微信聊天记录生成每日工作日报，汇总当天进展、待办和风险。";
const DAILY_SCHEDULE_RETRY_SECONDS: i64 = 600;

#[derive(Serialize)]
struct DailyScheduleStatus {
    topic_id: Option<i64>,
    enabled: bool,
    time_local: String,
    timezone: String,
    last_run_at: Option<String>,
    last_error: Option<String>,
    message: String,
}

static DAILY_SCHEDULE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct DailyIdentity {
    data: PathBuf,
    base: String,
    credentials: desktop_credentials::Credentials,
    generation: u64,
    space_id: String,
    account_id: String,
    device_id: String,
    enabled: bool,
    time_local: String,
    topic_id: Option<i64>,
    topic_space_id: Option<String>,
}

impl DailyIdentity {
    fn validate_settings(&self, current: &settings::AppSettings) -> Result<(), String> {
        let (credentials, generation) = desktop_credentials::load_with_generation(&self.data, &self.base)?;
        if credentials != self.credentials || generation != self.generation
            || current.devify.resolved_base_url()? != self.base
            || current.devify.active_space_id.as_deref() != Some(self.space_id.as_str())
            || current.devify.device_id != self.device_id
            || current.active_account_id != self.account_id
            || (current.automation.enabled && current.automation.daily_summary_enabled) != self.enabled
            || current.automation.daily_time.trim() != self.time_local
            || current.automation.daily_topic_id != self.topic_id
            || current.automation.daily_topic_space_id != self.topic_space_id {
            return Err("日报计划的登录状态或配置已改变，请重试。".into());
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), String> { self.validate_settings(&settings::load(&self.data)?) }
}

async fn daily_request(context: &DailyIdentity, method: reqwest::Method, path: String, body: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    context.validate()?;
    let client = http_client::strict();
    let policy = RetryPolicy::for_method(&method);
    let mut request = client.request(method, format!("{}{path}", context.base)).bearer_auth(&context.credentials.access);
    if let Some(body) = body { request = request.json(&body); }
    let response = request.send_traced_retrying(&context.data, diagnostics::Module::App, policy).await;
    context.validate()?;
    let mut response = response.map_err(|_| "日报请求失败，请检查网络连接。")?;
    if !response.status().is_success() { return Err(format!("日报请求失败：HTTP {}。", response.status().as_u16())); }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "无法读取日报响应。")? {
        append_retry_identity_body(&mut body, &chunk)?;
    }
    context.validate()?;
    let payload: serde_json::Value = serde_json::from_slice(&body).map_err(|_| "日报响应格式无效。")?;
    Ok(payload.get("data").cloned().unwrap_or(payload))
}

fn daily_stale_topic_warning(value: &settings::AppSettings, space_id: &str) -> Option<&'static str> {
    (value.automation.daily_topic_id.is_some() && value.automation.daily_topic_space_id.as_deref() != Some(space_id))
        .then_some("旧空间日报计划无法确认归属，请先在服务端检查并关闭。")
}

fn local_timezone() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "Asia/Shanghai".to_string())
}

fn daily_schedule_signature(value: &settings::AppSettings, token: &str) -> String {
    let fingerprint = URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(token.as_bytes()));
    format!(
        "{}|{}|{}|{}|{}",
        value.automation.enabled && value.automation.daily_summary_enabled,
        value.automation.daily_time.trim(),
        value.devify.active_space_id.as_deref().unwrap_or_default(),
        local_timezone(),
        fingerprint,
    )
}

fn schedule_attempt_is_recent(value: &settings::AppSettings) -> bool {
    value
        .automation
        .last_schedule_sync_at
        .as_deref()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .is_some_and(|at| {
            Utc::now()
                .signed_duration_since(at.with_timezone(&Utc))
                .num_seconds()
                < DAILY_SCHEDULE_RETRY_SECONDS
        })
}

/// Create or update the server-side TopicSubscription that runs the daily report.
///
/// The schedule lives on Devify so it still runs when the desktop app is closed.
/// The app only keeps the returned topic id and the last known run status.
async fn sync_daily_schedule(
    app: &AppHandle,
    force: bool,
) -> Result<DailyScheduleStatus, String> {
    let _schedule = DAILY_SCHEDULE_LOCK.lock().await;
    let data = app_data(app);
    let initial = settings::load(&data)?;
    let base = initial.devify.resolved_base_url()?;
    let token = devify_access_token_async(&data, &base).await?;
    let value = settings::load(&data)?;
    let (credentials, generation) = desktop_credentials::load_with_generation(&data, &base)?;
    if value.devify.resolved_base_url()? != base || credentials.access != token {
        return Err("日报计划的登录状态已改变，请重试。".into());
    }
    let space_id = value.devify.active_space_id.clone()
        .ok_or("请先在「设置 / 账户」中登录并选择 空间。")?;
    validate_devify_space(&value, &space_id)?;
    let enabled = value.automation.enabled && value.automation.daily_summary_enabled;
    let time_local = value.automation.daily_time.trim().to_string();
    if !settings::is_valid_time(&time_local) {
        return Err("日报时间必须使用 HH:MM 格式。".into());
    }
    let timezone = local_timezone();
    // A cache entry from another login must never suppress the new user's sync.
    let signature = daily_schedule_signature(&value, &credentials.access);
    let current_topic_id = value.automation.daily_topic_id;
    let context = DailyIdentity {
        data: data.clone(), base, credentials, generation, space_id: space_id.clone(),
        account_id: value.active_account_id.clone(), device_id: value.devify.device_id.clone(),
        enabled, time_local: time_local.clone(), topic_id: value.automation.daily_topic_id,
        topic_space_id: value.automation.daily_topic_space_id.clone(),
    };
    context.validate()?;
    if let Some(warning) = daily_stale_topic_warning(&value, &space_id) {
        settings::update(&data, |stored| {
            context.validate_settings(stored)?;
            stored.automation.last_schedule_error = Some(warning.to_string());
            Ok(())
        })?;
        return Err(warning.into());
    }
    if !force
        && value.automation.last_schedule_signature.as_deref() == Some(signature.as_str())
        && schedule_attempt_is_recent(&value)
    {
        return Ok(DailyScheduleStatus {
            topic_id: current_topic_id,
            enabled,
            time_local,
            timezone,
            last_run_at: value.automation.last_daily_run.clone(),
            last_error: value.automation.last_schedule_error.clone(),
            message: "日报计划已是服务端最新状态。".into(),
        });
    }

    let attempt_at = Utc::now().to_rfc3339();
    let result: Result<serde_json::Value, String> = async {
        let topics = daily_request(
            &context,
            reqwest::Method::GET,
            format!("/api/v1/spaces/{space_id}/topics"),
            None,
        )
        .await?;
        let existing = topics.as_array().and_then(|items| {
            items.iter().find(|topic| {
                topic.get("id").and_then(|value| value.as_i64()) == current_topic_id
                    || topic.get("title").and_then(|value| value.as_str())
                        == Some(DAILY_TOPIC_TITLE)
            })
        });
        if let Some(topic) = existing {
            let topic_id = topic
                .get("id")
                .and_then(|value| value.as_i64())
                .ok_or("服务端主题缺少标识。")?;
            let body = serde_json::json!({
                "period": "day",
                "time_local": time_local.clone(),
                "timezone": timezone.clone(),
                "enabled": enabled,
                "include_open": true,
            });
            daily_request(
                &context,
                reqwest::Method::PATCH,
                format!("/api/v1/topics/{topic_id}"),
                Some(body),
            )
            .await
        } else if enabled {
            let body = serde_json::json!({
                "title": DAILY_TOPIC_TITLE,
                "instruction": DAILY_TOPIC_INSTRUCTION,
                "period": "day",
                "time_local": time_local.clone(),
                "timezone": timezone.clone(),
                "enabled": true,
                "include_open": true,
            });
            daily_request(
                &context,
                reqwest::Method::POST,
                format!("/api/v1/spaces/{space_id}/topics"),
                Some(body),
            )
            .await
        } else {
            Ok(serde_json::json!({}))
        }
    }
    .await;

    context.validate()?;
    match result {
        Ok(topic) => {
            let stored = settings::update(&data, |stored| {
                context.validate_settings(stored)?;
                stored.automation.last_schedule_sync_at = Some(attempt_at);
                stored.automation.daily_topic_id = topic.get("id").and_then(|value| value.as_i64()).or(current_topic_id);
                stored.automation.daily_topic_space_id = Some(space_id);
                stored.automation.last_schedule_signature = Some(signature);
                stored.automation.last_schedule_error = None;
                if let Some(run) = topic.get("last_run_at").and_then(|value| value.as_str()) {
                    stored.automation.last_daily_run = Some(run.to_string());
                }
                stored.automation.last_daily_error = topic.get("last_error").and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty()).map(str::to_string);
                stored.automation.last_daily_status = Some(if enabled { "scheduled" } else { "disabled" }.to_string());
                Ok(())
            })?;
            Ok(DailyScheduleStatus {
                topic_id: stored.automation.daily_topic_id,
                enabled,
                time_local,
                timezone,
                last_run_at: stored.automation.last_daily_run.clone(),
                last_error: stored.automation.last_daily_error.clone(),
                message: if enabled { "日报计划已同步到服务端。".into() } else { "日报已关闭。".into() },
            })
        }
        Err(error) => {
            settings::update(&data, |stored| {
                context.validate_settings(stored)?;
                stored.automation.last_schedule_sync_at = Some(attempt_at);
                stored.automation.last_schedule_error = Some(error.clone());
                Ok(())
            })?;
            Err(error)
        }
    }
}

async fn daily_schedule_tick(app: &AppHandle) {
    let Ok(value) = settings::load(&app_data(app)) else {
        return;
    };
    if value.devify.base_url.trim().is_empty() || value.devify.active_space_id.is_none() {
        return;
    }
    let enabled = value.automation.enabled && value.automation.daily_summary_enabled;
    if !enabled && value.automation.daily_topic_id.is_none() {
        return;
    }
    let credentials = desktop_credentials::load(&app_data(app), &value.devify.base_url).ok();
    if let Some(credentials) = credentials {
        let signature = daily_schedule_signature(&value, &credentials.access);
        if daily_stale_topic_warning(&value, value.devify.active_space_id.as_deref().unwrap_or_default()).is_none()
            && value.automation.last_schedule_signature.as_deref() == Some(signature.as_str())
            && schedule_attempt_is_recent(&value) {
            return;
        }
    }
    let _ = sync_daily_schedule(app, false).await;
}

#[tauri::command]
async fn devify_daily_schedule_sync(
    app: AppHandle,
) -> Result<DailyScheduleStatus, String> {
    sync_daily_schedule(&app, true).await
}

/// Scheduler tick for the backup plan saved in settings. Uploads only when the plan says a
/// run is due (interval elapsed, scheduled time reached, and the window allows it).
fn continuation_sessions(value: &settings::AppSettings, account_id: &str) -> Vec<String> {
    let allowed = value.chat.allowed_usernames.get(account_id);
    let mut names: Vec<_> = value.sync.continuation.get(account_id).into_iter().flat_map(|items| items.iter()).filter(|(name, scope)| (scope.manual == Some(true) || value.sync.enabled) && Some(scope.space_id.as_str()) == value.devify.active_space_id.as_deref() && settings::same_service_url(&scope.base_url, &value.devify.base_url) && allowed.is_some_and(|list| list.contains(name))).map(|(name, _)| name.clone()).collect();
    names.sort();
    names
}

fn initial_progress_matches(chunk: &settings::InitialChunk, space_id: &str, base_url: &str) -> bool {
    chunk.space_id == space_id && settings::same_service_url(&chunk.base_url, base_url)
}

#[derive(Serialize)]
struct ChatSyncSummary {
    status: String,
    uploaded: usize,
    skipped: usize,
    message_count: usize,
    batch_ids: Vec<i64>,
    error: Option<String>,
}

/// Bring queued upload jobs in line with the server before the next sync, so the local
/// "not uploaded yet" marks clear once parsing finishes.
fn apply_completed_history_baselines(value: &mut settings::AppSettings) {
    for job in &mut value.upload_queue {
        if job.status != "completed" { continue; }
        let entries = std::mem::take(&mut job.initial_baselines);
        if entries.is_empty() { continue; }
        let cursors = value.sync.cursors.entry(job.account_id.clone()).or_default();
        for (username, position) in entries { cursors.entry(username).or_insert(position); }
    }
}

fn apply_completed_uploads(value: &mut settings::AppSettings) {
    for job in &mut value.upload_queue {
        if job.status != "completed" { continue; }
        if job.continue_sync {
            if let Some(scope) = &job.scope {
                let entries = value.sync.continuation.entry(job.account_id.clone()).or_default();
                for username in &scope.usernames {
                    entries.insert(username.clone(), settings::ContinuationScope { space_id: job.space_id.clone(), base_url: scope.base_url.clone(), manual: job.continuation_manual });
                }
            }
            job.continue_sync = false;
        }
        for (username, chunk) in std::mem::take(&mut job.initial_chunks) {
            if chunk.final_chunk {
                let cursors = value.sync.cursors.entry(job.account_id.clone()).or_default();
                let replace = cursors.get(&username).map_or(true, |old| (chunk.baseline.time, chunk.baseline.local_id) > (old.time, old.local_id)
                    || ((chunk.baseline.time, chunk.baseline.local_id) == (old.time, old.local_id) && chunk.baseline.boundary_count > old.boundary_count));
                if replace { cursors.insert(username.clone(), chunk.baseline); }
                if let Some(progress) = value.sync.initial_progress.get_mut(&job.account_id) {
                    progress.remove(&username);
                    if progress.is_empty() { value.sync.initial_progress.remove(&job.account_id); }
                }
            } else {
                value.sync.initial_progress.entry(job.account_id.clone()).or_default().insert(username, chunk);
            }
        }
        if job.sync_advance {
            let cursors = value.sync.cursors.entry(job.account_id.clone()).or_default();
            for (username, position) in &job.covered {
                let replace = cursors.get(username).map_or(true, |old| (position.time, position.local_id) > (old.time, old.local_id) || ((position.time, position.local_id) == (old.time, old.local_id) && position.boundary_count > old.boundary_count));
                if replace { cursors.insert(username.clone(), *position); }
            }
            job.sync_advance = false;
        }
        let processed = std::mem::take(&mut job.voice_processed);
        let pending = std::mem::take(&mut job.voice_pending);
        if let Some(scope) = &job.scope {
            update_pending_voice(&mut value.sync, &job.account_id, &scope.usernames, &processed, &pending);
        }
    }
}

// This digest binds the locally stored last-confirmed coverage to the complete immutable
// ownership/scope snapshot. It is not a live guarantee against later server deletion.
fn upload_completion_digest(job: &settings::UploadJob) -> Option<String> {
    let owner = job.owner.as_ref()?;
    let scope = job.scope.as_ref()?;
    if job.status != "completed" || job.id.is_empty() || job.batch_id.filter(|id| *id > 0).is_none()
        || job.account_id != owner.account_id || job.space_id != owner.space_id
        || owner.user_id <= 0 || owner.space_id.trim().is_empty() || owner.account_id.is_empty()
        || owner.device_id.is_empty() || owner.device_id.trim() != owner.device_id
        || !settings::same_service_url(&scope.base_url, &owner.base_url) {
        return None;
    }
    let bytes = serde_json::to_vec(&("verified-completion-v1", &job.id, job.batch_id, owner,
        &job.account_id, &job.space_id, scope, &job.covered, &job.status, &job.created_at, &job.updated_at)).ok()?;
    Some(hex::encode(sha2::Sha256::digest(bytes)))
}

fn upload_completion_is_verified(job: &settings::UploadJob) -> bool {
    job.verified_completion_digest.as_ref().is_some_and(|saved| upload_completion_digest(job).as_ref() == Some(saved))
}

fn upload_job_has_pending_sync(job: &settings::UploadJob) -> bool {
    !job.initial_baselines.is_empty() || !job.initial_chunks.is_empty() || job.sync_advance
        || job.continue_sync || !job.voice_processed.is_empty() || !job.voice_pending.is_empty()
}

fn upload_job_needs_reconciliation(job: &settings::UploadJob) -> bool {
    matches!(job.status.as_str(), "queued" | "running" | "processing" | "uploading")
        || (matches!(job.status.as_str(), "failed" | "cancelled")
            && job.batch_id.is_some() && !job.legacy_failure_confirmed)
        || (job.status == "completed" && (upload_job_has_pending_sync(job) || !upload_completion_is_verified(job)))
}

fn is_unlinked_create_failure(job: &settings::UploadJob, owner: &settings::UploadOwner) -> bool {
    matches!(job.status.as_str(), "failed" | "running" | "uploading") && job.can_recover_create()
        && job.owner.as_ref() == Some(owner)
        && job.account_id == owner.account_id && job.space_id == owner.space_id
        && job.scope.as_ref().is_some_and(|scope| {
            !scope.usernames.is_empty() && settings::same_service_url(&scope.base_url, &owner.base_url)
        })
}

#[derive(Debug, PartialEq)]
enum CreateRecovery {
    ContinueSync,
    ReplanLegacyCollision,
    Link(i64),
    Submit(i64),
}

fn legacy_create_key_collision(job: &settings::UploadJob) -> bool {
    job.id.starts_with("sync-") && job.batch_id.is_none() && job.status == "failed" && job.message.contains("409")
        && job.create_key.as_deref().is_some_and(|key| key.starts_with("whatsapp.v1-") || key.starts_with("wechat-"))
}

fn legacy_recovery_matches(owner: &settings::UploadOwner, job: &settings::UploadJob, batch: &serde_json::Value) -> Result<bool, String> {
    let id = batch.get("id").and_then(serde_json::Value::as_i64).ok_or("批次列表缺少有效标识。")?;
    let exact = if let Some(key) = &job.create_key {
        batch.get("idempotency_key").and_then(serde_json::Value::as_str) == Some(key)
    } else {
        let digest = job.package_digest.as_deref().ok_or("缺少旧归档摘要。")?;
        batch.get("archive_sha256").and_then(serde_json::Value::as_str) == Some(digest)
    };
    Ok(exact && validate_remote_owner(owner, id, batch, job.create_key.as_deref()).is_ok())
}

// Older servers already expose the same archive digest and ownership fields on
// the upload list. A partial scan or a changing list cannot prove absence.
async fn legacy_create_lookup(data: &Path, owner: &settings::UploadOwner, token: &str, job: &settings::UploadJob) -> Result<serde_json::Value, String> {
    let mut seen = HashSet::new();
    let mut expected_total = None;
    let mut found = None;
    for page in 1..=20 {
        let path = format!("/api/v1/spaces/{}/chat/uploads?page={page}&page_size=50", owner.space_id);
        let data_page = fixed_identity_get(data, &owner.base_url, token, &path).await?;
        let rows = data_page.get("list").and_then(serde_json::Value::as_array).ok_or("批次列表格式无效。")?;
        let total = data_page.pointer("/pagination/total").and_then(serde_json::Value::as_u64).ok_or("批次列表缺少分页总数。")?;
        if expected_total.replace(total).is_some_and(|previous| previous != total) {
            return Err("云端批次列表正在变更，将在下次同步时重新核对。".into());
        }
        for batch in rows {
            let id = batch.get("id").and_then(serde_json::Value::as_i64).filter(|id| *id > 0).ok_or("批次列表缺少有效标识。")?;
            if !seen.insert(id) {
                return Err("云端批次列表正在变更，将在下次同步时重新核对。".into());
            }
            if legacy_recovery_matches(owner, job, batch)? && found.replace(batch.clone()).is_some() {
                return Err("存在多个匹配的云端批次，无法安全恢复。".into());
            }
        }
        let next = data_page.pointer("/pagination/next").ok_or("批次列表缺少分页信息。")?;
        if next.is_null() {
            if seen.len() as u64 != total {
                return Err("云端批次列表不完整，无法确认旧上传。".into());
            }
            return match found {
                None => Ok(serde_json::json!({"batch": null})),
                Some(batch) => {
                    let received = batch.get("archive_received_at").ok_or("批次列表缺少归档状态。")?;
                    Ok(serde_json::json!({"archive_received": !received.is_null(), "batch": batch}))
                }
            };
        }
        if next.as_str().is_none() {
            return Err("批次列表分页信息无效。".into());
        }
    }
    Err("旧服务器的批次列表超出安全核对范围。".into())
}

fn resolve_unlinked_create(owner: &settings::UploadOwner, job: &settings::UploadJob, lookup: &serde_json::Value) -> Result<CreateRecovery, String> {
    if !is_unlinked_create_failure(job, owner) {
        return Err("旧上传任务的身份或范围无法确认。".into());
    }
    let Some(batch) = lookup.get("batch") else { return Err("服务端未返回旧任务核对结果。".into()); };
    if batch.is_null() { return Ok(CreateRecovery::ContinueSync); }
    let batch_id = batch.get("id").and_then(serde_json::Value::as_i64).ok_or("核对结果缺少批次 ID。")?;
    validate_remote_owner(owner, batch_id, batch, job.create_key.as_deref())?;
    let remote_digest = batch.get("archive_sha256").and_then(serde_json::Value::as_str).filter(|value| !value.is_empty());
    let received = lookup.get("archive_received").and_then(serde_json::Value::as_bool).ok_or("核对结果缺少归档状态。")?;
    let status = batch.get("status").and_then(serde_json::Value::as_str).unwrap_or("");
    if job.create_key.is_none() || received || matches!(status, "processing" | "completed") {
        if remote_digest.is_none() || remote_digest != job.package_digest.as_deref() {
            if status == "completed" && remote_digest.is_some() && job.package_digest.is_some()
                && legacy_create_key_collision(job) {
                // This old content-derived key belongs to another completed
                // archive. Preserve the failed intent and re-read current scope;
                // never adopt its batch or advance this job's checkpoints.
                return Ok(CreateRecovery::ReplanLegacyCollision);
            }
            return Err("云端归档与旧任务的摘要不一致。".into());
        }
    }
    match status {
        "processing" | "completed" => Ok(CreateRecovery::Link(batch_id)),
        "created" | "uploading" if received => Ok(CreateRecovery::Submit(batch_id)),
        // Without a persisted batch ID the original client could not send an archive.
        // A blank or terminal batch does not advance the cursor; sync checks the
        // current selection and the server deduplicates messages at ingestion.
        "created" | "uploading" | "failed" | "cancelled" => Ok(CreateRecovery::ContinueSync),
        _ => Err("无法确认旧任务的云端状态。".into()),
    }
}

async fn reconcile_unlinked_creates(data: &Path, owner: &settings::UploadOwner, token: &str, usernames: &[String]) {
    static LAST_JOB_ID: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let deadline = tokio::time::Instant::now() + RECONCILE_TOTAL_BUDGET;
    let Ok(value) = settings::load(data) else { return; };
    let jobs: Vec<_> = value.upload_queue.into_iter().filter(|job| {
        is_unlinked_create_failure(job, owner)
            && job.scope.as_ref().is_some_and(|scope| scope.usernames.iter().any(|name| usernames.contains(name)))
    }).collect();
    for _ in 0..jobs.len().min(RECONCILE_MAX_BATCH_CHECKS) {
        if tokio::time::Instant::now() >= deadline { break; }
        let job = {
            let Ok(mut last) = LAST_JOB_ID.lock() else { break; };
            let Some(job) = next_reconcile_job(&jobs, last.as_deref()) else { break; };
            *last = Some(job.id.clone());
            job
        };
        let until = deadline.min(tokio::time::Instant::now() + RECONCILE_BATCH_TIMEOUT);
        let _ = tokio::time::timeout_at(until, async {
            validate_live_owned_job(data, &job.id, owner, token, job.scope.as_ref().ok_or("缺少旧任务范围。")?)?;
            let path = {
                let mut query = url::form_urlencoded::Serializer::new(String::new());
                query.append_pair("source_account_id", &owner.account_id).append_pair("device_id", &owner.device_id);
                if let Some(key) = &job.create_key {
                    query.append_pair("idempotency_key", key);
                } else {
                    query.append_pair("archive_sha256", job.package_digest.as_deref().ok_or("缺少归档摘要。")?);
                }
                format!("/api/v1/spaces/{}/chat/uploads/recovery?{}", owner.space_id, query.finish())
            };
            let lookup = match fixed_identity_get(data, &owner.base_url, token, &path).await {
                Err(error) if error == "无法读取云端数据：HTTP 404。" => legacy_create_lookup(data, owner, token, job).await?,
                result => result?,
            };
            let result = resolve_unlinked_create(owner, job, &lookup)?;
            if let CreateRecovery::Submit(id) = result {
                validate_live_owned_job(data, &job.id, owner, token, job.scope.as_ref().ok_or("缺少旧任务范围。")?)?;
                let response = http_client::upload().post(format!("{}/api/v1/chat/uploads/{id}/complete", owner.base_url))
                    .bearer_auth(token).send_traced(data, diagnostics::Module::Upload).await.map_err(|e| e.to_string())?;
                if !response.status().is_success() { return Err("旧归档提交尚未完成。".into()); }
            }
            settings::update(data, |current| {
                validate_owner_state(current, owner, token)?;
                if desktop_credentials::load(data, &owner.base_url)?.access != token {
                    return Err("登录状态已改变。".into());
                }
                let live = current.upload_queue.iter_mut().find(|item| item.id == job.id).ok_or("旧任务已删除。")?;
                if !is_unlinked_create_failure(live, owner) || live.scope != job.scope
                    || live.updated_at != job.updated_at || live.package_digest != job.package_digest || live.create_key != job.create_key {
                    return Err("旧任务状态已改变。".into());
                }
                match result {
                    CreateRecovery::ReplanLegacyCollision => {
                        live.status = "cancelled".into();
                        live.message = "已核对旧批次标识冲突，将按当前范围重新安排备份。".into();
                    }
                    CreateRecovery::ContinueSync => {
                        live.status = "failed".into();
                        live.legacy_failure_confirmed = true;
                        live.message = "已核对旧上传，后续同步可以继续。".into();
                    }
                    CreateRecovery::Link(id) | CreateRecovery::Submit(id) => {
                        live.batch_id = Some(id);
                        live.status = "processing".into();
                        live.message = "已找回原上传批次，正在核对结果。".into();
                    }
                }
                live.updated_at = chrono::Utc::now().to_rfc3339();
                Ok(())
            })?;
            Ok::<(), String>(())
        }).await;
    }
}

const RECONCILE_MAX_BATCH_CHECKS: usize = 8;
const RECONCILE_TOTAL_BUDGET: Duration = Duration::from_secs(20);
const RECONCILE_BATCH_TIMEOUT: Duration = Duration::from_secs(8);

// Advance by the last *attempted* ID, not a fixed page size: a timed-out round may
// manage only one request. With a stable pending set every job eventually gets a turn.
fn next_reconcile_job<'a>(jobs: &'a [settings::UploadJob], last: Option<&str>) -> Option<&'a settings::UploadJob> {
    jobs.iter().filter(|job| last.is_some_and(|id| job.id.as_str() > id))
        .min_by(|a, b| a.id.cmp(&b.id))
        .or_else(|| jobs.iter().min_by(|a, b| a.id.cmp(&b.id)))
}


async fn reconcile_owned_round(data: &Path, owner: &settings::UploadOwner, token: &str,
    pending: &[settings::UploadJob], last_id: &std::sync::Mutex<Option<String>>,
    deadline: tokio::time::Instant) -> usize {
    let mut attempted = 0;
    for _ in 0..pending.len().min(RECONCILE_MAX_BATCH_CHECKS) {
        if tokio::time::Instant::now() >= deadline { break; }
        let job = {
            let Ok(mut last) = last_id.lock() else { break; };
            let Some(job) = next_reconcile_job(pending, last.as_deref()) else { break; };
            *last = Some(job.id.clone());
            job
        };
        attempted += 1;
        let _ = reconcile_owned_job_until(data, owner, token, job, deadline).await;
    }
    attempted
}

// Cancellation before the response leaves the job untouched; the commit after a
// validated response is a synchronous settings transaction with its own CAS.
async fn reconcile_owned_job_until(data: &Path, owner: &settings::UploadOwner, token: &str,
    job: &settings::UploadJob, deadline: tokio::time::Instant) -> Result<(), String> {
    let until = deadline.min(tokio::time::Instant::now() + RECONCILE_BATCH_TIMEOUT);
    tokio::time::timeout_at(until, reconcile_owned_job(data, owner, token, job)).await
        .map_err(|_| "对账请求超时。".to_string())?
}

// The caller supplies a verified owner; commit rechecks current credentials and scope.
// Fetch from the fixed service/token; never reuse a response merely because batch ids match.
async fn reconcile_owned_job(data: &Path, owner: &settings::UploadOwner, token: &str, snapshot: &settings::UploadJob) -> Result<(), String> {
    let batch_id = snapshot.batch_id.filter(|id| *id > 0).ok_or("缺少云端批次。")?;
    if snapshot.id.is_empty() || snapshot.owner.as_ref() != Some(owner)
        || snapshot.account_id != owner.account_id || snapshot.space_id != owner.space_id
        || !upload_job_needs_reconciliation(snapshot)
        || !snapshot.scope.as_ref().is_some_and(|scope| settings::same_service_url(&scope.base_url, &owner.base_url)) {
        return Err("任务归属不完整或已改变。".into());
    }
    let batch = match fixed_identity_get(data, &owner.base_url, token, &format!("/api/v1/chat/uploads/{batch_id}")).await {
        Err(error) if error == "无法读取云端数据：HTTP 404。" => return release_absent_remote_batch(data, owner, token, snapshot),
        result => result?,
    };
    validate_remote_owner(owner, batch_id, &batch, None)?;
    let (state, message) = match batch.get("status").and_then(serde_json::Value::as_str) {
        Some("completed") => ("completed", format!("服务端解析完成，共 {} 条消息", batch.get("message_count").and_then(serde_json::Value::as_i64).unwrap_or_default())),
        Some("failed") => ("failed", batch.get("error_message").and_then(serde_json::Value::as_str).filter(|s| !s.is_empty()).unwrap_or("服务端解析失败").to_string()),
        Some("cancelled") => ("cancelled", "服务端任务已取消".to_string()),
        _ => return Ok(()),
    };
    // A deferred completed job may apply local effects only if the same remote batch
    // is still confirmed completed; never roll it back to another terminal state.
    if snapshot.status == "completed" && state != "completed" { return Err("云端批次完成状态已改变。".into()); }
    settings::update(data, |current| {
        validate_owner_state(current, owner, token)?;
        if desktop_credentials::load(data, &owner.base_url)?.access != token { return Err("登录状态已改变。".into()); }
        if current.upload_queue.iter().filter(|job| job.id == snapshot.id).count() != 1 { return Err("任务标识不唯一。".into()); }
        let index = current.upload_queue.iter().position(|job| job.id == snapshot.id).ok_or("任务已删除。")?;
        let live = &current.upload_queue[index];
        // The entire cursor-affecting snapshot must still match; create intent and digest may
        // independently advance and are intentionally neither compared nor overwritten.
        let mut before = serde_json::to_value(snapshot).map_err(|e| e.to_string())?;
        let mut now = serde_json::to_value(live).map_err(|e| e.to_string())?;
        for field in ["create_started", "create_key", "package_digest", "package_client_version"] {
            before.as_object_mut().ok_or("任务格式无效。")?.remove(field);
            now.as_object_mut().ok_or("任务格式无效。")?.remove(field);
        }
        if before != now || live.owner.as_ref() != Some(owner) || live.batch_id != Some(batch_id) {
            return Err("任务已被其他操作修改。".into());
        }
        let has_effects = state == "completed" && upload_job_has_pending_sync(live);
        let can_apply = !has_effects || validate_sync_owner(current, owner).is_ok();
        if snapshot.status == "completed" && !can_apply && upload_completion_is_verified(snapshot)
            && snapshot.message.contains("本地同步或语音进度属于其他或未知账号，已保留待确认。") {
            // The warning and deferred fields are already durable; avoid rewriting the
            // job on every scheduler tick while the conflicting owner remains unchanged.
            return Err("同步进度归属尚未确认。".into());
        }
        let mut isolated = current.clone();
        isolated.upload_queue = vec![live.clone()];
        let item = &mut isolated.upload_queue[0];
        item.status = state.to_string();
        item.message = if can_apply { message.clone() } else {
            format!("{message}；本地同步或语音进度属于其他或未知账号，已保留待确认。")
        };
        item.updated_at = chrono::Utc::now().to_rfc3339();
        if has_effects && can_apply {
            isolated.sync.owners.entry(owner.account_id.clone()).or_insert_with(|| owner.clone());
            apply_completed_history_baselines(&mut isolated);
            apply_completed_uploads(&mut isolated);
        }
        // Only a fresh successful remote owner/status check can create this marker.
        let completion_digest = if state == "completed" {
            Some(upload_completion_digest(&isolated.upload_queue[0]).ok_or("无法确认完成快照。")?)
        } else { None };
        isolated.upload_queue[0].verified_completion_digest = completion_digest;
        current.sync = isolated.sync;
        let processed = &isolated.upload_queue[0];
        let live = &mut current.upload_queue[index];
        live.status = processed.status.clone();
        live.message = processed.message.clone();
        live.updated_at = processed.updated_at.clone();
        live.verified_completion_digest = processed.verified_completion_digest.clone();
        live.legacy_failure_confirmed = matches!(state, "failed" | "cancelled");
        live.initial_baselines = processed.initial_baselines.clone();
        live.initial_chunks = processed.initial_chunks.clone();
        live.sync_advance = processed.sync_advance;
        live.continue_sync = processed.continue_sync;
        live.voice_processed = processed.voice_processed.clone();
        live.voice_pending = processed.voice_pending.clone();
        Ok(())
    })?;
    Ok(())
}

fn update_pending_voice(
    sync: &mut settings::SyncSettings,
    account_id: &str,
    usernames: &[String],
    processed: &BTreeMap<String, Vec<settings::VoiceRef>>,
    pending: &BTreeMap<String, Vec<settings::VoiceRef>>,
) {
    let account = sync
        .pending_voice
        .entry(account_id.to_string())
        .or_default();
    for username in usernames {
        let Some(done) = processed.get(username) else {
            continue;
        };
        let done: HashSet<_> = done.iter().cloned().collect();
        let current = account.entry(username.clone()).or_default();
        current.retain(|item| !done.contains(item));
        if let Some(next) = pending.get(username) {
            current.extend(next.iter().cloned());
        }
        current.sort_by_key(|item| (item.time, item.local_id, item.server_id));
        current.dedup();
        if current.is_empty() {
            account.remove(username);
        }
    }
    if account.is_empty() {
        sync.pending_voice.remove(account_id);
    }
}

fn sync_job(
    owner: &settings::UploadOwner,
    scope: settings::UploadScope,
) -> Result<settings::UploadJob, String> {
    let mut nonce = [0u8; 16];
    getrandom::getrandom(&mut nonce).map_err(|_| "无法生成唯一同步任务标识。")?;
    let now = chrono::Utc::now().to_rfc3339();
    Ok(settings::UploadJob {
        id: format!("sync-{}", hex::encode(nonce)),
        space_id: owner.space_id.clone(),
        account_id: owner.account_id.clone(),
        batch_id: None,
        owner: Some(owner.clone()),
        create_started: Some(false),
        create_key: None,
        package_digest: None,
        package_client_version: None,
        verified_completion_digest: None,
        legacy_failure_confirmed: false,
        status: "running".into(),
        message: "正在生成并上传聊天包".into(),
        created_at: now.clone(),
        updated_at: now,
        scope: Some(scope),
        covered: BTreeMap::new(),
        initial_baselines: BTreeMap::new(),
        initial_chunks: BTreeMap::new(),
        sync_advance: false,
        continue_sync: false,
        continuation_manual: None,
        manual_group: None,
        manual_index: 0,
        manual_total: 0,
        manual_fingerprint: None,
        voice_processed: BTreeMap::new(),
        voice_pending: BTreeMap::new(),
    })
}

#[cfg(test)]
fn sync_progress_conflict(error: &str) -> bool {
    error == "同步进度已改变，请重新同步。"
}


const SYNC_MESSAGES_PER_BATCH: usize = 1000;

const SYNC_OWNER_PAUSE: &str = "同步进度属于其他或未知云端账号、空间或设备；已暂停，请重新确认并创建采集。";

fn validate_sync_owner(value: &settings::AppSettings, owner: &settings::UploadOwner) -> Result<(), String> {
    let account = &owner.account_id;
    let has_legacy_state = value.sync.has_progress(account);
    match value.sync.owners.get(account) {
        Some(saved) if saved == owner => Ok(()),
        None if !has_legacy_state => Ok(()),
        _ => Err(SYNC_OWNER_PAUSE.into()),
    }
}

/// A manual sync can ask the user to confirm the first range. Automatic backup and
/// continuation must not stay paused after the cloud login changes.
#[cfg(test)]
fn should_pause_sync_for_owner_change(manual: bool, continuation_only: bool) -> bool {
    manual && !continuation_only
}

#[tauri::command]
async fn devify_sync_now(app: AppHandle, scope_confirmation: Option<String>) -> Result<ChatSyncSummary, String> {
    let _=scope_confirmation;
    let data=app_data(&app);let value=settings::load(&data)?;
    let owner=config_runtime::local_owner(&data,&value,&value.active_account_id)?;
    if !config_runtime::ready(&value,&owner) {return Err("请先完成连接设置；若已暂停，请在任务中继续备份。".into());}
    settings::update(&data,|s|{if let Some(source)=s.runtime.sources.get_mut(&owner.account_id) {source.last_run_at=None;source.scan_remaining=0;}Ok(())})?;
    TASK_WAKE.notify_one();
    Ok(ChatSyncSummary {status:"queued".into(),uploaded:0,skipped:0,message_count:0,batch_ids:Vec::new(),error:None})
}


static LOCAL_READ_BUDGET:tokio::sync::Semaphore=tokio::sync::Semaphore::const_new(2);
static TASK_WAKE:tokio::sync::Notify=tokio::sync::Notify::const_new();

#[tauri::command]
fn app_settings_get(app: AppHandle) -> Result<settings::AppSettings, String> {
    settings::load(&app_data(&app))
}

#[tauri::command]
async fn app_settings_save(
    app: AppHandle,
    mut value: settings::AppSettings,
) -> Result<settings::AppSettings, String> {
    let saved = settings::update(&app_data(&app), |stored| {
        if value.user_settings_revision != stored.user_settings_revision {
            return Err("SETTINGS_CONFLICT: 设置已在其他位置更改，请重新加载后重试。".into());
        }
        let context_changed = !settings::same_service_url(&value.devify.base_url, &stored.devify.base_url)
            || value.devify.active_space_id != stored.devify.active_space_id
            || value.devify.device_id != stored.devify.device_id;
        if context_changed {
            abort_in_flight_uploads();
            // Server/space switches never submit the outgoing profile as new settings.
            stored.devify = value.devify;
            config_runtime::activate_settings_profile(&app_data(&app), stored)?;
            return Ok(());
        }
        let backup_changed=serde_json::to_value((&value.chat,&value.backup,&value.connectors,value.sync.enabled)).ok()!=serde_json::to_value((&stored.chat,&stored.backup,&stored.connectors,stored.sync.enabled)).ok();
        if backup_changed {abort_in_flight_uploads();}
        value.upload_queue = stored.upload_queue.clone();
        value.runtime = stored.runtime.clone();
        let sync_enabled = value.sync.enabled;
        value.sync = stored.sync.clone();
        value.sync.enabled = sync_enabled;
        for (account,ranges) in &value.chat.initial_ranges {
            if let Some(progress)=value.sync.initial_progress.get_mut(account) {
                progress.retain(|name,_|serde_json::to_value(ranges.get(name)).ok()==serde_json::to_value(stored.chat.initial_ranges.get(account).and_then(|r|r.get(name))).ok());
            }
        }
        if backup_changed {
            for job in &mut value.upload_queue {
                if job.batch_id.is_none() && job.create_started==Some(false) && matches!(job.status.as_str(),"queued"|"running"|"failed") {
                    job.status="cancelled".into();job.legacy_failure_confirmed=true;job.message="备份配置已更新，未发送的批次将重新生成。".into();
                }
            }
            for source in value.runtime.sources.values_mut() {source.scan_remaining=0;source.last_run_at=None;source.retry_after=None;source.failures=0;}
        }
        value.automation.last_daily_run = stored.automation.last_daily_run.clone();
        value.automation.last_daily_status = stored.automation.last_daily_status.clone();
        value.automation.last_daily_error = stored.automation.last_daily_error.clone();
        value.automation.daily_topic_id = stored.automation.daily_topic_id;
        value.automation.daily_topic_space_id = stored.automation.daily_topic_space_id.clone();
        value.automation.last_schedule_sync_at = stored.automation.last_schedule_sync_at.clone();
        value.automation.last_schedule_signature = stored.automation.last_schedule_signature.clone();
        value.automation.last_schedule_error = stored.automation.last_schedule_error.clone();
        *stored = value;
        let accounts: Vec<_>=stored.chat.allowed_usernames.keys().cloned().collect();
        for account in accounts {
            if let Ok(owner)=config_runtime::local_owner(&app_data(&app),stored,&account) {
                if stored.runtime.sources.get(&account).and_then(|s|s.owner.as_ref()).is_none_or(|old|config_runtime::same_principal(old,&owner)) {config_runtime::enqueue(stored,owner)?;}
            }
        }
        Ok(())
    })?;
    if let Err(error)=flush_config_outbox(&app).await {return Err(format!("设置已在本地保存，等待同步：{error}"));}
    config_runtime::activate_profile(&app_data(&app))?;
    let saved=settings::load(&app_data(&app)).unwrap_or(saved);
    TASK_WAKE.notify_one();
    // Connecting or disconnecting WeChat changes whether the loopback service should exist.
    tauri::async_runtime::spawn(async move {
        reconcile_local_service(&app).await;
    });
    Ok(saved)
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

fn connector_environment(app: &AppHandle) -> connectors::Environment {
    connectors::Environment {
        capture_pending: capture_root(app).join(capture::STATE_NAME).exists(),
    }
}

fn require_connector_installed(
    app: &AppHandle,
    kind: connectors::ConnectorKind,
) -> Result<(), String> {
    connectors::require_installed(kind, &connector_environment(app))
}

/// `codesign` takes a moment, so detection runs off the main thread.
#[tauri::command]
async fn connector_detect(app: AppHandle, kind: String) -> Result<connectors::ConnectorDetect, String> {
    let kind = connectors::ConnectorKind::parse(&kind)
        .ok_or_else(|| format!("未知的连接器：{kind}"))?;
    let env = connector_environment(&app);
    tauri::async_runtime::spawn_blocking(move || connectors::detect(kind, &env))
        .await
        .map_err(|error| format!("检测本机应用失败：{error}"))
}

/// Read the local database copy with the verified keys already available to the app.
///
/// The installed WeChat bundle is intentionally not a prerequisite here: this path does not
/// execute or modify WeChat. Signature validation remains enforced by the in-app capture path,
/// which temporarily replaces the installed bundle.
fn read_new_wechat_data(
    data: &Path,
    env: &connectors::Environment,
    account: &discover::Account,
    store: &KeyStore,
    apply_wal: bool,
    only_chat: bool,
) -> Result<Vec<DecryptOutcome>, String> {
    let _ = env;
    Ok(decrypt::decrypt_account(data, account, store, apply_wal, only_chat))
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
    require_connector_installed(&app, connectors::ConnectorKind::Wechat)
        .map_err(|message| capture::CaptureError::new("wechat_not_installed", message))?;
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

/// Explicitly retain a verified official update and archive the conflicting old transaction.
#[tauri::command]
async fn wechat_capture_accept_official(app: AppHandle) -> Result<capture::AcceptOfficialResult, capture::CaptureError> {
    let root = capture_root(&app);
    tauri::async_runtime::spawn_blocking(move || capture::accept_official_installation(Path::new(capture::WECHAT_APP), &root))
        .await.map_err(|error| capture::CaptureError::new("command_failed", format!("微信校验任务异常结束：{error}")))?
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

#[derive(Serialize)]
struct WeChatPlatformCapabilities {
    platform: String,
    key_acquisition: String,
    supports_memory_scan: bool,
    supports_macos_capture: bool,
    supports_key_import: bool,
}

#[tauri::command]
fn wechat_platform_capabilities() -> WeChatPlatformCapabilities {
    #[cfg(target_os = "windows")]
    {
        return WeChatPlatformCapabilities {
            platform: "windows".into(),
            key_acquisition: "windows_memory_scan_or_import".into(),
            supports_memory_scan: true,
            supports_macos_capture: false,
            supports_key_import: true,
        };
    }
    #[cfg(target_os = "macos")]
    {
        return WeChatPlatformCapabilities {
            platform: "macos".into(),
            key_acquisition: "macos_capture_or_import".into(),
            supports_memory_scan: false,
            supports_macos_capture: true,
            supports_key_import: true,
        };
    }
    #[cfg(target_os = "linux")]
    {
        return WeChatPlatformCapabilities {
            platform: "linux".into(),
            key_acquisition: "import_only".into(),
            supports_memory_scan: false,
            supports_macos_capture: false,
            supports_key_import: true,
        };
    }
    #[allow(unreachable_code)]
    WeChatPlatformCapabilities {
        platform: std::env::consts::OS.into(),
        key_acquisition: "import_only".into(),
        supports_memory_scan: false,
        supports_macos_capture: false,
        supports_key_import: true,
    }
}

#[tauri::command]
async fn wechat_windows_key_scan(
    app: AppHandle,
    account_id: Option<String>,
) -> Result<KeyImportResult, String> {
    #[cfg(target_os = "windows")]
    {
        let candidates = windows_keys::scan()?;
        let text = candidates.join("\n");
        return import_key_text(&app, &text, account_id.as_deref());
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, account_id);
        Err("Windows 密钥扫描器只能在 Windows 上运行。".into())
    }
}

#[tauri::command]
async fn wechat_scan() -> ScanResult {
    discover::scan()
}

#[derive(Serialize)]
struct ChatAccount {
    id: String,
    /// `wechat` or `whatsapp` — the UI groups and labels by this instead of guessing.
    source: String,
    display: Option<String>,
    modified_at: Option<u64>,
}

/// Every local source that currently has readable chat data, in one list.
#[tauri::command]
async fn chat_accounts() -> Vec<ChatAccount> {
    let mut accounts = discover::scan()
        .accounts
        .into_iter()
        .map(|account| ChatAccount {
            id: account.id,
            source: "wechat".into(),
            display: None,
            modified_at: account.modified_at,
        })
        .collect::<Vec<_>>();
    if let Some(account) = whatsapp::account() {
        if whatsapp::is_installed() {
            let modified_at = whatsapp::data_dir()
                .map(|dir| dir.join("ChatStorage.sqlite"))
                .and_then(|path| std::fs::metadata(path).ok())
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_secs());
            accounts.push(ChatAccount {
                id: account.id,
                source: "whatsapp".into(),
                display: account.name.or_else(|| Some(account.phone)),
                modified_at,
            });
        }
    }
    accounts
}

#[tauri::command]
async fn whatsapp_status(app: AppHandle) -> whatsapp::Status {
    let data = app_data(&app);
    tauri::async_runtime::spawn_blocking(move || whatsapp::status(&data))
        .await
        .unwrap_or_else(|error| whatsapp::Status { error: Some(format!("读取 WhatsApp 状态失败：{error}")), ..Default::default() })
}

/// Remember which local account the chat page should show. The whitelist is per account,
/// so switching source here is the only state the chat page needs to re-read.
#[tauri::command]
async fn chat_account_activate(app: AppHandle, account_id: String) -> Result<(), String> {
    validate_account_id(&account_id)?;
    let data = app_data(&app);
    let available = discover::scan().accounts.iter().any(|account| account.id == account_id)
        || whatsapp::account().is_some_and(|account| account.id == account_id);
    if !available {
        return Err("这个账号当前不可用，请重新检测本机聊天来源。".into());
    }
    settings::update(&data, |value| {
        value.active_account_id = account_id.clone();
        Ok(())
    })
    .map(|_| ())
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
    read_new_wechat_data(
        &app_data(&app),
        &connector_environment(&app),
        &account,
        &store,
        apply_wal,
        only_chat,
    )
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
    let outcomes = read_new_wechat_data(
        &app_data(&app),
        &connector_environment(&app),
        &account,
        &store,
        true,
        true,
    );
    end_sync(&state, &account_id);
    outcomes
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct VoiceRequest {
    local_id: i64,
    server_id: String,
}

#[derive(Serialize)]
struct VoiceAudio {
    data_uri: String,
    duration_secs: f64,
}

/// Whitelist + visibility guard shared by the media and voice commands.
fn require_visible_session(
    app: &AppHandle,
    account_id: &str,
    username: &str,
) -> Result<(), String> {
    let cache = decrypt::cache_dir(&app_data(app), account_id);
    if !query::is_chat_session(&cache, username) {
        return Err("公众号、服务号和系统会话不在查看范围内。".into());
    }
    if !allowed_usernames(app, account_id)?.contains(username) {
        return Err("该会话不在当前策略白名单中。".into());
    }
    Ok(())
}

fn account_root(account_id: &str) -> Result<PathBuf, String> {
    discover::scan()
        .accounts
        .into_iter()
        .find(|account| account.id == account_id)
        .map(|account| PathBuf::from(account.dir))
        .ok_or_else(|| format!("找不到账号 {account_id} 的本机数据目录。"))
}

/// Per-message attachment manifest (images, videos, documents, voice).
#[tauri::command]
async fn wechat_message_media(
    app: AppHandle,
    account_id: String,
    username: String,
    since: i64,
    until: i64,
) -> Result<media::MediaReport, String> {
    require_visible_session(&app, &account_id, &username)?;
    let root = account_root(&account_id)?;
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    let raw = query::raw_messages_between(&cache, &username, since, until, 20000)?;
    let keys = media::resolve_image_keys(&root, &query::md5_hex(&username));
    Ok(media::resolve(&root, &username, &raw, keys.as_ref()))
}

/// Build (and cache) a display preview for one image message.
#[tauri::command]
async fn wechat_media_preview(
    app: AppHandle,
    account_id: String,
    local_id: i64,
    paths: Vec<String>,
) -> Result<Option<String>, String> {
    let root = account_root(&account_id)?;
    let targets: Vec<PathBuf> = paths.iter().map(|path| root.join(path)).collect();
    let data = local_scope::root(&app_data(&app));
    let _budget = LOCAL_READ_BUDGET.acquire().await.map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        Ok(media::make_preview(&data, &account_id, local_id, &root, &targets).ok().map(|p|p.to_string_lossy().to_string()))
    }).await.map_err(|e| e.to_string())?
}

/// Decode one encrypted image offline for the in-app viewer.
#[tauri::command]
async fn wechat_media_open_image(
    app: AppHandle,
    account_id: String,
    paths: Vec<String>,
) -> Result<String, String> {
    let root = account_root(&account_id)?;
    let targets: Vec<PathBuf> = paths
        .iter()
        .map(|path| {
            if Path::new(path).is_absolute() {
                PathBuf::from(path)
            } else {
                root.join(path)
            }
        })
        .collect();
    let data = local_scope::root(&app_data(&app));
    let _budget = LOCAL_READ_BUDGET.acquire().await.map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || media::open_image(&data, &account_id, &root, &targets)
        .map(|out| out.to_string_lossy().to_string())).await.map_err(|e| e.to_string())?
}

/// Reveal one attachment in Finder/Explorer. The path must stay inside the account folder.
#[tauri::command]
async fn wechat_media_reveal(account_id: String, path: String) -> Result<(), String> {
    let root = account_root(&account_id)?;
    let target = if Path::new(&path).is_absolute() {
        PathBuf::from(&path)
    } else {
        root.join(&path)
    };
    media::reveal(&root, &target)
}

/// Export transcript + manifest + plaintext attachments into `~/Downloads/AIMyChats/…`.
#[tauri::command]
async fn wechat_export_bundle(
    app: AppHandle,
    account_id: String,
    username: String,
    display: String,
    since: i64,
    until: i64,
) -> Result<media::ExportReport, String> {
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    entitlements::require(&data, &settings.devify.base_url, capabilities::BRIDGE_EXPORT)?;
    require_visible_session(&app, &account_id, &username)?;
    let root = account_root(&account_id)?;
    media::export_bundle(
        &app_data(&app),
        &root,
        &account_id,
        &username,
        &display,
        since,
        until,
    )
}

#[tauri::command]
async fn wechat_voice_status(app: AppHandle) -> voice::VoiceSettingsView {
    voice::view(&app_data(&app))
}

#[tauri::command]
async fn wechat_voice_settings_save(
    app: AppHandle,
    engine: String,
    model: String,
    language: String,
) -> Result<voice::VoiceSettingsView, String> {
    let settings = voice::VoiceSettings {
        engine: engine.trim().to_string(),
        model: model.trim().to_string(),
        language: if language.trim().is_empty() {
            "zh".to_string()
        } else {
            language.trim().to_string()
        },
    };
    voice::save_settings(&app_data(&app), &settings)?;
    Ok(voice::view(&app_data(&app)))
}

/// Decode and transcribe selected voice messages. Results are cached by `account|server_id`.
async fn voice_blobs_with_refresh(
    app: &AppHandle,
    state: State<'_, SyncState>,
    account_id: &str,
    ids: &[i64],
) -> Result<std::collections::BTreeMap<i64, Vec<u8>>, String> {
    let cache = decrypt::cache_dir(&app_data(app), account_id);
    let cached = voice::read_voice_blobs(&cache, ids).unwrap_or_default();
    if ids.iter().all(|id| cached.contains_key(id)) {
        return Ok(cached);
    }
    wechat_sync(app.clone(), state, account_id.to_string()).await
        .map_err(|error| format!("无法刷新语音数据：{error}"))?;
    voice::read_voice_blobs(&cache, ids)
}

#[tauri::command]
async fn wechat_voice_transcribe(
    app: AppHandle,
    state: State<'_, SyncState>,
    account_id: String,
    username: String,
    items: Vec<VoiceRequest>,
) -> Result<Vec<voice::VoiceResult>, String> {
    require_visible_session(&app, &account_id, &username)?;
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = items.iter().map(|item| item.server_id.parse().map_err(|_| "无效的语音标识。".to_string())).collect::<Result<_, _>>()?;
    let data_dir = app_data(&app);
    let private_data=local_scope::root(&data_dir);
    let settings = settings::load(&data_dir)?;
    entitlements::require(
        &data_dir,
        &settings.devify.base_url,
        capabilities::LOCAL_VOICE_TRANSCRIBE,
    )?;
    let blobs = voice_blobs_with_refresh(&app, state, &account_id, &ids).await?;
    let view = voice::view(&data_dir);
    if view.selected.is_none() {
        return Err(
            "没有检测到本地转写引擎。可安装 whisper.cpp（brew install whisper-cpp）、mlx-whisper 或 openai-whisper，然后在「设置 → 语音转写」里选择。"
                .into(),
        );
    }

    let mut transcripts = voice::load_transcripts(&private_data);
    let mut results = Vec::new();
    for (item, server_id) in items.into_iter().zip(ids) {
        let key = voice::transcript_key(&account_id, server_id);
        let mut result = voice::VoiceResult {
            server_id,
            local_id: item.local_id,
            wav: None,
            duration_secs: 0.0,
            text: None,
            cached: false,
            error: None,
        };
        let Some(blob) = blobs.get(&server_id) else {
            result.error = Some("本地暂未找到这条语音，请刷新微信数据后重试。".into());
            results.push(result);
            continue;
        };
        match voice::ensure_wav(&private_data, &account_id, server_id, blob) {
            Ok((wav, duration)) => {
                result.wav = Some(wav.to_string_lossy().to_string());
                result.duration_secs = duration;
                if let Some(entry) = transcripts.get(&key) {
                    result.text = Some(entry.text.clone());
                    result.cached = true;
                } else {
                    match voice::transcribe_file(&data_dir, &wav, 300) {
                        Ok((text, engine, model)) => {
                            transcripts.insert(
                                key,
                                voice::TranscriptEntry {
                                    text: text.clone(),
                                    engine,
                                    model,
                                    created_at: voice::now_secs(),
                                },
                            );
                            let _ = voice::save_transcripts(&private_data, &transcripts);
                            result.text = Some(text);
                        }
                        Err(error) => result.error = Some(error),
                    }
                }
            }
            Err(error) => result.error = Some(error),
        }
        results.push(result);
    }
    Ok(results)
}

/// Decode one voice message to a data URI so the webview can play it.
#[tauri::command]
async fn wechat_voice_audio(
    app: AppHandle,
    state: State<'_, SyncState>,
    account_id: String,
    username: String,
    server_id: String,
) -> Result<VoiceAudio, String> {
    require_visible_session(&app, &account_id, &username)?;
    let server_id: i64 = server_id.parse().map_err(|_| "无效的语音标识。".to_string())?;
    let data_dir = app_data(&app);
    let private_data=local_scope::root(&data_dir);
    let blobs = voice_blobs_with_refresh(&app, state, &account_id, &[server_id]).await?;
    let blob = blobs
        .get(&server_id)
        .ok_or_else(|| "本地暂未找到这条语音，请刷新微信数据后重试。".to_string())?;
    let (wav, duration) = voice::ensure_wav(&private_data, &account_id, server_id, blob)?;
    Ok(VoiceAudio {
        data_uri: voice::audio_data_uri(&wav)?,
        duration_secs: duration,
    })
}

#[tauri::command]
async fn chat_remote_lookup(app:AppHandle,account_id:String,username:String)->Result<serde_json::Value,String>{
    let data=app_data(&app);let s=settings::load(&data)?;let owner=config_runtime::local_owner(&data,&s,&account_id)?;
    if !config_runtime::browse_names(&s,&account_id).contains(&username) {return Err("该会话不在所选范围内。".into());}
    let path={let mut query=url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("platform",expected_source_type(&account_id)).append_pair("source_account_id",&account_id).append_pair("source_conversation_id",&username);
        format!("/api/v1/spaces/{}/desktop/conversation?{}",owner.space_id,query.finish())};
    devify_json_request(&app,reqwest::Method::GET,path,None).await
}
#[tauri::command]
async fn chat_remote_image(app:AppHandle,space_id:String,item_id:Option<i64>,sha256:Option<String>)->Result<String,String>{
    static IMAGES:tokio::sync::Semaphore=tokio::sync::Semaphore::const_new(2);
    let _budget=IMAGES.acquire().await.map_err(|e|e.to_string())?;
    let data=app_data(&app);let s=settings::load(&data)?;validate_devify_space(&s,&space_id)?;
    let media_path=if let Some(hash)=sha256 {if hash.len()!=64 || !hash.bytes().all(|b|b.is_ascii_hexdigit()) {return Err("图片摘要无效。".into());}format!("/api/v1/spaces/{space_id}/chat-media/{hash}")}
        else if let Some(id)=item_id.filter(|id|*id>0) {format!("/api/v1/spaces/{space_id}/media/{id}")} else {return Err("图片标识无效。".into());};
    let base=s.devify.resolved_base_url()?;let token=devify_access_token_async(&data,&base).await?;let generation=request_generation(&data,&base,&token)?;
    let mut response=http_client::api().get(format!("{base}{media_path}")).bearer_auth(&token).send().await.map_err(|e|devify_network_message("读取图片失败",&e))?;
    if !response.status().is_success() {return Err(format!("读取图片失败：HTTP {}",response.status()));}
    let mime=response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|h|h.to_str().ok()).unwrap_or("").split(';').next().unwrap_or("").to_string();
    if !matches!(mime.as_str(),"image/png"|"image/jpeg"|"image/gif"|"image/webp") {return Err("当前媒体没有可显示的图片。".into());}
    let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|e|e.to_string())? {
        if bytes.len().saturating_add(chunk.len())>8*1024*1024 {return Err("图片较大，请查看原记录。".into());}bytes.extend_from_slice(&chunk);
    }
    if request_generation(&data,&base,&token).ok()!=Some(generation) {return Err("登录状态已改变。".into());}
    Ok(format!("data:{mime};base64,{}",base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[tauri::command]
async fn chat_sessions(app: AppHandle, account_id: String, limit: Option<usize>, offset: Option<usize>, focus_username: Option<String>, focus_usernames: Option<Vec<String>>) -> Result<Vec<SessionItem>, String> {
    let _read_budget=LOCAL_READ_BUDGET.acquire().await.map_err(|e|e.to_string())?;
    validate_account_id(&account_id)?;
    let data=app_data(&app);let value=settings::load(&data)?;
    let mut allowed:HashSet<String>=config_runtime::browse_names(&value,&account_id).into_iter().collect();
    let principal=config_runtime::local_owner(&data,&value,&account_id)?;
    if value.runtime.sources.get(&account_id).and_then(|s|s.owner.as_ref()).or_else(||value.sync.owners.get(&account_id)).is_some_and(|old|!config_runtime::same_principal(old,&principal)) {return Ok(Vec::new());}
    if let Some(name)=focus_username {allowed.retain(|v|v==&name);}
    if let Some(names)=focus_usernames {
        if names.len()>40 {return Err("一次最多读取 40 个会话名称。".into());}
        let names:HashSet<_>=names.into_iter().collect();
        allowed.retain(|v|names.contains(v));
    }
    let wa=whatsapp::is_account(&account_id);let cache=decrypt::cache_dir(&data,&account_id);
    let cache_key=hex::encode(sha2::Sha256::digest(serde_json::to_vec(&("sessions-v1",&principal,limit.unwrap_or(50),offset.unwrap_or(0),&{let mut v:Vec<_>=allowed.iter().cloned().collect();v.sort();v})).map_err(|e|e.to_string())?));
    let read_data=data.clone();let read_key=cache_key.clone();
    tauri::async_runtime::spawn_blocking(move||{
        if !wa && allowed.iter().any(|name|name.ends_with("@qy_g") || name.ends_with("@qy_u")) {let _=decrypt::prepare_business_cache(&data,&account_id,false);}
        let result=if wa {whatsapp::sessions_page(&data,&allowed,limit.unwrap_or(50).clamp(1,100),offset.unwrap_or(0).min(1000))}
        else {query::sessions_page(&cache,Some(&allowed),Some(limit.unwrap_or(50).clamp(1,100)),offset.unwrap_or(0).min(1000))};
        match result {
            Ok(mut list)=>{
                if let Some(old)=task_store::cache_get(&read_data,&read_key).and_then(|v|serde_json::from_value::<Vec<SessionItem>>(v).ok()) {
                    for item in old {if allowed.contains(&item.username) && !list.iter().any(|s|s.username==item.username) {list.push(item);}}
                    list.sort_by(|a,b|b.last_time.cmp(&a.last_time));list.truncate(limit.unwrap_or(50).clamp(1,100));
                }
                if !list.is_empty(){if let Ok(body)=serde_json::to_value(&list){let _=task_store::cache_put(&read_data,&read_key,&body);}}Ok(list)
            },
            Err(error) if !wa && !cache.join("session/session.db").exists()=>{
                task_store::cache_get(&read_data,&read_key).and_then(|v|serde_json::from_value(v).ok()).ok_or(error)
            }
            Err(error)=>Err(error)
        }
    }).await.map_err(|e|format!("读取会话失败：{e}"))?
}

#[derive(Serialize)]
struct SessionUploadState {
    username: String,
    /// `pending` (local records newer than the upload coverage), `local` (never uploaded)
    /// or `synced`.
    state: String,
    /// Local records newer than the covered position; 0 unless `state` is `pending`.
    new_messages: usize,
}

/// Only a recorded pre-create failure is known not to have reached the server.
fn legacy_job_is_inert(job: &settings::UploadJob) -> bool {
    job.owner.is_none() && job.batch_id.is_none()
        && job.create_started == Some(false)
        && matches!(job.status.as_str(), "failed" | "cancelled")
        && !upload_job_has_pending_sync(job)
}

fn legacy_adoption_candidate(job: &settings::UploadJob, owner: &settings::UploadOwner) -> bool {
    let unowned = job.owner.is_none();
    // Jobs created before create intent was recorded have `create_started == None`; once
    // adopted, their terminal failure still needs one server confirmation.
    let adopted_failure = job.owner.as_ref() == Some(owner) && job.create_started.is_none()
        && matches!(job.status.as_str(), "failed" | "cancelled") && !job.legacy_failure_confirmed;
    (unowned || adopted_failure) && !job.id.is_empty() && job.batch_id.is_some_and(|id| id > 0)
        && job.account_id == owner.account_id && job.space_id == owner.space_id
        && matches!(job.status.as_str(), "completed" | "failed" | "cancelled")
        && job.scope.as_ref().is_some_and(|scope| settings::same_service_url(&scope.base_url, &owner.base_url))
}

fn upload_job_in_sync_scope(job: &settings::UploadJob, owner: &settings::UploadOwner) -> bool {
    match &job.owner {
        Some(saved) => saved == owner,
        None => job.account_id == owner.account_id && job.space_id == owner.space_id
            && job.scope.as_ref().map_or(true, |scope| settings::same_service_url(&scope.base_url, &owner.base_url)),
    }
}

fn recoverable_sync_job(job: &settings::UploadJob, owner: &settings::UploadOwner, username: &str) -> bool {
    job.id.starts_with("sync-")
        && job.status == "failed"
        && job.can_recover_create()
        && job.owner.as_ref() == Some(owner)
        && job.scope.as_ref().is_some_and(|scope| {
            scope.usernames.len() == 1
                && scope.usernames[0] == username
                && scope.message_selection.get(username).is_some_and(|messages| !messages.is_empty())
        })
}

fn interrupted_sync_job(job: &settings::UploadJob) -> bool {
    job.manual_group.is_none() && job.batch_id.is_none()
        && matches!(job.status.as_str(), "running" | "uploading")
}

/// A sync attempt that died before it had a server batch is not an active upload.
/// Jobs that already recorded a create stay failed without the release flag, so the
/// existing lookup can still attach the batch or release it.
fn abandon_interrupted_sync_job(job: &mut settings::UploadJob) -> bool {
    if !interrupted_sync_job(job) {
        return false;
    }
    job.status = "failed".into();
    if job.create_started != Some(true) {
        job.legacy_failure_confirmed = true;
        job.message = "本地上传在创建云端批次前中断，后续同步可以继续。".into();
    } else if !job.message.contains("正在核对") {
        job.message = "上传中断，正在核对是否已创建云端批次。".into();
    }
    job.updated_at = chrono::Utc::now().to_rfc3339();
    true
}

fn release_absent_remote_batch(data: &Path, owner: &settings::UploadOwner, token: &str, snapshot: &settings::UploadJob) -> Result<(), String> {
    settings::update(data, |current| {
        validate_owner_state(current, owner, token)?;
        if desktop_credentials::load(data, &owner.base_url)?.access != token { return Err("登录状态已改变。".into()); }
        if current.upload_queue.iter().filter(|job| job.id == snapshot.id).count() != 1 { return Err("任务标识不唯一。".into()); }
        let live = current.upload_queue.iter_mut().find(|job| job.id == snapshot.id).ok_or("任务已删除。")?;
        if live.owner.as_ref() != Some(owner) || live.batch_id != snapshot.batch_id || live.status != snapshot.status {
            return Err("任务已被其他操作修改。".into());
        }
        live.status = "failed".into();
        live.legacy_failure_confirmed = true;
        live.verified_completion_digest = None;
        live.message = "云端已没有这个批次，后续同步可以继续。".into();
        live.updated_at = chrono::Utc::now().to_rfc3339();
        Ok(())
    })?;
    Ok(())
}

/// A saved job blocks a conversation only when a not-yet-submitted manual batch names it.
/// Failed, ambiguous, processing, and empty-scope jobs must not skip later uploads.
fn upload_job_blocks_username(job: &settings::UploadJob, owner: &settings::UploadOwner, username: &str) -> bool {
    if !upload_job_in_sync_scope(job, owner) || job_never_blocks_sync(job) || job.status == "completed" {
        return false;
    }
    let listed = job.scope.as_ref().is_some_and(|scope| scope.usernames.iter().any(|name| name == username));
    listed && job.manual_group.is_some() && job.batch_id.is_none()
        && matches!(job.status.as_str(), "queued" | "running" | "uploading")
}

/// An inert job or a verified legacy failure cannot still ingest messages.
fn job_never_blocks_sync(job: &settings::UploadJob) -> bool {
    legacy_job_is_inert(job)
        || (job.legacy_failure_confirmed && matches!(job.status.as_str(), "failed" | "cancelled"))
}

/// Legacy cursors belong to `owner` only when every cursor position is covered by a
/// completed job whose completion was verified for that same owner. Without legacy state
/// there is nothing to adopt, so a fresh account is never pinned to an owner here.
fn legacy_cursors_explained(value: &settings::AppSettings, owner: &settings::UploadOwner) -> bool {
    let account = &owner.account_id;
    let has_state = value.sync.cursors.get(account).is_some_and(|v| !v.is_empty())
        || value.sync.pending_voice.get(account).is_some_and(|v| !v.is_empty());
    if !has_state || value.sync.initial_progress.get(account).is_some_and(|v| !v.is_empty())
        || value.sync.continuation.get(account).is_some_and(|v| !v.is_empty()) {
        return false;
    }
    let verified: Vec<_> = value.upload_queue.iter()
        .filter(|job| job.owner.as_ref() == Some(owner) && upload_completion_is_verified(job))
        .collect();
    value.sync.cursors.get(account).map_or(true, |cursors| cursors.iter().all(|(username, cursor)| {
        verified.iter().any(|job| job.covered.get(username)
            .is_some_and(|position| position.covers((cursor.time, cursor.local_id))))
    }))
}

const LEGACY_ADOPT_CONCURRENCY: usize = 8;


/// Bind pre-ownership history to the freshly verified owner, one cloud batch at a time.
/// A job is adopted only when the server confirms the batch's user, space, device, source
/// account and a terminal status consistent with the local one; anything else stays
/// unowned and keeps its conversations "unknown". The caller holds the identity lease.
async fn adopt_legacy_upload_history(data: &Path, owner: &settings::UploadOwner, token: &str,
    deadline: tokio::time::Instant) -> Result<(), String> {
    let current = settings::load(data)?;
    let mut counts = HashMap::new();
    for job in &current.upload_queue { *counts.entry(job.id.clone()).or_insert(0usize) += 1; }
    let candidates: Vec<(String, i64, String)> = current.upload_queue.iter()
        .filter(|job| legacy_adoption_candidate(job, owner) && counts.get(&job.id) == Some(&1))
        .map(|job| (job.id.clone(), job.batch_id.unwrap_or_default(), job.status.clone()))
        .collect();
    let mut confirmed = HashSet::new();
    for chunk in candidates.chunks(LEGACY_ADOPT_CONCURRENCY) {
        if tokio::time::Instant::now() >= deadline { break; }
        let handles: Vec<_> = chunk.iter().map(|(job_id, batch_id, status)| {
            let (base, token, path) = (owner.base_url.clone(), token.to_string(), format!("/api/v1/chat/uploads/{batch_id}"));
            let (job_id, batch_id, status) = (job_id.clone(), *batch_id, status.clone());
            let data = data.to_path_buf();
            tauri::async_runtime::spawn(async move {
                let batch = tokio::time::timeout_at(deadline, fixed_identity_get(&data, &base, &token, &path)).await.ok()?.ok()?;
                Some((job_id, batch_id, status, batch))
            })
        }).collect();
        for handle in handles {
            let Ok(Some((job_id, batch_id, status, batch))) = handle.await else { continue; };
            if validate_remote_owner(owner, batch_id, &batch, None).is_err() { continue; }
            let remote = batch.get("status").and_then(serde_json::Value::as_str).unwrap_or_default();
            let consistent = if status == "completed" { remote == "completed" }
                else { matches!(remote, "failed" | "cancelled") };
            if consistent { confirmed.insert((job_id, batch_id)); }
        }
    }
    if confirmed.is_empty() && (current.sync.owners.contains_key(&owner.account_id)
        || !legacy_cursors_explained(&current, owner)) {
        return Ok(());
    }
    settings::update(data, |current| {
        validate_owner_state(current, owner, token)?;
        if desktop_credentials::load(data, &owner.base_url)?.access != token { return Err("登录状态已改变。".into()); }
        let mut counts = HashMap::new();
        for job in &current.upload_queue { *counts.entry(job.id.clone()).or_insert(0usize) += 1; }
        for job in current.upload_queue.iter_mut() {
            if !legacy_adoption_candidate(job, owner) || counts.get(&job.id) != Some(&1)
                || !confirmed.contains(&(job.id.clone(), job.batch_id.unwrap_or_default())) {
                continue;
            }
            job.owner = Some(owner.clone());
            if job.status == "completed" {
                job.verified_completion_digest = Some(upload_completion_digest(job).ok_or("无法确认完成快照。")?);
            } else {
                job.verified_completion_digest = None;
                job.legacy_failure_confirmed = true;
            }
        }
        if !current.sync.owners.contains_key(&owner.account_id) && legacy_cursors_explained(current, owner) {
            current.sync.owners.insert(owner.account_id.clone(), owner.clone());
        }
        Ok(())
    })?;
    Ok(())
}

// Legacy history cannot establish ownership or coverage, but it must not be
// mistaken for proof that a conversation was never uploaded.
fn legacy_uncertain_coverage(value: &settings::AppSettings, owner: &settings::UploadOwner) -> (HashSet<String>, bool) {
    let mut uncertain = HashSet::new();
    let mut uncertain_all = false;
    if !value.sync.owners.contains_key(&owner.account_id) {
        if let Some(items) = value.sync.cursors.get(&owner.account_id) { uncertain.extend(items.keys().cloned()); }
        if let Some(items) = value.sync.initial_progress.get(&owner.account_id) { uncertain.extend(items.keys().cloned()); }
        if let Some(items) = value.sync.continuation.get(&owner.account_id) { uncertain.extend(items.keys().cloned()); }
        if let Some(items) = value.sync.pending_voice.get(&owner.account_id) { uncertain.extend(items.keys().cloned()); }
    }
    for job in &value.upload_queue {
        if job.owner.is_some() || legacy_job_is_inert(job) || job.account_id != owner.account_id
            || job.space_id != owner.space_id { continue; }
        match job.scope.as_ref() {
            Some(scope) => {
                let base = settings::canonical_api_base(&scope.base_url).ok().filter(|base| {
                    Url::parse(base).is_ok_and(|url| matches!(url.scheme(), "http" | "https")
                        && url.host_str().is_some() && url.username().is_empty()
                        && url.password().is_none() && url.query().is_none() && url.fragment().is_none())
                });
                match base {
                    Some(base) if settings::same_service_url(&base, &owner.base_url) => {
                        if scope.usernames.is_empty() { uncertain_all = true; }
                        else { uncertain.extend(scope.usernames.iter().cloned()); }
                    }
                    Some(_) => {},
                    None => uncertain_all = true,
                }
            }
            None => uncertain_all = true,
        }
    }
    (uncertain, uncertain_all)
}

// Completed jobs count only when their entire
// owner/batch/scope/coverage snapshot was previously verified by reconciliation. This is
// last-confirmed coverage, not a live check for later server deletion or mutation.
async fn verified_coverage_settings(data: &Path, owner: &settings::UploadOwner, token: &str) -> Result<(settings::AppSettings, HashSet<String>, bool), String> {
    let mut current = settings::load(data)?;
    validate_owner_state(&current, owner, token)?;
    if desktop_credentials::load(data, &owner.base_url)?.access != token { return Err("登录状态已改变。".into()); }
    let (mut uncertain, mut uncertain_all) = legacy_uncertain_coverage(&current, owner);
    let mut counts = HashMap::new();
    for job in &current.upload_queue { *counts.entry(job.id.clone()).or_insert(0usize) += 1; }
    current.upload_queue.retain(|job| {
        let same_owner = job.owner.as_ref() == Some(owner) && job.account_id == owner.account_id && job.space_id == owner.space_id;
        let scope_ok = job.scope.as_ref().is_some_and(|scope| settings::same_service_url(&scope.base_url, &owner.base_url));
        let checked = same_owner && !job.id.is_empty() && counts.get(&job.id) == Some(&1) && scope_ok
            && (job.status != "completed" || upload_completion_is_verified(job));
        if !checked && same_owner && job.status == "completed" {
            if let Some(scope) = &job.scope { uncertain.extend(scope.usernames.iter().cloned()); } else { uncertain_all = true; }
        }
        checked
    });
    Ok((current, uncertain, uncertain_all))
}

/// Newest explicitly covered position for the freshly verified current owner.
/// Jobs must first be filtered by verified_coverage_settings. Legacy ownerless cursors and
/// timestamp-only jobs cannot establish coverage.
fn upload_coverage(
    value: &settings::AppSettings,
    owner: &settings::UploadOwner,
) -> HashMap<String, settings::SyncCursor> {
    let mut coverage = if value.sync.owners.get(&owner.account_id) == Some(owner) {
        value.sync.cursors.get(&owner.account_id).cloned().unwrap_or_default()
    } else {
        HashMap::new()
    };
    for job in &value.upload_queue {
        if job.owner.as_ref() != Some(owner) || job.account_id != owner.account_id
            || job.space_id != owner.space_id || !upload_completion_is_verified(job) {
            continue;
        }
        let Some(scope) = job.scope.as_ref() else { continue; };
        if !settings::same_service_url(&scope.base_url, &owner.base_url) { continue; }
        for username in &scope.usernames {
            let Some(position) = job.covered.get(username).copied() else { continue; };
            let newer = coverage.get(username).map_or(true, |current| {
                (position.time, position.local_id) > (current.time, current.local_id)
                    || ((position.time, position.local_id) == (current.time, current.local_id)
                        && position.boundary_count > current.boundary_count)
            });
            if newer { coverage.insert(username.clone(), position); }
        }
    }
    coverage
}

fn conversation_job_state<'a>(value: &'a settings::AppSettings, owner: &settings::UploadOwner, username: &str) -> Option<&'a str> {
    let matches = |job: &&settings::UploadJob| job.owner.as_ref() == Some(owner) && job.account_id == owner.account_id && job.space_id == owner.space_id && (job.status != "completed" || upload_completion_is_verified(job)) && job.scope.as_ref().is_some_and(|scope| settings::same_service_url(&scope.base_url, &owner.base_url) && scope.usernames.iter().any(|name| name == username));
    if value.upload_queue.iter().filter(matches).any(|job| matches!(job.status.as_str(), "queued" | "running" | "uploading" | "processing")) { return Some("processing"); }
    let latest = value.upload_queue.iter().filter(matches).max_by(|a, b| a.created_at.cmp(&b.created_at));
    if value.upload_queue.iter().filter(matches).any(|job| job.status == "failed" && job.batch_id.is_some()) {
        return Some(if latest.is_some_and(|job| job.status == "completed") { "history_failed" } else { "failed" });
    }
    latest.map(|job| match job.status.as_str() { "queued" | "running" | "uploading" | "processing" => "processing", "failed" | "cancelled" => "failed", "completed" => "synced", _ => "unknown" })
}

/// Per-conversation upload state for the chat list.
///
/// The list cannot derive this from the queue alone: a failed job leaves records unuploaded,
/// and a conversation can gain new local records after its last completed job. So the state
/// is decided by comparing the newest local message with the newest uploaded position.
#[tauri::command]
async fn wechat_upload_state(
    app: AppHandle,
    account_id: String,
    usernames: Vec<String>,
) -> Result<Vec<SessionUploadState>, String> {
    validate_account_id(&account_id)?;
    let whatsapp_source = whatsapp::is_account(&account_id);
    // Reconciliation runs in the scheduler; do not fan out its whole queue from a list read.
    let data = app_data(&app);
    let initial = settings::load(&data)?;
    let connected = if whatsapp_source { initial.connectors.whatsapp_enabled } else { initial.connectors.wechat_enabled };
    if !connected { return Ok(Vec::new()); }
    let owner=config_runtime::local_owner(&data,&initial,&account_id)?;
    let token=desktop_credentials::load(&data,&owner.base_url)?.access;
    // Badges describe last-confirmed local coverage; opening the list makes no HTTP request.
    let (value, uncertain, uncertain_all) = verified_coverage_settings(&data, &owner, &token).await?;
    if whatsapp_source {
        if !value.connectors.whatsapp_enabled { return Ok(Vec::new()); }
    } else if !value.connectors.wechat_enabled {
        return Ok(Vec::new());
    }
    let allowed: HashSet<String> = value
        .chat
        .allowed_usernames
        .get(&account_id)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let coverage = upload_coverage(&value, &owner);
    let mut requested: Vec<String> = usernames
        .into_iter()
        .filter(|username| allowed.contains(username))
        .collect();
    requested.sort();
    requested.dedup();
    if requested.is_empty() {
        return Ok(Vec::new());
    }
    if requested.len() > 200 {
        return Err("请分页读取会话状态。".into());
    }
    // A hidden/slow WebView must not accumulate unconstrained shard scans.
    // Reject excess reads rather than queueing work which is already stale.
    let budget = LOCAL_READ_BUDGET.try_acquire()
        .map_err(|_| "本地记录正在读取，请稍后重试。".to_string())?;
    // The remaining work only reads the local database, so the sign-out lease is released
    // before a long list scan can hold it.
    let cache = decrypt::cache_dir(&data, &account_id);
    tauri::async_runtime::spawn_blocking(move || {
        let _budget = budget;
        let reader = (!whatsapp_source).then(|| query::UploadStateReader::new(&cache));
        let mut states = Vec::new();
        for username in requested {
            // A conversation without a local readable message cannot say anything about new
            // records; leaving it out keeps the client's last known badge instead of guessing.
            let position = if whatsapp_source {
                whatsapp::latest_position(&data, &username)
            } else {
                reader.as_ref().unwrap().latest_position(&username)
            };
            let Ok(Some((time, local_id))) = position else {
                continue;
            };
            let count_at = |time: i64| if whatsapp_source {
                whatsapp::count_at_time(&data, &username, time)
            } else {
                reader.as_ref().unwrap().count_at_time(&username, time)
            };
            let count_after = |time: i64, id: i64| if whatsapp_source {
                whatsapp::count_after(&data, &username, time, id)
            } else {
                reader.as_ref().unwrap().count_after(&username, time, id)
            };
            let (state, new_messages) = match coverage.get(&username).copied() {
                Some(position) if count_at(time).ok().is_some_and(|count| position.covers_boundary((time, local_id), count)) => ("synced", 0),
                Some(position) => (
                    "pending",
                    count_after(position.time, position.local_id).unwrap_or(0).max(1),
                ),
                None => ("local", 0),
            };
            let state = if uncertain_all || uncertain.contains(&username) { "unknown" }
                else if state == "synced" { conversation_job_state(&value, &owner, &username).unwrap_or("synced") } else { state };
            states.push(SessionUploadState {
                username,
                state: state.to_string(),
                new_messages,
            });
        }
        states
    })
    .await
    .map_err(|e| format!("读取上传状态失败：{e}"))
}

#[derive(Serialize)]
struct AvatarItem {
    username: String,
    data_url: String,
}

#[derive(Serialize)]
struct WechatAccountProfile {
    nickname: Option<String>,
    alias: Option<String>,
    avatar: Option<String>,
}

#[tauri::command]
async fn wechat_account_profile(app: AppHandle, account_id: String) -> Option<WechatAccountProfile> {
    if whatsapp::is_account(&account_id) {
        let account = whatsapp::account()?;
        return Some(WechatAccountProfile {
            nickname: account.name.or_else(|| Some(account.phone)),
            alias: Some(account.id),
            avatar: None,
        });
    }
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    let profile = query::account_profile(&cache, &account_id)?;
    let avatar = wechat::avatar::avatars(&cache, &[profile.username]).into_iter().next().map(|item| item.data_url);
    Some(WechatAccountProfile { nickname: profile.nickname, alias: profile.alias, avatar })
}

/// Avatars for the given usernames, as `data:` URLs. Missing names are simply absent, so the
/// caller keeps its letter avatars; nothing here is an error worth interrupting a chat for.
#[tauri::command]
async fn wechat_avatars(
    app: AppHandle,
    account_id: String,
    usernames: Vec<String>,
) -> Vec<AvatarItem> {
    if whatsapp::is_account(&account_id) {
        return Vec::new();
    }
    let Ok(_budget) = LOCAL_READ_BUDGET.acquire().await else {return Vec::new();};
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    tauri::async_runtime::spawn_blocking(move||wechat::avatar::avatars(&cache, &usernames.into_iter().take(100).collect::<Vec<_>>())
        .into_iter()
        .map(|avatar| AvatarItem {
            username: avatar.username,
            data_url: avatar.data_url,
        })
        .collect()).await.unwrap_or_default()
}

#[tauri::command]
async fn wechat_session_catalog(
    app: AppHandle,
    account_id: String,
) -> Result<Vec<SessionItem>, String> {
    if whatsapp::is_account(&account_id) {
        validate_account_id(&account_id)?;
        let data = app_data(&app);
        return tauri::async_runtime::spawn_blocking(move || whatsapp::sessions(&data))
            .await
            .map_err(|error| format!("读取 WhatsApp 会话失败：{error}"))?;
    }
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
    before_id: Option<i64>,
    before_shard: Option<String>,
) -> Result<Vec<MessageItem>, String> {
    let _read_budget=LOCAL_READ_BUDGET.acquire().await.map_err(|e|e.to_string())?;
    if whatsapp::is_account(&account_id) {
        let allowed = whatsapp_allowed_usernames(&app, &account_id)?;
        if !allowed.contains(&username) {
            return Err("该会话不在当前策略白名单中。".into());
        }
        let cursor = before.map(|time| match before_id {
            Some(id) => query::MessageCursor { time, local_id: id, source_shard: before_shard.unwrap_or_default() },
            None => query::MessageCursor::before_time(time),
        });
        let data = app_data(&app);
        return tauri::async_runtime::spawn_blocking(move || {
            let rows=whatsapp::messages(&data,&username,limit.unwrap_or(100).clamp(1,100),cursor)?;
            if rows.is_empty() && before.is_none() {return Err("LOCAL_CHAT_MISSING: 本地没有这段聊天记录。".into());}Ok(rows)
        })
        .await
        .map_err(|error| format!("读取 WhatsApp 消息失败：{error}"))?;
    }
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    let allowed = allowed_usernames(&app, &account_id)?;
    if !allowed.contains(&username) {
        return Err("该会话不在当前策略白名单中。".into());
    }
    // Both halves of the cursor come from the last row already on screen; without the id
    // this degrades to the old timestamp-only paging, which cannot see past a shared second.
    let cursor = before.map(|time| match before_id {
        Some(id) => query::MessageCursor { time, local_id: id, source_shard: before_shard.unwrap_or_default() },
        None => query::MessageCursor::before_time(time),
    });
    if !query::is_visible_chat_username(&username) {return Err("该会话不在查看范围内。".into());}
    tauri::async_runtime::spawn_blocking(move||{
        if username.ends_with("@qy_g") || username.ends_with("@qy_u") {decrypt::prepare_business_cache(&app_data(&app),&account_id,true)?;}
        if !query::chat_present(&cache,&username)? {return Err("LOCAL_CHAT_MISSING: 本地没有这段聊天记录。".into());}
        let rows=query::messages(&cache,&username,limit.unwrap_or(100).clamp(1,100),cursor)?;
        if rows.is_empty() && before.is_none() {return Err("LOCAL_CHAT_MISSING: 本地没有这段聊天记录。".into());}
        Ok(rows)
    }).await.map_err(|e|e.to_string())?
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
    if whatsapp::is_account(&account_id) {
        let allowed = whatsapp_allowed_usernames(&app, &account_id)?;
        let data = app_data(&app);
        return tauri::async_runtime::spawn_blocking(move || {
            let mut report = whatsapp::search(&data, &keyword, max_sessions.unwrap_or(200), Some(&allowed))?;
            // Rust enforces the whitelist even though the search only ever returns names.
            report.hits.retain(|hit| allowed.contains(&hit.username));
            if !groups_only.unwrap_or(false) { return Ok(report); }
            report.hits.retain(|hit| hit.is_group);
            Ok(report)
        })
        .await
        .map_err(|error| format!("搜索失败：{error}"))?;
    }
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
    history::list(&local_scope::root(&app_data(&app)))
}

/// Persist a summary after it has been shown to the user. The backend owns the id and the
/// timestamp, and enforces the record cap, so the drawer cannot grow the file unbounded.
#[tauri::command]
async fn summary_history_add(
    app: AppHandle,
    record: history::NewRecord,
) -> Result<Vec<history::SummaryRecord>, String> {
    history::add(&local_scope::root(&app_data(&app)), record)
}

#[tauri::command]
async fn summary_history_delete(
    app: AppHandle,
    id: String,
) -> Result<Vec<history::SummaryRecord>, String> {
    history::remove(&local_scope::root(&app_data(&app)), &id)
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
    entitlements::require(
        &data,
        &value.devify.base_url,
        capabilities::AI_AGENT_SUMMARY,
    )?;
    if value.agent.provider_id.trim().is_empty() {
        return Err("尚未在「Agent 总结」页面选择本地 Agent。".into());
    }
    // Each source has its own connect switch, so a WhatsApp summary must not be refused
    // just because WeChat happens to be disconnected.
    let allowed = if whatsapp::is_account(&account_id) {
        whatsapp_allowed_usernames(&app, &account_id)?
    } else {
        allowed_usernames(&app, &account_id)?
    };
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
    let language = value.language.summary_language().to_string();
    let run = tauri::async_runtime::spawn_blocking(move || {
        agent::summarize(&agent_settings, &input, &language, &data)
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
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(SyncState::default())
        .manage(UploadState::default())
        .manage(DevifyAuthState::default())
        .setup(|app| {
            let instance=instance_lock::acquire(&app_data(app.handle())).map_err(std::io::Error::other)?;
            app.manage(instance);
            let diagnostics_root = app_data(app.handle());
            let _ = diagnostics::record(&diagnostics_root, diagnostics::EventInput { module: diagnostics::Module::App, operation: diagnostics::Operation::Start, code: None, batch_id: None, duration_ms: None, request_id: None, server_request_id: None, http_status: None, request: None });
            let previous_hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                let _ = diagnostics::record(&diagnostics_root, diagnostics::EventInput { module: diagnostics::Module::App, operation: diagnostics::Operation::Panic, code: Some(diagnostics::Code::RustPanic), batch_id: None, duration_ms: None, request_id: None, server_request_id: None, http_status: None, request: None });
                previous_hook(info);
            }));
            #[cfg(desktop)]
            {
                app.handle().plugin(tauri_plugin_deep_link::init())?;
            }
            let mcp_data = app_data(app.handle());
            let mcp_bind = mcp::configured_bind();
            let (mcp_token, token_error) = match mcp::load_or_create_token(&mcp_data) {
                Ok(token) => (Some(token), None),
                Err(error) => (None, Some(error)),
            };
            let mcp_state = McpState::new(&mcp_bind, mcp_token);
            if let Some(error) = token_error {
                if let Ok(mut status) = mcp_state.status.lock() {
                    status.error = Some(error);
                }
            }
            app.manage(mcp_state);
            let reconcile_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // The port starts only after a signed-in account with a bridge capability is
                // known; the refresh afterwards picks up a plan change on restored sessions.
                reconcile_local_service(&reconcile_handle).await;
                refresh_entitlements_then_reconcile(&reconcile_handle).await;
            });
            let _=recover_local_logout(&app_data(app.handle()));
            let _=local_scope::root(&app_data(app.handle()));
            let _=summary_jobs::recover(&app_data(app.handle()));
            // Configuration acknowledgements and read-only remote reconciliation must
            // keep progressing while the sole upload executor prepares a slow local
            // database/voice batch. This monitor never plans or sends an archive.
            let status_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let _=flush_config_outbox(&status_handle).await;
                    task_runtime::reconcile_current_jobs(&status_handle).await;
                    emit_task_runtime(&status_handle);
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
            });
            let sync_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    summary_jobs::tick(&sync_handle).await;
                    task_runtime::backup_tick(&sync_handle).await;
                    emit_task_runtime(&sync_handle);
                    let idle=sync_handle.state::<UploadState>().syncing.uploading_accounts().is_ok_and(|v|v.is_empty());
                    if idle {let data=app_data(&sync_handle);let _=tauri::async_runtime::spawn_blocking(move||task_store::idle_maintenance(&data)).await;}
                    reconcile_local_service(&sync_handle).await;
                    // A short fixed tick keeps "每天定点" close to the minute it promises;
                    // `backup::due` decides whether this tick actually uploads.
                    tokio::select! { _=tokio::time::sleep(Duration::from_secs(30))=>{}, _=TASK_WAKE.notified()=>{} }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            feedback::feedback_draft_get,
            feedback::feedback_draft_save,
            feedback::feedback_draft_discard,
            feedback::feedback_diagnostics,
            feedback::diagnostic_record,
            feedback::feedback_submit,
            feedback::feedback_list,
            connector_detect,
            update_begin_install,
            update_upload_accounts,
            update_cancel_install,
            devify_auth_start,
            devify_auth_open,
            devify_auth_callback,
            devify_auth_cancel,
            devify_auth_status,
            devify_current_user,
            commands::analysis::devify_analysis_preferences_get,
            cloud_summary::cloud_summary_list,
            cloud_summary::cloud_summary_action,
            summary_jobs::summary_task_enqueue,
            summary_jobs::summary_task_retry,
            summary_jobs::summary_task_delete,
            commands::analysis::devify_analysis_preferences_save,
            commands::analysis::devify_analysis_manual_start,
            capabilities_list,
            entitlements_get,
            entitlements_refresh,
            devify_device_register,
            devify_login_password,
            devify_logout,
            devify_spaces,
            devify_space_ensure,
            commands::analysis::devify_report_create,
            commands::analysis::devify_analysis_list,
            commands::analysis::devify_analysis_detail,
            commands::events::devify_event_notes,
            commands::events::devify_event_update,
            commands::events::devify_event_note_remove,
            commands::events::devify_event_note_assign,
            commands::events::devify_event_note_create,
            commands::events::devify_event_relations,
            commands::events::devify_event_relation_create,
            commands::events::devify_event_relation_delete,
            devify_conversations,
            devify_conversation_detail,
            devify_conversation_messages,
            devify_message_detail,
            devify_sync_now,
            commands::topics::devify_topics,
            commands::topics::devify_topic_create,
            commands::topics::devify_topic_detail,
            commands::topics::devify_topic_update,
            commands::topics::devify_daily_plan_update,
            commands::topics::devify_report_plan_update,
            commands::topics::devify_topic_delete,
            commands::questions::devify_questions,
            commands::questions::devify_question_detail,
            commands::questions::devify_question_ask,
            commands::topics::devify_note_detail,
            wechat_collection_messages,
            wechat_collection_dates,
            wechat_catalog_dates,
            devify_upload_preview,
            devify_upload_identity,
            devify_upload_selected,
            devify_upload_retry_preview,
            devify_upload_retry,
            devify_upload_status,
            devify_daily_schedule_sync,
            devify_upload_list,
            runtime_commands::connector_setup_state,
            task_runtime::task_runtime_retry,
            runtime_commands::task_runtime_pause,
            runtime_commands::task_runtime_status,
            app_settings_get,
            app_settings_save,
            wechat_scan,
            chat_accounts,
            whatsapp_status,
            chat_account_activate,
            wechat_platform_capabilities,
            wechat_windows_key_scan,
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
            wechat_capture_accept_official,
            wechat_clear_keys,
            wechat_import_keys,
            wechat_import_saved_key,
            wechat_decrypt,
            wechat_sync,
            chat_remote_lookup,
            chat_remote_image,
            chat_sessions,
            wechat_upload_state,
            wechat_avatars,
            wechat_account_profile,
            wechat_session_catalog,
            wechat_messages,
            wechat_message_media,
            wechat_media_reveal,
            wechat_media_open_image,
            wechat_media_preview,
            wechat_export_bundle,
            wechat_voice_status,
            wechat_voice_settings_save,
            wechat_voice_transcribe,
            wechat_voice_audio,
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
#[path = "tests/cloud_retry.rs"]
mod cloud_retry_tests;

#[cfg(test)]
#[path = "tests/upload_list.rs"]
mod upload_list_tests;

#[cfg(test)]
#[path = "tests/attachment.rs"]
mod attachment_tests;

/// The upload path used to abort with "分页遇到相同时间戳" whenever a full page ended in a
/// second shared by more messages. These tests page a synthetic shard to prove every row
/// comes back exactly once.
#[cfg(test)]
#[path = "tests/message_paging.rs"]
mod message_paging_tests;

#[cfg(test)]
#[path = "tests/devify_error.rs"]
mod devify_error_tests;

#[cfg(test)]
#[path = "tests/upload_package.rs"]
mod upload_package_tests;
#[cfg(test)]
#[path = "tests/key_flow.rs"]
mod key_flow_tests;

#[cfg(test)]
#[path = "tests/space.rs"]
mod space_tests;

#[cfg(test)]
#[path = "tests/upload_coverage.rs"]
mod upload_coverage_tests;
