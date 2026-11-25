//! Fixed, read-only cloud resources. Credentials stay inside the desktop request layer.
use crate::{
    devify_http::{RequestTrace, RetryPolicy},
    entitlements, http_client,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug)]
pub(super) struct Error {
    pub status: u16,
    pub message: String,
}
impl From<String> for Error {
    fn from(message: String) -> Self {
        Self {
            status: 400,
            message,
        }
    }
}
impl From<&str> for Error {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}
impl Error {
    pub fn json(&self) -> Value {
        json!({"error":self.message,"status":self.status})
    }
}
#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Args {
    pub space_id: Option<String>,
    pub id: Option<i64>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub viewer_relevance: Option<String>,
    pub status: Option<String>,
    pub q: Option<String>,
}
impl Args {
    fn path(&self, operation: &str, space: &str) -> Result<String, Error> {
        if self.page == Some(0) || self.page_size.is_some_and(|s| !(1..=100).contains(&s)) {
            return Err("page 必须大于 0；page_size 范围为 1–100。".into());
        }
        let list = matches!(operation, "todos" | "reports" | "topics");
        let detail = matches!(operation, "todo" | "report" | "topic" | "topic-reports");
        if !list && !detail {
            return Err("不支持的云端只读操作。".into());
        }
        if (list && self.id.is_some()) || (detail && self.id.is_none_or(|id| id <= 0)) {
            return Err("详情需要正整数 id；列表不接受 id。".into());
        }
        if detail && (self.page.is_some() || self.page_size.is_some()) {
            return Err("该详情入口不支持分页；主题报告最多返回最近 50 份。".into());
        }
        if operation != "todos" && (self.viewer_relevance.is_some() || self.q.is_some()) {
            return Err("个人分类和关键词仅支持待办列表。".into());
        }
        if self.status.is_some() && !matches!(operation, "todos" | "reports") {
            return Err("该入口不支持状态筛选。".into());
        }
        if self
            .viewer_relevance
            .as_deref()
            .is_some_and(|v| !matches!(v, "my_todo" | "related" | "other"))
        {
            return Err("个人分类无效。".into());
        }
        if let Some(status) = self.status.as_deref() {
            let valid = if operation == "todos" {
                matches!(status, "unfinished" | "done")
            } else {
                matches!(status, "queued" | "ready" | "partial" | "failed")
            };
            if !valid {
                return Err("状态筛选无效；全部状态请省略 status。".into());
            }
        }
        if self.q.as_ref().is_some_and(|q| q.chars().count() > 200) {
            return Err("搜索内容最多 200 个字符。".into());
        }
        if detail {
            let resource = match operation {
                "todo" => "events",
                "report" => "reports",
                _ => "topics",
            };
            let suffix = if operation == "topic-reports" {
                "/reports"
            } else {
                ""
            };
            return Ok(format!("/api/v1/{resource}/{}{suffix}", self.id.unwrap()));
        }
        let resource = match operation {
            "todos" => "events",
            _ => operation,
        };
        let path = format!("/api/v1/spaces/{space}/{resource}");
        // Topics currently return an unpaginated array. Page it locally, never invent server filters.
        if operation == "topics" {
            return Ok(path);
        }
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query
            .append_pair("page", &self.page.unwrap_or(1).to_string())
            .append_pair("page_size", &self.page_size.unwrap_or(25).to_string());
        if let Some(v) = self.viewer_relevance.as_deref() {
            query.append_pair("viewer_relevance", v);
        }
        if let Some(v) = self.status.as_deref() {
            query.append_pair("status", v);
        }
        if let Some(v) = self.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            query.append_pair("q", v);
        }
        if operation == "todos" {
            query.append_pair("include_relevance_counts", "true");
        }
        Ok(format!("{path}?{}", query.finish()))
    }
}

