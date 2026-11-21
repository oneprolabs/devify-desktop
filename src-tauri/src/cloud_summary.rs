//! The control server owns summary jobs and results; local tools are independent.
use crate::{app_data, devify_json_request, settings, validate_devify_space};
use serde_json::Value;
use tauri::AppHandle;

pub(crate) fn validate_source(
    app: &AppHandle,
    space: &str,
    account: &str,
    username: &str,
) -> Result<String, String> {
    let data = app_data(app);
    let settings = settings::load(&data)?;
    validate_devify_space(&settings, space)?;
    let allowed = if crate::whatsapp::is_account(account) {
        crate::whatsapp_allowed_usernames(app, account)?
    } else {
        crate::allowed_usernames(app, account)?
    };
    if !allowed.contains(username) {
        return Err("该会话不在当前导入范围内。".into());
    }
    Ok(if crate::whatsapp::is_account(account) {
        "whatsapp"
    } else {
        "wechat"
    }
    .into())
}

#[tauri::command]
pub async fn cloud_summary_list(
    app: AppHandle,
    space_id: String,
    account_id: String,
    username: String,
    page: Option<u32>,
) -> Result<Value, String> {
    let platform = validate_source(&app, &space_id, &account_id, &username)?;
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("platform", &platform)
        .append_pair("source_namespace", &account_id)
        .append_pair("source_conversation_id", &username)
        .append_pair("page", &page.unwrap_or(1).clamp(1, 1000).to_string())
        .finish();
    let mut result = devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/spaces/{space_id}/chat-summaries?{query}"),
        None,
    )
    .await
    .unwrap_or_else(
        |_| serde_json::json!({"list":[],"pagination":{"total":0,"next":null},"offline":true}),
    );
    if let Some(rows) = result["list"].as_array() {
        crate::summary_jobs::sync_cloud_records(&app, rows);
    }
    result["local"] = crate::summary_jobs::records(&app, &account_id, &username)?.into();
    Ok(result)
}

#[tauri::command]
pub async fn cloud_summary_action(
    app: AppHandle,
    space_id: String,
    summary_id: i64,
    action: String,
) -> Result<Value, String> {
    validate_devify_space(&settings::load(&app_data(&app))?, &space_id)?;
    if summary_id <= 0 {
        return Err("总结标识无效。".into());
    }
    let method = match action.as_str() {
        "retry" => reqwest::Method::POST,
        "delete" => reqwest::Method::DELETE,
        _ => return Err("总结操作无效。".into()),
    };
    // The server rechecks the initiating user, space and entitlement on every action.
    let result = devify_json_request(
        &app,
        method,
        format!("/api/v1/spaces/{space_id}/chat-summaries/{summary_id}"),
        None,
    )
    .await?;
    if action == "delete" {
        crate::summary_jobs::forget_remote(&app, summary_id)?;
    }
    Ok(result)
}
