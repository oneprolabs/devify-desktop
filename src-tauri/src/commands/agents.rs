//! Local Agent discovery, access setup and summary invocation through native IPC.
use crate::chat::access::{allowed_usernames, whatsapp_allowed_usernames};
use crate::mcp::runtime::{access_state, McpState, McpStatus};
use crate::wechat::{decrypt, query};
use crate::{access, agent, app_data, capabilities, entitlements, mcp, settings, whatsapp};
use serde::Serialize;
use tauri::{AppHandle, State};

#[tauri::command]
pub(crate) fn mcp_status(state: State<McpState>) -> Result<McpStatus, String> {
    state
        .status
        .lock()
        .map(|status| status.clone())
        .map_err(|_| "读取 MCP 服务状态失败。".to_string())
}

/// The agent access kit installs a read-only CLI into other agents; it needs both an
/// internal account and the bridge capability. A missing snapshot means "no".
fn require_internal_agent(data: &std::path::Path, base_url: &str) -> Result<(), String> {
    entitlements::require_agent_access(data, base_url)
}

#[tauri::command]
pub(crate) async fn agent_access_status(
    app: AppHandle,
    state: State<'_, McpState>,
) -> Result<access::AccessStatus, String> {
    let service = access_state(&state)?;
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    require_internal_agent(&data, &settings.devify.base_url)?;
    tauri::async_runtime::spawn_blocking(move || {
        access::configured_status(
            access::status(
                &data,
                &settings.agent.provider_id,
                &service.address,
                service.running,
                service.reason,
            ),
            &settings.agent,
        )
    })
    .await
    .map_err(|error| format!("读取 Skills 状态失败：{error}"))
}

/// Install only the app-owned read-only CLI and bundled Skills. No shell script or network download.
#[tauri::command]
pub(crate) async fn agent_access_install(
    app: AppHandle,
    state: State<'_, McpState>,
    tool: String,
    skills_dir: Option<String>,
) -> Result<access::install::InstallResult, String> {
    let data = app_data(&app);
    let value = settings::load(&data)?;
    require_internal_agent(&data, &value.devify.base_url)?;
    let service = access_state(&state)?;
    if !service.running {
        return Err("本机接入暂不可用，请确认已登录并重新打开智能体集成。".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let status = access::configured_status(
            access::status(
                &data,
                &value.agent.provider_id,
                &service.address,
                service.running,
                service.reason,
            ),
            &value.agent,
        );
        let target = access::installation_target(&status.targets, &tool, skills_dir.as_deref())?;
        let cli =
            access::resolve_cli(&data).ok_or("缺少本机读取工具，请重新安装完整版 AIMyChats。")?;
        access::install::install(&data, &target, &cli, &status.base_url)
    })
    .await
    .map_err(|error| format!("安装 Skills 失败：{error}"))?
}

#[tauri::command]
pub(crate) fn agent_access_prompt(
    app: AppHandle,
    state: State<McpState>,
    tool: String,
    bin_dir: String,
    skills_dir: String,
) -> Result<access::PromptOut, String> {
    if crate::build_flavor::DEVELOPMENT {
        return Err("开发版请使用页面中的「安装 Skills」，避免覆盖正式版工具。".into());
    }
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    require_internal_agent(&data, &settings.devify.base_url)?;
    let service = access_state(&state)?;
    let targets = access::targets();
    let target = access::target_by_id(&targets, &tool).unwrap_or_else(|| {
        access::target_by_id(&targets, "custom")
            .unwrap_or_else(|| targets[targets.len() - 1].clone())
    });
    let bin = access::expand_path(&bin_dir)
        .ok_or_else(|| "CLI 目录必须是绝对路径或以 ~/ 开头。".to_string())?;
    let skills = access::expand_path(&skills_dir)
        .ok_or_else(|| "Skills 目录必须是绝对路径或以 ~/ 开头。".to_string())?;
    let cli = access::resolve_cli(&data);
    let base_url = access::client_base(&service.address);
    let prompt = access::build_prompt(
        &target,
        &bin,
        &skills,
        &base_url,
        &mcp::token_path(&data),
        service.running,
        cli.is_some(),
    );
    Ok(access::PromptOut {
        prompt,
        bin_dir: bin.to_string_lossy().to_string(),
        skills_dir: skills.to_string_lossy().to_string(),
    })
}

