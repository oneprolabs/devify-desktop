//! Durable, owner-bound summary intents, processed by the existing coordinator.
//! Cloud execution is delegated; local CLI execution is serialized with preparation.
use super::*;
use crate::chat::access::{allowed_usernames, whatsapp_allowed_usernames};
use crate::summary_config::{batches, prompt};
use rusqlite::{params, TransactionBehavior};

static ADMISSION: std::sync::Mutex<()> = std::sync::Mutex::new(());
static LOCAL_AGENT_ACTIVE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Job {
    pub id: String,
    pub owner: settings::UploadOwner,
    pub engine: String,
    pub agent: Option<settings::AgentSettings>,
    pub title: String,
    pub username: String,
    pub status: String,
    pub message: String,
    pub updated_at: String,
    pub created_at: String,
    pub input_hash: String,
    pub body: serde_json::Value,
    pub remote_id: Option<i64>,
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub prompt: Option<crate::summary_config::prompt::Frozen>,
    #[serde(default)]
    pub config_retried: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    batches: Option<batches::Progress>,
}

fn owner_key(owner: &settings::UploadOwner) -> String {
    hex::encode(sha2::Sha256::digest(
        serde_json::to_vec(owner).unwrap_or_default(),
    ))
}

fn all(data: &Path) -> Result<Vec<Job>, String> {
    let db = task_store::open(data)?;
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='summary_tasks')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !exists {
        return Ok(Vec::new());
    }
    let mut statement = db
        .prepare("SELECT body FROM summary_tasks ORDER BY updated_at DESC LIMIT 500")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    rows.map(|raw| {
        serde_json::from_str(&raw.map_err(|e| e.to_string())?)
            .map_err(|e| format!("总结任务记录无效：{e}"))
    })
    .collect()
}

