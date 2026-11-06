use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, oneshot};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use crate::settings::{self, AppSettings};
use crate::wechat::{decrypt, query};
use crate::access;

const PROTOCOL_VERSION: &str = "2024-11-05";
pub const DEFAULT_BIND: &str = "127.0.0.1:18765";

#[derive(Clone)]
struct AppState {
    app_data: PathBuf,
    token: Option<String>,
    base_url: String,
    next_session: Arc<AtomicU64>,
    sessions: Arc<Mutex<HashMap<String, broadcast::Sender<String>>>>,
}

#[derive(Deserialize)]
struct SessionQuery {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

#[derive(Deserialize, Default)]
struct AccountArgs {
    account_id: Option<String>,
}

#[derive(Deserialize)]
struct MessagesArgs {
    account_id: Option<String>,
    username: String,
    limit: Option<usize>,
    /// Timestamp of the oldest message already read; pair it with `before_id`.
    before: Option<i64>,
    /// Local id of that same message, so pages never skip a shared timestamp.
    before_id: Option<i64>,
    before_shard: Option<String>,
}

#[derive(Deserialize)]
struct SearchArgs {
    account_id: Option<String>,
    keyword: String,
    groups_only: Option<bool>,
    max_sessions: Option<usize>,
}

#[derive(Deserialize)]
struct DigestArgs {
    account_id: Option<String>,
    usernames: Vec<String>,
    since: Option<i64>,
    until: Option<i64>,
}

fn app_data_dir() -> Result<PathBuf, String> {
    env::var_os("WECHAT_DEVIFY_APP_DATA_DIR")
        .or_else(|| env::var_os("WECHAT_MCP_APP_DATA_DIR"))
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .ok_or_else(|| "请设置 WECHAT_DEVIFY_APP_DATA_DIR，指向应用数据目录。".into())
}

pub fn configured_token() -> Option<String> {
    ["WECHAT_AGENT_TOKEN", "WECHAT_MCP_TOKEN"]
        .iter()
        .find_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Listener address. `WECHAT_AGENT_BIND` is the current name; the MCP-era variable stays
/// as a compatibility alias for existing scripts.
pub fn configured_bind() -> String {
    ["WECHAT_AGENT_BIND", "WECHAT_MCP_BIND"]
        .iter()
        .find_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_BIND.into())
}

fn authorized(headers: &HeaderMap, expected: &Option<String>) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|value| value == expected)
}

fn safe_account_id(account_id: &str) -> Result<(), String> {
    if account_id.is_empty()
        || account_id.len() > 256
        || account_id == "."
        || account_id == ".."
        || account_id.contains('/')
        || account_id.contains('\\')
    {
        return Err("账号标识无效。".into());
    }
    Ok(())
}

struct Context {
    account_id: String,
    cache: PathBuf,
    allowed: HashSet<String>,
    settings: AppSettings,
}

fn context(app_data: &Path, account_id: Option<String>) -> Result<Context, String> {
    let settings = settings::load(app_data)?;
    if !settings.connectors.wechat_enabled {
        return Err("微信连接已断开，重新连接后才能通过 MCP 读取聊天。".into());
    }
    crate::entitlements::require(
        app_data,
        &settings.devify.base_url,
        crate::capabilities::BRIDGE_MCP,
    )?;
    let account_id = account_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| settings.active_account_id.clone());
    safe_account_id(&account_id)?;
    let allowed = settings
        .chat
        .allowed_usernames
        .get(&account_id)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    Ok(Context {
        cache: decrypt::cache_dir(app_data, &account_id),
        account_id,
        allowed,
        settings,
    })
}

fn sessions(context: &Context) -> Result<Vec<query::SessionItem>, String> {
    query::sessions_filtered(&context.cache, Some(&context.allowed))
}

fn ensure_allowed(context: &Context, username: &str) -> Result<(), String> {
    if !query::is_chat_session(&context.cache, username) {
        return Err("公众号、服务号和系统会话不在 MCP 查看范围内。".into());
    }
    if !context.allowed.contains(username) {
        return Err("该会话不在聊天白名单中。".into());
    }
    Ok(())
}

