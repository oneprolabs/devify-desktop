//! Explicit cloud fallback, paged conversation reads and lazy cloud images.
use crate::{
    app_data, config_runtime, devify_access_token_async, devify_json_request,
    devify_network_message, expected_source_type, http_client, request_generation, settings,
    validate_devify_space,
};
use base64::Engine as _;
use tauri::AppHandle;

#[tauri::command]
pub(crate) async fn devify_conversations(
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
pub(crate) async fn devify_conversation_detail(
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
pub(crate) async fn devify_conversation_messages(
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
    let mut path =
        format!(
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
pub(crate) async fn devify_message_detail(
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

#[tauri::command]
pub(crate) async fn chat_remote_lookup(
    app: AppHandle,
    account_id: String,
    username: String,
) -> Result<serde_json::Value, String> {
    let data = app_data(&app);
    let s = settings::load(&data)?;
    let owner = config_runtime::local_owner(&data, &s, &account_id)?;
    if !config_runtime::browse_names(&s, &account_id).contains(&username) {
        return Err("该会话不在所选范围内。".into());
    }
    let path = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query
            .append_pair("platform", expected_source_type(&account_id))
            .append_pair("source_account_id", &account_id)
            .append_pair("source_conversation_id", &username);
        format!(
            "/api/v1/spaces/{}/desktop/conversation?{}",
            owner.space_id,
            query.finish()
        )
    };
    devify_json_request(&app, reqwest::Method::GET, path, None).await
}

#[tauri::command]
pub(crate) async fn chat_remote_image(
    app: AppHandle,
    space_id: String,
    item_id: Option<i64>,
    sha256: Option<String>,
) -> Result<String, String> {
    static IMAGES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let _budget = IMAGES.acquire().await.map_err(|e| e.to_string())?;
    let data = app_data(&app);
    let s = settings::load(&data)?;
    validate_devify_space(&s, &space_id)?;
    let media_path = if let Some(hash) = sha256 {
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("图片摘要无效。".into());
        }
        format!("/api/v1/spaces/{space_id}/chat-media/{hash}")
    } else if let Some(id) = item_id.filter(|id| *id > 0) {
        format!("/api/v1/spaces/{space_id}/media/{id}")
    } else {
        return Err("图片标识无效。".into());
    };
    let base = s.devify.resolved_base_url()?;
    let token = devify_access_token_async(&data, &base).await?;
    let generation = request_generation(&data, &base, &token)?;
    let mut response = http_client::api()
        .get(format!("{base}{media_path}"))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| devify_network_message("读取图片失败", &e))?;
    if !response.status().is_success() {
        return Err(format!("读取图片失败：HTTP {}", response.status()));
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .to_string();
    if !matches!(
        mime.as_str(),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) {
        return Err("当前媒体没有可显示的图片。".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if bytes.len().saturating_add(chunk.len()) > 8 * 1024 * 1024 {
            return Err("图片较大，请查看原记录。".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if request_generation(&data, &base, &token).ok() != Some(generation) {
        return Err("登录状态已改变。".into());
    }
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}
