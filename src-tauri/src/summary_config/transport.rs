//! Conditional, identity-bound HTTP. No timer and no template fetch at task execution.
use super::*;
use crate::{
    devify_http::{RequestTrace, RetryPolicy},
    diagnostics, http_client,
};
use reqwest::{Method, StatusCode};
use tauri::AppHandle;

static UPDATE: tokio::sync::Mutex<Option<AuthRetry>> = tokio::sync::Mutex::const_new(None);
const BODY_LIMIT: usize = 2 * 1024 * 1024;
const AUTH_RETRY_SECONDS: i64 = 15;

/// Old credentials can briefly coalesce failed requests without blocking a new login.
pub(super) struct AuthRetry {
    generation: u64,
    retry_at: i64,
    message: String,
}

impl AuthRetry {
    pub(super) fn new(generation: u64, now: i64, message: String) -> Self {
        Self {
            generation,
            retry_at: now + AUTH_RETRY_SECONDS,
            message,
        }
    }

    pub(super) fn for_generation(&self, generation: u64, now: i64) -> Option<ConfigError> {
        (self.generation == generation && now < self.retry_at).then(|| ConfigError {
            message: self.message.clone(),
            retry_at: Some(self.retry_at),
        })
    }
}

enum FetchError {
    Authentication(String),
    Response(String),
}

impl From<String> for FetchError {
    fn from(message: String) -> Self {
        Self::Response(message)
    }
}

impl From<&str> for FetchError {
    fn from(message: &str) -> Self {
        Self::Response(message.into())
    }
}