fn tools() -> Value {
    json!({
        "tools": [
            {
                "name": "wechat_get_policy",
                "description": "读取当前账号和白名单会话。只返回群聊和单聊，不返回公众号、服务号或系统会话。",
                "inputSchema": {"type": "object", "properties": {"account_id": {"type": "string"}}}
            },
            {
                "name": "wechat_list_sessions",
                "description": "列出当前白名单内的聊天会话。白名单为空时返回空列表。",
                "inputSchema": {"type": "object", "properties": {"account_id": {"type": "string"}}}
            },
            {
                "name": "wechat_get_messages",
                "description": "读取一个白名单会话的消息。支持分页读取更早消息。",
                "inputSchema": {
                    "type": "object",
                    "required": ["username"],
                    "properties": {
                        "account_id": {"type": "string"},
                        "username": {"type": "string"},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 500, "default": 100},
                        "before": {"type": "integer", "description": "只返回这个时间戳之前的消息；与 before_id 一起使用时从上一页最后一条之后继续"},
                        "before_id": {"type": "integer", "description": "上一页最后一条消息的 local_id"},
                        "before_shard": {"type": "string", "description": "上一页最后一条消息的来源分片；与 before 和 before_id 一起传入"}
                    }
                }
            },
            {
                "name": "wechat_search_messages",
                "description": "在白名单会话中搜索消息。搜索范围最多 200 个会话。",
                "inputSchema": {
                    "type": "object",
                    "required": ["keyword"],
                    "properties": {
                        "account_id": {"type": "string"},
                        "keyword": {"type": "string"},
                        "groups_only": {"type": "boolean", "default": false},
                        "max_sessions": {"type": "integer", "minimum": 1, "maximum": 200, "default": 200}
                    }
                }
            },
            {
                "name": "wechat_digest",
                "description": "对指定的白名单会话做规则整理，输出待办、决定、问题、风险、数据和时间线素材。不是本地 AI 结论。",
                "inputSchema": {
                    "type": "object",
                    "required": ["usernames"],
                    "properties": {
                        "account_id": {"type": "string"},
                        "usernames": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 100},
                        "since": {"type": "integer"},
                        "until": {"type": "integer"}
                    }
                }
            }
        ]
    })
}

fn result(value: Value) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into());
    json!({"content": [{"type": "text", "text": text}]})
}

fn tool_error(message: impl Into<String>) -> Value {
    json!({"content": [{"type": "text", "text": message.into()}], "isError": true})
}

fn arguments<T: serde::de::DeserializeOwned>(params: &Value) -> Result<T, String> {
    serde_json::from_value(params.get("arguments").cloned().unwrap_or_else(|| json!({})))
        .map_err(|error| format!("工具参数无效：{error}"))
}

fn policy_json(app_data: &Path, args: AccountArgs) -> Result<Value, String> {
    let context = context(app_data, args.account_id)?;
    let sessions = sessions(&context)?;
    Ok(json!({
        "account_id": context.account_id,
        "whitelist_count": context.allowed.len(),
        "sessions": sessions,
        "automation_enabled": context.settings.automation.enabled
    }))
}

fn sessions_json(app_data: &Path, args: AccountArgs) -> Result<Value, String> {
    let context = context(app_data, args.account_id)?;
    Ok(json!({"account_id": context.account_id, "sessions": sessions(&context)?}))
}

fn messages_json(app_data: &Path, args: MessagesArgs) -> Result<Value, String> {
    let context = context(app_data, args.account_id)?;
    ensure_allowed(&context, &args.username)?;
    let limit = args.limit.unwrap_or(100).clamp(1, 500);
    let cursor = args.before.map(|time| match args.before_id {
        Some(id) => query::MessageCursor { time, local_id: id, source_shard: args.before_shard.unwrap_or_default() },
        None => query::MessageCursor::before_time(time),
    });
    let messages = query::messages(&context.cache, &args.username, limit, cursor)?;
    let has_more = messages.len() == limit;
    Ok(json!({
        "account_id": context.account_id,
        "username": args.username,
        "messages": messages,
        "has_more": has_more,
        "next_cursor": messages.last().map(|m| json!({"before": m.time, "before_id": m.local_id, "before_shard": m.source_shard}))
    }))
}

