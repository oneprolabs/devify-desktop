//! Conditional, identity-bound HTTP. No timer and no template fetch at task execution.
use super::*;
use crate::{
    devify_http::{RequestTrace, RetryPolicy},
    diagnostics, http_client,
};
use reqwest::{Method, StatusCode};
use tauri::AppHandle;

static UPDATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
const BODY_LIMIT: usize = 2 * 1024 * 1024;

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

pub(super) fn refresh_needed(
    cache: &Cache,
    observed: Option<&str>,
    now: i64,
    force: bool,
) -> Result<bool, String> {
    if cache.retry_at > now {
        return Err("总结配置更新暂不可用，请稍后重试；已有配置仍可使用。".into());
    }
    if cache.config.is_some() && observed != Some(cache.refresh_id.as_str()) {
        return Ok(false);
    }
    let interval = cache
        .config
        .as_ref()
        .map_or(21600, |c| c.min_check_interval_seconds.max(21600));
    Ok(force || cache.config.is_none() || now.saturating_sub(cache.checked_at) >= interval)
}

pub(crate) async fn refresh(app: &AppHandle, force: bool) -> Result<(), String> {
    let data = crate::app_data(app);
    let before = scope(&data)?;
    let observed = load(&data, &before)
        .filter(|c| c.config.is_some())
        .map(|c| c.refresh_id);
    let _single = UPDATE.lock().await;
    if scope(&data)? != before {
        return Err("登录账号或空间已改变。".into());
    }
    let mut cache = load(&data, &before).unwrap_or(Cache {
        scope: before.clone(),
        config: None,
        etag: None,
        checked_at: 0,
        retry_at: 0,
        refresh_id: String::new(),
    });
    let now = chrono::Utc::now().timestamp();
    if !refresh_needed(&cache, observed.as_deref(), now, force)? {
        return Ok(());
    }
    let token = crate::devify_access_token_async(&data, &before.server).await?;
    let generation = crate::request_generation(&data, &before.server, &token)?;
    if !still_current(&data, &before, &token, generation) {
        return Err("登录账号或空间已改变。".into());
    }
    let result = fetch(app, &before, cache.etag.as_deref(), &token, generation).await;
    if !still_current(&data, &before, &token, generation) {
        return Err("登录账号或空间已改变，已丢弃旧总结配置。".into());
    }
    match result {
        Ok(Some((config, etag))) => {
            cache.config = Some(config);
            cache.etag = etag;
        }
        Ok(None) if cache.config.is_some() => {}
        Ok(None) => return Err("总结配置尚未初始化，无法使用缓存响应。".into()),
        Err(error) => {
            cache.retry_at = now
                + cache
                    .config
                    .as_ref()
                    .map_or(900, |c| c.retry_after_seconds.max(900));
            commit(&data, &cache, &token, generation)?;
            return Err(error);
        }
    }
    cache.checked_at = chrono::Utc::now().timestamp();
    cache.retry_at = 0;
    let mut id = [0u8; 16];
    getrandom::getrandom(&mut id).map_err(|_| "无法保存总结配置更新标识。")?;
    cache.refresh_id = hex::encode(id);
    commit(&data, &cache, &token, generation)
}

async fn fetch(
    app: &AppHandle,
    context: &Scope,
    etag: Option<&str>,
    token: &str,
    generation: u64,
) -> Result<Option<(Catalog, Option<String>)>, String> {
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
        .map_err(|_| "无法初始化总结方式，请检查网络后重试。")?;
    let status = response.status();
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 512)
        .map(str::to_string);
    // A normal 304 has no JSON body.
    if status == StatusCode::NOT_MODIFIED {
        return decode_catalog(status, etag, &[], context);
    }
    let body = bytes(response).await?;
    if !still_current(&data, context, token, generation) {
        return Err("登录账号或空间已改变。".into());
    }
    if status == StatusCode::UNAUTHORIZED {
        let payload = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        return Err(crate::devify_response_error(
            app,
            status,
            &payload,
            "无法初始化总结方式。",
            &context.server,
            token,
            generation,
        ));
    }
    decode_catalog(status, etag, &body, context)
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
    if status == StatusCode::NOT_FOUND {
        return Err("当前服务暂不支持新的总结方式，请升级服务端后重新初始化。".into());
    }
    let payload: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| "总结配置响应格式无效。")?;
    if !status.is_success() {
        return Err(crate::devify_error_text(&payload, "无法初始化总结方式。"));
    }
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
