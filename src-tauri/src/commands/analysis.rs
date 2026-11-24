//! Event/report browsing and analysis preferences exposed through native IPC.
use crate::devify_http::{RequestTrace, RetryPolicy};
use crate::{
    app_data, capabilities, devify_access_token_async, devify_json_request, devify_network_message,
    devify_response_error, diagnostics, entitlements, http_client, request_generation, settings,
    validate_devify_space,
};
use tauri::AppHandle;

fn analysis_list_query(
    kind: &str,
    page: u32,
    page_size: Option<u32>,
    conversation_id: Option<i64>,
    viewer_relevance: Option<&str>,
    include_counts: bool,
    search_query: Option<&str>,
    completion_status: Option<&str>,
) -> Result<String, String> {
    if page_size.is_some_and(|size| !(1..=100).contains(&size)) {
        return Err("分页大小无效。".into());
    }
    if viewer_relevance.is_some_and(|value| !matches!(value, "my_todo" | "related" | "other"))
        || (kind != "events" && (viewer_relevance.is_some() || include_counts))
    {
        return Err("待办个人分类无效。".into());
    }
    if completion_status
        .is_some_and(|value| kind != "events" || !matches!(value, "unfinished" | "done"))
    {
        return Err("待办完成状态无效。".into());
    }
    let mut query = format!(
        "page={}&page_size={}",
        page.max(1),
        page_size.unwrap_or(100)
    );
    if let Some(id) = conversation_id {
        query.push_str(&format!("&conversation_id={id}"));
    }
    if let Some(value) = viewer_relevance {
        query.push_str(&format!("&viewer_relevance={value}"));
    }
    if let Some(value) = completion_status {
        query.push_str(&format!("&status={value}"));
    }
    if include_counts {
        query.push_str("&include_relevance_counts=true");
    }
    if let Some(keyword) = search_query {
        if kind != "events" {
            return Err("报告不支持待办搜索。".into());
        }
        let keyword = keyword.trim();
        if keyword.chars().count() > 200 {
            return Err("搜索内容最多输入 200 个字符。".into());
        }
        if !keyword.is_empty() {
            query.push('&');
            query.push_str(
                &url::form_urlencoded::Serializer::new(String::new())
                    .append_pair("q", keyword)
                    .finish(),
            );
        }
    }
    Ok(query)
}