fn search_json(app_data: &Path, args: SearchArgs) -> Result<Value, String> {
    let context = context(app_data, args.account_id)?;
    let report = query::search_filtered(
        &context.cache,
        &args.keyword,
        args.groups_only.unwrap_or(false),
        args.max_sessions.unwrap_or(200).clamp(1, 200),
        Some(&context.allowed),
    )?;
    serde_json::to_value(report).map_err(|error| error.to_string())
}

fn digest_json(app_data: &Path, args: DigestArgs) -> Result<Value, String> {
    if args.usernames.is_empty() || args.usernames.len() > 100 {
        return Err("usernames 必须包含 1 到 100 个会话。".into());
    }
    let context = context(app_data, args.account_id)?;
    for username in &args.usernames {
        ensure_allowed(&context, username)?;
    }
    let digest = query::digest_filtered(
        &context.cache,
        &args.usernames,
        args.since,
        args.until,
        Some(&context.allowed),
    )?;
    serde_json::to_value(digest).map_err(|error| error.to_string())
}

fn call_tool(app_data: &Path, name: &str, params: &Value) -> Value {
    let output = match name {
        "wechat_get_policy" => arguments::<AccountArgs>(params).and_then(|args| policy_json(app_data, args)),
        "wechat_list_sessions" => arguments::<AccountArgs>(params).and_then(|args| sessions_json(app_data, args)),
        "wechat_get_messages" => arguments::<MessagesArgs>(params).and_then(|args| messages_json(app_data, args)),
        "wechat_search_messages" => arguments::<SearchArgs>(params).and_then(|args| search_json(app_data, args)),
        "wechat_digest" => arguments::<DigestArgs>(params).and_then(|args| digest_json(app_data, args)),
        _ => Err(format!("未知工具：{name}")),
    };
    match output {
        Ok(value) => result(value),
        Err(error) => tool_error(error),
    }
}

fn rpc_response(request: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": request.get("id").cloned().unwrap_or(Value::Null), "result": result})
}

fn rpc_error(request: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": request.get("id").cloned().unwrap_or(Value::Null), "error": {"code": code, "message": message}})
}

fn handle_rpc(app_data: &Path, request: Value) -> Option<Value> {
    let method = request.get("method")?.as_str()?;
    let has_id = request.get("id").is_some();
    if method == "notifications/initialized" || method == "notifications/cancelled" {
        return None;
    }
    let response = match method {
        "initialize" => rpc_response(
            &request,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "wechat-devify-bridge", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "只读访问本地已解密缓存；所有工具都受聊天白名单约束。"
            }),
        ),
        "ping" => rpc_response(&request, json!({})),
        "tools/list" => rpc_response(&request, tools()),
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(rpc_error(&request, -32602, "tools/call 缺少 name。"));
            };
            rpc_response(&request, call_tool(app_data, name, &params))
        }
        "shutdown" => rpc_response(&request, Value::Null),
        _ => rpc_error(&request, -32601, "方法不存在。"),
    };
    has_id.then_some(response)
}

async fn sse(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !authorized(&headers, &state.token) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    let session_id = state.next_session.fetch_add(1, Ordering::Relaxed).to_string();
    let (sender, receiver) = broadcast::channel(32);
    if let Ok(mut sessions) = state.sessions.lock() {
        sessions.insert(session_id.clone(), sender);
    }
    let endpoint = format!("/messages?sessionId={session_id}");
    let initial = tokio_stream::once(Ok::<Event, Infallible>(
        Event::default().event("endpoint").data(endpoint),
    ));
    let messages = BroadcastStream::new(receiver)
        .filter_map(|message| message.ok())
        .map(|message| Ok::<Event, Infallible>(Event::default().event("message").data(message)));
    Sse::new(initial.chain(messages))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keep-alive"))
        .into_response()
}