fn store(data: &Path, job: &Job, expected: Option<&str>) -> Result<bool, String> {
    let mut db = task_store::open(data)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS summary_tasks(id TEXT PRIMARY KEY, owner TEXT NOT NULL, status TEXT NOT NULL, body TEXT NOT NULL, updated_at TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS summary_tasks_owner_time ON summary_tasks(owner, updated_at);").map_err(|e| e.to_string())?;
    let body = serde_json::to_string(job).map_err(|e| e.to_string())?;
    let changed = if let Some(status) = expected {
        tx.execute("UPDATE summary_tasks SET status=?1, body=?2, updated_at=?3 WHERE id=?4 AND owner=?5 AND status=?6", params![job.status, body, job.updated_at, job.id, owner_key(&job.owner), status])
    } else {
        tx.execute("INSERT OR IGNORE INTO summary_tasks(id,owner,status,body,updated_at) VALUES(?1,?2,?3,?4,?5)", params![job.id, owner_key(&job.owner), job.status, body, job.updated_at])
    }.map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(changed > 0)
}

pub(crate) fn current(app: &AppHandle) -> Result<Vec<Job>, String> {
    let data = app_data(app);
    let value = settings::load(&data)?;
    Ok(all(&data)?
        .into_iter()
        .filter(|j| {
            config_runtime::local_owner(&data, &value, &j.owner.account_id)
                .is_ok_and(|owner| config_runtime::same_principal(&owner, &j.owner))
        })
        .collect())
}

pub(crate) fn records(
    app: &AppHandle,
    account: &str,
    username: &str,
) -> Result<Vec<serde_json::Value>, String> {
    Ok(current(app)?
        .iter()
        .filter(|j| j.owner.account_id == account && j.username == username)
        .take(50)
        .map(record)
        .collect())
}

fn record(job: &Job) -> serde_json::Value {
    let mut record = job.result.clone().unwrap_or_else(|| {
        serde_json::json!({
            "id":format!("task:{}",job.id),"request_key":job.id,"title":job.title,
            "range_label":job.body["range_label"],"mode":job.body["mode"],
            "period_start":job.body["period_start"],"period_end":job.body["period_end"],
            "message_count":job.body["message_count"],"sessions":job.body["sessions"],
            "scenario":job.body["scenario"],"language":job.body["language"],"instructions":job.body["instructions"],
            "prompt_version":job.body["prompt_version"],"prompt_digest":job.body["prompt_digest"],
            "body":"","created_at":job.created_at,"model":""
        })
    });
    record["task_id"] = job.id.clone().into();
    record["engine"] = job.engine.clone().into();
    record["caller"] = job
        .agent
        .as_ref()
        .map(|a| agent::agent_name(&a.provider_id))
        .unwrap_or_else(|| "云端模型".into())
        .into();
    record["initiator_id"] = job.owner.user_id.into();
    record["status"] = match job.status.as_str() {
        "completed" => "ready",
        "processing" => "running",
        other => other,
    }
    .into();
    record["error"] = if job.status == "failed" {
        job.message.clone()
    } else {
        String::new()
    }
    .into();
    record
}

#[tauri::command]
pub(crate) async fn summary_task_enqueue(
    app: AppHandle,
    space_id: String,
    account_id: String,
    username: String,
    engine: String,
    mut body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    if !body.is_object() { return Err("总结请求格式无效。".into()); }
    crate::cloud_summary::validate_source(&app, &space_id, &account_id, &username)?;
    if !matches!(engine.as_str(), "cloud" | "local") {
        return Err("无效的总结方式。".into());
    }
    let data = app_data(&app);
    let value = settings::load(&data)?;
    let owner = config_runtime::local_owner(&data, &value, &account_id)?;
    // An explicit per-request choice wins for both engines.
    body["language"] = crate::summary_config::prompt::language(body["language"].as_str(), &value.language)?.into();
    body["instructions"] = body["instructions"].as_str().unwrap_or("").trim().to_string().into();
    let id = body["request_key"]
        .as_str()
        .ok_or("缺少总结请求标识。")?
        .to_string();
    if id.len() != 36
        || !id.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
    {
        return Err("无效的总结请求标识。".into());
    }
    let messages = body["messages"].as_array().ok_or("缺少总结消息。")?;
    let all_messages = engine == "local" && crate::summary_config::prompt::is_all_messages(&body);
    if messages.is_empty() || (!all_messages && messages.len() > 2000) {
        return Err("请选择 1 至 2000 条文字消息。".into());
    }
    let allowed = if whatsapp::is_account(&account_id) {
        whatsapp_allowed_usernames(&app, &account_id)?
    } else {
        allowed_usernames(&app, &account_id)?
    };
    if messages
        .iter()
        .any(|m| !m["username"].as_str().is_some_and(|u| allowed.contains(u)))
    {
        return Err("总结包含未导入的会话。".into());
    }
    let agent = if engine == "local" {
        if !entitlements::is_internal(&data, &value.devify.base_url) {
            return Err("本地工具总结仅对内部账号开放。".into());
        }
        entitlements::require(
            &data,
            &value.devify.base_url,
            capabilities::AI_AGENT_SUMMARY,
        )?;
        let selected = agent::selected(&value.agent)?;
        Some(selected)
    } else {
        None
    };
    body["platform"] = if whatsapp::is_account(&account_id) {
        "whatsapp"
    } else {
        "wechat"
    }
    .into();
    body["source_namespace"] = account_id.clone().into();
    body["source_conversation_id"] = username.clone().into();
    let _admission = ADMISSION
        .lock()
        .map_err(|_| "总结任务暂时无法创建，请重试。")?;
    let input_hash = hex::encode(sha2::Sha256::digest(
        serde_json::to_vec(&body).map_err(|e| e.to_string())?,
    ));
    if let Some(existing) = all(&data)?.into_iter().find(|j| j.id == id) {
        if existing.input_hash != input_hash || existing.engine != engine || existing.owner != owner
        {
            return Err("总结请求已变化，请重新提交。".into());
        }
        return Ok(record(&existing));
    }
    let config = crate::summary_config::catalog(&data)?;
    validate_catalog_owner(&config, &owner)?;
    let (prompt, plan) = prompt::freeze(&config, &mut body, &value.language, &engine, true)?;
    let batch_progress = plan
        .filter(|plan| plan.ends.len() > 1)
        .map(batches::Progress::new);
    let count = body["messages"].as_array().map_or(0, Vec::len);
    body["message_count"] = count.into();
    let mut sessions = BTreeMap::<String, serde_json::Value>::new();
    for message in body["messages"].as_array().unwrap() {
        let username = message["username"].as_str().unwrap().to_string();
        let entry = sessions.entry(username).or_insert_with(|| serde_json::json!({"username":message["username"],"display":message["display"],"messages":0}));
        entry["messages"] = (entry["messages"].as_u64().unwrap_or(0) + 1).into();
    }
    body["sessions"] = sessions.into_values().collect::<Vec<_>>().into();
    let now = Utc::now().to_rfc3339();
    let job = Job {
        id,
        owner,
        engine,
        agent,
        username,
        title: body["title"].as_str().unwrap_or("聊天总结").into(),
        status: "queued".into(),
        message: "等待生成总结。".into(),
        updated_at: now.clone(),
        created_at: now,
        input_hash,
        body,
        remote_id: None,
        result: None,
        prompt: Some(prompt),
        config_retried: false,
        batches: batch_progress,
    };
    if current(&app)?
        .iter()
        .filter(|j| matches!(j.status.as_str(), "queued" | "running" | "processing"))
        .count()
        >= 3
    {
        return Err("已有 3 个总结正在处理，请稍后再试。".into());
    }
    let _identity = UPLOAD_IDENTITY_LOCK.try_read().map_err(|_| "登录状态正在切换，请稍后重试。")?;
    if config_runtime::local_owner(&data, &settings::load(&data)?, &account_id)? != job.owner { return Err("总结任务所属登录状态已改变。".into()); }
    let stored = store(&data, &job, None)?;
    if !stored {
        return Err("总结已提交，请刷新查看。".into());
    }
    TASK_WAKE.notify_one();
    emit_task_runtime(&app);
    Ok(record(&job))
}

#[tauri::command]
pub(crate) fn summary_task_retry(app: AppHandle, task_id: String) -> Result<(), String> {
    let _admission = ADMISSION.lock().map_err(|_| "总结任务暂时无法重试。")?;
    let jobs = current(&app)?;
    if jobs
        .iter()
        .filter(|j| matches!(j.status.as_str(), "queued" | "running" | "processing"))
        .count()
        >= 3
    {
        return Err("已有 3 个总结正在处理，请稍后再试。".into());
    }
    let mut job = jobs
        .into_iter()
        .find(|j| j.id == task_id)
        .ok_or("找不到当前账号的总结任务。")?;
    if job.status != "failed" {
        return Err("任务状态已变化，请刷新查看。".into());
    }
    job.status = "queued".into();
    job.message = "已安排重试。".into();
    job.updated_at = Utc::now().to_rfc3339();
    store(&app_data(&app), &job, Some("failed"))?;
    TASK_WAKE.notify_one();
    emit_task_runtime(&app);
    Ok(())
}

#[tauri::command]
pub(crate) fn summary_task_delete(app: AppHandle, task_id: String) -> Result<(), String> {
    let job = current(&app)?
        .into_iter()
        .find(|j| j.id == task_id)
        .ok_or("找不到总结任务。")?;
    if !matches!(job.status.as_str(), "failed" | "completed") {
        return Err("请等待总结结束后再删除。".into());
    }
    if job.remote_id.is_some() {
        return Err("请先删除云端总结。".into());
    }
    let db = task_store::open(&app_data(&app))?;
    db.execute(
        "DELETE FROM summary_tasks WHERE id=?1 AND owner=?2 AND status IN ('completed','failed')",
        params![job.id, owner_key(&job.owner)],
    )
    .map_err(|e| e.to_string())?;
    emit_task_runtime(&app);
    Ok(())
}

pub(crate) fn recover(data: &Path) -> Result<(), String> {
    for mut job in all(data)? {
        if job.status == "running" && job.engine == "local" {
            job.status = "failed".into();
            job.message = "本地总结因应用退出而中断，请重试。".into();
            job.updated_at = Utc::now().to_rfc3339();
            store(data, &job, Some("running"))?;
        }
    }
    Ok(())
}

pub(crate) async fn tick(app: &AppHandle) {
    let Ok(jobs) = current(app) else {
        return;
    };
    let data = app_data(app);
    // Check a bounded number of already-submitted cloud jobs, independent of backup pause.
    for mut job in jobs
        .iter()
        .filter(|j| j.status == "processing")
        .take(2)
        .cloned()
    {
        let result = cloud_request(app, &job, reqwest::Method::GET, job.remote_id, None).await;
        match result {
            Ok(result) => update_remote(&data, &mut job, result, "processing"),
            Err(error) if error.message.contains("HTTP 404") || error.message.contains("HTTP 403") => {
                job.status = "failed".into();
                job.message = "云端总结已不存在或访问权限已改变，请重新生成。".into();
                job.updated_at = Utc::now().to_rfc3339();
                let _ = store(&data, &job, Some("processing"));
            }
            Err(_) => {} // Temporary network/auth failures preserve the remote job for reconciliation.
        }
    }
    let Some(mut job) = jobs.into_iter().rev().find(|j| j.status == "queued") else {
        return;
    };
    if job.engine == "cloud" {
        let result = submit_cloud(app, &mut job).await;
        match result {
            Ok(result) => update_remote(&data, &mut job, result, "queued"),
            Err(error) => {
                job.status = "failed".into();
                job.message = error.message;
                job.updated_at = Utc::now().to_rfc3339();
                let _ = store(&data, &job, Some("queued"));
            }
        }
        emit_task_runtime(app);
        return;
    }
    let Ok(lease) = LOCAL_AGENT_ACTIVE.try_lock() else {
        return;
    };
    job.status = "running".into();
    job.message = "正在通过本地工具生成总结。".into();
    job.updated_at = Utc::now().to_rfc3339();
    if !store(&data, &job, Some("queued")).unwrap_or(false) {
        return;
    }
    emit_task_runtime(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        run_local(&app, &data, job).await;
        drop(lease);
        TASK_WAKE.notify_one();
    });
}

