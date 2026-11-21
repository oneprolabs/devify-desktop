//! Topic subscriptions, original notes and scheduled report plans.
use crate::{app_data, devify_json_request, report_plan, settings, validate_devify_space};
use tauri::AppHandle;

#[tauri::command]
pub(crate) async fn devify_topics(
    app: AppHandle,
    space_id: String,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
    devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/spaces/{space_id}/topics"),
        None,
    )
    .await
}

#[tauri::command]
pub(crate) async fn devify_topic_create(
    app: AppHandle,
    space_id: String,
    title: String,
    instruction: String,
    period: String,
    time_local: Option<String>,
    timezone: Option<String>,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
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
pub(crate) async fn devify_topic_detail(
    app: AppHandle,
    space_id: String,
    topic_id: i64,
) -> Result<serde_json::Value, String> {
    if topic_id <= 0 {
        return Err("主题标识无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
    let topic = devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/topics/{topic_id}"),
        None,
    )
    .await?;
    if topic
        .get("space_id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|topic_space_id| topic_space_id != space_id)
    {
        return Err("主题不属于当前 空间。".into());
    }
    Ok(topic)
}

#[tauri::command]
pub(crate) async fn devify_note_detail(
    app: AppHandle,
    space_id: String,
    note_id: i64,
) -> Result<serde_json::Value, String> {
    if note_id <= 0 {
        return Err("原文标识无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
    let note = devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/notes/{note_id}"),
        None,
    )
    .await?;
    if note
        .get("space_id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|note_space_id| note_space_id != space_id)
    {
        return Err("原文不属于当前 空间。".into());
    }
    Ok(note)
}

#[tauri::command]
pub(crate) async fn devify_topic_update(
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
pub(crate) async fn devify_daily_plan_update(
    app: AppHandle,
    space_id: String,
    topic_id: i64,
    enabled: Option<bool>,
    time_local: Option<String>,
) -> Result<serde_json::Value, String> {
    devify_report_plan_update(app, space_id, topic_id, "day".into(), enabled, time_local).await
}

#[tauri::command]
pub(crate) async fn devify_report_plan_update(
    app: AppHandle,
    space_id: String,
    topic_id: i64,
    period: String,
    enabled: Option<bool>,
    time_local: Option<String>,
) -> Result<serde_json::Value, String> {
    let topic = devify_topic_detail(app.clone(), space_id.clone(), topic_id).await?;
    let body = report_plan::update_body(&topic, &period, enabled, time_local.as_deref())?;
    let current = settings::load(&app_data(&app))?;
    validate_devify_space(&current, &space_id)?;
    devify_json_request(
        &app,
        reqwest::Method::PATCH,
        format!("/api/v1/topics/{topic_id}"),
        Some(body),
    )
    .await
}

#[tauri::command]
pub(crate) async fn devify_topic_delete(app: AppHandle, topic_id: i64) -> Result<(), String> {
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