async fn messages(
    State(state): State<AppState>,
    Query(query): Query<SessionQuery>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> impl IntoResponse {
    if !authorized(&headers, &state.token) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    let Some(session_id) = query.session_id else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "缺少 sessionId。"}))).into_response();
    };
    let sender = state.sessions.lock().ok().and_then(|sessions| sessions.get(&session_id).cloned());
    let Some(sender) = sender else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "SSE 会话不存在或已过期。"}))).into_response();
    };
    let app_data = state.app_data.clone();
    let response = tokio::task::spawn_blocking(move || {
        handle_rpc(&app_data, request).map(|value| serde_json::to_string(&value).unwrap_or_else(|_| "{}".into()))
    })
    .await
    .ok()
    .flatten();
    if let Some(response) = response {
        if sender.send(response).is_err() {
            return (StatusCode::GONE, Json(json!({"error": "SSE 客户端已断开。"}))).into_response();
        }
    }
    StatusCode::ACCEPTED.into_response()
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let _ = state;
    Json(json!({"ok": true, "service": "wechat-devify-bridge", "protocol": "mcp-sse"}))
}

/* ── read-only REST adapter ─────────────────────────────────────────────────── */
//
// Same handlers as the MCP tools, exposed as plain HTTP so a small CLI (or curl)
// can read chats without an MCP client. Read-only, loopback by default, and the
// same whitelist/token checks apply.

fn api_ok(result: Result<Value, String>) -> Response {
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => (StatusCode::BAD_REQUEST, Json(json!({"error": error}))).into_response(),
    }
}

fn api_unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response()
}

async fn api_task<F>(f: F) -> Response
where
    F: FnOnce() -> Result<Value, String> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => api_ok(result),
        Err(_) => api_ok(Err("内部任务失败。".into())),
    }
}

async fn api_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(args): Query<AccountArgs>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return api_unauthorized();
    }
    let app_data = state.app_data.clone();
    api_task(move || policy_json(&app_data, args)).await
}

async fn api_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(args): Query<AccountArgs>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return api_unauthorized();
    }
    let app_data = state.app_data.clone();
    api_task(move || sessions_json(&app_data, args)).await
}

async fn api_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(args): Query<MessagesArgs>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return api_unauthorized();
    }
    let app_data = state.app_data.clone();
    api_task(move || messages_json(&app_data, args)).await
}

async fn api_search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(args): Query<SearchArgs>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return api_unauthorized();
    }
    let app_data = state.app_data.clone();
    api_task(move || search_json(&app_data, args)).await
}

async fn api_digest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(args): Json<DigestArgs>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return api_unauthorized();
    }
    let app_data = state.app_data.clone();
    api_task(move || digest_json(&app_data, args)).await
}

/* ── agent access kit ───────────────────────────────────────────────────────── */
//
// The Agent 集成 → 工具接入 prompt downloads the read-only CLI and the SKILL.md documents from
// these routes. They never expose chat data; the token rule is the same as `/api/*`.

#[derive(Deserialize, Default)]
struct InstallArgs {
    bin: Option<String>,
    skills: Option<String>,
}

fn agent_unavailable(message: &str) -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": message }))).into_response()
}

async fn agent_manifest(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.token) {
        return api_unauthorized();
    }
    let wxq = access::resolve_wxq(&state.app_data);
    let skills: Vec<Value> = access::SKILLS
        .iter()
        .map(|skill| {
            json!({
                "name": skill.name,
                "description": access::skill_description(skill.content),
                "url": format!("/agent/skills/{}", skill.name),
                "path": format!("{}/SKILL.md", skill.name),
            })
        })
        .collect();
    Json(json!({
        "service": "wechat-devify-bridge",
        "version": env!("CARGO_PKG_VERSION"),
        "base_url": state.base_url,
        "cli": {
            "name": access::CLI_NAME,
            "available": wxq.is_some(),
            "url": "/agent/wxq",
            "size": wxq.as_ref().and_then(|path| std::fs::metadata(path).ok()).map(|meta| meta.len()),
        },
        "skills": skills,
        "install": {
            "url": "/agent/install.sh",
            "query": "?bin=<dir>&skills=<dir>",
        }
    }))
    .into_response()
}

