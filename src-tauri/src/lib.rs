use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{Local, Utc};
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
mod desktop_credentials;
mod diagnostics;
mod feedback;
pub mod entitlements;
pub mod history;
pub mod perm;
pub mod settings;
pub mod wechat;

pub mod mcp;
mod network;
mod update_gate;
#[cfg(test)]
mod reconcile_owner_tests;
#[cfg(test)]
mod connector_gate_tests;
#[cfg(test)]
mod manual_upload_tests;
#[cfg(test)]
mod security_identity_tests;

/// Network calls to Devify must never leave the UI spinning forever. Without a
/// timeout, a wrong LAN address or an unreachable host keeps "checking login"
/// pending until the kernel gives up, which can be minutes.
const DEVIFY_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DEVIFY_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Chat uploads carry a whole batch and can legitimately take much longer.
const DEVIFY_UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);

fn devify_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(DEVIFY_CONNECT_TIMEOUT)
        .timeout(DEVIFY_REQUEST_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn devify_upload_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(DEVIFY_CONNECT_TIMEOUT)
        .timeout(DEVIFY_UPLOAD_TIMEOUT)
        .build()
        .map_err(|_| "无法创建上传请求。".to_string())
}

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
/// treated as "the session expired": the server simply could not be reached.
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
fn update_begin_install(state: State<'_, UploadState>) -> Result<(), String> { state.syncing.begin_install() }

#[tauri::command]
fn update_cancel_install(state: State<'_, UploadState>) { state.syncing.cancel_install(); }

struct PendingDevifyAuth {
    state: String,
    verifier: String,
    base_url: String,
    generation: u64,
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

#[derive(serde::Deserialize)]
struct DevifyAuthConfig {
    client_id: String,
    browser_authorize_path: String,
    token_path: String,
    redirect_uri: String,
}

fn auth_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30)).build().map_err(|_| "无法创建登录请求。".into())
}

async fn devify_auth_config(
    settings: &settings::DevifySettings,
) -> Result<DevifyAuthConfig, String> {
    let base_url = settings.resolved_base_url()?;
    let response = auth_http_client()?
        .get(format!("{}/api/v1/auth/desktop/config", base_url))
        .send()
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
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
}

/// Uploads hold a read lease over the fixed credential/context; identity changes fail
/// rather than switching the user, service or device in the middle of an upload.
pub(crate) static UPLOAD_IDENTITY_LOCK: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());
static CREDENTIAL_REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static AUTH_START_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn devify_access_token_async(app_data: &Path, base_url: &str) -> Result<String, DevifyTokenError> {
    devify_access_token_with_cleanup(app_data, base_url, &mut None).await
}

async fn devify_access_token_with_cleanup(app_data: &Path, base_url: &str, cleared_generation: &mut Option<u64>) -> Result<String, DevifyTokenError> {
    devify_access_token_resolve(app_data, base_url, cleared_generation, false).await
}

async fn devify_access_token_resolve(app_data: &Path, base_url: &str, cleared_generation: &mut Option<u64>, force_refresh: bool) -> Result<String, DevifyTokenError> {
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
    let refresh = if tokens.refresh.is_empty() {
        return Err("服务端刷新令牌不存在，请重新登录。".into());
    } else {
        &tokens.refresh
    };
    let response = auth_http_client()?
        .post(format!(
            "{}/api/v1/auth/token/refresh",
            base_url.trim_end_matches('/')
        ))
        .json(&serde_json::json!({"refresh": refresh}))
        .send()
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
    if !desktop_credentials::save_if_current(app_data, token_generation, &original_tokens, &tokens)? {
        return Err("服务端登录状态已改变，请重新操作。".into());
    }
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
    let response = devify_client()
        .get(format!(
            "{}/api/v1/chat/uploads/{batch_id}",
            api_base
        ))
        .bearer_auth(&token)
        .send()
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

fn upload_list_base_url(settings: &settings::AppSettings, space_id: i64) -> Result<String, String> {
    validate_devify_space(settings, space_id)?;
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
    base_url: &str,
    space_id: i64,
    page: u32,
    page_size: u32,
    token: &str,
) -> Result<(reqwest::StatusCode, Vec<u8>), String> {
    // Never forward a bearer token via an HTTP redirect or return request URLs in errors.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| "无法创建上传列表请求。")?;
    let mut response = client
        .get(format!("{base_url}/api/v1/spaces/{space_id}/chat/uploads"))
        .query(&[("page", page), ("page_size", page_size)])
        .bearer_auth(token)
        .send()
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
    space_id: i64,
    page: Option<u32>,
    page_size: Option<u32>,
) -> Result<serde_json::Value, String> {
    let (page, page_size) = upload_list_pagination(page, page_size)?;
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    let base_url = upload_list_base_url(&settings, space_id)?;
    let token = match devify_access_token_async(&data, &base_url).await {
        Ok(token) => token,
        // Only a network failure gets the "check the connection" wording; an expired
        // session must still tell the user to sign in again.
        Err(DevifyTokenError::Unavailable(_)) => {
            return Err("刷新服务端登录状态失败，请检查网络连接。".into());
        }
        Err(error) => return Err(error.to_string()),
    };
    if upload_list_base_url(&settings::load(&data)?, space_id)? != base_url {
        return Err("服务端地址已改变，请重新读取上传列表。".into());
    }
    if desktop_credentials::load(&data, &base_url)?.access != token {
        return Err("服务端登录状态已改变，请重新读取上传列表。".into());
    }
    let generation = request_generation(&data, &base_url, &token)?;
    let (status, body) = fetch_upload_list(&base_url, space_id, page, page_size, &token).await?;
    // A response for an old selection must not overwrite the new space or service.
    let current = settings::load(&data)?;
    if upload_list_base_url(&current, space_id)? != base_url {
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
    let api_base = settings.devify.resolved_base_url()?;
    let token = devify_access_token_async(&app_data(&app), &api_base).await?;
    let response = devify_client()
        .post(format!(
            "{}/api/v1/spaces/{space_id}/takeover",
            api_base
        ))
        .bearer_auth(token)
        .json(&serde_json::json!({"reason": reason.trim()}))
        .send()
        .await
        .map_err(|e| devify_network_message("接管空间失败", &e))?;
    if !response.status().is_success() {
        return Err(format!("接管空间失败：{}", response.status()));
    }
    Ok(())
}

/// Prefer the personal space the server creates for every account; fall back to the first
/// space the account can see. Used after login and by `devify_space_ensure`.
fn pick_personal_space(spaces: &[DevifySpace]) -> Option<i64> {
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
        .map(|space| space.id)
}

/// Login should not leave the app without an upload target: select the personal space when
/// nothing is chosen yet (or when the stored space no longer exists).
async fn ensure_active_space(app: &AppHandle) -> Option<i64> {
    let mut ignored_generation = None;
    ensure_active_space_for(app, None, &mut ignored_generation).await
}

async fn ensure_active_space_for(app: &AppHandle, expected: Option<(&desktop_credentials::Credentials, u64)>, completed_generation: &mut Option<u64>) -> Option<i64> {
    let data = app_data(app);
    let settings = settings::load(&data).ok()?;
    let base = settings.devify.resolved_base_url().ok()?;
    let token = if let Some((credentials, _)) = expected {
        credentials.access.clone()
    } else {
        devify_access_token_async(&data, &base).await.ok()?
    };
    let identity = UPLOAD_IDENTITY_LOCK.read().await;
    let (credentials, generation) = desktop_credentials::load_with_generation(&data, &base).ok()?;
    let current = settings::load(&data).ok()?;
    if credentials.access != token || current.devify.resolved_base_url().ok()? != base
        || expected.is_some_and(|(expected, expected_generation)| credentials != *expected || generation != expected_generation)
        || current.devify.active_space_id != settings.devify.active_space_id
        || current.devify.device_id != settings.devify.device_id
        || current.active_account_id != settings.active_account_id { return None; }
    let payload = fixed_identity_get(&base, &token, "/api/v1/spaces").await.ok()?;
    let spaces: Vec<DevifySpace> = serde_json::from_value(payload).ok()?;
    let known = |id: i64| spaces.iter().any(|space| space.id == id);
    let chosen = settings.devify.active_space_id.filter(|id| known(*id))
        .or_else(|| pick_personal_space(&spaces));
    drop(identity);
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
        space_changed = current.devify.active_space_id != chosen;
        current.devify.active_space_id = chosen;
        Ok(())
    }).ok()?;
    *completed_generation = Some(generation.wrapping_add(u64::from(space_changed)));
    chosen
}

#[tauri::command]
async fn devify_space_ensure(app: AppHandle) -> Result<Option<i64>, String> {
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
    let response = devify_client()
        .get(format!(
            "{}/api/v1/spaces",
            api_base
        ))
        .bearer_auth(&token)
        .send()
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

/// Create a report snapshot on the server. The server generates the body asynchronously
/// (`202` + status `queued`); listing again picks up the finished report.
#[tauri::command]
async fn devify_report_create(
    app: AppHandle,
    space_id: i64,
    title: String,
    period_start: Option<String>,
    period_end: Option<String>,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
    entitlements::require(
        &app_data(&app),
        &settings.devify.base_url,
        capabilities::CLOUD_REPORTS_GENERATE,
    )?;
    let title = title.trim();
    if title.is_empty() {
        return Err("报告标题不能为空。".into());
    }
    // `request_key` makes the create idempotent server-side; a fresh key per click means a
    // deliberate second generation is not treated as a retry of the first one.
    let request_key = format!(
        "desktop-{}-{}",
        chrono::Utc::now().timestamp_millis(),
        chrono::Utc::now().timestamp_subsec_nanos()
    );
    devify_json_request(
        &app,
        reqwest::Method::POST,
        format!("/api/v1/spaces/{space_id}/reports"),
        Some(serde_json::json!({
            "request_key": request_key,
            "title": title,
            "period_start": period_start,
            "period_end": period_end,
        })),
    )
    .await
}

#[tauri::command]
async fn devify_analysis_list(
    app: AppHandle,
    kind: String,
    space_id: i64,
    page: Option<u32>,
    conversation_id: Option<i64>,
) -> Result<serde_json::Value, String> {
    if !matches!(kind.as_str(), "events" | "reports") {
        return Err("不支持的服务端分析类型。".into());
    }
    if space_id <= 0 {
        return Err("请先选择 空间。".into());
    }
    if conversation_id.is_some_and(|id| id <= 0) || (conversation_id.is_some() && kind != "events") {
        return Err("会话标识无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    if settings.devify.active_space_id != Some(space_id) {
        return Err("空间已改变，请重新选择后再同步。".into());
    }
    let resource = if kind == "events" {
        "events"
    } else {
        "reports"
    };
    let first_page = page.unwrap_or(1).max(1);
    let client = devify_client();
    let token = devify_access_token_async(&app_data(&app), &settings.devify.base_url).await?;
    let generation = request_generation(&app_data(&app), &settings.devify.base_url, &token)?;
    let mut all_items = Vec::new();
    let mut current_page = first_page;
    let mut total = None;
    for _ in 0..100 {
        let mut url = format!(
            "{}/api/v1/spaces/{space_id}/{resource}?page={current_page}&page_size=100",
            settings.devify.resolved_base_url()?
        );
        if let Some(id) = conversation_id {
            url.push_str(&format!("&conversation_id={id}"));
        }
        let response = client
            .get(url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| devify_network_message(&format!("同步服务端{kind}失败"), &e))?;
        let status = response.status();
        let payload: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("读取服务端{kind}响应失败：{e}"))?;
        if !status.is_success() {
            let detail = devify_response_error(&app, status, &payload, "服务端拒绝了请求", &settings.devify.base_url, &token, generation);
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
/// user can act on. The API hides the space when the caller is not a member
/// (or an admin without a takeover), so both cases share the same 404.
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
    let client = devify_client();
    let mut request = client
        .request(
            method,
            format!("{}{}", settings.devify.resolved_base_url()?, path),
        )
        .bearer_auth(&token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|e| devify_network_message("访问服务端失败", &e))?;
    let status = response.status();
    if status == reqwest::StatusCode::NO_CONTENT {
        return Ok(serde_json::Value::Null);
    }
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("读取服务端响应失败：{e}"))?;
    if !status.is_success() {
        let detail = devify_response_error(app, status, &payload, "服务端拒绝了请求", &settings.devify.base_url, &token, generation);
        return Err(format!("服务端请求失败：{detail}"));
    }
    Ok(payload.get("data").cloned().unwrap_or(payload))
}

#[tauri::command]
async fn devify_current_user(app: AppHandle) -> Result<serde_json::Value, String> {
    let user = devify_json_request(&app, reqwest::Method::GET, "/api/v1/auth/user".into(), None).await?;
    Ok(serde_json::json!({ "id": user.get("id"), "display_name": user.get("display_name"), "username": user.get("username") }))
}

#[tauri::command]
async fn devify_analysis_preferences_get(app: AppHandle, space_id: i64) -> Result<serde_json::Value, String> {
    validate_devify_space(&settings::load(&app_data(&app))?, space_id)?;
    let data = devify_json_request(&app, reqwest::Method::GET, format!("/api/v1/spaces/{space_id}/workflow-config"), None).await?;
    data.get("chat_analysis_preferences").cloned().ok_or_else(|| "服务端尚不支持分析偏好设置，请更新服务端。".to_string())
}

#[tauri::command]
async fn devify_analysis_preferences_save(app: AppHandle, space_id: i64, preferences: serde_json::Value) -> Result<serde_json::Value, String> {
    validate_devify_space(&settings::load(&app_data(&app))?, space_id)?;
    let body = serde_json::json!({"chat_analysis_preferences": preferences});
    let data = devify_json_request(&app, reqwest::Method::PATCH, format!("/api/v1/spaces/{space_id}/workflow-config"), Some(body)).await?;
    data.get("chat_analysis_preferences").cloned().ok_or_else(|| "服务端未确认分析偏好，请更新服务端。".to_string())
}

#[tauri::command]
async fn devify_analysis_manual_start(app: AppHandle, space_id: i64, conversation_id: i64) -> Result<serde_json::Value, String> {
    validate_devify_space(&settings::load(&app_data(&app))?, space_id)?;
    if conversation_id <= 0 { return Err("会话标识无效。".into()); }
    devify_json_request(&app, reqwest::Method::POST, format!("/api/v1/spaces/{space_id}/analysis-jobs/manual"), Some(serde_json::json!({"conversation_id": conversation_id}))).await
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
    let resource = if kind == "events" {
        "events"
    } else {
        "reports"
    };
    devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/{resource}/{item_id}"),
        None,
    )
    .await
}

/// The server serialises id lists as an array on some endpoints and as a JSON string
/// (``"[22]"``) on others, so accept both before touching the notes.
fn id_list(value: Option<&serde_json::Value>) -> Vec<i64> {
    let Some(value) = value else {
        return Vec::new();
    };
    let parsed = match value {
        serde_json::Value::String(text) => match serde_json::from_str(text) {
            Ok(parsed) => parsed,
            Err(_) => return Vec::new(),
        },
        other => other.clone(),
    };
    parsed
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            item.as_i64()
                .or_else(|| item.as_str().and_then(|text| text.parse().ok()))
        })
        .collect()
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
        return Err("事件不属于当前 空间。".into());
    }
    let mut notes = Vec::new();
    for id in id_list(event.get("note_ids")) {
        if let Ok(note) = devify_json_request(
            &app,
            reqwest::Method::GET,
            format!("/api/v1/notes/{id}"),
            None,
        )
        .await
        {
            notes.push(note);
        }
    }
    Ok(serde_json::Value::Array(notes))
}

/// Detach one note (a step in the timeline) from an event. The note itself is
/// kept; only the event membership is removed.
#[tauri::command]
async fn devify_event_note_remove(
    app: AppHandle,
    space_id: i64,
    event_id: i64,
    note_id: i64,
) -> Result<serde_json::Value, String> {
    if event_id <= 0 || note_id <= 0 {
        return Err("进展记录无效。".into());
    }
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
        return Err("事件不属于当前 空间。".into());
    }
    devify_json_request(
        &app,
        reqwest::Method::DELETE,
        format!("/api/v1/events/{event_id}/notes/{note_id}"),
        None,
    )
    .await
}

/// Create a manual note (a progress update that happened outside chat) and
/// attach it to the event, mirroring the prototype's「补充进展」row.
#[tauri::command]
async fn devify_event_note_create(
    app: AppHandle,
    space_id: i64,
    event_id: i64,
    text: String,
    start_at: Option<String>,
) -> Result<serde_json::Value, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("请先填写进展内容。".into());
    }
    if event_id <= 0 {
        return Err("事件无效。".into());
    }
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
        return Err("事件不属于当前 空间。".into());
    }
    let now = chrono::Utc::now();
    let note_key = format!("manual:{event_id}:{}", now.timestamp_millis());
    let body = serde_json::json!({
        "note_key": note_key,
        "title": "［我补充］聊天之外的信息",
        "content": text,
        "start_at": start_at.unwrap_or_else(|| now.to_rfc3339()),
    });
    let note = devify_json_request(
        &app,
        reqwest::Method::POST,
        format!("/api/v1/spaces/{space_id}/notes"),
        Some(body),
    )
    .await?;
    let note_id = note
        .get("id")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| "服务端没有返回新的进展记录。".to_string())?;
    devify_json_request(
        &app,
        reqwest::Method::POST,
        format!("/api/v1/events/{event_id}/notes"),
        Some(serde_json::json!({ "note_ids": [note_id] })),
    )
    .await?;
    Ok(note)
}

/// Re-attach a note to an event (used by the timeline's undo action).
#[tauri::command]
async fn devify_event_note_assign(
    app: AppHandle,
    space_id: i64,
    event_id: i64,
    note_id: i64,
) -> Result<serde_json::Value, String> {
    if event_id <= 0 || note_id <= 0 {
        return Err("进展记录无效。".into());
    }
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
        return Err("事件不属于当前 空间。".into());
    }
    devify_json_request(
        &app,
        reqwest::Method::POST,
        format!("/api/v1/events/{event_id}/notes"),
        Some(serde_json::json!({ "note_ids": [note_id] })),
    )
    .await
}

