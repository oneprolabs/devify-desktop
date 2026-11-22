//! Versioned chat-summary rules. Cache scope never grants a capability.
pub(crate) mod prompt;
mod transport;
use crate::{desktop_credentials, entitlements, settings};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
pub(crate) use transport::{cloud_request, refresh, CloudError};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Capabilities {
    pub enforced: bool,
    pub features: Vec<String>,
    pub internal: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Scope {
    pub server: String,
    /// Devify user, never the WeChat/WhatsApp source account.
    pub account_id: String,
    pub space_id: String,
    pub capabilities: Capabilities,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Budget {
    pub unit: String,
    pub max_request_bytes: usize,
    pub prompt_bytes: usize,
    pub reserved_overhead_bytes: usize,
    pub max_content_bytes: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Budgets {
    pub cloud: Budget,
    pub local_cli: Budget,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Scenario {
    pub id: String,
    pub name: String,
    pub description: String,
    pub requires_instructions: bool,
    pub template: String,
    pub sha256: String,
    pub budgets: Budgets,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Catalog {
    pub schema_version: u32,
    pub version: String,
    pub scope: Scope,
    pub default_scenario: String,
    pub scenarios: Vec<Scenario>,
    pub max_instructions_chars: usize,
    pub output_format: String,
    pub min_check_interval_seconds: i64,
    pub retry_after_seconds: i64,
    pub no_cache_behavior: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Cache {
    pub scope: Scope,
    pub config: Option<Catalog>,
    pub etag: Option<String>,
    pub checked_at: i64,
    pub retry_at: i64,
    #[serde(default)]
    pub refresh_id: String,
}

pub(crate) fn scope(data: &Path) -> Result<Scope, String> {
    let value = settings::load(data)?;
    let server = value.devify.resolved_base_url()?;
    let credentials = desktop_credentials::load(data, &server)?;
    let account_id = crate::access_token_claim(&credentials.access, "user_id")
        .filter(|id| id.parse::<i64>().is_ok_and(|id| id > 0))
        .ok_or("请先登录。")?;
    let space_id = value.devify.active_space_id.ok_or("请先选择空间。")?;
    if space_id.len() > 128 || !space_id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
        return Err("空间标识无效，请重新选择空间。".into());
    }
    let view = entitlements::effective(data, &server).ok_or("正在初始化账号权限，请稍后重试。")?;
    let mut features = view.features;
    features.sort();
    features.dedup();
    Ok(Scope {
        server,
        account_id,
        space_id,
        capabilities: Capabilities {
            enforced: view.enforced,
            features,
            internal: view.internal,
        },
    })
}

fn validate_budget(budget: &Budget) -> bool {
    budget.unit == "utf8_bytes"
        && budget.max_request_bytes <= 16 * 1024 * 1024
        && budget.prompt_bytes <= 1024 * 1024
        && budget.reserved_overhead_bytes <= 1024 * 1024
        && budget.max_content_bytes
            == budget.max_request_bytes.saturating_sub(
                budget
                    .prompt_bytes
                    .saturating_add(budget.reserved_overhead_bytes),
            )
}
pub(crate) fn validate(config: &Catalog, expected: &Scope) -> Result<(), String> {
    if config.schema_version != 1 {
        return Err("总结配置版本暂不支持，请升级客户端。".into());
    }
    if &config.scope != expected {
        return Err("总结配置所属账号或空间不一致，请重新初始化。".into());
    }
    if config.version.is_empty()
        || config.version.len() > 128
        || config.version.chars().any(char::is_control)
        || config.output_format != "markdown"
        || config.no_cache_behavior != "require_online_initialization"
        || !(1..=16000).contains(&config.max_instructions_chars)
        || !(60..=7 * 86400).contains(&config.min_check_interval_seconds)
        || !(1..=86400).contains(&config.retry_after_seconds)
        || config.scenarios.is_empty()
        || config.scenarios.len() > 32
    {
        return Err("总结配置格式不完整，请稍后重新初始化。".into());
    }
    let mut ids = std::collections::HashSet::new();
    for item in &config.scenarios {
        if item.id.is_empty()
            || item.id.len() > 64
            || !item
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
            || !ids.insert(&item.id)
            || item.name.is_empty()
            || item.name.len() > 256
            || item.description.len() > 4096
            || item.template.is_empty()
            || item.template.len() > 1024 * 1024
            || item.sha256 != hex::encode(Sha256::digest(item.template.as_bytes()))
            || !validate_budget(&item.budgets.cloud)
            || !validate_budget(&item.budgets.local_cli)
            || item.budgets.local_cli.prompt_bytes != item.template.len()
        {
            return Err("总结配置校验失败，已保留原有配置。".into());
        }
    }
    if !ids.contains(&config.default_scenario) {
        return Err("默认总结方式不可用。".into());
    }
    Ok(())
}

pub(crate) fn path(data: &Path, scope: &Scope) -> PathBuf {
    let key = hex::encode(Sha256::digest(
        serde_json::to_vec(scope).unwrap_or_default(),
    ));
    data.join("summary-config").join(format!("{key}.json"))
}
pub(crate) fn load(data: &Path, scope: &Scope) -> Option<Cache> {
    let file = fs::File::open(path(data, scope)).ok()?;
    if file.metadata().ok()?.len() > 2 * 1024 * 1024 {
        return None;
    }
    let cache: Cache = serde_json::from_reader(file).ok()?;
    if cache.scope != *scope
        || cache
            .etag
            .as_ref()
            .is_some_and(|s| s.len() > 512 || s.contains(['\r', '\n']))
    {
        return None;
    }
    if let Some(config) = &cache.config {
        validate(config, scope).ok()?;
    }
    Some(cache)
}
pub(crate) fn write(data: &Path, cache: &Cache) -> Result<(), String> {
    if let Some(config) = &cache.config {
        validate(config, &cache.scope)?;
    }
    let target = path(data, &cache.scope);
    let parent = target.parent().ok_or("总结配置目录无效。")?;
    fs::create_dir_all(parent).map_err(|_| "无法创建总结配置目录。")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|_| "无法暂存总结配置。")?;
    let bytes = serde_json::to_vec(cache).map_err(|_| "总结配置格式无效。")?;
    temp.write_all(&bytes)
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|_| "保存总结配置失败。")?;
    temp.persist(&target)
        .map_err(|_| "更新总结配置失败，原配置已保留。")?;
    #[cfg(unix)]
    fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|_| "刷新总结配置目录失败。")?;
    Ok(())
}
pub(crate) fn catalog(data: &Path) -> Result<Catalog, String> {
    let current = scope(data)?;
    load(data, &current)
        .and_then(|c| c.config)
        .ok_or_else(|| "总结方式尚未初始化，请联网后点击「重新初始化」。".into())
}

/// Cache-only UI read: opening a chat must not download templates.
#[tauri::command]
pub(crate) fn summary_config_get(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let data = crate::app_data(&app);
    let language = settings::load(&data)?
        .language
        .summary_language()
        .to_string();
    Ok(
        serde_json::json!({"config": catalog(&data)?, "language": language, "ui_language": settings::load(&data)?.language.ui,
        "instructions": settings::load(&data)?.agent.instructions}),
    )
}
#[tauri::command]
pub(crate) async fn summary_config_refresh(app: tauri::AppHandle) -> Result<(), String> {
    refresh(&app, true).await
}

#[cfg(test)]
mod tests;
