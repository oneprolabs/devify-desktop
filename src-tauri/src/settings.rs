use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

const SETTINGS_FILE: &str = "app-settings.json";

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct AppSettings {
    pub active_account_id: String,
    pub chat: ChatPolicy,
    pub devify: DevifySettings,
    pub automation: AutomationSettings,
    pub agent: AgentSettings,
    pub keys: KeysSettings,
    pub upload_queue: Vec<UploadJob>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct UploadJob {
    pub id: String,
    pub space_id: i64,
    pub account_id: String,
    pub batch_id: Option<i64>,
    pub status: String,
    pub message: String,
    pub created_at: String,
    pub updated_at: String,
    /// The exact range and allowlisted conversations chosen when the job was created.
    /// Missing for legacy jobs, which cannot be safely retried.
    pub scope: Option<UploadScope>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct MessageRef {
    pub local_id: i64,
    pub time: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UploadScope {
    pub usernames: Vec<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub base_url: String,
    #[serde(default)]
    pub message_selection: BTreeMap<String, Vec<MessageRef>>,
}

impl UploadJob {
    pub fn retry_scope<'a>(&'a self, settings: &AppSettings) -> Result<&'a UploadScope, String> {
        if self.status != "failed" {
            return Err("只能重试失败的上传任务。".into());
        }
        let scope = self
            .scope
            .as_ref()
            .ok_or("此旧任务没有保存原始范围，请重新创建采集任务。")?;
        if scope.base_url.trim_end_matches('/') != settings.devify.base_url.trim_end_matches('/') {
            return Err("服务地址已更改，请重新创建采集任务。".into());
        }
        if scope.usernames.is_empty()
            || scope
                .since
                .zip(scope.until)
                .is_some_and(|(start, end)| start >= end)
        {
            return Err("原始上传范围无效，请重新创建采集任务。".into());
        }
        let allowed = settings.chat.allowed_usernames.get(&self.account_id);
        if scope
            .usernames
            .iter()
            .any(|username| !allowed.is_some_and(|list| list.contains(username)))
        {
            return Err("原任务中的会话不再全部位于白名单，请重新创建采集任务。".into());
        }
        Ok(scope)
    }
}

impl Default for UploadJob {
    fn default() -> Self {
        Self {
            id: String::new(),
            space_id: 0,
            account_id: String::new(),
            batch_id: None,
            status: "queued".into(),
            message: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            scope: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct ChatPolicy {
    pub allowed_usernames: HashMap<String, Vec<String>>,
    pub sync_interval_seconds: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct DevifySettings {
    pub base_url: String,
    pub device_label: String,
    pub active_space_id: Option<i64>,
    pub recipient: String,
    pub subject: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct AutomationSettings {
    pub enabled: bool,
    pub daily_summary_enabled: bool,
    pub daily_time: String,
    pub last_daily_run: Option<String>,
    pub last_daily_status: Option<String>,
    pub last_daily_error: Option<String>,
}

/// Which local agent CLI performs summaries. Credentials and model selection stay in the
/// agent's own configuration; this only stores the choice, an optional pinned path and the
/// generation parameters.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct AgentSettings {
    pub provider_id: String,
    pub command: String,
    pub model: String,
    pub timeout_seconds: u64,
    pub instructions: String,
    pub use_for_daily: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            active_account_id: String::new(),
            chat: ChatPolicy::default(),
            devify: DevifySettings::default(),
            automation: AutomationSettings::default(),
            agent: AgentSettings::default(),
            keys: KeysSettings::default(),
            upload_queue: Vec::new(),
        }
    }
}

impl Default for ChatPolicy {
    fn default() -> Self {
        Self {
            allowed_usernames: HashMap::new(),
            sync_interval_seconds: 30,
        }
    }
}

impl Default for DevifySettings {
    fn default() -> Self {
        Self {
            base_url: if cfg!(debug_assertions) {
                "http://127.0.0.1:8001".to_string()
            } else {
                "https://app.aimychats.com".to_string()
            },
            device_label: "AImyChats Desktop".to_string(),
            active_space_id: None,
            recipient: String::new(),
            subject: "微信工作日报".to_string(),
        }
    }
}

impl Default for AutomationSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            daily_summary_enabled: true,
            daily_time: "18:30".to_string(),
            last_daily_run: None,
            last_daily_status: None,
            last_daily_error: None,
        }
    }
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            provider_id: String::new(),
            command: String::new(),
            model: String::new(),
            timeout_seconds: 180,
            instructions: String::new(),
            use_for_daily: true,
        }
    }
}