/// Update an event's own fields. The server accepts `title`, `content` and
/// `status`; the desktop never sends the timestamp fields.
#[tauri::command]
async fn devify_event_update(
    app: AppHandle,
    space_id: i64,
    event_id: i64,
    title: Option<String>,
    content: Option<String>,
    status: Option<String>,
) -> Result<serde_json::Value, String> {
    if event_id <= 0 {
        return Err("事件无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
    let mut body = serde_json::Map::new();
    if let Some(title) = title {
        let title = title.trim();
        if title.is_empty() {
            return Err("事件标题不能为空。".into());
        }
        body.insert("title".into(), serde_json::Value::String(title.to_string()));
    }
    if let Some(content) = content {
        body.insert("content".into(), serde_json::Value::String(content));
    }
    if let Some(status) = status {
        if !matches!(status.as_str(), "open" | "in_progress" | "done") {
            return Err("不支持的事件状态。".into());
        }
        body.insert("status".into(), serde_json::Value::String(status));
    }
    if body.is_empty() {
        return Err("没有需要保存的修改。".into());
    }
    devify_json_request(
        &app,
        reqwest::Method::PATCH,
        format!("/api/v1/events/{event_id}"),
        Some(serde_json::Value::Object(body)),
    )
    .await
}

/// All event relations in one space; the event detail only returns relation ids,
/// so the client joins these records against the event list to show titles.
#[tauri::command]
async fn devify_event_relations(
    app: AppHandle,
    space_id: i64,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
    let mut all = Vec::new();
    for page in 1..=50u32 {
        let value = devify_json_request(
            &app,
            reqwest::Method::GET,
            format!("/api/v1/spaces/{space_id}/event-relations?page={page}&page_size=100"),
            None,
        )
        .await?;
        let list = value
            .get("list")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let count = list.len();
        all.extend(list);
        let has_next = value
            .get("pagination")
            .and_then(|pagination| pagination.get("next"))
            .and_then(serde_json::Value::as_str)
            .map(|next| !next.is_empty())
            .unwrap_or(false);
        if !has_next || count == 0 {
            break;
        }
    }
    Ok(serde_json::Value::Array(all))
}

/// Record a judgement about two events. The desktop uses it for the
/// 「关联事件」 action: related + accepted.
#[tauri::command]
async fn devify_event_relation_create(
    app: AppHandle,
    space_id: i64,
    left_event_id: i64,
    right_event_id: i64,
    relation_kind: Option<String>,
    decision: Option<String>,
    reason: Option<String>,
) -> Result<serde_json::Value, String> {
    if left_event_id <= 0 || right_event_id <= 0 || left_event_id == right_event_id {
        return Err("请选择两个不同的事件。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
    let kind = relation_kind.unwrap_or_else(|| "related".into());
    if !matches!(
        kind.as_str(),
        "same" | "related" | "separate" | "depends_on"
    ) {
        return Err("不支持的事件关系。".into());
    }
    let decision = decision.unwrap_or_else(|| "accepted".into());
    if !matches!(decision.as_str(), "pending" | "accepted" | "rejected") {
        return Err("不支持的关系判定。".into());
    }
    let body = serde_json::json!({
        "left_event_id": left_event_id,
        "right_event_id": right_event_id,
        "relation_kind": kind,
        "decision": decision,
        "reason": reason.unwrap_or_default(),
    });
    devify_json_request(
        &app,
        reqwest::Method::POST,
        format!("/api/v1/spaces/{space_id}/event-relations"),
        Some(body),
    )
    .await
}

#[tauri::command]
async fn devify_event_relation_delete(
    app: AppHandle,
    space_id: i64,
    relation_id: i64,
) -> Result<serde_json::Value, String> {
    if relation_id <= 0 {
        return Err("关联记录无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
    devify_json_request(
        &app,
        reqwest::Method::DELETE,
        format!("/api/v1/event-relations/{relation_id}"),
        None,
    )
    .await
}

#[tauri::command]
async fn devify_conversations(
    app: AppHandle,
    space_id: i64,
    search: Option<String>,
    page: Option<u32>,
    page_size: Option<u32>,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
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
    space_id: i64,
    conversation_id: i64,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
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
    space_id: i64,
    conversation_id: i64,
    page: Option<u32>,
    page_size: Option<u32>,
    focus_message_id: Option<i64>,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
    if conversation_id <= 0 {
        return Err("会话标识无效。".into());
    }
    let mut path = format!(
        "/api/v1/spaces/{space_id}/conversations/{conversation_id}/messages?page={}&page_size={}",
        page.unwrap_or(1).max(1),
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
    space_id: i64,
    conversation_id: i64,
    message_id: i64,
) -> Result<serde_json::Value, String> {
    if conversation_id <= 0 || message_id <= 0 {
        return Err("消息标识无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
    devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/spaces/{space_id}/conversations/{conversation_id}/messages/{message_id}"),
        None,
    )
    .await
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
    time_local: Option<String>,
    timezone: Option<String>,
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
    let mut body = serde_json::json!({
        "title": title,
        "instruction": instruction,
        "period": period,
    });
    if let Some(time_local) = time_local.filter(|value| !value.trim().is_empty()) {
        if !settings::is_valid_time(time_local.trim()) {
            return Err("执行时间必须使用 HH:MM 格式。".into());
        }
        body["time_local"] = serde_json::json!(time_local.trim());
    }
    if let Some(timezone) = timezone.filter(|value| !value.trim().is_empty()) {
        body["timezone"] = serde_json::json!(timezone.trim());
    }
    devify_json_request(
        &app,
        reqwest::Method::POST,
        format!("/api/v1/spaces/{space_id}/topics"),
        Some(body),
    )
    .await
}

#[tauri::command]
async fn devify_topic_detail(
    app: AppHandle,
    space_id: i64,
    topic_id: i64,
) -> Result<serde_json::Value, String> {
    if topic_id <= 0 {
        return Err("主题标识无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
    let topic = devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/topics/{topic_id}"),
        None,
    )
    .await?;
    if topic
        .get("space_id")
        .and_then(serde_json::Value::as_i64)
        .is_some_and(|topic_space_id| topic_space_id != space_id)
    {
        return Err("主题不属于当前 空间。".into());
    }
    Ok(topic)
}

#[tauri::command]
async fn devify_note_detail(
    app: AppHandle,
    space_id: i64,
    note_id: i64,
) -> Result<serde_json::Value, String> {
    if note_id <= 0 {
        return Err("原文标识无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
    let note = devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/notes/{note_id}"),
        None,
    )
    .await?;
    if note
        .get("space_id")
        .and_then(serde_json::Value::as_i64)
        .is_some_and(|note_space_id| note_space_id != space_id)
    {
        return Err("原文不属于当前 空间。".into());
    }
    Ok(note)
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
async fn devify_daily_plan_update(
    app: AppHandle, space_id: i64, topic_id: i64, enabled: Option<bool>, time_local: Option<String>,
) -> Result<serde_json::Value, String> {
    if enabled.is_none() && time_local.is_none() { return Err("没有要修改的日报设置。".into()); }
    let topic = devify_topic_detail(app.clone(), space_id, topic_id).await?;
    if !matches!(topic.get("title").and_then(|v| v.as_str()), Some(DAILY_TOPIC_TITLE | "每日报告"))
        || topic.get("period").and_then(|v| v.as_str()) != Some("day") {
        return Err("无法确认日报计划，请刷新后重试。".into());
    }
    let mut body = serde_json::json!({});
    if let Some(enabled) = enabled { body["enabled"] = enabled.into(); }
    if let Some(time) = time_local {
        if !settings::is_valid_time(&time) { return Err("日报时间必须使用 HH:MM 格式。".into()); }
        body["time_local"] = time.into();
    }
    let current = settings::load(&app_data(&app))?;
    validate_devify_space(&current, space_id)?;
    devify_json_request(&app, reqwest::Method::PATCH, format!("/api/v1/topics/{topic_id}"), Some(body)).await
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
async fn devify_question_detail(
    app: AppHandle,
    space_id: i64,
    question_id: i64,
) -> Result<serde_json::Value, String> {
    if question_id <= 0 {
        return Err("问答标识无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, space_id)?;
    devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/spaces/{space_id}/questions/{question_id}"),
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
    entitlements::require(&app_data(&app), &settings.devify.base_url, capabilities::CLOUD_ASK)?;
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

async fn fixed_identity_get(base: &str, token: &str, path: &str) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30)).build().map_err(|_| "无法创建云端请求。")?;
    let mut response = client.get(format!("{base}{path}")).bearer_auth(token)
        .send().await.map_err(|_| "无法读取云端数据，请检查网络连接。")?;
    if !response.status().is_success() { return Err(format!("无法读取云端数据：HTTP {}。", response.status().as_u16())); }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "无法读取云端响应。")? {
        append_retry_identity_body(&mut body, &chunk)?;
    }
    let payload: serde_json::Value = serde_json::from_slice(&body).map_err(|_| "云端响应格式无效。")?;
    Ok(payload.get("data").cloned().unwrap_or(payload))
}

/// Read existing legacy message keys before assigning new identities. Never assume that a
/// historic `src:<local_id>` belongs to the earliest or latest local shard.
async fn load_legacy_upload_keys(app: &AppHandle, space_id: i64, account_id: &str, usernames: &[String], fixed: Option<(&str, &str)>) -> Result<LegacyUploadKeys, String> {
    let mut result = LegacyUploadKeys::new();
    for username in usernames {
        let encoded: String = url::form_urlencoded::byte_serialize(username.as_bytes()).collect();
        let mut conversation = None;
        for page in 1..=100 {
            let path = format!("/api/v1/spaces/{space_id}/conversations?search={encoded}&page={page}&page_size=100");
            let data = if let Some((base, token)) = fixed { fixed_identity_get(base, token, &path).await? } else { devify_json_request(app, reqwest::Method::GET, path, None).await? };
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
        let mut keys = ExistingUploadKeys::default();
        if let Some(id) = conversation {
            for page in 1..=10000 {
                let path = format!("/api/v1/spaces/{space_id}/conversations/{id}/messages?page={page}&page_size=100");
                let data = if let Some((base, token)) = fixed { fixed_identity_get(base, token, &path).await? } else { devify_json_request(app, reqwest::Method::GET, path, None).await? };
                let items = data.get("list").and_then(|v| v.as_array()).ok_or("无法读取云端消息列表。")?;
                for item in items {
                    let key = item.get("message_key").and_then(|v| v.as_str()).unwrap_or_default();
                    if key.starts_with("srv:") || key.starts_with("local:") { keys.stable.insert(key.to_string()); }
                    if let Some(id) = key.strip_prefix("src:").and_then(|s| s.parse::<i64>().ok()) {
                        let time = item.get("occurred_at").and_then(|v| v.as_str()).and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok()).map(|v| v.timestamp());
                        if keys.legacy.insert(id, time).is_some() { return Err("云端存在冲突的旧消息标识，无法安全上传。".into()); }
                    }
                }
                let next = data.pointer("/pagination/next").ok_or("云端消息分页信息不完整，无法安全校验旧消息。")?;
                if next.is_null() { break; }
                if page == 10000 { return Err("云端消息过多，无法完整校验旧消息。".into()); }
            }
        }
        result.insert(username.clone(), keys);
    }
    Ok(result)
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
            || value.devify.active_space_id != Some(job.space_id) { return false; }
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
    space_id: i64,
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
const ATTACHMENT_TOTAL_LIMIT_BYTES: u64 = 256 * 1024 * 1024;

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

fn prepare_upload_with_voice(
    app_data: &Path,
    cache: &Path,
    media_root: &Path,
    account_id: &str,
    space_id: i64,
    base_url: &str,
    usernames: &[String],
    since: Option<i64>,
    until: Option<i64>,
    message_selection: &BTreeMap<String, Vec<settings::MessageRef>>,
    voice_settings: &settings::VoiceUploadSettings,
    attachments: &str,
    legacy_keys: &LegacyUploadKeys,
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
        let mut session_messages =
            collect_upload_messages(cache, &session.username, since, until, None)?;
        session_messages = filter_upload_messages(
            session_messages,
            message_selection.get(&session.username).map(Vec::as_slice),
        )?;
        let inventory = query::local_id_inventory(cache, &session.username)?;
        let server_counts = query::server_id_counts(cache, &session.username)?;
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
            if let Some((bytes, kind)) = decoded {
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
            let mut transcripts = voice::load_transcripts(app_data);
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
                            voice::ensure_wav(app_data, account_id, item.server_id, blob)
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
                        let wav = voice::ensure_wav(app_data, account_id, item.server_id, blob);
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
                let _ = voice::save_transcripts(app_data, &transcripts);
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
async fn wechat_collection_dates(app: AppHandle, account_id: String, usernames: Vec<String>) -> Result<BTreeMap<String, Vec<(String, usize)>>, String> {
    let allowed = allowed_usernames(&app, &account_id)?;
    if usernames.iter().any(|name| !allowed.contains(name)) { return Err("会话不在已选范围中。".into()); }
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    tauri::async_runtime::spawn_blocking(move || usernames.into_iter().map(|name| query::message_dates(&cache, &name).map(|dates| (name, dates))).collect()).await.map_err(|e| format!("读取日期失败：{e}"))?
}

#[tauri::command]
async fn wechat_catalog_dates(app: AppHandle, account_id: String, username: String) -> Result<Vec<(String, usize)>, String> {
    validate_account_id(&account_id)?;
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
    entitlements::require(&data, &settings.devify.base_url, capabilities::INGEST_UPLOAD)?;
    let (owner, token) = verified_upload_identity(&data, &account_id, space_id).await?;
    let _identity_guard = UPLOAD_IDENTITY_LOCK.read().await;
    let latest = settings::load(&data)?;
    validate_owner_state(&latest, &owner, &token)?;
    validate_upload_scope(&latest, &account_id, space_id, &usernames)?;
    if desktop_credentials::load(&data, &owner.base_url)?.access != token {
        return Err("登录状态已改变，请重新确认采集任务。".into());
    }
    let cache = decrypt::cache_dir(&data, &account_id);
    let media_root = account_root(&account_id).unwrap_or_else(|_| cache.clone());
    let base_url = owner.base_url.clone();
    let voice_settings = latest.voice.clone();
    let attachments = latest.backup.attachments.clone();
    let selection = message_selection.unwrap_or_default();
    let legacy_keys = load_legacy_upload_keys(&app, space_id, &account_id, &usernames, Some((&owner.base_url, &token))).await?;
    let data_for_prepare = data.clone();
    let preview = tauri::async_runtime::spawn_blocking(move || {
        prepare_upload_with_voice(
            &data_for_prepare,
            &cache,
            &media_root,
            &account_id,
            space_id,
            &base_url,
            &usernames,
            since,
            until,
            &selection,
            &voice_settings,
            &attachments,
            &legacy_keys,
        )
        .map(|prepared| prepared.preview)
    })
    .await
    .map_err(|e| format!("读取上传预览失败：{e}"))??;
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
    validate_upload_scope(&value, &job.account_id, job.space_id, &scope.usernames)?;
    if !settings::same_service_url(&scope.base_url, &value.devify.base_url) {
        return Err("服务地址已更改，无法继续原上传任务。".into());
    }
    let (owner, token) = verified_upload_identity(&data, &job.account_id, job.space_id).await?;
    validate_owned_job(&value, &job, &owner, &token, &scope)?;
    if !matches!(job.status.as_str(), "queued" | "running") || job.batch_id.is_some() || job.create_started != Some(false) {
        return Err("无法确认本机批次尚未提交，请检查服务端记录。".into());
    }
    let preview = devify_upload_preview(app.clone(), job.account_id.clone(), job.space_id, scope.usernames.clone(), scope.since, scope.until, Some(scope.message_selection.clone())).await?;
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
    let result = devify_upload_wechat(app.clone(), job_id.to_string(), job.space_id, job.account_id.clone(), scope.usernames.clone(), scope.since, scope.until, Some(scope.base_url.clone()), Some(preview.fingerprint), scope.message_selection.clone(), UploadTrigger::Manual, None).await;
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
    space_id: i64,
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
    validate_upload_scope(&settings, &account_id, space_id, &usernames)?;
    entitlements::require(&app_data(&app), &settings.devify.base_url, capabilities::INGEST_UPLOAD)?;
    if fingerprint.len() != 64 || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("请先预览并确认上传范围。".into());
    }
    let message_selection = message_selection.unwrap_or_default();
    let (owner, owner_token) = verified_upload_identity(&app_data(&app), &account_id, space_id).await?;
    let mut initial_baselines = BTreeMap::new();
    if initial_history.unwrap_or(false) {
        if !message_selection.is_empty() { return Err("首次历史上传请选择整段记录。".into()); }
        let cache = decrypt::cache_dir(&app_data(&app), &account_id);
        for username in &usernames {
            if settings.sync.cursors.get(&account_id).is_some_and(|items| items.contains_key(username)) {
                return Err("所选会话已有自动同步记录，不能重新设置首次范围。".into());
            }
            if settings.upload_queue.iter().any(|job| job.account_id == account_id && job.initial_baselines.contains_key(username)) {
                return Err("所选会话已有尚未完成的首次历史上传，请先处理该任务。".into());
            }
            let (time, local_id) = query::latest_position(&cache, username)?.ok_or("所选会话没有本地消息。")?;
            let boundary_count = query::count_at_time(&cache, username, time)?;
            initial_baselines.insert(username.clone(), settings::SyncCursor { time, local_id, boundary_count: Some(boundary_count) });
        }
    }
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    let mut selected_refs = BTreeMap::new();
    for username in &usernames {
        let messages = collect_upload_messages(&cache, username, since, until, None)?;
        let messages = filter_upload_messages(messages, message_selection.get(username).map(Vec::as_slice))?;
        selected_refs.insert(username.clone(), messages.into_iter().map(|m| settings::MessageRef { local_id: m.local_id, time: m.time, source_shard: m.source_shard }).collect());
    }
    let batches = manual_batch_plan(&selected_refs, SYNC_MESSAGES_PER_BATCH)?;
    if batches.len() > 1 {
        let preview = devify_upload_preview(app.clone(), account_id.clone(), space_id, usernames.clone(), since, until, Some(message_selection.clone())).await?;
        verify_upload_preview(Some(&fingerprint), &preview.fingerprint)?;
        let group = format!("manual-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default());
        let total = batches.len();
        let now = chrono::Utc::now().to_rfc3339();
        let data = app_data(&app);
        let mut pending = Vec::with_capacity(total);
        for (index, selection) in batches.into_iter().enumerate() {
            let names = selection.keys().cloned().collect();
            pending.push(settings::UploadJob {
                id: format!("{group}-{index}"), space_id, account_id: account_id.clone(),
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
            validate_upload_scope(current, &account_id, space_id, &usernames)?;
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
        space_id,
        account_id: account_id.clone(),
        batch_id: None,
        owner: Some(owner.clone()),
        create_started: Some(false),
        package_digest: None,
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
        validate_upload_scope(current, &account_id, space_id, &usernames)?;
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
    let settings = settings::load(&data)?;
    let job = settings
        .upload_queue
        .iter()
        .find(|item| item.id == job_id)
        .cloned()
        .ok_or_else(|| "找不到上传任务。".to_string())?;
    let scope = job.retry_scope(&settings)?.clone();
    if job.owner.is_none() || (job.batch_id.is_none() && job.create_started != Some(false)) {
        return Err("无法确认原任务的账号或服务端批次，请核对后重新创建采集任务。".into());
    }
    if job.batch_id.is_some() && job.package_digest.is_none() {
        return Err("原任务没有保存归档摘要，无法安全重试，请重新创建采集任务。".into());
    }
    let (owner, token) = verified_upload_identity(&data, &job.account_id, job.space_id).await?;
    let _identity_guard = UPLOAD_IDENTITY_LOCK.read().await;
    validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
    let latest = settings::load(&data)?;
    let cache = decrypt::cache_dir(&data, &job.account_id);
    let voice_settings = latest.voice.clone();
    let attachments = latest.backup.attachments.clone();
    let account_id = job.account_id.clone();
    let media_root = account_root(&account_id).unwrap_or_else(|_| cache.clone());
    let legacy_keys = load_legacy_upload_keys(&app, job.space_id, &account_id, &scope.usernames, Some((&owner.base_url, &token))).await?;
    let data_for_prepare = data.clone();
    let scope_for_prepare = scope.clone();
    let space_id = job.space_id;
    let preview = tauri::async_runtime::spawn_blocking(move || {
        prepare_upload_with_voice(
            &data_for_prepare,
            &cache,
            &media_root,
            &account_id,
            space_id,
            &scope_for_prepare.base_url,
            &scope_for_prepare.usernames,
            scope_for_prepare.since,
            scope_for_prepare.until,
            &scope_for_prepare.message_selection,
            &voice_settings,
            &attachments,
            &legacy_keys,
        )
        .map(|prepared| prepared.preview)
    })
    .await
    .map_err(|e| format!("读取上传预览失败：{e}"))??;
    validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
    Ok(preview)
}

// The token is deliberately never serialized, formatted, or logged.
struct CloudRetryAuth {
    token: String,
    base_url: String,
    job_id: String,
    space_id: i64,
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

async fn verified_upload_identity(data: &Path, account_id: &str, space_id: i64) -> Result<(settings::UploadOwner, String), String> {
    let settings = settings::load(data)?;
    if settings.active_account_id != account_id { return Err("当前微信账号已改变。".into()); }
    let base_url = upload_list_base_url(&settings, space_id)?;
    let token = devify_access_token_async(data, &base_url).await?;
    let device_id = cloud_retry_device_id(&settings.devify.device_id, &token)?;
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30)).build().map_err(|_| "无法创建身份请求。")?;
    let mut response = client.get(format!("{base_url}/api/v1/auth/user"))
        .bearer_auth(&token).send().await.map_err(|_| "无法确认云端登录用户。")?;
    if !response.status().is_success() { return Err("无法确认云端登录用户。".into()); }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "无法读取云端用户。")? {
        append_retry_identity_body(&mut body, &chunk)?;
    }
    let payload: serde_json::Value = serde_json::from_slice(&body).map_err(|_| "云端用户响应无效。")?;
    let user = payload.get("data").unwrap_or(&payload);
    let user_id = user.get("id").and_then(serde_json::Value::as_i64).filter(|id| *id > 0)
        .ok_or("无法确认云端用户标识。")?;
    // A configured device without a JWT claim is not proof of ownership. Verify its
    // active registration using the very same authenticated token as the user lookup.
    let devices = fixed_identity_get(&base_url, &token, "/api/v1/auth/desktop/devices").await?;
    let items = devices.as_array().or_else(|| devices.get("list").and_then(serde_json::Value::as_array))
        .ok_or("无法确认当前设备登记。")?;
    if items.iter().filter(|item| item.get("device_id").and_then(serde_json::Value::as_str) == Some(device_id.as_str())
        && item.get("status").and_then(serde_json::Value::as_str) == Some("active")).count() != 1 {
        return Err("当前设备不属于该用户或已停用，无法上传。".into());
    }
    let owner = settings::UploadOwner { base_url, space_id, account_id: account_id.to_string(), user_id, device_id };
    validate_owner_state(&settings::load(data)?, &owner, &token)?;
    if desktop_credentials::load(data, &owner.base_url)?.access != token {
        return Err("服务端登录状态已改变，请重新操作。".into());
    }
    Ok((owner, token))
}

fn validate_owner_state(settings: &settings::AppSettings, owner: &settings::UploadOwner, token: &str) -> Result<(), String> {
    if owner.user_id <= 0 || owner.device_id.is_empty() || owner.device_id.trim() != owner.device_id
        || owner.account_id.is_empty() || settings.active_account_id != owner.account_id
        || upload_list_base_url(settings, owner.space_id).ok().as_ref()
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
    let (owner, _) = verified_upload_identity(&data, &settings.active_account_id, space).await?;
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
    validate_upload_scope(settings, &owner.account_id, owner.space_id, &scope.usernames)
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
        || upload_list_base_url(settings, auth.space_id).ok().as_ref()
            .is_none_or(|base| !settings::same_service_url(base, &auth.base_url))
        || cloud_retry_device_id(&settings.devify.device_id, current_token).ok().as_deref() != Some(auth.device_id.as_str())
    {
        return Err(invalid());
    }
    validate_upload_scope(settings, &auth.account_id, auth.space_id, &auth.scope.usernames)
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

fn validate_remote_owner(owner: &settings::UploadOwner, batch_id: i64, batch: &serde_json::Value, key: Option<&str>) -> Result<(), String> {
    if batch_id <= 0 || batch.get("id").and_then(serde_json::Value::as_i64) != Some(batch_id)
        || batch.get("space_id").and_then(serde_json::Value::as_i64) != Some(owner.space_id)
        || batch.get("uploaded_by_id").and_then(serde_json::Value::as_i64) != Some(owner.user_id)
        || batch.get("source_type").and_then(serde_json::Value::as_str) != Some("wechat")
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
        || batch.get("space_id").and_then(serde_json::Value::as_i64) != Some(auth.space_id)
        || batch.get("uploaded_by_id").and_then(serde_json::Value::as_i64) != user_id
        || batch.get("source_type").and_then(serde_json::Value::as_str) != Some("wechat")
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

fn validate_cloud_retry_package(auth: &CloudRetryAuth, space_id: i64, account_id: &str, key: &str) -> Result<(), String> {
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

async fn retry_identity_get(client: &reqwest::Client, auth: &CloudRetryAuth, path: &str) -> Result<serde_json::Value, String> {
    let mut response = client.get(format!("{}{path}", auth.base_url))
        .bearer_auth(&auth.token).send().await
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

async fn verify_cloud_retry_remote(auth: &CloudRetryAuth) -> Result<(i64, String), String> {
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30)).build().map_err(|_| "无法创建云端重试身份请求。")?;
    let user = retry_identity_get(&client, auth, "/api/v1/auth/user").await?;
    let batch = retry_identity_get(&client, auth, &format!("/api/v1/chat/uploads/{}", auth.batch_id)).await?;
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
    if job.owner.is_none() || (job.batch_id.is_none() && job.create_started != Some(false)) {
        return Err("无法确认原任务的账号或服务端批次，请核对后重新创建采集任务。".into());
    }
    if job.batch_id.is_some() && job.package_digest.is_none() {
        return Err("原任务没有保存归档摘要，无法安全重试。".into());
    }
    let (owner, token) = verified_upload_identity(&data, &job.account_id, job.space_id).await?;
    let identity_guard = UPLOAD_IDENTITY_LOCK.read().await;
    validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
    let cloud_retry = if let Some(batch_id) = job.batch_id {
        if batch_id <= 0 { return Err("原上传批次无效，不能重试。".into()); }
        let mut auth = CloudRetryAuth {
            token: token.clone(), base_url: owner.base_url.clone(), job_id: job.id.clone(),
            space_id: job.space_id, account_id: job.account_id.clone(), batch_id,
            device_id: owner.device_id.clone(), user_id: 0, idempotency_key: String::new(), scope: scope.clone(),
        };
        validate_live_cloud_retry(&data, &auth)?;
        let (user_id, key) = verify_cloud_retry_remote(&auth).await?;
        if user_id != owner.user_id { return Err("云端上传者与原任务账号不一致，不能重试。".into()); }
        auth.user_id = user_id;
        auth.idempotency_key = key;
        validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
        validate_live_cloud_retry(&data, &auth)?;
        let confirmed_key = format!("wechat-{}-{}", job.account_id, &fingerprint[..32]);
        validate_cloud_retry_package(&auth, job.space_id, &job.account_id, &confirmed_key)?;
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
    drop(identity_guard);
    let retry_usernames = scope.usernames.clone();
    let retry_since = scope.since;
    let retry_until = scope.until;
    let retry_base_url = scope.base_url.clone();
    let retry_selection = scope.message_selection.clone();
    let result = devify_upload_wechat(
        app.clone(),
        job_id.clone(),
        job.space_id,
        job.account_id.clone(),
        retry_usernames,
        retry_since,
        retry_until,
        Some(retry_base_url),
        Some(fingerprint),
        retry_selection,
        UploadTrigger::Manual,
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
    trigger: UploadTrigger,
) -> serde_json::Value {
    serde_json::json!({
        "source_type": "wechat",
        "trigger": trigger,
        "source_account_id": account_id,
        "idempotency_key": key,
        "device_id": device_id,
        "device_label": device_label,
        "platform": platform,
    })
}

async fn devify_upload_wechat(
    app: AppHandle,
    job_id: String,
    space_id: i64,
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
    let allowed = allowed_usernames(&app, &account_id)?;
    if usernames.iter().any(|name| !allowed.contains(name)) {
        return Err("上传范围包含不在当前白名单中的会话。".into());
    }
    let data = app_data(&app);
    let (owner, token) = verified_upload_identity(&data, &account_id, space_id).await?;
    let _identity_guard = UPLOAD_IDENTITY_LOCK.read().await;
    let current_settings = settings::load(&data)?;
    let scope = current_settings.upload_queue.iter().find(|job| job.id == job_id)
        .and_then(|job| job.scope.clone()).ok_or("找不到原始上传范围。")?;
    if scope.usernames != usernames || scope.since != since || scope.until != until
        || scope.message_selection != message_selection {
        return Err("上传范围已改变，请重新创建采集任务。".into());
    }
    validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
    let cache = decrypt::cache_dir(&data, &account_id);
    let base_url = expected_base_url
        .clone()
        .unwrap_or_else(|| current_settings.devify.base_url.clone());
    if !settings::same_service_url(&base_url, &current_settings.devify.base_url) {
        return Err("服务地址已更改，无法安全匹配旧消息。".into());
    }
    let legacy_keys = load_legacy_upload_keys(&app, space_id, &account_id, &usernames, Some((&owner.base_url, &token))).await?;
    let account_for_prepare = account_id.clone();
    let names_for_prepare = usernames.clone();
    let voice_settings = current_settings.voice.clone();
    let attachments = current_settings.backup.attachments.clone();
    let selection_for_prepare = message_selection.clone();
    let data_for_prepare = data.clone();
    let cache_for_prepare = cache.clone();
    let media_root_for_prepare = account_root(&account_id).unwrap_or_else(|_| cache.clone());
    let base_url_for_prepare = base_url.clone();
    let prepared = tauri::async_runtime::spawn_blocking(move || {
        prepare_upload_with_voice(
            &data_for_prepare,
            &cache_for_prepare,
            &media_root_for_prepare,
            &account_for_prepare,
            space_id,
            &base_url_for_prepare,
            &names_for_prepare,
            since,
            until,
            &selection_for_prepare,
            &voice_settings,
            &attachments,
            &legacy_keys,
        )
    })
    .await
    .map_err(|e| format!("准备上传失败：{e}"))??;
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
    let manifest = serde_json::json!({
        "schema_version": "wechat.v1",
        "platform": "wechat",
        "source_namespace": account_id,
        "counts": { "conversations": conversations.len(), "messages": messages.len() },
        "scope": { "conversation_ids": usernames },
        "whitelist_snapshot": whitelist,
        "client": { "name": "AIMyChats", "version": env!("CARGO_PKG_VERSION") },
    });
    let mut archive = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut archive);
        let options = SimpleFileOptions::default();
        for (name, records) in [
            ("conversations.jsonl", conversations),
            ("messages.jsonl", messages),
            ("message_parts.jsonl", parts),
            ("avatars.jsonl", avatars),
        ] {
            if name == "avatars.jsonl" && records.is_empty() {
                continue;
            }
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
        for (name, bytes) in avatar_files.iter().chain(media_files.iter()) {
            zip.start_file(name.as_str(), options)
                .map_err(|e| e.to_string())?;
            zip.write_all(bytes).map_err(|e| e.to_string())?;
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
    let key = format!("wechat-{}-{}", account_id, &package_key[..32]);
    if let Some(auth) = &cloud_retry {
        if auth.token != token || auth.user_id != owner.user_id || auth.device_id != owner.device_id {
            return Err("重试任务的登录用户或设备已改变。".into());
        }
        validate_cloud_retry_package(auth, space_id, &account_id, &key)?;
        validate_live_cloud_retry(&data, auth)?;
        let (user_id, remote_key) = verify_cloud_retry_remote(auth).await?;
        if user_id != auth.user_id || remote_key != auth.idempotency_key {
            return Err("云端批次或用户已变化，请重新确认重试任务。".into());
        }
        validate_live_cloud_retry(&data, auth)?;
    }
    let digest = hex::encode(sha2::Sha256::digest(&body));
    // Record the exact ZIP and the possibility of a sent request *before* POST. If the
    // response is lost, the unknown batch cannot be blindly created a second time.
    settings::update(&data, |current| {
        let job = current.upload_queue.iter().find(|item| item.id == job_id).ok_or("找不到上传任务。")?;
        validate_owned_job(current, job, &owner, &token, &scope)?;
        if desktop_credentials::load(&data, &owner.base_url)?.access != token { return Err("登录状态已改变。".into()); }
        if cloud_retry.is_some() {
            if job.package_digest.as_deref() != Some(digest.as_str()) { return Err("无法确认原始归档完全一致，请重新创建采集任务。".into()); }
        } else if job.batch_id.is_some() || job.create_started != Some(false) {
            return Err("无法确认服务端是否已有批次，请核对后重新创建采集任务。".into());
        }
        let job = current.upload_queue.iter_mut().find(|item| item.id == job_id).ok_or("找不到上传任务。")?;
        job.create_started = Some(true);
        job.package_digest = Some(digest.clone());
        Ok(())
    })?;
    // Redirects stay disabled: a 3xx on the create/upload call must not silently
    // resend a whole package elsewhere.
    let client = devify_upload_client()?;
    let upload_platform = if settings.devify.platform.is_empty() {
        settings::default_platform()
    } else {
        settings.devify.platform.clone()
    };
    let upload_device_id = owner.device_id.clone();
    let upload_base_url = owner.base_url.as_str();
    let request = client
        .post(format!("{upload_base_url}/api/v1/spaces/{space_id}/chat/uploads"))
        .bearer_auth(&token)
        .json(&upload_create_body(
            &account_id,
            &key,
            &upload_device_id,
            &settings::resolved_device_label(&settings.devify),
            &upload_platform,
            trigger,
        ));
    validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
    if let Some(auth) = &cloud_retry {
        validate_live_cloud_retry(&data, auth)?;
    }
    let created = request.send().await.map_err(|error| {
        if cloud_retry.is_some() { "创建上传批次失败，请检查网络连接。".to_string() }
        else { format!("创建上传批次失败：{error}") }
    })?;
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
    if cloud_retry.as_ref().is_some_and(|auth| auth.batch_id != batch_id) {
        return Err("服务端返回了不同的上传批次，已取消上传。".into());
    }
    // Save the server ID and package coverage before any subsequent network request.
    // A failed save leaves create_started=true and prevents an ambiguous replay.
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
    let remote = fixed_identity_get(upload_base_url, &token, &format!("/api/v1/chat/uploads/{batch_id}")).await?;
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
    validate_live_owned_job(&data, &job_id, &owner, &token, &scope)?;
    if let Some(auth) = &cloud_retry {
        validate_live_cloud_retry(&data, auth)?;
    }
    let started = std::time::Instant::now();
    let uploaded = client
        .put(format!("{upload_base_url}/api/v1/chat/uploads/{batch_id}/archive"))
        .bearer_auth(&token)
        .multipart(form)
        .send()
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
        .send()
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
    let auth_config = devify_auth_config(&settings.devify).await?;
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
        pending.replace(PendingDevifyAuth { state: oauth_state.clone(), verifier, base_url: base_url.clone(), generation: auth_generation });
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
    let auth_config = devify_auth_config(&settings.devify).await?;
    let platform = if settings.devify.platform.is_empty() {
        settings::default_platform()
    } else {
        settings.devify.platform.clone()
    };
    let device_label = settings::resolved_device_label(&settings.devify);
    let response = auth_http_client()?
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
        .send()
        .await
        .map_err(|e| devify_network_message("换取服务端登录令牌失败", &e))?;
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
    let app_data = app_data(&app);
    let credentials = desktop_credentials::Credentials {
        base_url: pending.base_url.clone(), access: access.to_string(), refresh: refresh.to_string(),
    };
    settings::with_identity(&app_data, |current| {
        if current.devify.resolved_base_url()? != pending.base_url
            || !desktop_credentials::save_if_generation_locked(&app_data, pending.generation, &credentials)? {
            return Err("服务端登录状态已改变，请重新登录。".into());
        }
        Ok(())
    })?;
    let device_id = data.get("device_id").and_then(|v| v.as_str())
        .map(str::to_string).or_else(|| access_token_claim(access, "device_id")).unwrap_or_default();
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
    entitlements::refresh(&data, &settings.devify.base_url, &token).await
}

#[tauri::command]
async fn devify_auth_status(app: AppHandle) -> Result<DevifyAuthStatus, String> {
    let data = app_data(&app);
    let settings = settings::load(&data)?;
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
    match devify_client()
        .get(format!(
            "{}/api/v1/spaces",
            settings.devify.resolved_base_url()?
        ))
        .bearer_auth(&token)
        .send()
        .await
    {
        Ok(response) if response.status() == reqwest::StatusCode::UNAUTHORIZED => {
            if request_generation(&data, &settings.devify.base_url, &token)? != generation {
                return Err("登录状态已改变，请重新检查。".into());
            }
            let mut cleared = None;
            match devify_access_token_resolve(&data, &settings.devify.base_url, &mut cleared, true).await {
                Ok(refreshed) => {
                    let refreshed_generation = request_generation(&data, &settings.devify.base_url, &refreshed)?;
                    let response = devify_client().get(format!("{}/api/v1/spaces", settings.devify.resolved_base_url()?))
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
    devify_device_register_for(app, None).await
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
    let identity = UPLOAD_IDENTITY_LOCK.read().await;
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
    let response = auth_http_client()?
        .post(format!("{base}/api/v1/auth/desktop/devices"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "device_id": device_id.clone(),
            "label": device_label.clone(),
            "platform": platform.clone(),
        }))
        .send()
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
    let devices = fixed_identity_get(&base, &token, "/api/v1/auth/desktop/devices").await?;
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
    drop(identity);
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
    let auth_generation = desktop_credentials::begin_auth_attempt()?;
    let settings = settings::load(&data)?;
    let base_url = settings.devify.resolved_base_url()?;
    let response = auth_http_client()?
        .post(format!("{base_url}/api/v1/auth/login"))
        .json(&serde_json::json!({ "username": username, "password": password }))
        .send()
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
        base_url: base_url.clone(), access: access.to_string(), refresh: refresh.to_string(),
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
    let device_generation = device.as_ref().map(|value| value.generation).unwrap_or(committed_generation);
    let mut completed_generation = None;
    let _ = ensure_active_space_for(&app, Some((&credentials, device_generation)), &mut completed_generation).await;
    if !desktop_credentials::load_with_generation(&data, &base_url).ok().as_ref()
        .is_some_and(|(stored, generation)| stored == &credentials && *generation == completed_generation.unwrap_or(device_generation)) {
        return Err("服务端登录状态已改变，请重新登录。".into());
    }
    Ok(DevifyAuthResult {
        device_id: device.map(|value| value.device_id).unwrap_or_default(),
        expires_in: 3600,
    })
}

#[tauri::command]
fn devify_logout(app: AppHandle) -> Result<(), String> {
    desktop_credentials::clear(&app_data(&app))
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
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    entitlements::require(&data, &settings.devify.base_url, capabilities::BRIDGE_AGENT_CLI)?;
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
async fn agent_build_wxq(app: AppHandle) -> Result<String, String> {
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    entitlements::require(&data, &settings.devify.base_url, capabilities::BRIDGE_AGENT_CLI)?;
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
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        name: "AIMyChats".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
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

/// Shown whenever a read is blocked because the user disconnected WeChat.
const WECHAT_DISCONNECTED: &str = "微信连接已断开，重新连接后才能读取聊天。已上传的记录不受影响。";

fn wechat_connected(app: &AppHandle) -> Result<bool, String> {
    Ok(settings::load(&app_data(app))?.connectors.wechat_enabled)
}

/// Guard for every path that reads messages, including uploads and MCP. Disconnecting keeps
/// the local keys and the decrypted snapshot, so reconnecting does not need a new key import.
fn ensure_wechat_connected(app: &AppHandle) -> Result<(), String> {
    if wechat_connected(app)? {
        Ok(())
    } else {
        Err(WECHAT_DISCONNECTED.into())
    }
}

fn allowed_usernames(app: &AppHandle, account_id: &str) -> Result<HashSet<String>, String> {
    validate_account_id(account_id)?;
    let settings = settings::load(&app_data(app))?;
    if !settings.connectors.wechat_enabled {
        return Err(WECHAT_DISCONNECTED.into());
    }
    Ok(settings
        .chat
        .allowed_usernames
        .get(account_id)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect())
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
    space_id: i64,
    account_id: String,
    device_id: String,
    enabled: bool,
    time_local: String,
    topic_id: Option<i64>,
    topic_space_id: Option<i64>,
}

impl DailyIdentity {
    fn validate_settings(&self, current: &settings::AppSettings) -> Result<(), String> {
        let (credentials, generation) = desktop_credentials::load_with_generation(&self.data, &self.base)?;
        if credentials != self.credentials || generation != self.generation
            || current.devify.resolved_base_url()? != self.base
            || current.devify.active_space_id != Some(self.space_id)
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
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30)).build().map_err(|_| "无法创建日报请求。")?;
    let mut request = client.request(method, format!("{}{path}", context.base)).bearer_auth(&context.credentials.access);
    if let Some(body) = body { request = request.json(&body); }
    let response = request.send().await;
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

fn daily_stale_topic_warning(value: &settings::AppSettings, space_id: i64) -> Option<&'static str> {
    (value.automation.daily_topic_id.is_some() && value.automation.daily_topic_space_id != Some(space_id))
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
        value.devify.active_space_id.unwrap_or(0),
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
    let _identity = UPLOAD_IDENTITY_LOCK.read().await;
    let value = settings::load(&data)?;
    let (credentials, generation) = desktop_credentials::load_with_generation(&data, &base)?;
    if value.devify.resolved_base_url()? != base || credentials.access != token {
        return Err("日报计划的登录状态已改变，请重试。".into());
    }
    let space_id = value.devify.active_space_id
        .ok_or("请先在「设置 / 账户」中登录并选择 空间。")?;
    validate_devify_space(&value, space_id)?;
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
        data: data.clone(), base, credentials, generation, space_id,
        account_id: value.active_account_id.clone(), device_id: value.devify.device_id.clone(),
        enabled, time_local: time_local.clone(), topic_id: value.automation.daily_topic_id,
        topic_space_id: value.automation.daily_topic_space_id,
    };
    context.validate()?;
    if let Some(warning) = daily_stale_topic_warning(&value, space_id) {
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
        if daily_stale_topic_warning(&value, value.devify.active_space_id.unwrap_or(0)).is_none()
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
    let mut names: Vec<_> = value.sync.continuation.get(account_id).into_iter().flat_map(|items| items.iter()).filter(|(name, scope)| (scope.manual == Some(true) || value.sync.enabled) && Some(scope.space_id) == value.devify.active_space_id && settings::same_service_url(&scope.base_url, &value.devify.base_url) && allowed.is_some_and(|list| list.contains(name))).map(|(name, _)| name.clone()).collect();
    names.sort();
    names
}

fn initial_progress_matches(chunk: &settings::InitialChunk, space_id: i64, base_url: &str) -> bool {
    chunk.space_id == space_id && settings::same_service_url(&chunk.base_url, base_url)
}

async fn backup_tick(app: &AppHandle) {
    let data = app_data(app);
    // Finished server batches must be recorded even when the automatic plan is off,
    // otherwise a manual upload keeps its "not uploaded yet" badge forever.
    reconcile_upload_jobs(app).await;
    let Ok(value) = settings::load(&data) else { return; };
    // A disconnected WeChat still gets its finished batches recorded, but nothing new is
    // read from the local database or uploaded.
    if !value.connectors.wechat_enabled {
        return;
    }
    let has_manual_candidates = value.upload_queue.iter().any(|job| {
        job.manual_group.is_some() && job.owner.is_some()
            && job.account_id == value.active_account_id
            && value.devify.active_space_id == Some(job.space_id)
            && matches!(job.status.as_str(), "queued" | "running")
            && job.batch_id.is_none() && job.create_started == Some(false)
    });
    if has_manual_candidates {
        if let Some(space_id) = value.devify.active_space_id {
            if entitlements::require(&data, &value.devify.base_url, capabilities::INGEST_UPLOAD).is_ok()
                && network::allow_upload(value.backup.wifi_only) {
                if let Ok((owner, token)) = verified_upload_identity(&data, &value.active_account_id, space_id).await {
                    if let Some(job_id) = next_manual_batch(&value, &owner) {
                        let upload = app.state::<UploadState>();
                        if let Ok(_guard) = begin_account_upload(&upload, &value.active_account_id) {
                            if let Err(error) = submit_manual_batch(app, &job_id).await {
                                if let Err(save_error) = mark_manual_batch_failed(app, &job_id, &owner, &token, &error) {
                                    eprintln!("无法保存上传任务状态：{save_error}");
                                }
                            }
                        }
                        return;
                    }
                }
            }
        }
    }
    if !continuation_sessions(&value, &value.active_account_id).is_empty() {
        if entitlements::require(&data, &value.devify.base_url, capabilities::INGEST_UPLOAD).is_ok() && network::allow_upload(value.backup.wifi_only) {
            let _ = perform_chat_sync(app, true, true).await;
        }
        return;
    }
    if !value.sync.enabled {
        return;
    }
    // Automatic upload costs server storage and parsing, so it follows the plan. A lapsed
    // entitlement pauses the schedule; it never deletes anything already uploaded.
    if entitlements::require(&data, &value.devify.base_url, capabilities::INGEST_AUTO).is_err() {
        return;
    }
    let last_run = value
        .sync
        .last_run_at
        .as_deref()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .map(|at| at.with_timezone(&Local));
    if !backup::due(
        &value.backup,
        value.chat.sync_interval_seconds,
        last_run,
        Local::now(),
    ) {
        return;
    }
    // "仅在 Wi-Fi 下上传" holds the run back instead of skipping it: the next tick retries
    // once the machine is back on Wi-Fi or a wired link.
    if !network::allow_upload(value.backup.wifi_only) {
        return;
    }
    sync_automation_tick(app).await;
    chat_sync_tick(app).await;
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
    let env = connector_environment(app);
    let _ = tauri::async_runtime::spawn_blocking(move || {
        read_new_wechat_data(&data_for_sync, &env, &account, &store, true, true)
    })
    .await;
    end_sync(&sync_state, &account_id);
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
                    entries.insert(username.clone(), settings::ContinuationScope { space_id: job.space_id, base_url: scope.base_url.clone(), manual: job.continuation_manual });
                }
            }
            job.continue_sync = false;
        }
        for (username, chunk) in std::mem::take(&mut job.initial_chunks) {
            if chunk.final_chunk {
                let cursors = value.sync.cursors.entry(job.account_id.clone()).or_default();
                let replace = cursors.get(&username).map_or(true, |old| (chunk.baseline.time, chunk.baseline.local_id) > (old.time, old.local_id));
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
        || owner.user_id <= 0 || owner.space_id <= 0 || owner.account_id.is_empty()
        || owner.device_id.is_empty() || owner.device_id.trim() != owner.device_id
        || !settings::same_service_url(&scope.base_url, &owner.base_url) {
        return None;
    }
    let bytes = serde_json::to_vec(&("verified-completion-v1", &job.id, job.batch_id, owner,
        &job.account_id, job.space_id, scope, &job.covered, &job.status, &job.created_at, &job.updated_at)).ok()?;
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
        || (job.status == "completed" && (upload_job_has_pending_sync(job) || !upload_completion_is_verified(job)))
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

async fn reconcile_upload_jobs(app: &AppHandle) {
    // Concurrent scheduler/manual ticks must not duplicate a round or race its cursor.
    static ROUND: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    static LAST_JOB_ID: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let Ok(_round) = ROUND.try_lock() else { return; };
    let deadline = tokio::time::Instant::now() + RECONCILE_TOTAL_BUDGET;
    let data = app_data(app);
    let Ok(value) = settings::load(&data) else { return; };
    let Some(space_id) = value.devify.active_space_id else { return; };
    if !value.upload_queue.iter().any(|job| job.batch_id.is_some() && job.owner.is_some()
        && upload_job_needs_reconciliation(job)) {
        return;
    }
    // Refresh can require the identity write lock, so verify first, then take the read lease
    // and recheck the snapshot before making any batch requests or committing results.
    let Ok(Ok((owner, token))) = tokio::time::timeout_at(deadline, verified_upload_identity(&data, &value.active_account_id, space_id)).await else { return; };
    let Ok(_identity_guard) = tokio::time::timeout_at(deadline, UPLOAD_IDENTITY_LOCK.read()).await else { return; };
    let Ok(current) = settings::load(&data) else { return; };
    if validate_owner_state(&current, &owner, &token).is_err()
        || desktop_credentials::load(&data, &owner.base_url).ok().as_ref().map(|c| c.access.as_str()) != Some(token.as_str()) {
        return;
    }
    let pending: Vec<_> = current.upload_queue.iter().filter(|job| {
        job.owner.as_ref() == Some(&owner) && job.batch_id.is_some()
            && upload_job_needs_reconciliation(job)
    }).cloned().collect();
    reconcile_owned_round(&data, &owner, &token, &pending, &LAST_JOB_ID, deadline).await;
}

// A single caller owns the round gate; the cursor is updated before each cancellable GET.
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

// The caller holds UPLOAD_IDENTITY_LOCK's read lease and supplies a freshly verified owner.
// Fetch from the fixed service/token; never reuse a response merely because batch ids match.
async fn reconcile_owned_job(data: &Path, owner: &settings::UploadOwner, token: &str, snapshot: &settings::UploadJob) -> Result<(), String> {
    let batch_id = snapshot.batch_id.filter(|id| *id > 0).ok_or("缺少云端批次。")?;
    if snapshot.id.is_empty() || snapshot.owner.as_ref() != Some(owner)
        || snapshot.account_id != owner.account_id || snapshot.space_id != owner.space_id
        || !upload_job_needs_reconciliation(snapshot)
        || !snapshot.scope.as_ref().is_some_and(|scope| settings::same_service_url(&scope.base_url, &owner.base_url)) {
        return Err("任务归属不完整或已改变。".into());
    }
    let batch = fixed_identity_get(&owner.base_url, token, &format!("/api/v1/chat/uploads/{batch_id}")).await?;
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
        for field in ["create_started", "package_digest"] {
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
        space_id: owner.space_id,
        account_id: owner.account_id.clone(),
        batch_id: None,
        owner: Some(owner.clone()),
        create_started: Some(false),
        package_digest: None,
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

/// True when the stored run status is missing or too old to answer "when did it last run".
fn sync_status_is_stale(sync: &settings::SyncSettings) -> bool {
    if sync.last_status.as_deref() != Some("success") {
        return true;
    }
    let Some(at) = sync
        .last_run_at
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
    else {
        return true;
    };
    (chrono::Utc::now() - at.with_timezone(&chrono::Utc)).num_seconds() >= 60
}

/// Sessions uploaded per tick. Probing a conversation with nothing new is a single
/// indexed query, so only actual uploads are capped.
const SYNC_UPLOADS_PER_TICK: usize = 50;
const SYNC_MESSAGES_PER_BATCH: usize = 1000;

fn validate_sync_owner(value: &settings::AppSettings, owner: &settings::UploadOwner) -> Result<(), String> {
    let account = &owner.account_id;
    let has_legacy_state = value.sync.cursors.get(account).is_some_and(|v| !v.is_empty())
        || value.sync.initial_progress.get(account).is_some_and(|v| !v.is_empty())
        || value.sync.continuation.get(account).is_some_and(|v| !v.is_empty())
        || value.sync.pending_voice.get(account).is_some_and(|v| !v.is_empty());
    match value.sync.owners.get(account) {
        Some(saved) if saved == owner => Ok(()),
        None if !has_legacy_state => Ok(()),
        _ => Err("同步进度属于其他或未知云端账号、空间或设备；已暂停，请重新确认并创建采集。".into()),
    }
}

/// Decrypt the local cache, then upload every allowlisted conversation that has messages
/// newer than its cursor. The first run of a conversation uploads its full local history.
async fn perform_chat_sync(app: &AppHandle, manual: bool, continuation_only: bool) -> Result<ChatSyncSummary, String> {
    let started = std::time::Instant::now();
    let result = perform_chat_sync_inner(app, manual, continuation_only).await;
    let _ = diagnostics::record(&app_data(app), diagnostics::EventInput {
        module: diagnostics::Module::Upload,
        operation: if result.is_ok() { diagnostics::Operation::Success } else { diagnostics::Operation::Failure },
        code: if result.is_err() { Some(diagnostics::Code::CommandFailed) } else { None },
        batch_id: None,
        duration_ms: Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64),
    });
    result
}

async fn perform_chat_sync_inner(app: &AppHandle, manual: bool, continuation_only: bool) -> Result<ChatSyncSummary, String> {
    let data = app_data(app);
    let mut value = settings::load(&data)?;
    ensure_wechat_connected(app)?;
    if !manual && !value.sync.enabled {
        return Ok(ChatSyncSummary {
            status: "idle".into(),
            uploaded: 0,
            skipped: 0,
            message_count: 0,
            batch_ids: Vec::new(),
            error: None,
        });
    }
    let account_id = value.active_account_id.clone();
    if account_id.is_empty() {
        return Err("尚未选择微信账号。".into());
    }
    let upload_state = app.state::<UploadState>();
    let _upload_guard = begin_account_upload(&upload_state, &account_id)?;
    let Some(space_id) = value.devify.active_space_id else {
        return Err("请先在服务端配置中选择要同步到的空间。".into());
    };
    let mut whitelist = value
        .chat
        .allowed_usernames
        .get(&account_id)
        .cloned()
        .unwrap_or_default();
    if whitelist.is_empty() {
        return Err("聊天白名单为空，没有可同步的会话。".into());
    }
    whitelist.sort();
    whitelist.dedup();
    if continuation_only {
        whitelist.retain(|name| continuation_sessions(&value, &account_id).contains(name));
        if whitelist.is_empty() { return Err("没有可继续的上传任务。".into()); }
    }
    let base_url = value.devify.base_url.clone();
    let (sync_owner, sync_token) = verified_upload_identity(&data, &account_id, space_id).await?;
    {
        let _identity_guard = UPLOAD_IDENTITY_LOCK.read().await;
        let deadline = tokio::time::Instant::now() + LEGACY_ADOPT_BUDGET;
        let _ = adopt_legacy_upload_history(&data, &sync_owner, &sync_token, deadline).await;
    }
    value = settings::load(&data)?;
    validate_sync_owner(&value, &sync_owner)?;
    reconcile_upload_jobs(app).await;
    settings::update(&data, |current| {
        validate_owner_state(current, &sync_owner, &sync_token)?;
        if desktop_credentials::load(&data, &sync_owner.base_url)?.access != sync_token {
            return Err("服务端登录状态已改变。".into());
        }
        validate_sync_owner(current, &sync_owner)?;
        // Completed legacy sync jobs may still contain unapplied progress. Never bind
        // an unknown or different historical owner to the newly verified identity.
        if current.upload_queue.iter().any(|job| job.account_id == account_id
            && job.id.starts_with("sync-") && job.owner.as_ref() != Some(&sync_owner)
            && !legacy_job_is_inert(job)) {
            return Err("已有旧同步任务的云端账号或设备未知或不匹配；已暂停，请先核对。".into());
        }
        current.sync.owners.entry(account_id.clone()).or_insert_with(|| sync_owner.clone());
        Ok(())
    })?;
    value = settings::load(&data)?;

    let scan = discover::scan();
    let Some(account) = scan
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .cloned()
    else {
        return Err("未找到本机微信数据目录，请先在聊天记录页完成定位。".into());
    };
    let sync_state = app.state::<SyncState>();
    if !begin_sync(&sync_state, &account_id) {
        return Err("该账号正在同步，请稍后再试。".into());
    }
    let store = keys::load(&keys::store_path(&data));
    let data_for_decrypt = data.clone();
    let env = connector_environment(app);
    let read = tauri::async_runtime::spawn_blocking(move || {
        read_new_wechat_data(&data_for_decrypt, &env, &account, &store, true, true)
    })
    .await;
    end_sync(&sync_state, &account_id);
    let outcomes = read.map_err(|e| format!("读取本机聊天记录失败：{e}"))??;
    if let Some(failure) = outcomes
        .iter()
        .find(|outcome| outcome.status == "failed" || outcome.status == "no-key")
    {
        let reason = failure
            .error
            .clone()
            .unwrap_or_else(|| format!("{} 未能解密", failure.rel));
        // Naming the database matters: a key that is missing for one new shard otherwise
        // looks like "keys are configured but nothing works".
        return Err(format!("读取本机聊天记录失败（{}）：{reason}", failure.rel));
    }

    let cache = decrypt::cache_dir(&data, &account_id);
    let mut uploaded = 0usize;
    let mut skipped = 0usize;
    let mut message_count = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut batch_ids = Vec::new();
    let mut touched = false;

    // A failed or interrupted create may already exist remotely. Legacy jobs without an
    // owner, scope, or create marker are equally ambiguous and must not be replayed.
    let blocks_sync = |current: &settings::AppSettings, username: &str| {
        current.upload_queue.iter().any(|job| {
            job.account_id == account_id && job.status != "completed" && !job_never_blocks_sync(job)
                && job.scope.as_ref().map_or(true, |scope| scope.usernames.is_empty() || scope.usernames.iter().any(|name| name == username))
                && (matches!(job.status.as_str(), "queued" | "running" | "processing" | "uploading")
                    || job.owner.as_ref() != Some(&sync_owner)
                    || job.create_started != Some(false)
                    || job.batch_id.is_some())
        })
    };

    for username in &whitelist {
        if !manual && uploaded >= SYNC_UPLOADS_PER_TICK {
            break;
        }
        if blocks_sync(&value, username) {
            skipped += 1;
            failures.push(format!("{username}：已有未完成或身份不明的上传任务，请在任务中核对状态"));
            continue;
        }
        let cursor = value.sync.cursors.get(&account_id).and_then(|items| items.get(username)).copied();
        let progress = value.sync.initial_progress.get(&account_id).and_then(|items| items.get(username)).cloned();
        if progress.as_ref().is_some_and(|p| !initial_progress_matches(p, space_id, &base_url)) {
            failures.push(format!("{username}：首次历史上传属于其他空间，请切回原空间继续"));
            continue;
        }
        let initial_range = if cursor.is_none() { value.chat.initial_ranges.get(&account_id).and_then(|ranges| ranges.get(username)) } else { None };
        let voice_retry_due = value.voice.upload_transcripts
            && voice::view(&data).selected.is_some()
            && value
                .sync
                .pending_voice
                .get(&account_id)
                .and_then(|items| items.get(username))
                .is_some_and(|items| !items.is_empty());
        // Cheap probe first: packaging a conversation that has nothing new would only
        // re-create the same idempotent batch every tick. A pending voice transcript is
        // also a reason to retry, even when no newer chat message exists.
        let needs_upload = if voice_retry_due {
            true
        } else {
            match query::latest_message(&cache, username) {
            Ok(None) => false,
            Ok(Some(newest)) => {
                let count = query::count_at_time(&cache, username, newest.0).ok();
                !cursor.is_some_and(|current| count.is_some_and(|count| current.covers_boundary(newest, count)))
            },
            // A whitelisted session without a local table is skipped, not retried forever.
            Err(error) if error.contains("没有找到会话") => {
                skipped += 1;
                continue;
            }
            // The probe is an optimization only; let the uploader report the real error.
            Err(_) => true,
            }
        };
        if !needs_upload {
            // If the process died after a completed final batch was saved, its old
            // continuation marker may remain. Clear only after a fresh, unchanged cursor
            // proves there is no work and no ambiguous job for this conversation.
            if continuation_only && cursor.is_some() {
                let expected = serde_json::to_value((
                    value.sync.cursors.get(&account_id).and_then(|items| items.get(username)),
                    value.sync.continuation.get(&account_id).and_then(|items| items.get(username)),
                )).map_err(|e| format!("无法核对后续同步：{e}"))?;
                value = settings::update(&data, |current| {
                    validate_owner_state(current, &sync_owner, &sync_token)?;
                    validate_sync_owner(current, &sync_owner)?;
                    if desktop_credentials::load(&data, &sync_owner.base_url)?.access != sync_token {
                        return Err("服务端登录状态已改变。".into());
                    }
                    let live = serde_json::to_value((
                        current.sync.cursors.get(&account_id).and_then(|items| items.get(username)),
                        current.sync.continuation.get(&account_id).and_then(|items| items.get(username)),
                    )).map_err(|e| format!("无法核对后续同步：{e}"))?;
                    if live == expected && !blocks_sync(current, username) {
                        if let Some(items) = current.sync.continuation.get_mut(&account_id) {
                            items.remove(username);
                            if items.is_empty() { current.sync.continuation.remove(&account_id); }
                        }
                    }
                    Ok(())
                })?;
                touched = true;
            }
            skipped += 1;
            continue;
        }
        let pending_since = voice_retry_due.then(|| value
            .sync
            .pending_voice
            .get(&account_id)
            .and_then(|items| items.get(username))
            .and_then(|items| items.iter().map(|item| item.time).min())).flatten();
        let since = match (cursor, pending_since) {
            (Some(current), Some(pending)) => Some(current.time.min(pending)),
            (Some(current), None) => Some(current.time),
            (None, _) => progress.as_ref().and_then(|p| p.since).or_else(|| initial_range.and_then(|range| range.since)),
        };
        let initial_baseline = if cursor.is_none() {
            if let Some(p) = &progress { Some(p.baseline) } else {
            match query::latest_position(&cache, username) {
                Ok(Some((time, local_id))) => match query::count_at_time(&cache, username, time) {
                    Ok(count) => Some(settings::SyncCursor { time, local_id, boundary_count: Some(count) }),
                    Err(error) => { failures.push(format!("{username}：{error}")); continue; }
                },
                Ok(None) => { skipped += 1; continue; },
                Err(error) => { failures.push(format!("{username}：{error}")); continue; }
            }}
        } else { None };
        let until = if let Some(p) = &progress { p.until } else if let Some(baseline) = initial_baseline {
            let snapshot_end = baseline.time.saturating_add(1);
            Some(initial_range.and_then(|range| range.until).map_or(snapshot_end, |end| end.min(snapshot_end)))
        } else { None };
        let mut selection = BTreeMap::new();
        let messages = match collect_upload_messages(&cache, username, since, until, None) {
            Ok(messages) => messages,
            Err(error) => { failures.push(format!("{username}：{error}")); break; }
        };
        let refs: Vec<_> = messages.iter().map(|m| settings::MessageRef { local_id: m.local_id, time: m.time, source_shard: m.source_shard.clone() }).collect();
        let pending = if cursor.is_some() && voice_retry_due { value.sync.pending_voice.get(&account_id).and_then(|items| items.get(username)).cloned().unwrap_or_default() } else { Vec::new() };
        let mut pending_refs: Vec<_> = refs.iter().filter(|m| pending.iter().any(|v| v.local_id == m.local_id && v.time == m.time && (v.source_shard.is_empty() || v.source_shard == m.source_shard))).take(SYNC_MESSAGES_PER_BATCH / 2).cloned().collect();
        let position = progress.as_ref().map(|p| p.position).or(cursor);
        let mut plan = plan_sync_batch(&refs, position, SYNC_MESSAGES_PER_BATCH - pending_refs.len());
        if plan.is_err() && !pending_refs.is_empty() {
            // A single timestamp may need the whole batch; retry voice attachments later.
            pending_refs.clear();
            plan = plan_sync_batch(&refs, position, SYNC_MESSAGES_PER_BATCH);
        }
        let planned = match plan {
            Ok(planned) => planned,
            Err(error) => { failures.push(format!("{username}：{error}")); continue; }
        };
        let has_new = !planned.messages.is_empty();
        let mut selected = planned.messages;
        let mut seen: HashSet<_> = selected.iter().cloned().collect();
        selected.extend(pending_refs.drain(..).filter(|item| seen.insert(item.clone())));
        if selected.is_empty() {
            skipped += 1;
            if cursor.is_none() { failures.push(format!("{username}：所选历史日期范围内没有可上传的消息，请在导入管理调整日期")); }
            continue;
        }
        let last = selected.iter().max_by_key(|item| (item.time, item.local_id)).expect("selection is not empty");
        let selected_position = settings::SyncCursor {
            time: last.time,
            local_id: last.local_id,
            boundary_count: Some(selected.iter().filter(|item| item.time == last.time).count()),
        };
        selection.insert(username.clone(), selected);
        let original_manual = value.sync.continuation.get(&account_id)
            .and_then(|entries| entries.get(username))
            .and_then(|scope| scope.manual);
        let trigger = sync_upload_trigger(manual, continuation_only, original_manual);
        let scope = settings::UploadScope {
            usernames: vec![username.clone()], since, until,
            base_url: sync_owner.base_url.clone(), message_selection: selection.clone(),
        };
        let mut job = sync_job(&sync_owner, scope.clone())?;
        // These fields must survive a crash after the uploader saves a batch ID but before
        // this function receives its result. The selected position matches the exact scope.
        if let Some(baseline) = initial_baseline {
            job.initial_chunks.insert(username.clone(), settings::InitialChunk {
                since, until, baseline, position: selected_position, final_chunk: !planned.has_more,
                space_id, base_url: sync_owner.base_url.clone(),
            });
        }
        job.continue_sync = planned.has_more;
        job.continuation_manual = if continuation_only { original_manual } else { Some(manual) };
        job.sync_advance = cursor.is_some() && has_new;
        let job_id = job.id.clone();
        let snapshot = serde_json::to_value((
            value.sync.cursors.get(&account_id), value.sync.initial_progress.get(&account_id),
            value.sync.continuation.get(&account_id), value.sync.pending_voice.get(&account_id),
            value.chat.initial_ranges.get(&account_id),
        )).map_err(|e| format!("无法核对同步进度：{e}"))?;
        let inserted = settings::update(&data, |current| {
            validate_owner_state(current, &sync_owner, &sync_token)?;
            if desktop_credentials::load(&data, &sync_owner.base_url)?.access != sync_token {
                return Err("服务端登录状态已改变。".into());
            }
            validate_sync_owner(current, &sync_owner)?;
            let current_snapshot = serde_json::to_value((
                current.sync.cursors.get(&account_id), current.sync.initial_progress.get(&account_id),
                current.sync.continuation.get(&account_id), current.sync.pending_voice.get(&account_id),
                current.chat.initial_ranges.get(&account_id),
            )).map_err(|e| format!("无法核对同步进度：{e}"))?;
            if current_snapshot != snapshot { return Err("同步进度已改变，请重新同步。".into()); }
            if blocks_sync(current, username) { return Err("已有未完成或身份不明的上传任务，请先核对。".into()); }
            if current.upload_queue.iter().any(|item| item.id == job_id) { return Err("同步任务标识冲突，请重试。".into()); }
            validate_owned_job(current, &job, &sync_owner, &sync_token, &scope)?;
            current.upload_queue.insert(0, job.clone());
            Ok(())
        });
        value = match inserted {
            Ok(current) => current,
            Err(error) => { failures.push(format!("{username}：{error}")); break; }
        };
        let result = devify_upload_wechat(
            app.clone(), job_id.clone(), space_id, account_id.clone(), vec![username.clone()],
            since, until, Some(base_url.clone()), None, selection, trigger, None,
        ).await;
        let continuation_snapshot = serde_json::to_value(
            value.sync.continuation.get(&account_id).and_then(|items| items.get(username))
        ).map_err(|e| format!("无法核对后续同步：{e}"))?;
        let updated = settings::update(&data, |current| {
            let mut consume_continuation = false;
            let mut job_changed = false;
            let job = current.upload_queue.iter_mut().find(|item| item.id == job_id)
                .ok_or("找不到原始同步任务，无法保存结果。")?;
            if job.owner.as_ref() != Some(&sync_owner) || job.scope.as_ref() != Some(&scope) {
                return Err("原始同步任务身份或范围已改变，无法保存结果。".into());
            }
            match &result {
                Ok(result) => {
                    if job.batch_id != Some(result.batch_id) || job.create_started != Some(true) {
                        return Err("同步批次未安全保存，请核对任务。".into());
                    }
                    if matches!(job.status.as_str(), "running" | "queued" | "uploading" | "processing") {
                        job.status = "processing".into();
                        job.message = format!("自动同步：已提交 {} 条消息", result.message_count);
                        job_changed = true;
                        consume_continuation = true;
                    }
                }
                Err(error) => {
                    // A known ID stays queryable; an unknown POST result stays failed and
                    // blocks any new create. Never replace the durable fields with a clone.
                    if matches!(job.status.as_str(), "running" | "queued" | "uploading" | "processing") {
                        job.status = if job.batch_id.is_some() { "processing" } else { "failed" }.into();
                        job.message = error.clone();
                        job_changed = true;
                    }
                }
            }
            if job_changed { job.updated_at = chrono::Utc::now().to_rfc3339(); }
            if consume_continuation && continuation_only && current.sync.owners.get(&account_id) == Some(&sync_owner) {
                let live = serde_json::to_value(current.sync.continuation.get(&account_id)
                    .and_then(|items| items.get(username))).map_err(|e| format!("无法核对后续同步：{e}"))?;
                if live == continuation_snapshot {
                    if let Some(items) = current.sync.continuation.get_mut(&account_id) {
                        items.remove(username);
                        if items.is_empty() { current.sync.continuation.remove(&account_id); }
                    }
                }
            }
            Ok(())
        }).map_err(|error| format!("保存同步任务结果失败：{error}；请在任务中核对，勿重复上传。"))?;
        let saved = updated.upload_queue.iter().find(|item| item.id == job_id)
            .ok_or("找不到已保存的同步任务。")?;
        if let Some(id) = saved.batch_id { batch_ids.push(id); }
        let result = if result.is_ok() && matches!(saved.status.as_str(), "failed" | "cancelled") {
            Err(format!("服务端任务已{}：{}", if saved.status == "failed" { "失败" } else { "取消" }, saved.message))
        } else { result };
        value = updated;
        match result {
            Ok(result) => { uploaded += 1; message_count += result.message_count; touched = true; }
            Err(error) => { failures.push(format!("{username}：{error}")); break; }
        }
    }

    // A run with nothing to do is still a run: refresh the status, but do not rewrite the
    // settings file on every idle tick.
    if touched || !failures.is_empty() || sync_status_is_stale(&value.sync) {
        settings::update(&data, |next| {
            next.sync.last_run_at = Some(chrono::Utc::now().to_rfc3339());
            next.sync.last_status = Some(if failures.is_empty() { "success" } else { "failed" }.into());
            next.sync.last_error = if failures.is_empty() { None } else { Some(failures.join("；")) };
            next.sync.last_message_count = message_count as u64;
            Ok(())
        })?;
    }

    Ok(ChatSyncSummary {
        status: if failures.is_empty() {
            "success".into()
        } else {
            "failed".into()
        },
        uploaded,
        skipped,
        message_count,
        batch_ids,
        error: if failures.is_empty() {
            None
        } else {
            Some(failures.join("；"))
        },
    })
}

async fn chat_sync_tick(app: &AppHandle) {
    let data = app_data(app);
    let Ok(value) = settings::load(&data) else {
        return;
    };
    if !value.sync.enabled {
        return;
    }
    let Err(error) = perform_chat_sync(app, false, false).await else {
        return;
    };
    // Only persist when the failure reason changed, otherwise every tick rewrites the file.
    if value.sync.last_error.as_deref() == Some(error.as_str()) {
        return;
    }
    let mut next = settings::load(&data).unwrap_or(value);
    next.sync.last_run_at = Some(chrono::Utc::now().to_rfc3339());
    next.sync.last_status = Some("failed".into());
    next.sync.last_error = Some(error);
    let _ = settings::save(&data, &next);
}

#[tauri::command]
async fn devify_sync_now(app: AppHandle) -> Result<ChatSyncSummary, String> {
    perform_chat_sync(&app, true, false).await
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
    settings::update(&app_data(&app), |stored| {
        if value.user_settings_revision != stored.user_settings_revision {
            return Err("SETTINGS_CONFLICT: 设置已在其他位置更改，请重新加载后重试。".into());
        }
        value.upload_queue = stored.upload_queue.clone();
        let sync_enabled = value.sync.enabled;
        value.sync = stored.sync.clone();
        value.sync.enabled = sync_enabled;
        value.automation.last_daily_run = stored.automation.last_daily_run.clone();
        value.automation.last_daily_status = stored.automation.last_daily_status.clone();
        value.automation.last_daily_error = stored.automation.last_daily_error.clone();
        value.automation.daily_topic_id = stored.automation.daily_topic_id;
        value.automation.daily_topic_space_id = stored.automation.daily_topic_space_id;
        value.automation.last_schedule_sync_at = stored.automation.last_schedule_sync_at.clone();
        value.automation.last_schedule_signature = stored.automation.last_schedule_signature.clone();
        value.automation.last_schedule_error = stored.automation.last_schedule_error.clone();
        *stored = value;
        Ok(())
    })
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

/// The only path that reads new WeChat data, so an uninstalled or tampered WeChat stops
/// every new read (manual, scheduled and background) while imported data stays viewable.
fn read_new_wechat_data(
    data: &Path,
    env: &connectors::Environment,
    account: &discover::Account,
    store: &KeyStore,
    apply_wal: bool,
    only_chat: bool,
) -> Result<Vec<DecryptOutcome>, String> {
    connectors::require_installed(connectors::ConnectorKind::Wechat, env)?;
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
    require_connector_installed(app, connectors::ConnectorKind::Wechat)?;
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
    server_id: i64,
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
    match media::make_preview(&app_data(&app), &account_id, local_id, &root, &targets) {
        Ok(path) => Ok(Some(path.to_string_lossy().to_string())),
        Err(_) => Ok(None),
    }
}

/// Decode one encrypted image offline and open it in the system viewer.
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
    media::open_image(&app_data(&app), &account_id, &root, &targets)
        .map(|out| out.to_string_lossy().to_string())
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
#[tauri::command]
async fn wechat_voice_transcribe(
    app: AppHandle,
    account_id: String,
    username: String,
    items: Vec<VoiceRequest>,
) -> Result<Vec<voice::VoiceResult>, String> {
    require_visible_session(&app, &account_id, &username)?;
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = items.iter().map(|item| item.server_id).collect();
    let data_dir = app_data(&app);
    let settings = settings::load(&data_dir)?;
    entitlements::require(
        &data_dir,
        &settings.devify.base_url,
        capabilities::LOCAL_VOICE_TRANSCRIBE,
    )?;
    let cache = decrypt::cache_dir(&data_dir, &account_id);
    let blobs = voice::read_voice_blobs(&cache, &ids)?;
    let view = voice::view(&data_dir);
    if view.selected.is_none() {
        return Err(
            "没有检测到本地转写引擎。可安装 whisper.cpp（brew install whisper-cpp）、mlx-whisper 或 openai-whisper，然后在「设置 → 语音转写」里选择。"
                .into(),
        );
    }

    let mut transcripts = voice::load_transcripts(&data_dir);
    let mut results = Vec::new();
    for item in items {
        let key = voice::transcript_key(&account_id, item.server_id);
        let mut result = voice::VoiceResult {
            server_id: item.server_id,
            local_id: item.local_id,
            wav: None,
            duration_secs: 0.0,
            text: None,
            cached: false,
            error: None,
        };
        let Some(blob) = blobs.get(&item.server_id) else {
            result.error = Some("media_0.db 中没有这条语音。".into());
            results.push(result);
            continue;
        };
        match voice::ensure_wav(&data_dir, &account_id, item.server_id, blob) {
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
                            let _ = voice::save_transcripts(&data_dir, &transcripts);
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
    account_id: String,
    username: String,
    server_id: i64,
) -> Result<VoiceAudio, String> {
    require_visible_session(&app, &account_id, &username)?;
    let data_dir = app_data(&app);
    let cache = decrypt::cache_dir(&data_dir, &account_id);
    let blobs = voice::read_voice_blobs(&cache, &[server_id])?;
    let blob = blobs
        .get(&server_id)
        .ok_or_else(|| "media_0.db 中没有这条语音。".to_string())?;
    let (wav, duration) = voice::ensure_wav(&data_dir, &account_id, server_id, blob)?;
    Ok(VoiceAudio {
        data_uri: voice::audio_data_uri(&wav)?,
        duration_secs: duration,
    })
}

#[tauri::command]
async fn wechat_sessions(app: AppHandle, account_id: String) -> Result<Vec<SessionItem>, String> {
    validate_account_id(&account_id)?;
    let data = app_data(&app);
    // The session list is deliberately readable while disconnected: the chat screen still
    // renders its shell and the reconnect state, but no message content is returned.
    let allowed: HashSet<String> = settings::load(&data)?
        .chat
        .allowed_usernames
        .get(&account_id)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let cache = decrypt::cache_dir(&data, &account_id);
    query::sessions_filtered(&cache, Some(&allowed))
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

/// A pre-ownership job that never obtained a cloud batch and carries no deferred local
/// effects. Nothing of it exists remotely, so it cannot hide uploaded coverage.
fn legacy_job_is_inert(job: &settings::UploadJob) -> bool {
    job.owner.is_none() && job.batch_id.is_none()
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

/// A non-completed job that does not stop a conversation from syncing: it either never
/// reached the server, or the server confirmed its legacy batch ended without ingesting.
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
const LEGACY_ADOPT_BUDGET: Duration = Duration::from_secs(20);

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
            tauri::async_runtime::spawn(async move {
                let batch = tokio::time::timeout_at(deadline, fixed_identity_get(&base, &token, &path)).await.ok()?.ok()?;
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

// The caller holds the identity lease. Completed jobs count only when their entire
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
    // Reconciliation runs in the scheduler; do not fan out its whole queue from a list read.
    let data = app_data(&app);
    let initial = settings::load(&data)?;
    if !initial.connectors.wechat_enabled { return Ok(Vec::new()); }
    let space_id = initial.devify.active_space_id.ok_or("请先选择 空间。")?;
    let (owner, token) = tokio::time::timeout(Duration::from_secs(5), verified_upload_identity(&data, &account_id, space_id)).await
        .map_err(|_| "确认云端身份超时，请稍后重试。")??;
    let _identity_guard = UPLOAD_IDENTITY_LOCK.read().await;
    let deadline = tokio::time::Instant::now() + LEGACY_ADOPT_BUDGET;
    let _ = adopt_legacy_upload_history(&data, &owner, &token, deadline).await;
    let (value, uncertain, uncertain_all) = verified_coverage_settings(&data, &owner, &token).await?;
    if !value.connectors.wechat_enabled { return Ok(Vec::new()); }
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
    let cache = decrypt::cache_dir(&data, &account_id);
    tauri::async_runtime::spawn_blocking(move || {
        let mut states = Vec::new();
        for username in requested {
            // A conversation without a local table cannot say anything about new records;
            // leaving it out keeps the client's last known badge instead of guessing.
            let Ok(Some((time, local_id))) = query::latest_position(&cache, &username) else {
                continue;
            };
            let (state, new_messages) = match coverage.get(&username).copied() {
                Some(position) if query::count_at_time(&cache, &username, time).ok().is_some_and(|count| position.covers_boundary((time, local_id), count)) => ("synced", 0),
                Some(position) => (
                    "pending",
                    query::count_after(&cache, &username, position.time, position.local_id)
                        .unwrap_or(0).max(1),
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
    before_id: Option<i64>,
    before_shard: Option<String>,
) -> Result<Vec<MessageItem>, String> {
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    if !query::is_chat_session(&cache, &username) {
        return Err("公众号、服务号和系统会话不在查看范围内。".into());
    }
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
    query::messages(&cache, &username, limit.unwrap_or(100), cursor)
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
    entitlements::require(
        &data,
        &value.devify.base_url,
        capabilities::AI_AGENT_SUMMARY,
    )?;
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
    let language = value.language.summary.clone();
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
        .manage(McpState::default())
        .setup(|app| {
            let diagnostics_root = app_data(app.handle());
            let _ = diagnostics::record(&diagnostics_root, diagnostics::EventInput { module: diagnostics::Module::App, operation: diagnostics::Operation::Start, code: None, batch_id: None, duration_ms: None });
            let previous_hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                let _ = diagnostics::record(&diagnostics_root, diagnostics::EventInput { module: diagnostics::Module::App, operation: diagnostics::Operation::Panic, code: Some(diagnostics::Code::RustPanic), batch_id: None, duration_ms: None });
                previous_hook(info);
            }));
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
            let sync_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    backup_tick(&sync_handle).await;
                    // A short fixed tick keeps "每天定点" close to the minute it promises;
                    // `backup::due` decides whether this tick actually uploads.
                    tokio::time::sleep(Duration::from_secs(30)).await;
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
            feedback::feedback_export,
            connector_detect,
            update_begin_install,
            update_cancel_install,
            devify_auth_start,
            devify_auth_open,
            devify_auth_callback,
            devify_auth_cancel,
            devify_auth_status,
            devify_current_user,
            devify_analysis_preferences_get,
            devify_analysis_preferences_save,
            devify_analysis_manual_start,
            capabilities_list,
            entitlements_get,
            entitlements_refresh,
            devify_device_register,
            devify_login_password,
            devify_logout,
            devify_spaces,
            devify_space_ensure,
            devify_report_create,
            devify_analysis_list,
            devify_analysis_detail,
            devify_event_notes,
            devify_event_update,
            devify_event_note_remove,
            devify_event_note_assign,
            devify_event_note_create,
            devify_event_relations,
            devify_event_relation_create,
            devify_event_relation_delete,
            devify_conversations,
            devify_conversation_detail,
            devify_conversation_messages,
            devify_message_detail,
            devify_sync_now,
            devify_topics,
            devify_topic_create,
            devify_topic_detail,
            devify_topic_update,
            devify_daily_plan_update,
            devify_topic_delete,
            devify_questions,
            devify_question_detail,
            devify_question_ask,
            devify_note_detail,
            devify_space_takeover,
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
            app_settings_get,
            app_settings_save,
            wechat_scan,
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
            wechat_clear_keys,
            wechat_import_keys,
            wechat_import_saved_key,
            wechat_decrypt,
            wechat_sync,
            wechat_sessions,
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
mod cloud_retry_tests {
    use super::*;

    fn fixture() -> (settings::AppSettings, settings::UploadJob, CloudRetryAuth) {
        let scope = settings::UploadScope {
            usernames: vec!["chat".into()], since: Some(1), until: Some(5),
            base_url: "https://example.test".into(), message_selection: BTreeMap::new(),
        };
        let job = settings::UploadJob {
            id: "job".into(), space_id: 4, account_id: "account".into(),
            batch_id: Some(8), status: "failed".into(), scope: Some(scope.clone()),
            ..Default::default()
        };
        let mut settings = settings::AppSettings::default();
        settings.active_account_id = "account".into();
        settings.devify.active_space_id = Some(4);
        settings.devify.base_url = "https://example.test".into();
        settings.devify.device_id = "device".into();
        settings.chat.allowed_usernames.insert("account".into(), vec!["chat".into()]);
        let auth = CloudRetryAuth {
            token: "synthetic-token".into(), base_url: "https://example.test".into(),
            job_id: "job".into(), space_id: 4, account_id: "account".into(),
            batch_id: 8, device_id: "device".into(), user_id: 3,
            idempotency_key: "package-key".into(), scope,
        };
        (settings, job, auth)
    }

    fn remote_batch() -> serde_json::Value {
        serde_json::json!({"id": 8, "space_id": 4, "uploaded_by_id": 3,
            "source_type": "wechat", "source_account_id": "account",
            "device_id": "device", "status": "failed", "idempotency_key": "package-key"})
    }

    #[test]
    fn remote_identity_requires_every_field_to_match() {
        let (_, _, auth) = fixture();
        let user = serde_json::json!({"id": 3});
        assert_eq!(validate_cloud_retry_remote(&auth, &user, &remote_batch()).unwrap(), (3, "package-key".into()));
        for (field, wrong) in [
            ("id", serde_json::json!(9)), ("space_id", serde_json::json!(5)),
            ("uploaded_by_id", serde_json::json!(2)), ("source_type", serde_json::json!("other")),
            ("source_account_id", serde_json::json!("other")), ("device_id", serde_json::json!("other")),
            ("status", serde_json::json!("processing")), ("idempotency_key", serde_json::json!("")),
        ] {
            let mut batch = remote_batch();
            batch[field] = wrong;
            assert!(validate_cloud_retry_remote(&auth, &user, &batch).is_err(), "{field}");
            batch.as_object_mut().unwrap().remove(field);
            assert!(validate_cloud_retry_remote(&auth, &user, &batch).is_err(), "missing {field}");
        }
        assert!(validate_cloud_retry_remote(&auth, &serde_json::json!({}), &remote_batch()).is_err());
        assert!(validate_cloud_retry_remote(&auth, &serde_json::json!({"id": 0}), &remote_batch()).is_err());
        assert!(validate_cloud_retry_remote(&auth, &serde_json::json!({"id": 2}), &remote_batch()).is_err());
    }

    #[test]
    fn retry_context_requires_original_account_space_service_device_scope_and_token() {
        let (settings, job, auth) = fixture();
        assert!(validate_cloud_retry_context(&settings, &job, &auth, "synthetic-token").is_ok());
        assert!(validate_cloud_retry_context(&settings, &job, &auth, "changed-token").is_err());
        let mut changed = settings.clone(); changed.active_account_id = "other".into();
        assert!(validate_cloud_retry_context(&changed, &job, &auth, "synthetic-token").is_err());
        let mut changed = settings.clone(); changed.devify.active_space_id = Some(5);
        assert!(validate_cloud_retry_context(&changed, &job, &auth, "synthetic-token").is_err());
        let mut changed = settings.clone(); changed.devify.base_url = "https://other.test".into();
        assert!(validate_cloud_retry_context(&changed, &job, &auth, "synthetic-token").is_err());
        let mut changed = settings.clone(); changed.devify.device_id = "other".into();
        assert!(validate_cloud_retry_context(&changed, &job, &auth, "synthetic-token").is_err());
        let mut changed = settings.clone(); changed.devify.device_id.clear();
        assert!(validate_cloud_retry_context(&changed, &job, &auth, "synthetic-token").is_err());
        let mut changed = settings.clone(); changed.chat.allowed_usernames.clear();
        assert!(validate_cloud_retry_context(&changed, &job, &auth, "synthetic-token").is_err());
        let mut changed = job.clone(); changed.batch_id = Some(9);
        assert!(validate_cloud_retry_context(&settings, &changed, &auth, "synthetic-token").is_err());
        let mut changed = job.clone(); changed.status = "processing".into();
        assert!(validate_cloud_retry_context(&settings, &changed, &auth, "synthetic-token").is_err());
        let mut changed = job.clone(); changed.scope.as_mut().unwrap().until = Some(6);
        assert!(validate_cloud_retry_context(&settings, &changed, &auth, "synthetic-token").is_err());
    }

    #[test]
    fn retry_identity_response_allows_five_mib_but_rejects_more() {
        let mut body = Vec::new();
        let first = vec![0u8; 1024 * 1024 + 1];
        append_retry_identity_body(&mut body, &first).unwrap();
        let rest = vec![0u8; UPLOAD_LIST_MAX_RESPONSE_BYTES - first.len()];
        append_retry_identity_body(&mut body, &rest).unwrap();
        assert_eq!(body.len(), UPLOAD_LIST_MAX_RESPONSE_BYTES);
        assert!(append_retry_identity_body(&mut body, &[0]).is_err());
        assert_eq!(body.len(), UPLOAD_LIST_MAX_RESPONSE_BYTES);
    }

    #[test]
    fn package_key_and_device_claim_are_not_guessed() {
        let (_, _, auth) = fixture();
        assert!(validate_cloud_retry_package(&auth, 4, "account", "package-key").is_ok());
        assert!(validate_cloud_retry_package(&auth, 4, "account", "changed-key").is_err());
        assert!(validate_cloud_retry_package(&auth, 5, "account", "package-key").is_err());
        assert!(validate_cloud_retry_package(&auth, 4, "other", "package-key").is_err());
        let token = format!("header.{}.signature", URL_SAFE_NO_PAD.encode(br#"{"device_id":"device"}"#));
        assert_eq!(cloud_retry_device_id("", &token).unwrap(), "device");
        assert_eq!(cloud_retry_device_id("device", &token).unwrap(), "device");
        assert!(cloud_retry_device_id("other", &token).is_err());
        assert!(cloud_retry_device_id("", "synthetic-token").is_err());
        assert!(cloud_retry_device_id(" device", "synthetic-token").is_err());
    }
}

#[cfg(test)]
mod upload_list_tests {
    use super::*;
    use std::net::TcpListener as StdListener;

    fn valid_page() -> serde_json::Value {
        serde_json::json!({
            "list": [{"id": 9, "status": "processing", "trigger": "scheduled"}],
            "pagination": {"total": 1, "page": 1, "page_size": 100, "next": null, "previous": null}
        })
    }

    #[test]
    fn pagination_rejects_zero_and_overflow() {
        assert_eq!(upload_list_pagination(None, None).unwrap(), (1, 100));
        assert_eq!(upload_list_pagination(Some(2), Some(1)).unwrap(), (2, 1));
        for (page, size) in [(0, 1), (1, 0), (1, 101), (UPLOAD_LIST_MAX_PAGE + 1, 1)] {
            assert!(upload_list_pagination(Some(page), Some(size)).is_err());
        }
    }

    #[test]
    fn upload_list_requires_complete_pagination_and_keeps_server_rows() {
        let expected = valid_page();
        assert_eq!(parse_upload_list(serde_json::json!({"code": 0, "data": expected})).unwrap(), expected);
        assert_eq!(parse_upload_list(expected.clone()).unwrap(), expected);
        for payload in [
            serde_json::json!({"data": null}),
            serde_json::json!({"list": null, "pagination": {}}),
            serde_json::json!({"list": [null], "pagination": {"total": 1, "page": 1, "page_size": 10, "next": null, "previous": null}}),
            serde_json::json!({"list": [], "pagination": {"total": 0, "page": 1, "page_size": 10, "next": null}}),
            serde_json::json!({"list": [], "pagination": {"total": 0, "page": 0, "page_size": 10, "next": null, "previous": null}}),
            serde_json::json!({"list": [], "pagination": {"total": 0, "page": 1, "page_size": 101, "next": null, "previous": null}}),
        ] { assert!(parse_upload_list(payload).is_err()); }
    }

    #[test]
    fn list_rejects_wrong_space_and_unsafe_service_address() {
        let mut settings = settings::AppSettings::default();
        settings.devify.active_space_id = Some(4);
        settings.devify.base_url = "https://example.test/".into();
        assert_eq!(upload_list_base_url(&settings, 4).unwrap(), "https://example.test");
        assert!(upload_list_base_url(&settings, 3).is_err());
        assert!(upload_list_base_url(&settings, 0).is_err());
        for base in ["https://user:password@example.test", "https://example.test/?token=secret", "file:///tmp/data", "https://example.test/#fragment"] {
            settings.devify.base_url = base.into();
            assert!(upload_list_base_url(&settings, 4).is_err());
        }
    }

    #[test]
    fn list_http_request_is_paged_authenticated_and_does_not_follow_redirect() {
        let listener = StdListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 1024];
            while !request.ends_with(b"\r\n\r\n") {
                let count = stream.read(&mut buf).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buf[..count]);
            }
            stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/other\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            String::from_utf8(request).unwrap()
        });
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (status, body) = runtime.block_on(fetch_upload_list(&format!("http://{address}"), 42, 2, 30, "test-token")).unwrap();
        assert_eq!(status, reqwest::StatusCode::FOUND);
        assert!(body.is_empty());
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /api/v1/spaces/42/chat/uploads?page=2&page_size=30 http/1.1\r\n"));
        assert!(request.contains("authorization: bearer test-token\r\n"));
    }

    #[test]
    fn trigger_respects_original_sync_source_and_unknown_history() {
        assert_eq!(sync_upload_trigger(true, false, None), UploadTrigger::Manual);
        assert_eq!(sync_upload_trigger(false, false, None), UploadTrigger::Scheduled);
        assert_eq!(sync_upload_trigger(true, true, Some(true)), UploadTrigger::Manual);
        assert_eq!(sync_upload_trigger(true, true, Some(false)), UploadTrigger::Scheduled);
        assert_eq!(sync_upload_trigger(true, true, None), UploadTrigger::Unknown);
        for (trigger, expected) in [(UploadTrigger::Manual, "manual"), (UploadTrigger::Scheduled, "scheduled"), (UploadTrigger::Unknown, "unknown")] {
            let body = upload_create_body("account", "key", "device", "label", "mac", trigger);
            assert_eq!(body["trigger"], expected);
            assert_eq!(body["source_type"], "wechat");
            assert_eq!(body["idempotency_key"], "key");
        }
        let legacy: settings::ContinuationScope = serde_json::from_value(serde_json::json!({"space_id": 1, "base_url": "https://example.test"})).unwrap();
        assert_eq!(legacy.manual, None);
        let legacy_job: settings::UploadJob = serde_json::from_value(serde_json::json!({"id": "old"})).unwrap();
        assert_eq!(legacy_job.continuation_manual, None);
        for source in [Some(true), Some(false), None] {
            let mut value = settings::AppSettings::default();
            value.upload_queue.push(settings::UploadJob {
                account_id: "account".into(), space_id: 1, status: "completed".into(),
                continue_sync: true, continuation_manual: source,
                scope: Some(settings::UploadScope {
                    usernames: vec!["chat".into()], since: None, until: None,
                    base_url: "https://example.test".into(), message_selection: BTreeMap::new(),
                }),
                ..Default::default()
            });
            apply_completed_uploads(&mut value);
            assert_eq!(value.sync.continuation["account"]["chat"].manual, source);
        }
    }
}

#[cfg(test)]
mod attachment_tests {
    use super::*;
    #[test]
    fn extension_keeps_only_safe_suffixes() {
        assert_eq!(attachment_extension(Some("report.pdf")), "pdf");
        assert_eq!(attachment_extension(Some("HOLIDAY.JPG")), "jpg");
        assert_eq!(attachment_extension(Some("archive.tar.gz")), "gz");
    }

    #[test]
    fn extension_falls_back_when_missing_or_unsafe() {
        assert_eq!(attachment_extension(None), "bin");
        assert_eq!(attachment_extension(Some("no-extension")), "bin");
        assert_eq!(attachment_extension(Some("../../etc/passwd")), "bin");
        assert_eq!(attachment_extension(Some("name.verylongextension")), "bin");
        assert_eq!(attachment_extension(Some("name.we ird")), "bin");
    }
}

/// The upload path used to abort with "分页遇到相同时间戳" whenever a full page ended in a
/// second shared by more messages. These tests page a synthetic shard to prove every row
/// comes back exactly once.
#[cfg(test)]
mod message_paging_tests {
    use super::*;
    use rusqlite::Connection;

    fn cache_with_messages(username: &str, rows: &[(i64, i64)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "wc-paging-test-{}-{}",
            std::process::id(),
            rows.len()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("message")).unwrap();
        let table = format!("Msg_{}", query::md5_hex(username));
        let conn = Connection::open(root.join("message").join("message_0.db")).unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE \"{table}\" (
                local_id INTEGER PRIMARY KEY,
                server_id INTEGER,
                create_time INTEGER,
                message_content BLOB,
                local_type INTEGER,
                real_sender_id INTEGER
            );"
        ))
        .unwrap();
        for (local_id, time) in rows {
            conn.execute(
                &format!(
                    "INSERT INTO \"{table}\" (local_id, server_id, create_time, message_content, local_type, real_sender_id)
                     VALUES (?1, ?1, ?2, ?3, 1, NULL)"
                ),
                rusqlite::params![local_id, time, format!("消息 {local_id}").into_bytes()],
            )
            .unwrap();
        }
        root
    }

    #[test]
    fn a_full_page_sharing_one_second_no_longer_aborts_the_upload() {
        let username = "123456789@chatroom";
        // 1200 messages, all inside the same second: the old timestamp cursor stopped at
        // 500 and the upload refused to continue.
        let rows: Vec<(i64, i64)> = (1..=1200).map(|id| (id, 1_700_000_000)).collect();
        let root = cache_with_messages(username, &rows);

        let messages = collect_upload_messages(&root, username, None, None, None).unwrap();
        assert_eq!(messages.len(), 1200, "每一条都必须读到");
        let ids: HashSet<i64> = messages.iter().map(|message| message.local_id).collect();
        assert_eq!(ids.len(), 1200, "不能重复");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn paging_continues_inside_a_second_that_straddles_the_page_boundary() {
        let username = "wxid_boundary";
        // Newest 200 share second 200, then 400 older ones share second 100: the cursor
        // lands mid-second in both cases.
        let mut rows: Vec<(i64, i64)> = (1..=200).map(|id| (id, 200)).collect();
        rows.extend((201..=600).map(|id| (id, 100)));
        let root = cache_with_messages(username, &rows);

        let messages = collect_upload_messages(&root, username, None, None, None).unwrap();
        assert_eq!(messages.len(), 600);
        let times: Vec<i64> = messages.iter().map(|message| message.time).collect();
        assert!(times.windows(2).all(|pair| pair[0] >= pair[1]), "保持从新到旧");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn since_stops_paging_without_dropping_the_boundary_second() {
        let username = "wxid_since";
        let rows: Vec<(i64, i64)> = (1..=900).map(|id| (id, 1_000 + id)).collect();
        let root = cache_with_messages(username, &rows);

        let messages = collect_upload_messages(&root, username, Some(1_500), None, None).unwrap();
        assert_eq!(messages.len(), 401, "只保留 1500 及以后的消息");
        assert!(messages.iter().all(|message| message.time >= 1_500));

        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod devify_error_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_detail_nested_in_the_api_envelope() {
        let payload = json!({
            "code": "user_not_found",
            "message": "failed",
            "data": {"detail": "User not found", "code": "user_not_found"}
        });
        assert_eq!(
            devify_error_message(&payload).as_deref(),
            Some("User not found")
        );
    }

    #[test]
    fn reads_validation_field_errors_nested_in_the_envelope() {
        let payload = json!({
            "code": 400,
            "message": "failed",
            "data": {"title": ["不能为空。"], "period": ["周期无效。"]}
        });
        let message = devify_error_message(&payload).expect("message");
        assert!(message.contains("title：不能为空。"), "{message}");
        assert!(message.contains("period：周期无效。"), "{message}");
    }

    #[test]
    fn falls_back_to_plain_top_level_errors() {
        let payload = json!({"detail": "Method \"GET\" not allowed."});
        assert_eq!(
            devify_error_message(&payload).as_deref(),
            Some("Method \"GET\" not allowed.")
        );
    }

    #[test]
    fn ignores_empty_error_payloads() {
        assert_eq!(devify_error_message(&json!({})), None);
    }

    #[test]
    fn falls_back_when_no_reason_is_present() {
        assert_eq!(
            devify_error_text(&json!({}), "服务端拒绝了请求"),
            "服务端拒绝了请求"
        );
    }

    #[test]
    fn points_at_space_selection_when_the_space_is_gone() {
        assert_eq!(
            devify_friendly_error(reqwest::StatusCode::NOT_FOUND, "Space not found."),
            "当前 空间不存在或无权访问，请在「设置 / 账户」重新选择空间。"
        );
        assert_eq!(
            devify_friendly_error(reqwest::StatusCode::NOT_FOUND, "Event not found."),
            "Event not found."
        );
    }
}

#[cfg(test)]
mod key_flow_tests {
    use super::*;

    #[test]
    fn upload_preview_distinguishes_same_local_id_across_shards() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("session")).unwrap();
        std::fs::create_dir_all(dir.path().join("message")).unwrap();
        let sessions = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
        sessions.execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','test');").unwrap();
        let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
        for (shard, timestamp, body) in [(0, 100, "first"), (1, 200, "second")] {
            let db = rusqlite::Connection::open(dir.path().join(format!("message/message_{shard}.db"))).unwrap();
            db.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); INSERT INTO {table} VALUES (1,{timestamp},'{body}',1);")).unwrap();
        }
        let result = prepare_upload(dir.path(), "account", 7, "https://example.test", &["person".into()], None, None, &BTreeMap::new()).unwrap();
        assert_eq!(result.preview.message_count, 2);
        assert_ne!(result.messages[0]["message_key"], result.messages[1]["message_key"]);
    }

    #[test]
    fn pagination_keeps_same_id_and_second_from_different_shards() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("message")).unwrap();
        let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
        for shard in [0, 1] {
            let db = rusqlite::Connection::open(dir.path().join(format!("message/message_{shard}.db"))).unwrap();
            db.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, create_time INTEGER, message_content TEXT); INSERT INTO {table} VALUES (1,100,'test');")).unwrap();
        }
        let first = query::messages(dir.path(), "person", 1, None).unwrap();
        let second = query::messages(dir.path(), "person", 1, Some(query::MessageCursor::from_message(&first[0]))).unwrap();
        assert_eq!(second.len(), 1);
        assert_ne!(first[0].source_shard, second[0].source_shard);
    }

    #[test]
    fn legacy_key_is_reused_only_for_matching_message() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("session")).unwrap();
        std::fs::create_dir_all(dir.path().join("message")).unwrap();
        let sessions = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
        sessions.execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','test');").unwrap();
        let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
        for (shard, timestamp, server_id) in [(0, 100, 101), (1, 200, 202)] {
            let db = rusqlite::Connection::open(dir.path().join(format!("message/message_{shard}.db"))).unwrap();
            db.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, server_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); INSERT INTO {table} VALUES (1,{server_id},{timestamp},'test',1);")).unwrap();
        }
        let mut history = LegacyUploadKeys::new();
        history.insert("person".into(), ExistingUploadKeys { legacy: HashMap::from([(1, Some(200))]), stable: HashSet::new() });
        let prepare = |since, until| prepare_upload_with_voice(dir.path(), dir.path(), dir.path(), "account", 7, "https://example.test", &["person".into()], since, until, &BTreeMap::new(), &settings::VoiceUploadSettings::default(), "text", &history).unwrap();
        let both = prepare(None, None);
        let one = prepare(Some(100), Some(150));
        assert_eq!(both.messages[0]["message_key"], "srv:101");
        assert_eq!(both.messages[1]["message_key"], "src:1");
        assert_eq!(one.messages[0]["message_key"], both.messages[0]["message_key"]);
    }

    #[test]
    fn ambiguous_legacy_message_is_rejected_instead_of_misidentified() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("session")).unwrap();
        std::fs::create_dir_all(dir.path().join("message")).unwrap();
        let sessions = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
        sessions.execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','test');").unwrap();
        let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
        for shard in [0, 1] {
            let db = rusqlite::Connection::open(dir.path().join(format!("message/message_{shard}.db"))).unwrap();
            db.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); INSERT INTO {table} VALUES (1,100,'test',1);")).unwrap();
        }
        let mut history = LegacyUploadKeys::new();
        history.insert("person".into(), ExistingUploadKeys { legacy: HashMap::from([(1, Some(100))]), stable: HashSet::new() });
        let result = prepare_upload_with_voice(dir.path(), dir.path(), dir.path(), "account", 7, "https://example.test", &["person".into()], None, None, &BTreeMap::new(), &settings::VoiceUploadSettings::default(), "text", &history);
        assert!(matches!(result, Err(error) if error.contains("旧消息与多个本地记录匹配")));
    }

    #[test]
    fn colliding_media_remains_attached_to_its_own_message() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("session")).unwrap();
        std::fs::create_dir_all(dir.path().join("message")).unwrap();
        let sessions = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
        sessions.execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','test');").unwrap();
        let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
        for (shard, timestamp, server_id) in [(0, 100, 101), (1, 200, 202)] {
            let db = rusqlite::Connection::open(dir.path().join(format!("message/message_{shard}.db"))).unwrap();
            db.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, server_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); INSERT INTO {table} VALUES (1,{server_id},{timestamp},'<msg><img /></msg>',3);")).unwrap();
        }
        let result = prepare_upload_with_voice(dir.path(), dir.path(), dir.path(), "account", 7, "https://example.test", &["person".into()], None, None, &BTreeMap::new(), &settings::VoiceUploadSettings::default(), "images", &LegacyUploadKeys::new()).unwrap();
        let image_keys: HashSet<_> = result.parts.iter().filter(|p| p["kind"] == "image").filter_map(|p| p["message_key"].as_str()).collect();
        assert_eq!(image_keys, HashSet::from(["srv:101", "srv:202"]));
    }

    #[test]
    fn upload_reads_image_from_account_not_database_cache() {
        let cache = tempfile::tempdir().unwrap();
        let account = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cache.path().join("session")).unwrap();
        std::fs::create_dir_all(cache.path().join("message")).unwrap();
        let sessions = rusqlite::Connection::open(cache.path().join("session/session.db")).unwrap();
        sessions.execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','test');").unwrap();
        let timestamp = 1_762_000_000i64;
        let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
        let db = rusqlite::Connection::open(cache.path().join("message/message_0.db")).unwrap();
        db.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, server_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); INSERT INTO {table} VALUES (1,101,{timestamp},'<msg><img /></msg>',3);")).unwrap();
        let month = chrono::DateTime::from_timestamp(timestamp, 0).unwrap().with_timezone(&chrono::Local).format("%Y-%m").to_string();
        let image_dir = account.path().join("cache").join(month).join("Message").join(hex::encode(md5::Md5::digest(b"person"))).join("Thumb");
        std::fs::create_dir_all(&image_dir).unwrap();
        let bytes = b"\xff\xd8\xfffixture";
        std::fs::write(image_dir.join(format!("1_{timestamp}_thumb.jpg")), bytes).unwrap();
        let result = prepare_upload_with_voice(cache.path(), cache.path(), account.path(), "account", 7, "https://example.test", &["person".into()], None, None, &BTreeMap::new(), &settings::VoiceUploadSettings::default(), "images", &LegacyUploadKeys::new()).unwrap();
        let part = result.parts.iter().find(|p| p["kind"] == "image").unwrap();
        let path = part["upload_item_path"].as_str().unwrap();
        assert!(result.media_files.iter().any(|(name, content)| name == path && content == bytes));
    }

    #[test]
    fn colliding_image_uses_wechat_resource_identity() {
        let cache = tempfile::tempdir().unwrap();
        let account = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cache.path().join("session")).unwrap();
        std::fs::create_dir_all(cache.path().join("message")).unwrap();
        let sessions = rusqlite::Connection::open(cache.path().join("session/session.db")).unwrap();
        sessions.execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','test');").unwrap();
        let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
        for (shard, timestamp, server_id) in [(0, 100, 101), (1, 200, 202)] {
            let db = rusqlite::Connection::open(cache.path().join(format!("message/message_{shard}.db"))).unwrap();
            db.execute_batch(&format!("CREATE TABLE {table} (local_id INTEGER, server_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); INSERT INTO {table} VALUES (1,{server_id},{timestamp},'<msg><img length=\"10\" /></msg>',3);")).unwrap();
        }
        let resource = rusqlite::Connection::open(cache.path().join("message/message_resource.db")).unwrap();
        resource.execute_batch("CREATE TABLE MessageResourceInfo (message_svr_id INTEGER, message_local_id INTEGER, message_create_time INTEGER, packed_info BLOB);").unwrap();
        resource.execute("INSERT INTO MessageResourceInfo VALUES (101, 1, 100, ?1)", [b"abc12345678901234567890123456789".as_slice()]).unwrap();
        let month = chrono::DateTime::from_timestamp(100, 0).unwrap().with_timezone(&chrono::Local).format("%Y-%m").to_string();
        let image_dir = account.path().join("msg/attach").join(hex::encode(md5::Md5::digest(b"person"))).join(month).join("Img");
        std::fs::create_dir_all(&image_dir).unwrap();
        let bytes = b"\xff\xd8\xfffixture";
        std::fs::write(image_dir.join("abc12345678901234567890123456789.dat"), bytes).unwrap();
        let result = prepare_upload_with_voice(cache.path(), cache.path(), account.path(), "account", 7, "https://example.test", &["person".into()], None, None, &BTreeMap::new(), &settings::VoiceUploadSettings::default(), "images", &LegacyUploadKeys::new()).unwrap();
        let first = result.parts.iter().find(|p| p["kind"] == "image" && p["message_key"] == "srv:101").unwrap();
        assert!(first["upload_item_path"].is_string());
        let second = result.parts.iter().find(|p| p["kind"] == "image" && p["message_key"] == "srv:202").unwrap();
        assert_eq!(second["binding_state"], "missing");
    }

    #[test]
    fn manual_batch_plan_preserves_every_selected_message() {
        let refs = (0..2301).map(|n| settings::MessageRef { local_id: n, time: n / 3, source_shard: format!("message_{}.db", n % 2) }).collect::<Vec<_>>();
        let scope = BTreeMap::from([("chat".to_string(), refs.clone())]);
        let batches = super::manual_batch_plan(&scope, 1000).unwrap();
        assert_eq!(batches.iter().map(|batch| batch.values().map(Vec::len).sum::<usize>()).collect::<Vec<_>>(), vec![1000,1000,301]);
        assert_eq!(batches.into_iter().flat_map(|batch| batch.into_values().flatten()).collect::<Vec<_>>(), refs);
        let duplicate = BTreeMap::from([("chat".to_string(), vec![refs[0].clone(), refs[0].clone()])]);
        assert!(super::manual_batch_plan(&duplicate, 1000).is_err());
    }

    #[test]
    fn manual_batches_resume_in_order_and_stop_on_failure() {
        let mut value = settings::AppSettings::default();
        value.active_account_id = "account".into();
        value.devify.active_space_id = Some(7);
        let owner = settings::UploadOwner { base_url: "https://example.invalid".into(), space_id: 7,
            account_id: "account".into(), user_id: 1, device_id: "device".into() };
        value.upload_queue = (0..3).map(|index| settings::UploadJob {
            id: format!("batch-{index}"), account_id: "account".into(), space_id: 7,
            owner: Some(owner.clone()), create_started: Some(false),
            status: "queued".into(), manual_group: Some("group".into()), manual_index: index,
            manual_total: 3, ..Default::default()
        }).collect();
        assert_eq!(super::next_manual_batch(&value, &owner).as_deref(), Some("batch-0"));
        let mut other_user = owner.clone(); other_user.user_id = 2;
        assert!(super::next_manual_batch(&value, &other_user).is_none());
        assert_eq!(value.upload_queue[0].status, "queued");
        value.upload_queue[0].status = "running".into();
        value.upload_queue[0].manual_fingerprint = Some("saved".into());
        let mut value: settings::AppSettings = serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
        assert_eq!(super::next_manual_batch(&value, &owner).as_deref(), Some("batch-0"));
        value.upload_queue[0].create_started = Some(true);
        assert!(super::next_manual_batch(&value, &owner).is_none());
        value.upload_queue[0].status = "failed".into();
        assert!(super::next_manual_batch(&value, &owner).is_none());
        value.upload_queue[0].status = "completed".into();
        assert_eq!(super::next_manual_batch(&value, &owner).as_deref(), Some("batch-1"));
        value.upload_queue[1].status = "processing".into();
        value.upload_queue[1].batch_id = Some(1);
        assert!(super::next_manual_batch(&value, &owner).is_none());
        value.upload_queue.push(settings::UploadJob { id: "old-stopped".into(), account_id: "account".into(), space_id: 7, status: "failed".into(), ..Default::default() });
        assert!(super::next_manual_batch(&value, &owner).is_none());
        value.devify.active_space_id = Some(8);
        assert!(super::next_manual_batch(&value, &owner).is_none());
    }

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
                source_shard: String::new(),
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
                source_shard: String::new(),
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
    fn upload_package_carries_conversation_avatars() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("session")).unwrap();
        std::fs::create_dir_all(dir.path().join("message")).unwrap();
        std::fs::create_dir_all(dir.path().join("head_image")).unwrap();
        let session = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
        session
            .execute_batch(
                "CREATE TABLE Session (username TEXT, display_name TEXT); \
                 INSERT INTO Session VALUES ('person','测试会话');",
            )
            .unwrap();
        let message = rusqlite::Connection::open(dir.path().join("message/message_0.db")).unwrap();
        let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
        message
            .execute_batch(&format!(
                "CREATE TABLE {table} (local_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); \
                 INSERT INTO {table} VALUES (1,100,'hello',1);"
            ))
            .unwrap();
        let head = rusqlite::Connection::open(dir.path().join("head_image/head_image.db")).unwrap();
        head.execute_batch(
            "CREATE TABLE head_image (username TEXT PRIMARY KEY, md5 TEXT, image_buffer BLOB, update_time INTEGER);",
        )
        .unwrap();
        let png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];
        head.execute(
            "INSERT INTO head_image VALUES ('person','md5',?1,0)",
            [&png],
        )
        .unwrap();

        let prepared = prepare_upload(
            dir.path(),
            "account",
            7,
            "https://example.test",
            &["person".to_string()],
            Some(0),
            Some(1000),
            &BTreeMap::new(),
        )
        .unwrap();

        assert_eq!(prepared.avatar_files.len(), 1);
        assert!(prepared.avatar_files[0].0.starts_with("media/avatars/"));
        assert!(prepared.avatar_files[0].0.ends_with(".png"));
        assert_eq!(prepared.avatar_files[0].1, png);
        assert!(prepared
            .avatars
            .iter()
            .any(|row| row["scope"] == "conversation" && row["conversation_id"] == "person"));
    }

    #[test]
    fn upload_package_includes_cached_voice_transcript_without_audio_bytes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("session")).unwrap();
        std::fs::create_dir_all(dir.path().join("message")).unwrap();
        let session = rusqlite::Connection::open(dir.path().join("session/session.db")).unwrap();
        session
            .execute_batch("CREATE TABLE Session (username TEXT, display_name TEXT); INSERT INTO Session VALUES ('person','测试会话');")
            .unwrap();
        let message = rusqlite::Connection::open(dir.path().join("message/message_0.db")).unwrap();
        let table = format!("Msg_{}", hex::encode(md5::Md5::digest(b"person")));
        message
            .execute_batch(&format!(
                "CREATE TABLE {table} (local_id INTEGER, server_id INTEGER, create_time INTEGER, message_content TEXT, local_type INTEGER); INSERT INTO {table} VALUES (1,4242,100,'<msg><voicemsg voicelength=\\\"2300\\\" /></msg>',34);"
            ))
            .unwrap();
        let media = rusqlite::Connection::open(dir.path().join("message/media_0.db")).unwrap();
        media
            .execute_batch("CREATE TABLE VoiceInfo (svr_id INTEGER, voice_data BLOB); INSERT INTO VoiceInfo VALUES (4242, X'02');")
            .unwrap();
        voice::save_transcripts(
            dir.path(),
            &BTreeMap::from([(
                voice::transcript_key("account", 4242),
                voice::TranscriptEntry {
                    text: "明天下午三点开会".into(),
                    engine: "test".into(),
                    model: "fixture".into(),
                    created_at: 1,
                },
            )]),
        )
        .unwrap();

        let prepared = prepare_upload_with_voice(
            dir.path(),
            dir.path(),
            dir.path(),
            "account",
            7,
            "https://example.test",
            &["person".into()],
            None,
            None,
            &BTreeMap::new(),
            &settings::VoiceUploadSettings {
                auto_transcribe: true,
                upload_transcripts: true,
            },
            "images",
            &LegacyUploadKeys::new(),
        )
        .unwrap();

        assert!(prepared
            .parts
            .iter()
            .any(|part| { part["kind"] == "audio" && part["binding_state"] == "missing" }));
        assert!(prepared.parts.iter().any(|part| {
            part["kind"] == "text"
                && part["part_key"] == "voice-transcript-4242"
                && part["text"] == "明天下午三点开会"
                && part["source_header"]["engine"] == "test"
        }));
        assert!(prepared.voice_pending.is_empty());
    }

    #[test]
    fn upload_preview_reads_a_shared_second_past_the_page_boundary() {
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
        // 501 messages inside one second: the cursor must walk the whole second instead of
        // giving up with "分页遇到相同时间戳".
        let prepared = prepare_upload(
            dir.path(),
            "account",
            7,
            "https://example.test",
            &["person".into()],
            None,
            None,
            &BTreeMap::new(),
        )
        .expect("分页必须读完整秒");
        assert_eq!(prepared.preview.message_count, 501);
        assert_eq!(prepared.messages.len(), 501);
        let ids: HashSet<_> = prepared
            .messages
            .iter()
            .map(|message| message["source_message_id"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(ids.len(), 501);
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
            server_id: 0,
            source_shard: String::new(),
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
            source_shard: String::new(),
            local_id: 2,
            time: 200,
        }];
        let selected = filter_upload_messages(all.clone(), Some(&refs)).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].text, "b");
        assert!(filter_upload_messages(
            all.clone(),
            Some(&[settings::MessageRef {
                source_shard: String::new(),
                local_id: 3,
                time: 300
            }])
        )
        .is_err());
        assert!(filter_upload_messages(
            vec![all[0].clone(), all[0].clone()],
            Some(&[settings::MessageRef {
                source_shard: String::new(),
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

#[cfg(test)]
mod space_tests {
    use super::{pick_personal_space, DevifySpace};

    fn space(id: i64, name: &str, kind: &str) -> DevifySpace {
        DevifySpace {
            id,
            name: name.into(),
            kind: kind.into(),
            status: "active".into(),
            role: None,
            is_global_admin: false,
            admin_takeover_active: false,
        }
    }

    #[test]
    fn personal_space_is_the_default_target() {
        let spaces = vec![
            space(1, "团队空间", "team"),
            space(2, "个人空间", "personal"),
        ];
        assert_eq!(pick_personal_space(&spaces), Some(2));
    }

    #[test]
    fn falls_back_to_name_then_first_space() {
        let named = vec![space(3, "团队", "team"), space(4, "我的个人空间", "shared")];
        assert_eq!(pick_personal_space(&named), Some(4));
        let unnamed = vec![space(7, "Alpha", "team"), space(8, "Beta", "team")];
        assert_eq!(pick_personal_space(&unnamed), Some(7));
        assert_eq!(pick_personal_space(&[]), None);
    }
}

#[cfg(test)]
mod upload_coverage_tests {
    use super::upload_coverage;
    use crate::settings::{self, SyncCursor, UploadJob, UploadScope};
    use std::collections::BTreeMap;

    #[test]
    fn batch_plan_preserves_same_second_and_shard_identity() {
        use crate::settings::MessageRef;
        let make = |time, local_id, source_shard: &str| MessageRef { time, local_id, source_shard: source_shard.into() };
        let messages = vec![make(20, 2, "b"), make(10, 1, "b"), make(10, 1, "a"), make(20, 2, "a"), make(30, 3, "a")];
        let first = super::plan_sync_batch(&messages, None, 3).unwrap();
        assert_eq!(first.messages, vec![make(10, 1, "a"), make(10, 1, "b")]);
        assert!(first.has_more);
        let second = super::plan_sync_batch(&messages, Some(SyncCursor { time: 10, local_id: 1, boundary_count: Some(2) }), 3).unwrap();
        assert_eq!(second.messages, vec![make(20, 2, "a"), make(20, 2, "b"), make(30, 3, "a")]);
        assert!(!second.has_more);
    }

    #[test]
    fn batch_plan_refuses_to_split_oversized_second() {
        use crate::settings::MessageRef;
        let messages: Vec<_> = (0..4).map(|id| MessageRef { time: 10, local_id: id, source_shard: "a".into() }).collect();
        assert!(super::plan_sync_batch(&messages, None, 3).is_err());
    }

    #[test]
    fn oversized_manual_upload_is_rejected_before_submission() {
        assert!(super::validate_upload_batch_size(1000).is_ok());
        assert!(super::validate_upload_batch_size(1001).is_err());
    }

    fn owner() -> settings::UploadOwner {
        settings::UploadOwner { base_url: "https://example.test".into(), space_id: 1, account_id: "acct".into(), user_id: 7, device_id: "device".into() }
    }

    fn job(status: &str, covered: SyncCursor, fallback_at: &str) -> UploadJob {
        let mut covered_map = BTreeMap::new();
        covered_map.insert("one".to_string(), covered);
        UploadJob {
            id: "job".into(),
            account_id: "acct".into(),
            space_id: 1,
            batch_id: Some(1),
            owner: Some(owner()),
            status: status.into(),
            created_at: fallback_at.into(),
            updated_at: fallback_at.into(),
            scope: Some(UploadScope {
                usernames: vec!["one".into()],
                since: None,
                until: None,
                base_url: "https://example.test".into(),
                message_selection: BTreeMap::new(),
            }),
            covered: covered_map,
            ..UploadJob::default()
        }
    }

    fn with_jobs(jobs: Vec<UploadJob>) -> settings::AppSettings {
        let mut value = settings::AppSettings::default();
        value.upload_queue = jobs.into_iter().map(|mut item| {
            item.verified_completion_digest = super::upload_completion_digest(&item);
            item
        }).collect();
        value.sync.owners.insert("acct".into(), owner());
        value
    }

    #[test]
    fn equal_position_preserves_precise_boundary_count() {
        let mut value = with_jobs(vec![job("completed", SyncCursor { time: 100, local_id: 5, boundary_count: None }, "")]);
        value.sync.cursors.entry("acct".into()).or_default().insert("one".into(), SyncCursor { time: 100, local_id: 5, boundary_count: Some(1) });
        assert_eq!(upload_coverage(&value, &owner())["one"].boundary_count, Some(1));
    }

    #[test]
    fn voice_retry_selects_only_pending_and_incremental_messages() {
        use crate::settings::{MessageRef, VoiceRef};
        let r = |time, local_id| MessageRef { time, local_id, source_shard: "message_0.db".into() };
        let messages = vec![r(10, 1), r(20, 2), r(30, 3), r(40, 4)];
        let voice = vec![VoiceRef { time: 10, local_id: 1, server_id: 11, source_shard: "message_0.db".into() }];
        assert_eq!(super::sync_message_selection(&messages, Some(SyncCursor { time: 30, local_id: 3, boundary_count: Some(1) }), &voice), vec![r(10, 1), r(30, 3), r(40, 4)]);
    }

    #[test]
    fn submitted_job_is_not_reported_as_completed() {
        let position = SyncCursor { time: 100, local_id: 5, boundary_count: Some(1) };
        let mut value = with_jobs(vec![job("processing", position, "2026-11-06T00:00:00Z")]);
        value.devify.active_space_id = Some(1);
        assert_eq!(super::conversation_job_state(&value, &owner(), "one"), Some("processing"));
    }

    #[test]
    fn older_processing_job_is_not_hidden_by_newer_completed_job() {
        let position = SyncCursor { time: 100, local_id: 5, boundary_count: Some(1) };
        let mut value = with_jobs(vec![job("completed", position, "2026-11-06T01:00:00Z"), job("processing", position, "2026-11-06T00:00:00Z")]);
        value.devify.active_space_id = Some(1);
        assert_eq!(super::conversation_job_state(&value, &owner(), "one"), Some("processing"));
    }

    #[test]
    fn older_failed_job_is_not_hidden_by_newer_completed_job() {
        let position = SyncCursor { time: 100, local_id: 5, boundary_count: Some(1) };
        let mut value = with_jobs(vec![job("completed", position, "2026-11-06T01:00:00Z"), job("failed", position, "2026-11-06T00:00:00Z")]);
        value.devify.active_space_id = Some(1);
        assert_eq!(super::conversation_job_state(&value, &owner(), "one"), Some("history_failed"));
    }

    #[test]
    fn newer_failed_job_remains_failure() {
        let position = SyncCursor { time: 100, local_id: 5, boundary_count: Some(1) };
        let mut value = with_jobs(vec![job("completed", position, "2026-11-06T00:00:00Z"), job("failed", position, "2026-11-06T01:00:00Z")]);
        value.devify.active_space_id = Some(1);
        assert_eq!(super::conversation_job_state(&value, &owner(), "one"), Some("failed"));
    }

    #[test]
    fn automatic_cursor_advances_only_after_completion() {
        let position = SyncCursor { time: 100, local_id: 5, boundary_count: Some(1) };
        let mut item = job("processing", position, "2026-11-06T00:00:00Z");
        item.sync_advance = true;
        let mut value = with_jobs(vec![item]);
        super::apply_completed_uploads(&mut value);
        assert!(!value.sync.cursors.contains_key("acct"));
        value.upload_queue[0].status = "completed".into();
        super::apply_completed_uploads(&mut value);
        assert_eq!(value.sync.cursors["acct"]["one"].time, 100);
    }

    #[test]
    fn continuation_is_requested_only_after_completed_batch() {
        let position = SyncCursor { time: 100, local_id: 1, boundary_count: Some(1) };
        let mut item = job("processing", position, "");
        item.continue_sync = true;
        let mut value = with_jobs(vec![item]);
        super::apply_completed_uploads(&mut value);
        assert!(value.sync.continuation.is_empty());
        value.upload_queue[0].status = "failed".into();
        super::apply_completed_uploads(&mut value);
        assert!(value.sync.continuation.is_empty());
        value.upload_queue[0].status = "completed".into();
        super::apply_completed_uploads(&mut value);
        assert_eq!(value.sync.continuation["acct"]["one"].space_id, 1);
        assert!(!value.upload_queue[0].continue_sync);
    }

    #[test]
    fn continuation_stays_with_original_space_and_allowed_chat() {
        let mut value = with_jobs(vec![]);
        value.devify.active_space_id = Some(1);
        value.devify.base_url = "https://example.test".into();
        value.chat.allowed_usernames.insert("acct".into(), vec!["one".into()]);
        value.sync.continuation.entry("acct".into()).or_default().insert("one".into(), settings::ContinuationScope { space_id: 1, base_url: "https://example.test".into(), manual: Some(true) });
        assert_eq!(super::continuation_sessions(&value, "acct"), vec!["one".to_string()]);
        value.devify.active_space_id = Some(2);
        assert!(super::continuation_sessions(&value, "acct").is_empty());
        value.devify.active_space_id = Some(1);
        value.chat.allowed_usernames.clear();
        assert!(super::continuation_sessions(&value, "acct").is_empty());
        value.chat.allowed_usernames.insert("acct".into(), vec!["one".into()]);
        value.sync.continuation.get_mut("acct").unwrap().get_mut("one").unwrap().manual = Some(false);
        assert!(super::continuation_sessions(&value, "acct").is_empty());
        value.sync.enabled = true;
        assert_eq!(super::continuation_sessions(&value, "acct"), vec!["one".to_string()]);
    }

    #[test]
    fn voice_retry_is_kept_until_server_completes() {
        let voice = settings::VoiceRef { local_id: 1, time: 10, server_id: 2, source_shard: "message_0.db".into() };
        let mut item = job("processing", SyncCursor { time: 10, local_id: 1, boundary_count: Some(1) }, "");
        item.voice_processed.insert("one".into(), vec![voice.clone()]);
        let mut value = with_jobs(vec![item]);
        value.sync.pending_voice.entry("acct".into()).or_default().insert("one".into(), vec![voice]);
        super::apply_completed_uploads(&mut value);
        assert_eq!(value.sync.pending_voice["acct"]["one"].len(), 1);
        value.upload_queue[0].status = "failed".into();
        super::apply_completed_uploads(&mut value);
        assert_eq!(value.sync.pending_voice["acct"]["one"].len(), 1);
        value.upload_queue[0].status = "completed".into();
        super::apply_completed_uploads(&mut value);
        assert!(!value.sync.pending_voice.contains_key("acct"));
    }

    #[test]
    fn initial_history_baseline_waits_for_completion() {
        let cursor = SyncCursor { time: 300, local_id: 3, boundary_count: Some(1) };
        let mut item = job("processing", cursor, "2026-11-06T00:00:00Z");
        item.initial_baselines.insert("one".into(), cursor);
        let mut value = with_jobs(vec![item]);
        super::apply_completed_history_baselines(&mut value);
        assert!(!value.sync.cursors.contains_key("acct"));
        value.upload_queue[0].status = "completed".into();
        super::apply_completed_history_baselines(&mut value);
        assert_eq!(value.sync.cursors["acct"]["one"].time, 300);
    }

    #[test]
    fn initial_chunk_checkpoint_survives_failure_and_finishes_only_on_last_batch() {
        let baseline = SyncCursor { time: 300, local_id: 3, boundary_count: Some(1) };
        let first = SyncCursor { time: 100, local_id: 1, boundary_count: Some(1) };
        let mut item = job("processing", first, "2026-11-06T00:00:00Z");
        item.initial_chunks.insert("one".into(), settings::InitialChunk { since: Some(10), until: Some(200), baseline, position: first, final_chunk: false, space_id: 1, base_url: "https://example.test".into() });
        let mut value = with_jobs(vec![item]);
        super::apply_completed_uploads(&mut value);
        assert!(value.sync.initial_progress.is_empty());
        value.upload_queue[0].status = "failed".into();
        super::apply_completed_uploads(&mut value);
        assert!(value.sync.initial_progress.is_empty());
        assert!(!value.upload_queue[0].initial_chunks.is_empty());
        value.upload_queue[0].status = "completed".into();
        super::apply_completed_uploads(&mut value);
        assert_eq!(value.sync.initial_progress["acct"]["one"].position.time, 100);
        assert!(value.sync.cursors.get("acct").is_none());
        let mut last = job("completed", first, "2026-11-06T00:01:00Z");
        last.initial_chunks.insert("one".into(), settings::InitialChunk { since: Some(10), until: Some(200), baseline, position: first, final_chunk: true, space_id: 1, base_url: "https://example.test".into() });
        value.upload_queue.push(last);
        super::apply_completed_uploads(&mut value);
        assert!(!value.sync.initial_progress.contains_key("acct"));
        assert_eq!(value.sync.cursors["acct"]["one"].time, 300);
    }

    #[test]
    fn initial_batches_resume_after_restart_and_retry_without_skipping() {
        use crate::settings::MessageRef;
        let records: Vec<_> = (0..2300).map(|n| MessageRef { time: 100 + n / 2, local_id: n / 2, source_shard: format!("shard-{}", n % 2) }).collect();
        let baseline = SyncCursor { time: 2000, local_id: 9999, boundary_count: Some(1) };
        let mut value = with_jobs(vec![]);
        let mut sent = std::collections::HashSet::new();
        for expected in [1000, 1000, 300] {
            let position = value.sync.initial_progress.get("acct").and_then(|v| v.get("one")).map(|p| p.position);
            let batch = super::plan_sync_batch(&records, position, 1000).unwrap();
            assert_eq!(batch.messages.len(), expected);
            assert!(batch.messages.iter().all(|item| sent.insert(item.clone())));
            let last = batch.messages.last().unwrap();
            let boundary_count = batch.messages.iter().filter(|item| item.time == last.time).count();
            let position = SyncCursor { time: last.time, local_id: last.local_id, boundary_count: Some(boundary_count) };
            let mut item = job("failed", position, "");
            item.initial_chunks.insert("one".into(), settings::InitialChunk { since: Some(100), until: Some(1250), baseline, position, final_chunk: !batch.has_more, space_id: 1, base_url: "https://example.test".into() });
            value.upload_queue.push(item);
            super::apply_completed_uploads(&mut value);
            assert!(value.sync.cursors.get("acct").is_none());
            value.upload_queue.last_mut().unwrap().status = "completed".into();
            super::apply_completed_uploads(&mut value);
            value = serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
        }
        assert_eq!(sent.len(), records.len());
        assert!(value.sync.initial_progress.is_empty());
        assert_eq!(value.sync.cursors["acct"]["one"].time, baseline.time);
    }

    #[test]
    fn initial_progress_cannot_be_reused_for_another_space() {
        let chunk = settings::InitialChunk { since: None, until: Some(100), baseline: SyncCursor::default(), position: SyncCursor::default(), final_chunk: false, space_id: 1, base_url: "https://example.test".into() };
        assert!(super::initial_progress_matches(&chunk, 1, "https://example.test/"));
        assert!(!super::initial_progress_matches(&chunk, 2, "https://example.test"));
        assert!(!super::initial_progress_matches(&chunk, 1, "https://another.test"));
    }

    #[test]
    fn only_completed_jobs_advance_coverage() {
        let position = SyncCursor {
            boundary_count: None,
            time: 180,
            local_id: 9,
        };
        assert!(
            upload_coverage(&with_jobs(vec![job("processing", position, "")]), &owner()).is_empty()
        );
        assert!(upload_coverage(&with_jobs(vec![job("failed", position, "")]), &owner()).is_empty());
        let completed = upload_coverage(&with_jobs(vec![job("completed", position, "")]), &owner());
        assert_eq!(
            completed.get("one").map(|c| (c.time, c.local_id)),
            Some((180, 9))
        );
    }

    #[test]
    fn keeps_the_newest_position_and_ignores_other_accounts() {
        let older = SyncCursor {
            boundary_count: None,
            time: 100,
            local_id: 1,
        };
        let newer = SyncCursor {
            boundary_count: None,
            time: 200,
            local_id: 2,
        };
        let mut foreign = job("completed", newer, "");
        foreign.account_id = "other".into();
        let coverage = upload_coverage(
            &with_jobs(vec![
                job("completed", newer, ""),
                job("completed", older, ""),
                foreign,
            ]),
            &owner(),
        );
        assert_eq!(
            coverage.get("one").map(|c| (c.time, c.local_id)),
            Some((200, 2))
        );
    }

    #[test]
    fn jobs_without_coverage_do_not_infer_completion_time() {
        let mut legacy = job(
            "completed",
            SyncCursor::default(),
            "2026-10-01T10:00:00+00:00",
        );
        legacy.covered.clear();
        let coverage = upload_coverage(&with_jobs(vec![legacy]), &owner());
        assert!(coverage.is_empty());
    }

    #[test]
    fn modern_jobs_do_not_claim_conversations_they_skipped() {
        let mut modern = job(
            "completed",
            SyncCursor {
                boundary_count: None,
                time: 200,
                local_id: 2,
            },
            "2026-10-01T10:00:00+00:00",
        );
        // Another conversation was uploaded, this one was not part of the package.
        modern.scope.as_mut().unwrap().usernames = vec!["one".into(), "two".into()];
        let coverage = upload_coverage(&with_jobs(vec![modern]), &owner());
        assert_eq!(coverage.get("one").map(|c| c.time), Some(200));
        assert!(coverage.get("two").is_none());
    }
}

#[cfg(test)]
mod id_list_tests {
    use super::id_list;

    #[test]
    fn accepts_arrays_and_json_strings() {
        let array = serde_json::json!([22, 23]);
        assert_eq!(id_list(Some(&array)), vec![22, 23]);
        let text = serde_json::json!("[22, 23]");
        assert_eq!(id_list(Some(&text)), vec![22, 23]);
        let strings = serde_json::json!("[\"22\"]");
        assert_eq!(id_list(Some(&strings)), vec![22]);
        let empty = serde_json::json!("[]");
        assert!(id_list(Some(&empty)).is_empty());
        assert!(id_list(None).is_empty());
        assert!(id_list(Some(&serde_json::json!("not json"))).is_empty());
    }
}