fn scope(data: &Path) -> Result<crate::summary_config::Scope, Error> {
    let scope = crate::summary_config::scope(data).map_err(|_| Error {
        status: 401,
        message: "请先登录并选择空间，确认账号权限已初始化。".into(),
    })?;
    if !entitlements::grants(data, &scope.server, crate::capabilities::BRIDGE_MCP)
        && !entitlements::agent_access(data, &scope.server)
    {
        return Err(Error {
            status: 403,
            message: "当前账户未开通本机智能体读取能力。".into(),
        });
    }
    Ok(scope)
}
fn ensure_current(
    data: &Path,
    expected: &crate::summary_config::Scope,
    token: &str,
    generation: u64,
) -> Result<(), Error> {
    if scope(data).is_ok_and(|s| s == *expected)
        && crate::request_generation(data, &expected.server, token).ok() == Some(generation)
    {
        Ok(())
    } else {
        Err("登录账号、服务、权限或空间已改变，已丢弃旧结果。".into())
    }
}
async fn get(
    data: &Path,
    scope: &crate::summary_config::Scope,
    path: &str,
) -> Result<Value, Error> {
    let token = crate::devify_access_token_async(data, &scope.server)
        .await
        .map_err(|_| Error {
            status: 401,
            message: "登录已失效，请在客户端重新登录。".into(),
        })?;
    let generation = crate::request_generation(data, &scope.server, &token)?;
    ensure_current(data, scope, &token, generation)?;
    let mut response = http_client::strict()
        .get(format!("{}{path}", scope.server))
        .bearer_auth(&token)
        .send_traced_retrying(data, crate::diagnostics::Module::App, RetryPolicy::READ)
        .await
        .map_err(|_| Error {
            status: 502,
            message: "无法读取云端结果，请检查客户端连接设置后重试。".into(),
        })?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::from("读取云端响应失败。"))?
    {
        if bytes.len().saturating_add(chunk.len()) > 2 * 1024 * 1024 {
            return Err("云端响应超过 2 MiB，请缩小查询范围。".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    ensure_current(data, scope, &token, generation)?;
    if !(200..300).contains(&status) {
        return Err(Error {
            status,
            message: match status {
                401 => "云端登录已失效，请在客户端重新登录。",
                403 => "无权读取该云端资源。",
                404 => "资源不存在、无权访问或当前服务尚不支持此读取入口。",
                _ => "云端读取失败，请稍后重试。",
            }
            .into(),
        });
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| Error::from("云端响应不是有效 JSON。"))?;
    Ok(value.get("data").cloned().unwrap_or(value))
}
fn check_space(item: &Value, space: &str) -> Result<(), Error> {
    if item["space_id"].as_str() != Some(space) {
        return Err(Error {
            status: 403,
            message: "无法确认资源属于当前空间。".into(),
        });
    }
    Ok(())
}
fn output(operation: &str, args: &Args, space: &str, mut value: Value) -> Result<Value, Error> {
    if matches!(operation, "todo" | "report" | "topic") {
        check_space(&value, space)?;
        if value["id"].as_i64() != args.id {
            return Err("云端详情标识不匹配。".into());
        }
        return Ok(json!({"source":"remote","space_id":space,"item":value}));
    }
    if matches!(operation, "topics" | "topic-reports") {
        let rows = value.as_array().ok_or("云端列表格式无效。")?;
        for item in rows {
            check_space(item, space)?;
            if operation == "topic-reports"
                && item["source_info"]["subscription_id"].as_i64() != args.id
            {
                return Err("无法确认报告所属主题。".into());
            }
        }
        if operation == "topic-reports" {
            if rows.len() > 50 {
                return Err("主题报告超出已知接口边界，请确认服务端契约。".into());
            }
            return Ok(
                json!({"source":"remote","space_id":space,"topic_id":args.id,"list":rows,"server_limit":50,
                "history_complete":rows.len()<50,"pagination_supported":false,
                "limitation":"此入口最多返回最近 50 份主题报告；更早历史请逐页浏览 reports 并核对 source_info.subscription_id。"}),
            );
        }
        let size = args.page_size.unwrap_or(25) as usize;
        let page = args.page.unwrap_or(1);
        let start = (page as usize - 1).saturating_mul(size);
        return Ok(
            json!({"source":"remote","space_id":space,"list":rows.iter().skip(start).take(size).collect::<Vec<_>>(),
            "pagination":{"page":page,"page_size":size,"total":rows.len()},
            "next_page":if start.saturating_add(size)<rows.len() {page.checked_add(1)} else {None},
            "pagination_mode":"local_over_server_array"}),
        );
    }
    let rows = value["list"].as_array().ok_or("云端分页列表格式无效。")?;
    if rows.len() > args.page_size.unwrap_or(25) as usize {
        return Err("服务端未遵守单页数量限制。".into());
    }
    for item in rows {
        check_space(item, space)?;
    }
    if operation == "todos" {
        if args.viewer_relevance.is_some()
            && ["my_todo", "related", "other"]
                .iter()
                .any(|key| value["relevance_counts"][*key].as_u64().is_none())
        {
            return Err("服务端未确认个人分类统计，请检查接口契约。".into());
        }
        if args.status.is_some() && value["completion_filter_supported"] != true {
            return Err("当前服务未确认支持完成状态筛选，请更新服务端或省略筛选。".into());
        }
        if args.q.as_ref().is_some_and(|q| !q.trim().is_empty())
            && value["search_supported"] != true
        {
            return Err("当前服务未确认支持待办搜索。".into());
        }
        if args.viewer_relevance.is_some()
            && rows
                .iter()
                .any(|item| item["viewer_relevance"].as_str() != args.viewer_relevance.as_deref())
        {
            return Err("服务端未确认个人分类筛选，请检查接口契约。".into());
        }
    }
    let pagination = value
        .get("pagination")
        .and_then(Value::as_object)
        .ok_or("云端缺少分页信息。")?;
    let page = args.page.unwrap_or(1);
    if pagination.get("page").and_then(Value::as_u64) != Some(page as u64) {
        return Err("服务端分页与请求不一致。".into());
    }
    let next = match pagination.get("next") {
        Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(Value::String(_)) => page.checked_add(1),
        _ => return Err("云端下一页信息无效。".into()),
    };
    // Never follow server-returned URLs; clients request the next numeric page on this fixed resource.
    value["next_page"] = json!(next);
    value["source"] = json!("remote");
    value["space_id"] = json!(space);
    Ok(value)
}

pub(super) async fn read(data: &Path, operation: &str, args: Args) -> Result<Value, Error> {
    let context = scope(data)?;
    if operation == "context" {
        if serde_json::to_value(&args)
            .unwrap()
            .as_object()
            .unwrap()
            .values()
            .any(|v| !v.is_null())
        {
            return Err("context 不接受查询参数。".into());
        }
        return Ok(
            json!({"source":"remote","service_url":context.server,"space_id":context.space_id,
            "page_size_max":100,"topic_reports_limit":50,"read_only":true,
            "report_filters":["status"],"todo_filters":["viewer_relevance","status","q"],
            "note":"云端读取复用客户端登录；不同于本机聊天缓存。报告读取不会触发生成。"}),
        );
    }
    if args.space_id.as_deref() != Some(context.space_id.as_str()) {
        return Err("请先读取 cloud-context，并传入当前 space_id；空间可能已改变。".into());
    }
    let capability = match operation {
        "todos" | "todo" => "cloud.events",
        "reports" | "report" => "cloud.reports",
        "topics" | "topic" | "topic-reports" => "cloud.topics",
        _ => return Err("不支持的只读操作。".into()),
    };
    entitlements::require(data, &context.server, capability).map_err(|message| Error {
        status: 403,
        message,
    })?;
    if operation == "topic-reports" {
        entitlements::require(data, &context.server, "cloud.reports").map_err(|message| Error {
            status: 403,
            message,
        })?;
    }
    let path = args.path(operation, &context.space_id)?;
    let value = get(data, &context, &path).await?;
    output(operation, &args, &context.space_id, value)
}

pub(super) fn operation(name: &str) -> Option<&'static str> {
    match name {
        "aimychats_get_cloud_context" => Some("context"),
        "aimychats_list_todos" => Some("todos"),
        "aimychats_get_todo" => Some("todo"),
        "aimychats_list_reports" => Some("reports"),
        "aimychats_get_report" => Some("report"),
        "aimychats_list_topics" => Some("topics"),
        "aimychats_get_topic" => Some("topic"),
        "aimychats_list_topic_reports" => Some("topic-reports"),
        _ => None,
    }
}
pub(super) fn tools() -> Vec<Value> {
    let mut tools = Vec::new();
    for (name,description,fields) in [
        ("aimychats_get_cloud_context","发现当前客户端的云端读取空间和接口限制；不返回凭据。",vec![]),
        ("aimychats_list_todos","分页读取已有待办（不是 digest 规则素材），保留个人分类、进度和证据引用。",vec!["space_id","page","page_size","viewer_relevance","status","q"]),
        ("aimychats_get_todo","只读获取当前空间已有待办详情、进度和证据引用。",vec!["space_id","id"]),
        ("aimychats_list_reports","分页读取已有报告，保留标题、周期、状态和 source_info；只支持 status 筛选，不触发生成。",vec!["space_id","page","page_size","status"]),
        ("aimychats_get_report","获取已有报告完整正文和 sections，不触发生成；保留 queued/failed 等状态。",vec!["space_id","id"]),
        ("aimychats_list_topics","读取主题订阅配置；服务端返回数组，本机分页，不是报告正文。",vec!["space_id","page","page_size"]),
        ("aimychats_get_topic","读取一个主题订阅配置，不创建或修改主题。",vec!["space_id","id"]),
        ("aimychats_list_topic_reports","只读获取指定主题最近最多 50 份已有报告，检查主题关联；history_complete=false 时不代表完整历史。",vec!["space_id","id"]),
    ] {
        let mut properties=serde_json::Map::new();
        for field in &fields {
            let schema=match *field {
                "page"|"id"=>json!({"type":"integer","minimum":1}),
                "page_size"=>json!({"type":"integer","minimum":1,"maximum":100,"default":25}),
                "viewer_relevance"=>json!({"type":"string","enum":["my_todo","related","other"]}),
                "status" if name=="aimychats_list_todos"=>json!({"type":"string","enum":["unfinished","done"]}),
                "status"=>json!({"type":"string","enum":["queued","ready","partial","failed"]}),
                "q"=>json!({"type":"string","maxLength":200}),_=>json!({"type":"string"}),
            };properties.insert(field.to_string(),schema);
        }
        let required:Vec<_>=fields.iter().filter(|f|matches!(**f,"space_id"|"id")).collect();
        tools.push(json!({"name":name,"description":format!("{description} 云端远程只读，复用桌面登录。"),"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false}}));
    }
    tools
}

#[cfg(test)]
#[path = "cloud_tests.rs"]
mod tests;