fn validate_local_owner(data: &Path, owner: &settings::UploadOwner) -> Result<(), String> {
    let settings = settings::load(data)?;
    if !config_runtime::local_owner(data, &settings, &owner.account_id)
        .is_ok_and(|current| current == *owner)
        || !entitlements::is_internal(data, &owner.base_url)
    {
        return Err("登录状态或本地工具权限已改变，请重新操作。".into());
    }
    entitlements::require(data, &owner.base_url, capabilities::AI_AGENT_SUMMARY)?;
    Ok(())
}

async fn run_local(app: &AppHandle, data: &Path, mut job: Job) {
    let result = execute_local(app, data, &mut job).await;
    match result {
        Ok(output) => {
            let mut record = record(&job);
            record["body"] = output.into();
            record["status"] = "ready".into();
            job.result = Some(record);
            job.status = "completed".into();
            job.message = "总结已完成。".into();
            job.body["messages"] = serde_json::json!([]);
            job.batches = None;
        }
        Err(error) => {
            job.status = "failed".into();
            job.message = error;
        }
    }
    job.updated_at = Utc::now().to_rfc3339();
    let _ = store(data, &job, Some("running"));
    emit_task_runtime(app);
}

async fn execute_local(app: &AppHandle, data: &Path, job: &mut Job) -> Result<String, String> {
    validate_local_owner(data, &job.owner)?;
    let selected = job.agent.clone().ok_or("未选择本地工具。")?;
    let root = data.join("summary-runs").join(owner_key(&job.owner));
    let Some(snapshot) = job.prompt.clone() else {
        let messages = job.body["messages"]
            .as_array()
            .ok_or("缺少总结消息。")?
            .iter()
            .cloned()
            .map(|mut value| {
                if value.get("is_group").is_none() {
                    value["is_group"] = false.into();
                }
                serde_json::from_value::<query::SummaryMessage>(value)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "总结消息格式无效。")?;
        let input = query::summarize_messages(messages);
        if input.truncated {
            return Err("本地工具的单次处理范围已超限，请减少选择后重试。".into());
        }
        let language = job.body["language"].as_str().unwrap_or("中文").to_string();
        let result = tauri::async_runtime::spawn_blocking(move || {
            agent::summarize(&selected, &input, &language, &root)
        })
        .await
        .map_err(|error| format!("本地总结任务失败：{error}"))??;
        validate_local_owner(data, &job.owner)?;
        return Ok(result.output);
    };

    loop {
        validate_local_owner(data, &job.owner)?;
        let step = match job.batches.as_mut() {
            Some(progress) => progress.next(&snapshot, &job.body)?,
            None => batches::Step::Final(prompt::local_prompt(
                &snapshot,
                &job.body,
                &prompt::local_input(&job.body)?,
            )?),
        };
        let (agent, run_root, request) =
            (selected.clone(), root.clone(), step.prompt().to_string());
        let result = tauri::async_runtime::spawn_blocking(move || {
            agent::summarize_chat(&agent, &request, &run_root)
        })
        .await
        .map_err(|error| format!("本地总结任务失败：{error}"))??;
        validate_local_owner(data, &job.owner)?;
        let Some(progress) = job.batches.as_mut() else {
            return Ok(result.output);
        };
        if let Some(output) = progress.accept(&snapshot, &job.body, step, result.output)? {
            return Ok(output);
        }
        let (completed, total) = progress.counts();
        job.message = if completed == total {
            "正在合并分批摘要。".into()
        } else {
            format!("已完成 {completed} / {total} 批。")
        };
        job.updated_at = Utc::now().to_rfc3339();
        if !store(data, job, Some("running"))? {
            return Err("总结任务状态已改变。".into());
        }
        emit_task_runtime(app);
    }
}

async fn submit_cloud(app: &AppHandle, job: &mut Job) -> Result<serde_json::Value, crate::summary_config::CloudError> {
    let first = cloud_request(app, job, reqwest::Method::POST, job.remote_id, job.remote_id.is_none().then_some(&job.body)).await;
    match first {
        Err(error) if error.config_changed && job.remote_id.is_none() && !job.config_retried => {
            crate::summary_config::refresh(app, true).await?;
            let data = app_data(app);
            let value = settings::load(&data)?;
            let config = crate::summary_config::catalog(&data)?;
            validate_catalog_owner(&config, &job.owner)?;
            // This precise 409 guarantees no remote task exists. Keep the request key
            // and user choices; persist the replacement snapshot before sending again.
            let (prompt, _) = crate::summary_config::prompt::freeze(
                &config,
                &mut job.body,
                &value.language,
                &job.engine,
                false,
            )?;
            job.prompt = Some(prompt);
            job.config_retried = true;
            job.updated_at = Utc::now().to_rfc3339();
            if !store(&data, job, Some("queued"))? { return Err("总结任务状态已改变。".into()); }
            cloud_request(app, job, reqwest::Method::POST, None, Some(&job.body)).await
        }
        result => result,
    }
}

fn validate_catalog_owner(config: &crate::summary_config::Catalog, owner: &settings::UploadOwner) -> Result<(), String> {
    if config.scope.server != owner.base_url || config.scope.account_id != owner.user_id.to_string() || config.scope.space_id != owner.space_id {
        return Err("总结配置所属登录账号或空间已改变。".into());
    }
    Ok(())
}

async fn cloud_request(
    app: &AppHandle,
    job: &Job,
    method: reqwest::Method,
    id: Option<i64>,
    body: Option<&serde_json::Value>,
) -> Result<serde_json::Value, crate::summary_config::CloudError> {
    let data = app_data(app);
    let settings = settings::load(&data)?;
    let token = devify_access_token_async(&data, &job.owner.base_url).await.map_err(|e| e.to_string())?;
    validate_owner_state(&settings, &job.owner, &token)?;
    let suffix = id.map(|id| format!("/{id}")).unwrap_or_default();
    crate::summary_config::cloud_request(
        app,
        &job.owner.base_url,
        &token,
        method,
        &format!(
            "/api/v1/spaces/{}/chat-summaries{suffix}",
            job.owner.space_id
        ),
        body,
    )
    .await
}

fn update_remote(data: &Path, job: &mut Job, result: serde_json::Value, expected: &str) {
    let Some(id) = result["id"].as_i64().filter(|id| *id > 0) else {
        return;
    };
    job.remote_id = Some(id);
    match result["status"].as_str().unwrap_or("") {
        "ready" => {
            job.status = "completed".into();
            job.message = "云端总结已完成。".into();
            job.body["messages"] = serde_json::json!([]);
        }
        "failed" => {
            job.status = "failed".into();
            job.message = result["error"]
                .as_str()
                .unwrap_or("总结生成失败，请重试。")
                .into();
        }
        "queued" | "running" => {
            job.status = "processing".into();
            job.message = "云端正在生成总结，可继续使用应用。".into();
        }
        _ => return,
    }
    job.result = Some(result);
    job.updated_at = Utc::now().to_rfc3339();
    let _ = store(data, job, Some(expected));
}

pub(crate) fn sync_cloud_records(app: &AppHandle, rows: &[serde_json::Value]) {
    if let Ok(jobs) = current(app) {
        for mut job in jobs
            .into_iter()
            .filter(|j| j.engine == "cloud" && j.status == "processing")
        {
            if let Some(row) = rows.iter().find(|r| r["id"].as_i64() == job.remote_id) {
                update_remote(&app_data(app), &mut job, row.clone(), "processing");
            }
        }
    }
}

pub(crate) fn forget_remote(app: &AppHandle, id: i64) -> Result<(), String> {
    for job in current(app)?.iter().filter(|j| j.remote_id == Some(id)) {
        task_store::open(&app_data(app))?
            .execute(
                "DELETE FROM summary_tasks WHERE id=?1 AND owner=?2",
                params![job.id, owner_key(&job.owner)],
            )
            .map_err(|e| e.to_string())?;
    }
    emit_task_runtime(app);
    Ok(())
}

pub(crate) fn snapshots(app: &AppHandle) -> Vec<serde_json::Value> {
    current(app)
        .unwrap_or_default()
        .iter()
        .take(100)
        .map(|job| {
            let progress = job.batches.as_ref().map(|state| {
                let (completed, total) = state.counts();
                serde_json::json!({"completed": completed, "total": total})
            });
            serde_json::json!({
                "id": job.id, "kind": "summary", "account_id": job.owner.account_id,
                "status": job.status, "message": job.message, "updated_at": job.updated_at,
                "title": job.title, "engine": job.engine, "initiator_id": job.owner.user_id,
                "caller": job.agent.as_ref().map(|agent| agent::agent_name(&agent.provider_id))
                    .unwrap_or_else(|| "云端模型".into()),
                "scope": {"usernames": []}, "summary_progress": progress,
            })
        })
        .collect()
}

pub(crate) fn cleanup(data: &Path) -> Result<(), String> {
    let db = task_store::open(data)?;
    if !db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='summary_tasks')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|e| e.to_string())?
    {
        return Ok(());
    }
    let cutoff = (Utc::now() - chrono::Duration::days(30)).to_rfc3339();
    db.execute("DELETE FROM summary_tasks WHERE status IN ('completed','failed') AND (updated_at<?1 OR id IN (SELECT id FROM (SELECT id,ROW_NUMBER() OVER(PARTITION BY owner ORDER BY updated_at DESC) AS rank FROM summary_tasks WHERE status IN ('completed','failed')) WHERE rank>100))",[cutoff]).map_err(|e|e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn job() -> Job {
        Job {
            id: "test-id".into(),
            owner: settings::UploadOwner {
                base_url: "http://localhost:7000".into(),
                user_id: 1,
                device_id: "device".into(),
                space_id: "space".into(),
                account_id: "source".into(),
            },
            engine: "local".into(),
            agent: None,
            title: "Group".into(),
            username: "group".into(),
            status: "queued".into(),
            message: String::new(),
            updated_at: Utc::now().to_rfc3339(),
            created_at: Utc::now().to_rfc3339(),
            input_hash: "hash".into(),
            body: serde_json::json!({"messages":[]}),
            remote_id: None,
            result: None,
            prompt: None,
            config_retried: false,
            batches: None,
        }
    }
    #[test]
    fn intent_is_durable_and_status_owner_compare_and_swap_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = job();
        assert!(store(dir.path(), &j, None).unwrap());
        assert!(!store(dir.path(), &j, None).unwrap());
        assert_eq!(all(dir.path()).unwrap().len(), 1);
        j.status = "running".into();
        assert!(store(dir.path(), &j, Some("queued")).unwrap());
        let mut other = j.clone();
        other.owner.user_id = 2;
        other.status = "completed".into();
        assert!(!store(dir.path(), &other, Some("running")).unwrap());
        assert!(!store(dir.path(), &j, Some("queued")).unwrap());
        recover(dir.path()).unwrap();
        assert_eq!(all(dir.path()).unwrap()[0].status, "failed");
        j.status = "completed".into();
        assert!(
            !store(dir.path(), &j, Some("running")).unwrap(),
            "A late completion cannot overwrite interrupted work"
        );
    }
    #[test]
    fn cloud_work_survives_restart_and_cleanup_never_removes_pending_work() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = job();
        j.engine = "cloud".into();
        j.status = "processing".into();
        j.updated_at = (Utc::now() - chrono::Duration::days(40)).to_rfc3339();
        store(dir.path(), &j, None).unwrap();
        recover(dir.path()).unwrap();
        cleanup(dir.path()).unwrap();
        assert_eq!(all(dir.path()).unwrap()[0].status, "processing");
        j.status = "completed".into();
        store(dir.path(), &j, Some("processing")).unwrap();
        cleanup(dir.path()).unwrap();
        assert!(all(dir.path()).unwrap().is_empty());
    }
}