/// Where the reference key extractor lives, once the user picks it in the app. Keeping it in
/// settings means the guided flow never asks the user to set an environment variable and
/// restart.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct KeysSettings {
    pub extractor_path: String,
}

impl Default for KeysSettings {
    fn default() -> Self {
        Self {
            extractor_path: String::new(),
        }
    }
}

pub fn path(app_data: &Path) -> std::path::PathBuf {
    app_data.join(SETTINGS_FILE)
}

pub fn load(app_data: &Path) -> Result<AppSettings, String> {
    let file = path(app_data);
    if !file.exists() {
        return Ok(AppSettings::default());
    }
    let content = fs::read_to_string(&file).map_err(|e| format!("读取应用策略失败：{e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("应用策略格式无效：{e}"))
}

pub fn save(app_data: &Path, settings: &AppSettings) -> Result<(), String> {
    validate(settings)?;
    fs::create_dir_all(app_data).map_err(|e| format!("创建应用数据目录失败：{e}"))?;
    let file = path(app_data);
    let temp = file.with_extension("json.tmp");
    let content =
        serde_json::to_vec_pretty(settings).map_err(|e| format!("序列化应用策略失败：{e}"))?;
    fs::write(&temp, content).map_err(|e| format!("写入应用策略失败：{e}"))?;
    fs::rename(&temp, &file).map_err(|e| format!("保存应用策略失败：{e}"))?;
    Ok(())
}

pub fn validate(settings: &AppSettings) -> Result<(), String> {
    if !(1..=3600).contains(&settings.chat.sync_interval_seconds) {
        return Err("同步间隔必须在 1 到 3600 秒之间。".into());
    }
    if !is_valid_time(&settings.automation.daily_time) {
        return Err("日报时间必须使用 HH:MM 格式。".into());
    }
    if settings.devify.subject.trim().is_empty() {
        return Err("Devify 默认主题不能为空。".into());
    }
    if settings.devify.subject.len() > 200 || settings.devify.recipient.len() > 320 {
        return Err("Devify 配置过长。".into());
    }
    if settings.chat.allowed_usernames.len() > 20 {
        return Err("最多支持 20 个微信账号的白名单配置。".into());
    }
    for usernames in settings.chat.allowed_usernames.values() {
        if usernames.len() > 1000
            || usernames
                .iter()
                .any(|username| username.is_empty() || username.len() > 512)
        {
            return Err("单个账号最多配置 1000 个会话，且会话 ID 长度不能超过 512。".into());
        }
    }
    validate_agent(&settings.agent)?;
    validate_keys(&settings.keys)?;
    Ok(())
}

fn validate_keys(keys: &KeysSettings) -> Result<(), String> {
    if keys.extractor_path.chars().count() > 1024 || keys.extractor_path.contains('\n') {
        return Err("提取器路径无效。".into());
    }
    Ok(())
}

fn validate_agent(agent: &AgentSettings) -> Result<(), String> {
    if agent.provider_id.len() > 64 || agent.provider_id.chars().any(|ch| ch.is_control()) {
        return Err("Agent 标识无效。".into());
    }
    if agent.command.len() > 1024 || agent.command.contains('\n') {
        return Err("Agent 可执行文件路径无效。".into());
    }
    if agent.model.len() > 128 || agent.model.contains('\n') {
        return Err("模型名称不能超过 128 个字符，也不能包含换行。".into());
    }
    if !(30..=900).contains(&agent.timeout_seconds) {
        return Err("Agent 超时时间必须在 30 到 900 秒之间。".into());
    }
    if agent.instructions.chars().count() > 2000 {
        return Err("Agent 总结补充要求不能超过 2000 个字符。".into());
    }
    Ok(())
}

fn is_valid_time(value: &str) -> bool {
    let Some((hour, minute)) = value.split_once(':') else {
        return false;
    };
    hour.len() == 2
        && minute.len() == 2
        && hour.parse::<u8>().is_ok_and(|value| value < 24)
        && minute.parse::<u8>().is_ok_and(|value| value < 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retry_fixture() -> (AppSettings, UploadJob) {
        let mut settings = AppSettings::default();
        settings.devify.base_url = "https://example.test".into();
        settings.chat.allowed_usernames.insert(
            "account".into(),
            vec!["one".into(), "two".into(), "new".into()],
        );
        let job = UploadJob {
            account_id: "account".into(),
            status: "failed".into(),
            space_id: 7,
            scope: Some(UploadScope {
                usernames: vec!["one".into(), "two".into()],
                since: Some(100),
                until: Some(200),
                base_url: "https://example.test".into(),
                message_selection: BTreeMap::new(),
            }),
            ..UploadJob::default()
        };
        (settings, job)
    }

    #[test]
    fn retry_retains_original_scope_despite_new_allowlist_entries() {
        let (settings, job) = retry_fixture();
        let scope = job.retry_scope(&settings).unwrap();
        assert_eq!(scope.usernames, vec!["one", "two"]);
        assert_eq!((scope.since, scope.until), (Some(100), Some(200)));
    }

    #[test]
    fn retry_rejects_legacy_jobs_and_revoked_sessions() {
        let (mut settings, mut job) = retry_fixture();
        job.scope = None;
        assert!(job.retry_scope(&settings).is_err());
        let (_, original) = retry_fixture();
        settings
            .chat
            .allowed_usernames
            .insert("account".into(), vec!["one".into()]);
        assert!(original.retry_scope(&settings).is_err());
    }

    #[test]
    fn retry_rejects_different_server_or_nonfailed_job() {
        let (mut settings, mut job) = retry_fixture();
        settings.devify.base_url = "https://other.test".into();
        assert!(job.retry_scope(&settings).is_err());
        settings.devify.base_url = "https://example.test/".into();
        job.status = "processing".into();
        assert!(job.retry_scope(&settings).is_err());
    }

    #[test]
    fn upload_scope_round_trips_and_legacy_job_is_readable_but_not_retryable() {
        let (settings, mut job) = retry_fixture();
        job.scope.as_mut().unwrap().message_selection.insert(
            "one".into(),
            vec![MessageRef {
                local_id: 42,
                time: 123,
            }],
        );
        let json = serde_json::to_string(&job).unwrap();
        let restored: UploadJob = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored.retry_scope(&settings).unwrap().usernames,
            vec!["one", "two"]
        );
        assert_eq!(
            restored.retry_scope(&settings).unwrap().message_selection["one"],
            vec![MessageRef {
                local_id: 42,
                time: 123
            }]
        );
        let legacy: UploadJob =
            serde_json::from_str(r#"{"account_id":"account","status":"failed","space_id":7}"#)
                .unwrap();
        assert!(legacy.scope.is_none());
        assert!(legacy.retry_scope(&settings).is_err());
    }

    #[test]
    fn defaults_keep_automation_off_and_use_safe_daily_time() {
        let settings = AppSettings::default();
        assert!(!settings.automation.enabled);
        assert_eq!(settings.automation.daily_time, "18:30");
        assert!(settings.chat.allowed_usernames.is_empty());
    }

    #[test]
    fn validation_rejects_invalid_schedule_and_interval() {
        let mut settings = AppSettings::default();
        settings.chat.sync_interval_seconds = 0;
        assert!(validate(&settings).is_err());

        settings.chat.sync_interval_seconds = 30;
        settings.automation.daily_time = "25:00".to_string();
        assert!(validate(&settings).is_err());
    }

    #[test]
    fn agent_settings_are_bounded() {
        let mut settings = AppSettings::default();
        assert_eq!(settings.agent.timeout_seconds, 180);
        assert!(settings.agent.provider_id.is_empty());

        settings.agent.timeout_seconds = 5;
        assert!(validate(&settings).is_err());

        settings.agent.timeout_seconds = 900;
        settings.agent.instructions = "字".repeat(2001);
        assert!(validate(&settings).is_err());

        settings.agent.instructions = "重点看交付风险".to_string();
        settings.agent.provider_id = "codex".to_string();
        assert!(validate(&settings).is_ok());
    }
}