async fn bytes(mut response: reqwest::Response) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "读取总结响应失败。")? {
        if body.len().saturating_add(chunk.len()) > BODY_LIMIT {
            return Err("总结服务响应过大。".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn still_current(data: &Path, expected: &Scope, token: &str, generation: u64) -> bool {
    scope(data).is_ok_and(|s| s == *expected)
        && crate::request_generation(data, &expected.server, token).ok() == Some(generation)
}
pub(super) fn commit(
    data: &Path,
    cache: &Cache,
    token: &str,
    generation: u64,
) -> Result<(), String> {
    // This does not change identity. A short shared lease allows ordinary backup
    // readers to continue; never take the exclusive login/account-change lock.
    let _identity = crate::UPLOAD_IDENTITY_LOCK
        .try_read()
        .map_err(|_| "登录状态正在切换，请稍后重试。")?;
    if !still_current(data, &cache.scope, token, generation) {
        return Err("登录账号或空间已改变，已丢弃旧总结配置。".into());
    }
    write(data, cache)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Refresh {
    Background,
    Forced,
    Missing,
}

pub(super) fn refresh_needed(
    cache: &Cache,
    observed: Option<&str>,
    now: i64,
    mode: Refresh,
) -> Result<bool, ConfigError> {
    if mode == Refresh::Missing && cache.config.is_some() {
        return Ok(false);
    }
    if cache.retry_at > now {
        let fallback = if cache.config.is_some() {
            "总结配置更新暂不可用，请稍后重试；已有配置仍可使用。"
        } else {
            MISSING_CATALOG
        };
        return Err(ConfigError {
            message: cache.last_error.clone().unwrap_or_else(|| fallback.into()),
            retry_at: Some(cache.retry_at),
        });
    }
    if cache.config.is_some() && observed != Some(cache.refresh_id.as_str()) {
        return Ok(false);
    }
    let interval = cache
        .config
        .as_ref()
        .map_or(21600, |c| c.min_check_interval_seconds.max(21600));
    Ok(mode == Refresh::Forced
        || cache.config.is_none()
        || now.saturating_sub(cache.checked_at) >= interval)
}

pub(crate) async fn refresh(app: &AppHandle, force: bool) -> Result<(), String> {
    let data = crate::app_data(app);
    let before = scope(&data)?;
    let mode = if force {
        Refresh::Forced
    } else {
        Refresh::Background
    };
    refresh_for(app, &data, &before, mode)
        .await
        .map_err(|error| error.message)
}

pub(super) async fn ensure(
    app: &AppHandle,
    data: &Path,
    before: &Scope,
) -> Result<(), ConfigError> {
    refresh_for(app, data, before, Refresh::Missing).await
}

async fn refresh_for(
    app: &AppHandle,
    data: &Path,
    before: &Scope,
    mode: Refresh,
) -> Result<(), ConfigError> {
    let queued_generation = desktop_credentials::generation()?;
    let observed = load(data, before)
        .filter(|c| c.config.is_some())
        .map(|c| c.refresh_id);
    let mut auth_retry = UPDATE.lock().await;
    if scope(data)? != *before {
        return Err("服务端、登录账号或空间已改变，请重试。".into());
    }
    let mut cache = load(data, before).unwrap_or(Cache {
        scope: before.clone(),
        config: None,
        etag: None,
        checked_at: 0,
        retry_at: 0,
        refresh_id: String::new(),
        last_error: None,
    });
    if !refresh_needed(
        &cache,
        observed.as_deref(),
        chrono::Utc::now().timestamp(),
        mode,
    )? {
        return Ok(());
    }
    if desktop_credentials::generation()? != queued_generation {
        return Err("服务端、登录账号或空间已改变，请重试。".into());
    }
    let token = crate::devify_access_token_async(data, &before.server)
        .await
        .map_err(String::from)?;
    let generation = crate::request_generation(data, &before.server, &token)?;
    if !still_current(data, before, &token, generation) {
        return Err("服务端、登录账号或空间已改变，请重试。".into());
    }
    let now = chrono::Utc::now().timestamp();
    if let Some(error) = auth_retry
        .as_ref()
        .and_then(|failure| failure.for_generation(generation, now))
    {
        return Err(error);
    }
    *auth_retry = None;
    let etag = cache.config.as_ref().and(cache.etag.as_deref());
    let result = fetch(app, before, etag, &token, generation).await;
    if !still_current(data, before, &token, generation) {
        return Err("登录账号或空间已改变，已丢弃旧总结配置。".into());
    }
    let now = chrono::Utc::now().timestamp();
    let result = match result {
        Ok(response) => Ok(response),
        Err(FetchError::Response(message)) => Err(message),
        Err(FetchError::Authentication(message)) => {
            let failure = AuthRetry::new(generation, now, message);
            let error = ConfigError {
                message: failure.message.clone(),
                retry_at: Some(failure.retry_at),
            };
            *auth_retry = Some(failure);
            return Err(error);
        }
    };
    let outcome = apply_fetch(&mut cache, result, now);
    if outcome.is_ok() {
        let mut id = [0u8; 16];
        getrandom::getrandom(&mut id).map_err(|_| "无法保存总结配置更新标识。")?;
        cache.refresh_id = hex::encode(id);
    }
    commit(data, &cache, &token, generation)?;
    outcome
}

pub(super) fn apply_fetch(
    cache: &mut Cache,
    result: Result<Option<(Catalog, Option<String>)>, String>,
    now: i64,
) -> Result<(), ConfigError> {
    let result = match result {
        Ok(Some((config, etag))) => {
            cache.config = Some(config);
            cache.etag = etag;
            Ok(())
        }
        Ok(None) if cache.config.is_some() => Ok(()),
        Ok(None) => Err("服务返回了未修改，但本机没有可用的总结配置。".into()),
        Err(error) => Err(error),
    };
    if let Err(message) = result {
        cache.retry_at = now
            + cache
                .config
                .as_ref()
                .map_or(900, |c| c.retry_after_seconds.max(900));
        cache.last_error = Some(message.clone());
        return Err(ConfigError {
            message,
            retry_at: Some(cache.retry_at),
        });
    }
    cache.checked_at = now;
    cache.retry_at = 0;
    cache.last_error = None;
    Ok(())
}

async fn fetch(
    app: &AppHandle,
    context: &Scope,
    etag: Option<&str>,
    token: &str,
    generation: u64,
) -> Result<Option<(Catalog, Option<String>)>, FetchError> {
    let data = crate::app_data(app);
    let mut request = http_client::api()
        .get(format!(
            "{}/api/v1/spaces/{}/chat-summary-config?schema_version=1",
            context.server, context.space_id
        ))
        .bearer_auth(token);
    if let Some(etag) = etag {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let response = request
        .send_traced_retrying(&data, diagnostics::Module::App, RetryPolicy::READ)
        .await
        .map_err(|error| {
            if crate::devify_unreachable(&error) {
                "无法连接总结服务，请检查网络。"
            } else {
                "无法读取总结服务响应，请重试。"
            }
        })?;
    let status = response.status();
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 512)
        .map(str::to_string);
    // A normal 304 has no JSON body.
    if status == StatusCode::NOT_MODIFIED {
        return Ok(decode_catalog(status, etag, &[], context)?);
    }
    // A non-JSON error page is still a permission or service error, not a bad catalog.
    let body = if status.is_success() {
        bytes(response).await?
    } else if matches!(
        status,
        StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST
    ) {
        bytes(response).await.unwrap_or_default()
    } else {
        Vec::new()
    };
    if !still_current(&data, context, token, generation) {
        return Err("登录账号或空间已改变。".into());
    }
    if status == StatusCode::UNAUTHORIZED {
        let payload = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        return Err(FetchError::Authentication(crate::devify_response_error(
            app,
            status,
            &payload,
            "无法读取总结配置。",
            &context.server,
            token,
            generation,
        )));
    }
    Ok(decode_catalog(status, etag, &body, context)?)
}

pub(super) fn decode_catalog(
    status: StatusCode,
    etag: Option<String>,
    body: &[u8],
    context: &Scope,
) -> Result<Option<(Catalog, Option<String>)>, String> {
    if status == StatusCode::NOT_MODIFIED {
        return Ok(None);
    }
    if !status.is_success() {
        let detail = || {
            serde_json::from_slice(body)
                .ok()
                .and_then(|payload| crate::devify_error_message(&payload))
        };
        return Err(if status == StatusCode::FORBIDDEN {
            "没有权限读取当前空间的总结配置。"
        } else if status == StatusCode::NOT_FOUND {
            if detail().as_deref().is_some_and(|text| {
                matches!(
                    text.trim_end_matches('.'),
                    "Space not found" | "No Space matches the given query"
                )
            }) {
                "当前空间不存在或无权访问，请重新选择空间。"
            } else {
                "当前服务不支持聊天总结配置，请联系管理员升级服务端。"
            }
        } else if status == StatusCode::BAD_REQUEST
            && detail().as_deref() == Some("Unsupported summary configuration schema.")
        {
            "总结配置版本暂不支持，请检查客户端和服务端版本。"
        } else if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
            "总结服务暂不可用，请稍后重试。"
        } else {
            "无法读取总结配置，请重试。"
        }
        .into());
    }
    let payload: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| "总结配置响应格式无效。")?;
    let config: Catalog = serde_json::from_value(payload.get("data").cloned().unwrap_or(payload))
        .map_err(|_| "总结配置格式不完整。")?;
    validate(&config, context)?;
    Ok(Some((config, etag)))
}

#[derive(Debug)]
pub(crate) struct CloudError {
    pub message: String,
    pub config_changed: bool,
}
impl From<String> for CloudError {
    fn from(message: String) -> Self {
        Self {
            message,
            config_changed: false,
        }
    }
}
impl From<&str> for CloudError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}
pub(crate) fn cloud_error(status: StatusCode, payload: &serde_json::Value) -> CloudError {
    let code = payload
        .get("data")
        .and_then(|data| data.get("error_code"))
        .and_then(|v| v.as_str());
    CloudError {
        config_changed: status == StatusCode::CONFLICT && code == Some("summary_config_changed"),
        message: format!(
            "HTTP {}：{}",
            status.as_u16(),
            crate::devify_error_text(payload, "总结请求失败，请稍后重试。")
        ),
    }
}

pub(crate) async fn cloud_request(
    app: &AppHandle,
    base: &str,
    token: &str,
    method: Method,
    path: &str,
    body: Option<&serde_json::Value>,
) -> Result<serde_json::Value, CloudError> {
    let data = crate::app_data(app);
    let generation = crate::request_generation(&data, base, token)?;
    let mut request = http_client::strict()
        .request(method.clone(), format!("{base}{path}"))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request
        .send_traced_retrying(
            &data,
            diagnostics::Module::App,
            RetryPolicy::for_method(&method),
        )
        .await
        .map_err(|_| CloudError::from("无法访问总结服务，请检查网络后重试。"))?;
    let status = response.status();
    let raw = bytes(response).await?;
    if crate::request_generation(&data, base, token).ok() != Some(generation) {
        return Err("登录状态已改变。".into());
    }
    let value: serde_json::Value =
        serde_json::from_slice(&raw).map_err(|_| CloudError::from("总结响应格式无效。"))?;
    if !status.is_success() {
        if status == StatusCode::UNAUTHORIZED {
            return Err(crate::devify_response_error(
                app,
                status,
                &value,
                "请重新登录。",
                base,
                token,
                generation,
            )
            .into());
        }
        return Err(cloud_error(status, &value));
    }
    Ok(value.get("data").cloned().unwrap_or(value))
}