async fn agent_wxq(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.token) {
        return api_unauthorized();
    }
    let Some(path) = access::resolve_wxq(&state.app_data) else {
        return agent_unavailable("没有可下载的 wxq。请在 AIMyChats 源码目录构建后重启应用。");
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/octet-stream".to_string()),
                (header::CONTENT_DISPOSITION, "attachment; filename=\"wxq\"".to_string()),
                (header::CACHE_CONTROL, "no-store".to_string()),
            ],
            bytes,
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("读取 wxq 失败：{error}") })),
        )
            .into_response(),
    }
}

async fn agent_skills(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.token) {
        return api_unauthorized();
    }
    let skills: Vec<Value> = access::SKILLS
        .iter()
        .map(|skill| {
            json!({
                "name": skill.name,
                "description": access::skill_description(skill.content),
                "url": format!("/agent/skills/{}", skill.name),
            })
        })
        .collect();
    Json(json!({ "skills": skills })).into_response()
}

async fn agent_skill(
    AxumPath(name): AxumPath<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&headers, &state.token) {
        return api_unauthorized();
    }
    let Some(skill) = access::SKILLS.iter().find(|skill| skill.name == name) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "未知的 Skill。" }))).into_response();
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/markdown; charset=utf-8".to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        skill.content,
    )
        .into_response()
}

async fn agent_install_script(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(args): Query<InstallArgs>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return api_unauthorized();
    }
    let Some(bin_dir) = access::expand_path(args.bin.as_deref().unwrap_or("~/.local/bin")) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "bin 必须是绝对路径或以 ~/ 开头。" })),
        )
            .into_response();
    };
    let Some(skills_dir) = access::expand_path(args.skills.as_deref().unwrap_or("~/.agents/skills")) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "skills 必须是绝对路径或以 ~/ 开头。" })),
        )
            .into_response();
    };
    let script = access::build_install_script(&bin_dir, &skills_dir, &state.base_url);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8".to_string()),
            (header::CONTENT_DISPOSITION, "inline; filename=\"install.sh\"".to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        script,
    )
        .into_response()
}

pub fn requires_token_for_ui(bind: &str) -> bool {
    let host = bind.rsplit_once(':').map(|(host, _)| host).unwrap_or(bind);
    !matches!(host.trim_matches(['[', ']']), "127.0.0.1" | "localhost" | "::1")
}

pub async fn start(
    app_data: PathBuf,
    bind: String,
    token: Option<String>,
    ready: Option<oneshot::Sender<Result<(), String>>>,
) -> Result<(), String> {
    if requires_token_for_ui(&bind) && token.is_none() {
        let error = "非本机绑定必须设置 WECHAT_AGENT_TOKEN。".to_string();
        if let Some(ready) = ready {
            let _ = ready.send(Err(error.clone()));
        }
        return Err(error);
    }
    let state = AppState {
        app_data,
        token,
        base_url: access::client_base(&bind),
        next_session: Arc::new(AtomicU64::new(1)),
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };
    let app = Router::new()
        .route("/sse", get(sse))
        .route("/messages", post(messages))
        .route("/health", get(health))
        .route("/api/policy", get(api_policy))
        .route("/api/sessions", get(api_sessions))
        .route("/api/messages", get(api_messages))
        .route("/api/search", get(api_search))
        .route("/api/digest", post(api_digest))
        .route("/agent/manifest", get(agent_manifest))
        .route("/agent/wxq", get(agent_wxq))
        .route("/agent/skills", get(agent_skills))
        .route("/agent/skills/:name", get(agent_skill))
        .route("/agent/install.sh", get(agent_install_script))
        .with_state(state);
    let listener = match TcpListener::bind(&bind).await {
        Ok(listener) => listener,
        Err(error) => {
            let error = format!("本机只读服务监听失败：{error}");
            if let Some(ready) = ready {
                let _ = ready.send(Err(error.clone()));
            }
            return Err(error);
        }
    };
    if let Some(ready) = ready {
        let _ = ready.send(Ok(()));
    }
    eprintln!("wechat read-only service listening on http://{bind}");
    axum::serve(listener, app).await.map_err(|error| format!("本机只读服务退出：{error}"))
}

pub async fn run_from_env() -> Result<(), String> {
    let app_data = app_data_dir().map_err(|error| error.to_string())?;
    let bind = configured_bind();
    start(app_data, bind, configured_token(), None).await
}