/// Create a report snapshot on the server. The server generates the body asynchronously
/// (`202` + status `queued`); listing again picks up the finished report.
#[tauri::command]
pub(crate) async fn devify_report_create(
    app: AppHandle,
    space_id: String,
    title: String,
    period_start: Option<String>,
    period_end: Option<String>,
) -> Result<serde_json::Value, String> {
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
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
pub(crate) async fn devify_analysis_list(
    app: AppHandle,
    kind: String,
    space_id: String,
    page: Option<u32>,
    conversation_id: Option<i64>,
    page_size: Option<u32>,
    viewer_relevance: Option<String>,
    include_relevance_counts: Option<bool>,
    search_query: Option<String>,
    completion_status: Option<String>,
) -> Result<serde_json::Value, String> {
    if !matches!(kind.as_str(), "events" | "reports") {
        return Err("不支持的服务端分析类型。".into());
    }
    if space_id.trim().is_empty() {
        return Err("请先选择 空间。".into());
    }
    if conversation_id.is_some_and(|id| id <= 0) || (conversation_id.is_some() && kind != "events")
    {
        return Err("会话标识无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    if settings.devify.active_space_id.as_deref() != Some(space_id.as_str()) {
        return Err("空间已改变，请重新选择后再同步。".into());
    }
    let resource = if kind == "events" {
        "events"
    } else {
        "reports"
    };
    let first_page = page.unwrap_or(1).max(1);
    analysis_list_query(
        &kind,
        first_page,
        page_size,
        conversation_id,
        viewer_relevance.as_deref(),
        include_relevance_counts.unwrap_or(false),
        search_query.as_deref(),
        completion_status.as_deref(),
    )?;
    let client = http_client::api();
    let token = devify_access_token_async(&app_data(&app), &settings.devify.base_url).await?;
    let generation = request_generation(&app_data(&app), &settings.devify.base_url, &token)?;
    let mut all_items = Vec::new();
    let mut current_page = first_page;
    let mut total = None;
    for _ in 0..100 {
        let query = analysis_list_query(
            &kind,
            current_page,
            page_size,
            conversation_id,
            viewer_relevance.as_deref(),
            include_relevance_counts.unwrap_or(false),
            search_query.as_deref(),
            completion_status.as_deref(),
        )?;
        let url = format!(
            "{}/api/v1/spaces/{space_id}/{resource}?{query}",
            settings.devify.resolved_base_url()?
        );
        let response = client
            .get(url)
            .bearer_auth(&token)
            .send_traced_retrying(&app_data(&app), diagnostics::Module::App, RetryPolicy::READ)
            .await
            .map_err(|e| devify_network_message(&format!("同步服务端{kind}失败"), &e))?;
        let status = response.status();
        let payload: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("读取服务端{kind}响应失败：{e}"))?;
        if !status.is_success() {
            let detail = devify_response_error(
                &app,
                status,
                &payload,
                "服务端拒绝了请求",
                &settings.devify.base_url,
                &token,
                generation,
            );
            return Err(format!("同步服务端{kind}失败：{detail}"));
        }
        let data = payload.get("data").cloned().unwrap_or(payload);
        // Explicit paging is used by the personal event browser. Preserve the
        // server's counts and next-page metadata. Existing callers still aggregate.
        if page_size.is_some() {
            return Ok(data);
        }
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

#[tauri::command]
pub(crate) async fn devify_analysis_preferences_get(
    app: AppHandle,
    space_id: String,
) -> Result<serde_json::Value, String> {
    validate_devify_space(&settings::load(&app_data(&app))?, &space_id)?;
    let data = devify_json_request(
        &app,
        reqwest::Method::GET,
        format!("/api/v1/spaces/{space_id}/workflow-config"),
        None,
    )
    .await?;
    data.get("chat_analysis_preferences")
        .cloned()
        .ok_or_else(|| "服务端尚不支持分析偏好设置，请更新服务端。".to_string())
}

#[tauri::command]
pub(crate) async fn devify_analysis_preferences_save(
    app: AppHandle,
    space_id: String,
    preferences: serde_json::Value,
) -> Result<serde_json::Value, String> {
    validate_devify_space(&settings::load(&app_data(&app))?, &space_id)?;
    let body = serde_json::json!({"chat_analysis_preferences": preferences});
    let data = devify_json_request(
        &app,
        reqwest::Method::PATCH,
        format!("/api/v1/spaces/{space_id}/workflow-config"),
        Some(body),
    )
    .await?;
    data.get("chat_analysis_preferences")
        .cloned()
        .ok_or_else(|| "服务端未确认分析偏好，请更新服务端。".to_string())
}

#[tauri::command]
pub(crate) async fn devify_analysis_manual_start(
    app: AppHandle,
    space_id: String,
    conversation_id: i64,
) -> Result<serde_json::Value, String> {
    validate_devify_space(&settings::load(&app_data(&app))?, &space_id)?;
    if conversation_id <= 0 {
        return Err("会话标识无效。".into());
    }
    devify_json_request(
        &app,
        reqwest::Method::POST,
        format!("/api/v1/spaces/{space_id}/analysis-jobs/manual"),
        Some(serde_json::json!({"conversation_id": conversation_id})),
    )
    .await
}

#[tauri::command]
pub(crate) async fn devify_analysis_detail(
    app: AppHandle,
    kind: String,
    space_id: String,
    item_id: i64,
) -> Result<serde_json::Value, String> {
    if !matches!(kind.as_str(), "events" | "reports") || item_id <= 0 {
        return Err("服务端分析对象无效。".into());
    }
    let settings = settings::load(&app_data(&app))?;
    validate_devify_space(&settings, &space_id)?;
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

#[cfg(test)]
mod event_query_tests {
    use super::analysis_list_query;

    #[test]
    fn personal_filters_counts_and_pages_are_sent_independently_of_status() {
        assert_eq!(
            analysis_list_query("events", 1, Some(50), None, None, true, None, None).unwrap(),
            "page=1&page_size=50&include_relevance_counts=true"
        );
        assert_eq!(
            analysis_list_query(
                "events",
                1,
                Some(50),
                None,
                Some("my_todo"),
                false,
                None,
                None
            )
            .unwrap(),
            "page=1&page_size=50&viewer_relevance=my_todo"
        );
        assert_eq!(
            analysis_list_query(
                "events",
                2,
                Some(50),
                None,
                Some("related"),
                false,
                None,
                None
            )
            .unwrap(),
            "page=2&page_size=50&viewer_relevance=related"
        );
        assert_eq!(
            analysis_list_query("events", 1, None, Some(12), None, false, None, None).unwrap(),
            "page=1&page_size=100&conversation_id=12"
        );
        assert_eq!(
            analysis_list_query("reports", 1, None, None, None, false, None, None).unwrap(),
            "page=1&page_size=100"
        );
        for invalid in ["all", "open", "done", "my_todo&status=done"] {
            assert!(analysis_list_query(
                "events",
                1,
                Some(50),
                None,
                Some(invalid),
                false,
                None,
                None
            )
            .is_err());
        }
        assert!(
            analysis_list_query("reports", 1, None, None, Some("related"), false, None, None)
                .is_err()
        );
        assert!(analysis_list_query("reports", 1, None, None, None, true, None, None).is_err());
        assert!(analysis_list_query("events", 1, Some(0), None, None, false, None, None).is_err());
        assert!(
            analysis_list_query("events", 1, Some(101), None, None, false, None, None).is_err()
        );
    }

    #[test]
    fn completion_filters_compose_with_category_search_and_pagination() {
        let query = analysis_list_query(
            "events",
            2,
            Some(50),
            None,
            Some("my_todo"),
            false,
            Some("release"),
            Some("unfinished"),
        )
        .unwrap();
        assert_eq!(
            query,
            "page=2&page_size=50&viewer_relevance=my_todo&status=unfinished&q=release"
        );
        let query = analysis_list_query(
            "events",
            1,
            Some(50),
            None,
            Some("other"),
            false,
            None,
            Some("done"),
        )
        .unwrap();
        assert!(query.ends_with("viewer_relevance=other&status=done"));
        for invalid in ["all", "open", "done&status=unfinished"] {
            assert!(analysis_list_query(
                "events",
                1,
                Some(50),
                None,
                None,
                false,
                None,
                Some(invalid)
            )
            .is_err());
        }
        assert!(
            analysis_list_query("reports", 1, None, None, None, false, None, Some("done")).is_err()
        );
    }

    #[test]
    fn keyword_search_is_bounded_encoded_and_event_only() {
        let query = analysis_list_query(
            "events",
            2,
            Some(50),
            None,
            Some("related"),
            false,
            Some("  中文 A&B +#?  "),
            None,
        )
        .unwrap();
        let pairs: std::collections::HashMap<_, _> = url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        assert_eq!(pairs["q"], "中文 A&B +#?");
        assert_eq!(pairs["viewer_relevance"], "related");
        assert_eq!(pairs["page"], "2");
        assert_eq!(pairs.len(), 4);
        assert_eq!(
            analysis_list_query("events", 1, Some(50), None, None, false, Some(" \n "), None)
                .unwrap(),
            "page=1&page_size=50"
        );
        assert!(analysis_list_query(
            "events",
            1,
            Some(50),
            None,
            None,
            false,
            Some(&"😀".repeat(200)),
            None
        )
        .is_ok());
        assert!(analysis_list_query(
            "events",
            1,
            Some(50),
            None,
            None,
            false,
            Some(&"中".repeat(201)),
            None
        )
        .is_err());
        assert!(
            analysis_list_query("reports", 1, None, None, None, false, Some("test"), None).is_err()
        );
    }
}