/// Prepare an app-scoped access kit; the receiver chooses its own Skills directory.
#[tauri::command]
pub(crate) async fn agent_access_installation_prompt(
    app: AppHandle,
    state: State<'_, McpState>,
) -> Result<String, String> {
    let data = app_data(&app);
    let value = settings::load(&data)?;
    require_internal_agent(&data, &value.devify.base_url)?;
    let service = access_state(&state)?;
    if !service.running {
        return Err("本机接入暂不可用，请确认已登录并重新打开智能体集成。".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let cli = access::resolve_cli(&data).ok_or("缺少本机读取工具，请重新安装完整版 AIMyChats。")?;
        let endpoint = access::client_base(&service.address);
        let kit = access::install::prepare_prompt_kit(&data, &cli, &endpoint)?;
        Ok(access::prompt::build(&kit, &data, &endpoint))
    }).await.map_err(|error| format!("准备安装提示词失败：{error}"))?
}

/// Rebuild `aimychats-cli` from the source checkout so the download endpoint has something to serve.
/// Packaged installs without source get a clear error instead of a silent failure.
#[tauri::command]
pub(crate) async fn agent_build_wxq(app: AppHandle) -> Result<String, String> {
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    require_internal_agent(&data, &settings.devify.base_url)?;
    let Some(dir) = access::source_dir() else {
        return Err("当前安装没有附带源码，无法在应用内构建 aimychats-cli。".into());
    };
    let build_dir = dir.clone();
    let output = tauri::async_runtime::spawn_blocking(move || {
        std::process::Command::new("cargo")
            .args([
                "build",
                "--release",
                "--bin",
                access::CLI_NAME,
                "--features",
                if crate::build_flavor::DEVELOPMENT { "aux-binaries,dev-app" } else { "aux-binaries" },
            ])
            .current_dir(&build_dir)
            .output()
    })
    .await
    .map_err(|error| format!("构建任务失败：{error}"))?
    .map_err(|error| format!("启动 cargo 失败：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .trim()
            .chars()
            .rev()
            .take(900)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        return Err(format!("构建 aimychats-cli 失败：\n{tail}"));
    }
    let path = dir.join("target/release").join(access::CLI_NAME);
    if !path.is_file() {
        return Err("cargo 返回成功，但没有找到 target/release/aimychats-cli。".into());
    }
    if crate::build_flavor::DEVELOPMENT {
        let owned = access::install::prepare_cli(&data, &path)?;
        return Ok(owned.to_string_lossy().to_string());
    }
    Ok(path.to_string_lossy().to_string())
}

#[derive(Serialize)]
pub(crate) struct AgentCurrent {
    configured: bool,
    agent_id: String,
    agent_name: String,
    model: String,
    command: Option<String>,
    available: bool,
}

#[tauri::command]
pub(crate) async fn agent_current(app: AppHandle) -> Result<AgentCurrent, String> {
    let configured_settings = settings::load(&app_data(&app))?.agent;
    tauri::async_runtime::spawn_blocking(move || {
        let configured = configured_settings.is_enabled();
        let selected = agent::selected(&configured_settings).ok();
        let path = selected.as_ref().and_then(agent::resolve);
        Ok(AgentCurrent {
            configured,
            agent_id: selected
                .as_ref()
                .map(|value| value.provider_id.clone())
                .unwrap_or_default(),
            agent_name: selected
                .as_ref()
                .map(|value| agent::agent_name(&value.provider_id))
                .unwrap_or_default(),
            model: selected
                .as_ref()
                .map(|value| value.model.clone())
                .unwrap_or_default(),
            command: path.as_ref().map(|path| path.to_string_lossy().to_string()),
            available: path.is_some(),
        })
    })
    .await
    .map_err(|error| format!("读取智能体状态失败：{error}"))?
}

#[tauri::command]
pub(crate) async fn agent_scan(app: AppHandle) -> Result<Vec<agent::AgentInfo>, String> {
    let agent = settings::load(&app_data(&app))?.agent;
    tauri::async_runtime::spawn_blocking(move || agent::scan(&agent))
        .await
        .map_err(|error| format!("扫描本地 Agent 失败：{error}"))
}

/// Models the selected CLI reports as usable, so the UI can offer a list instead of
/// asking people to memorise provider/model strings.
#[tauri::command]
pub(crate) async fn agent_models(
    app: AppHandle,
    value: settings::AgentSettings,
) -> Result<Vec<String>, String> {
    let data = app_data(&app);
    tauri::async_runtime::spawn_blocking(move || agent::list_models(&value, &data))
        .await
        .map_err(|error| format!("读取模型列表失败：{error}"))?
}

#[tauri::command]
pub(crate) async fn agent_test(
    app: AppHandle,
    value: settings::AgentSettings,
) -> Result<agent::AgentRun, String> {
    let data = app_data(&app);
    tauri::async_runtime::spawn_blocking(move || agent::test(&value, &data))
        .await
        .map_err(|error| format!("测试 Agent 失败：{error}"))?
}

#[derive(Serialize)]
pub(crate) struct AgentSummary {
    agent_id: String,
    agent_name: String,
    model: String,
    output: String,
    sessions: usize,
    messages: usize,
    characters: usize,
    truncated: bool,
    elapsed_ms: u64,
}

/// Summarise the selected conversations with the configured local agent.
///
/// Chat content leaves the app only through that agent's own CLI. The whitelist is
/// re-checked here, not only in the UI, because the transcript contains message bodies.
/// `messages` carries exactly the preview the user reviewed; when it is absent the
/// transcript is rebuilt from the decrypted cache for the given range.
#[tauri::command]
pub(crate) async fn agent_summarize(
    app: AppHandle,
    account_id: String,
    usernames: Vec<String>,
    messages: Option<Vec<query::SummaryMessage>>,
    since: Option<i64>,
    until: Option<i64>,
) -> Result<AgentSummary, String> {
    let data = app_data(&app);
    let value = settings::load(&data)?;
    entitlements::require(
        &data,
        &value.devify.base_url,
        capabilities::AI_AGENT_SUMMARY,
    )?;
    let agent_settings = agent::selected(&value.agent)?;
    // Each source has its own connect switch, so a WhatsApp summary must not be refused
    // just because WeChat happens to be disconnected.
    let allowed = if whatsapp::is_account(&account_id) {
        whatsapp_allowed_usernames(&app, &account_id)?
    } else {
        allowed_usernames(&app, &account_id)?
    };
    if usernames.iter().any(|username| !allowed.contains(username)) {
        return Err("总结内容包含不在当前策略白名单中的会话。".into());
    }
    let input = match messages {
        Some(list) if !list.is_empty() => {
            if list
                .iter()
                .any(|message| !allowed.contains(&message.username))
            {
                return Err("总结内容包含不在当前策略白名单中的会话。".into());
            }
            let input = query::summarize_messages(list);
            if input.messages == 0 {
                return Err("没有可总结的文字消息。".into());
            }
            input
        }
        _ => {
            let cache = decrypt::cache_dir(&data, &account_id);
            query::summary_input(&cache, &usernames, since, until, Some(&allowed))?
        }
    };
    let stats = (
        input.sessions,
        input.messages,
        input.characters,
        input.truncated,
    );
    let selected_id = agent_settings.provider_id.clone();
    let selected_model = agent_settings.model.clone();
    let language = value.language.summary_language().to_string();
    let run = tauri::async_runtime::spawn_blocking(move || {
        agent::summarize(&agent_settings, &input, &language, &data)
    })
    .await
    .map_err(|error| format!("Agent 总结任务失败：{error}"))??;
    Ok(AgentSummary {
        agent_id: selected_id.clone(),
        agent_name: agent::agent_name(&selected_id),
        model: selected_model,
        output: run.output,
        sessions: stats.0,
        messages: stats.1,
        characters: stats.2,
        truncated: stats.3,
        elapsed_ms: run.elapsed_ms,
    })
}
