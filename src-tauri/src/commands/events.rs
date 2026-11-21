//! Event progress notes, editing and relations exposed through native IPC.
use crate::{app_data, devify_json_request, settings, validate_devify_space};
use tauri::AppHandle;

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
pub(crate) async fn devify_event_notes(
    app: AppHandle,
    space_id: String,
    event_id: i64,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
    let event = devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/events/{event_id}"),
        None,
    )
    .await?;
    if event.get("space_id").and_then(serde_json::Value::as_str) != Some(space_id.as_str()) {
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
pub(crate) async fn devify_event_note_remove(
    app: AppHandle,
    space_id: String,
    event_id: i64,
    note_id: i64,
) -> Result<serde_json::Value, String> {
    if event_id <= 0 || note_id <= 0 {
        return Err("进展记录无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
    let event = devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/events/{event_id}"),
        None,
    )
    .await?;
    if event.get("space_id").and_then(serde_json::Value::as_str) != Some(space_id.as_str()) {
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
pub(crate) async fn devify_event_note_create(
    app: AppHandle,
    space_id: String,
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
    validate_devify_space(&settings, &space_id)?;
    let event = devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/events/{event_id}"),
        None,
    )
    .await?;
    if event.get("space_id").and_then(serde_json::Value::as_str) != Some(space_id.as_str()) {
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
pub(crate) async fn devify_event_note_assign(
    app: AppHandle,
    space_id: String,
    event_id: i64,
    note_id: i64,
) -> Result<serde_json::Value, String> {
    if event_id <= 0 || note_id <= 0 {
        return Err("进展记录无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
    let event = devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/events/{event_id}"),
        None,
    )
    .await?;
    if event.get("space_id").and_then(serde_json::Value::as_str) != Some(space_id.as_str()) {
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
pub(crate) async fn devify_event_update(
    app: AppHandle,
    space_id: String,
    event_id: i64,
    title: Option<String>,
    content: Option<String>,
    status: Option<String>,
) -> Result<serde_json::Value, String> {
    if event_id <= 0 {
        return Err("事件无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
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
pub(crate) async fn devify_event_relations(
    app: AppHandle,
    space_id: String,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
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
pub(crate) async fn devify_event_relation_create(
    app: AppHandle,
    space_id: String,
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
    validate_devify_space(&settings, &space_id)?;
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
pub(crate) async fn devify_event_relation_delete(
    app: AppHandle,
    space_id: String,
    relation_id: i64,
) -> Result<serde_json::Value, String> {
    if relation_id <= 0 {
        return Err("关联记录无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
    devify_json_request(
        &app,
        reqwest::Method::DELETE,
        format!("/api/v1/event-relations/{relation_id}"),
        None,
    )
    .await
}

#[cfg(test)]
#[path = "../tests/id_list.rs"]
mod id_list_tests;
