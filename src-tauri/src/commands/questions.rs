//! Space-scoped questions and answers exposed through native IPC.
use crate::{
    app_data, capabilities, devify_json_request, entitlements, settings, validate_devify_space,
};
use tauri::AppHandle;

#[tauri::command]
pub(crate) async fn devify_questions(
    app: AppHandle,
    space_id: String,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
    devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/spaces/{space_id}/questions"),
        None,
    )
    .await
}

#[tauri::command]
pub(crate) async fn devify_question_detail(
    app: AppHandle,
    space_id: String,
    question_id: i64,
) -> Result<serde_json::Value, String> {
    if question_id <= 0 {
        return Err("问答标识无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
    devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/spaces/{space_id}/questions/{question_id}"),
        None,
    )
    .await
}

#[tauri::command]
pub(crate) async fn devify_question_ask(
    app: AppHandle,
    space_id: String,
    question: String,
    thread_id: Option<i64>,
    since: Option<String>,
    until: Option<String>,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
    entitlements::require(
        &app_data(&app),
        &settings.devify.base_url,
        capabilities::CLOUD_ASK,
    )?;
    let question = question.trim();
    if question.is_empty() {
        return Err("请输入问题。".into());
    }
    let mut body = serde_json::json!({"question": question});
    if let Some(thread_id) = thread_id {
        if thread_id <= 0 {
            return Err("问答会话无效。".into());
        }
        body["thread_id"] = thread_id.into();
    }
    if let Some(since) = since
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        body["since"] = since.into();
    }
    if let Some(until) = until
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        body["until"] = until.into();
    }
    devify_json_request(
        &app,
        reqwest::Method::POST,
        format!("/api/v1/spaces/{space_id}/questions"),
        Some(body),
    )
    .await
}
