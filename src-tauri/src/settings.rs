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
    pub connectors: ConnectorSettings,
    pub devify: DevifySettings,
    pub automation: AutomationSettings,
    pub agent: AgentSettings,
    pub keys: KeysSettings,
    pub sync: SyncSettings,
    pub voice: VoiceUploadSettings,
    pub backup: BackupSettings,
    pub language: LanguageSettings,
    pub upload_queue: Vec<UploadJob>,
}

/// When the automatic backup runs, and when it is allowed to run.
///
/// ``mode`` is ``interval`` (every ``chat.sync_interval_seconds``), ``times`` (at each entry of
/// ``times``, local time) or ``manual`` (never on its own). The window and weekday list only
/// apply when ``window_enabled`` is set: ``deny`` pauses uploads inside the window, ``allow``
/// restricts them to it.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct BackupSettings {
    pub mode: String,
    pub times: Vec<String>,
    pub window_enabled: bool,
    pub window_mode: String,
    pub window_start: String,
    pub window_end: String,
    /// 0 = 周一 … 6 = 周日, matching ``chrono``'s num_days_from_monday.
    pub days: Vec<u8>,
    /// Skip automatic runs unless the active link is Wi-Fi or wired. Tethering a phone
    /// counts as metered, so it is refused instead of burning the phone's data plan.
    pub wifi_only: bool,
    /// ``text`` (no attachment bytes), ``images`` (text + pictures) or ``all``
    /// (pictures plus video, files and voice audio up to [`ATTACHMENT_LIMIT_BYTES`]).
    pub attachments: String,
}

/// Per-attachment ceiling for the ``all`` scope. Bigger files stay referenced by their
/// message but are uploaded as ``omitted`` parts.
pub const ATTACHMENT_LIMIT_BYTES: u64 = 25 * 1024 * 1024;

/// Output language for generated text. ``ui`` is the interface language, ``summary`` what the
/// agent writes for summaries and daily reports, ``ask`` the preferred answer language.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct LanguageSettings {
    pub ui: String,
    pub summary: String,
    pub ask: String,
}

impl Default for LanguageSettings {
    fn default() -> Self {
        Self {
            ui: "中文".into(),
            summary: "跟随界面".into(),
            ask: "跟随提问".into(),
        }
    }
}

impl Default for BackupSettings {
    fn default() -> Self {
        Self {
            mode: "interval".into(),
            times: vec!["18:00".into()],
            window_enabled: false,
            window_mode: "deny".into(),
            window_start: "23:00".into(),
            window_end: "07:00".into(),
            days: (0..7).collect(),
            wifi_only: true,
            attachments: "images".into(),
        }
    }
}

/// Where a conversation's last successful upload ended.
///
/// ``time`` is the newest message time already uploaded, and ``local_id`` is that message's
/// identifier. The time alone cannot tell "nothing new" from "a new message landed in the
/// same second", so the pair decides whether a run is needed. Uploads still start at
/// ``time`` (inclusive) and the server deduplicates by ``message_key``.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
#[serde(default)]
pub struct SyncCursor {
    pub time: i64,
    pub local_id: i64,
    pub boundary_count: Option<usize>,
}

impl SyncCursor {
    /// True when ``newest`` is not newer than what was already uploaded.
    pub fn covers(&self, newest: (i64, i64)) -> bool {
        newest.0 < self.time || (newest.0 == self.time && newest.1 <= self.local_id)
    }

    pub fn covers_boundary(&self, newest: (i64, i64), count: usize) -> bool {
        newest.0 < self.time || (newest.0 == self.time && self.covers(newest) && self.boundary_count.is_some_and(|saved| count <= saved))
    }
}

/// Incremental upload state for the allowlisted conversations.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct VoiceUploadSettings {
    /// Automatically decode and transcribe voice messages during upload preparation.
    pub auto_transcribe: bool,
    /// Include successful voice transcripts as text parts in the upload archive.
    pub upload_transcripts: bool,
}

impl Default for VoiceUploadSettings {
    fn default() -> Self {
        Self {
            auto_transcribe: true,
            upload_transcripts: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct SyncSettings {
    pub enabled: bool,
    /// account id → conversation username → upload position.
    pub cursors: HashMap<String, HashMap<String, SyncCursor>>,
    pub last_run_at: Option<String>,
    /// idle | syncing | success | failed
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub last_message_count: u64,
    /// account id → conversation username → voice messages without a successful transcript.
    /// These are retried once automatic transcription becomes available.
    pub pending_voice: HashMap<String, HashMap<String, Vec<VoiceRef>>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct VoiceRef {
    pub local_id: i64,
    pub time: i64,
    pub server_id: i64,
    #[serde(default)]
    pub source_shard: String,
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            cursors: HashMap::new(),
            last_run_at: None,
            last_status: None,
            last_error: None,
            last_message_count: 0,
            pending_voice: HashMap::new(),
        }
    }
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
    /// Per-conversation newest message (time, local id) included in this package.
    ///
    /// Only completed jobs count, and only the chat list reads this: it answers "does the
    /// local history hold anything newer than what was already uploaded". The incremental
    /// sync keeps its own cursor and is never advanced from here.
    pub covered: BTreeMap<String, SyncCursor>,
    pub initial_baselines: BTreeMap<String, SyncCursor>,
    pub sync_advance: bool,
    pub voice_processed: BTreeMap<String, Vec<VoiceRef>>,
    pub voice_pending: BTreeMap<String, Vec<VoiceRef>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct MessageRef {
    pub local_id: i64,
    pub time: i64,
    #[serde(default)]
    pub source_shard: String,
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
            covered: BTreeMap::new(),
            initial_baselines: BTreeMap::new(),
            sync_advance: false,
            voice_processed: BTreeMap::new(),
            voice_pending: BTreeMap::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct ConnectorSettings {
    /// Candidate sources the user asked to be told about once they ship. Local only: the
    /// desktop app has no notification service, so this is a remembered list, not a promise.
    pub waitlist: Vec<String>,
    /// When false the app stops reading WeChat (and stops automatic backups) but keeps the
    /// stored keys and decrypted snapshot, so reconnecting needs no new key import. Uploaded
    /// records on the server are never touched.
    pub wechat_enabled: bool,
}

impl Default for ConnectorSettings {
    fn default() -> Self {
        Self {
            waitlist: Vec::new(),
            wechat_enabled: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct ChatPolicy {
    pub allowed_usernames: HashMap<String, Vec<String>>,
    pub initial_ranges: HashMap<String, HashMap<String, InitialRange>>,
    pub sync_interval_seconds: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct InitialRange {
    pub since: Option<i64>,
    pub until: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct DevifySettings {
    pub base_url: String,
    pub device_label: String,
    pub device_id: String,
    pub platform: String,
    pub active_space_id: Option<i64>,
    pub recipient: String,
    pub subject: String,
}

/// Normalize a service address for request building.
///
/// A brand-new install has no server, so an empty value must fail with an
/// actionable message instead of sending a request to a relative path.
pub fn normalize_base_url(value: &str) -> Result<String, String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        return Err("请先在登录窗口填写 Devify 服务地址。".into());
    }
    Ok(value.to_string())
}

impl DevifySettings {
    pub fn resolved_base_url(&self) -> Result<String, String> {
        normalize_base_url(&self.base_url)
    }
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
            connectors: ConnectorSettings::default(),
            devify: DevifySettings::default(),
            automation: AutomationSettings::default(),
            agent: AgentSettings::default(),
            keys: KeysSettings::default(),
            sync: SyncSettings::default(),
            voice: VoiceUploadSettings::default(),
            backup: BackupSettings::default(),
            language: LanguageSettings::default(),
            upload_queue: Vec::new(),
        }
    }
}

impl Default for ChatPolicy {
    fn default() -> Self {
        Self {
            allowed_usernames: HashMap::new(),
            initial_ranges: HashMap::new(),
            sync_interval_seconds: 1800,
        }
    }
}

pub fn device_default_label() -> String {
    hostname::get()
        .ok()
        .and_then(|name| name.into_string().ok())
        .map(|name| name.trim_end_matches(".local").trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "AIMyChats Desktop".to_string())
}

/// Use the machine name unless the user has given the device its own label.
pub fn resolved_device_label(settings: &DevifySettings) -> String {
    let label = settings.device_label.trim();
    if label.is_empty() || label == "AIMyChats" || label == "AIMyChats Desktop" {
        device_default_label()
    } else {
        label.to_string()
    }
}

impl Default for DevifySettings {
    fn default() -> Self {
        Self {
            // No server is bundled with the app: the user types it on the login gate.
            base_url: String::new(),
            device_label: device_default_label(),
            device_id: String::new(),
            platform: default_platform(),
            active_space_id: None,
            recipient: String::new(),
            subject: "微信工作日报".to_string(),
        }
    }
}

/// Stable platform name sent to Devify and shown on the web console.
pub fn default_platform() -> String {
    match std::env::consts::OS {
        "macos" => "macos".to_string(),
        "windows" => "windows".to_string(),
        "linux" => "linux".to_string(),
        other => other.to_string(),
    }
}

impl Default for AutomationSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            daily_summary_enabled: true,
            daily_time: "08:30".to_string(),
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
    let mut settings: AppSettings =
        serde_json::from_str(&content).map_err(|e| format!("应用策略格式无效：{e}"))?;
    // Older builds stored a 30-second default the stepper could never display (it rounds to
    // whole minutes). Fold it back to the 30-minute default, otherwise the next save fails
    // validation and the user is stuck on a value they cannot see or change.
    if settings.chat.sync_interval_seconds < 60 {
        settings.chat.sync_interval_seconds = 1800;
    }
    Ok(settings)
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
    // The UI lets the user pick a whole number of minutes or hours between 1 and 999, which
    // is stored in seconds. Anything shorter than a minute cannot be displayed or stepped.
    if !(60..=3_596_400).contains(&settings.chat.sync_interval_seconds) {
        return Err("同步间隔必须在 1 分钟到 999 小时之间。".into());
    }
    let backup = &settings.backup;
    if !matches!(backup.mode.as_str(), "interval" | "times" | "manual") {
        return Err("备份方式必须是按间隔、每天定点或仅手动。".into());
    }
    if backup.times.len() > 24 {
        return Err("每天定点最多 24 个时间。".into());
    }
    if backup.mode == "times" && backup.times.is_empty() {
        return Err("按每天定点备份时至少需要一个时间。".into());
    }
    if backup.times.iter().any(|time| !is_valid_time(time)) {
        return Err("备份时间必须使用 HH:MM 格式。".into());
    }
    if !matches!(backup.window_mode.as_str(), "deny" | "allow") {
        return Err("限制方式必须是只在此时段上传或此时段内不上传。".into());
    }
    if !is_valid_time(&backup.window_start) || !is_valid_time(&backup.window_end) {
        return Err("限制时段必须使用 HH:MM 格式。".into());
    }
    if backup.days.len() > 7 || backup.days.iter().any(|day| *day > 6) {
        return Err("生效日必须是周一到周日之间的 0 到 7 天。".into());
    }
    if backup.window_enabled && backup.days.is_empty() {
        return Err("限制上传时间时至少要选择一个生效日。".into());
    }
    if !matches!(backup.attachments.as_str(), "text" | "images" | "all") {
        return Err("附件范围必须是仅文本、文本 + 图片或全部。".into());
    }
    if !matches!(settings.language.ui.as_str(), "中文" | "English") {
        return Err("界面语言必须是中文或 English。".into());
    }
    if !matches!(
        settings.language.summary.as_str(),
        "跟随界面" | "中文" | "English" | "按原文语言"
    ) {
        return Err("总结与报告语言取值无效。".into());
    }
    if !matches!(
        settings.language.ask.as_str(),
        "跟随提问" | "跟随界面" | "中文" | "English"
    ) {
        return Err("问答回答语言取值无效。".into());
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
    if settings.chat.initial_ranges.len() > 20 || settings.chat.initial_ranges.values().any(|ranges| ranges.len() > 1000 || ranges.iter().any(|(name, range)| name.is_empty() || name.len() > 512 || range.since.zip(range.until).is_some_and(|(start, end)| start >= end))) {
        return Err("首次上传日期范围无效。".into());
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
    validate_sync(&settings.sync)?;
    Ok(())
}

fn validate_sync(sync: &SyncSettings) -> Result<(), String> {
    if sync.cursors.len() > 20 {
        return Err("最多支持 20 个微信账号的同步进度。".into());
    }
    for cursors in sync.cursors.values() {
        if cursors.len() > 1000
            || cursors.iter().any(|(username, cursor)| {
                username.is_empty()
                    || username.len() > 512
                    || cursor.time < 0
                    || cursor.local_id < 0
            })
        {
            return Err("同步进度无效，请重新保存自动化策略。".into());
        }
    }
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

    #[test]
    fn initial_history_ranges_round_trip_and_reject_reversed_dates() {
        let mut settings = AppSettings::default();
        settings.chat.initial_ranges.entry("acct".into()).or_default().insert("chat".into(), InitialRange { since: Some(100), until: Some(200) });
        let restored: AppSettings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(restored.chat.initial_ranges["acct"]["chat"].since, Some(100));
        settings.chat.initial_ranges.get_mut("acct").unwrap().get_mut("chat").unwrap().until = Some(100);
        assert!(validate(&settings).is_err());
    }

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
    fn base_url_is_normalized_and_empty_is_rejected() {
        assert!(normalize_base_url("").is_err());
        assert!(normalize_base_url("  /  ").is_err());
        assert_eq!(
            normalize_base_url(" https://example.test/ ").unwrap(),
            "https://example.test"
        );
    }

    #[test]
    fn retry_retains_original_scope_despite_new_allowlist_entries() {        let (settings, job) = retry_fixture();
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
                source_shard: String::new(),
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
                source_shard: String::new(),
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
    fn defaults_keep_automation_off_and_use_design_daily_time() {
        let settings = AppSettings::default();
        assert!(!settings.automation.enabled);
        assert_eq!(settings.automation.daily_time, "08:30");
        assert_eq!(settings.chat.sync_interval_seconds, 1800);
        assert!(settings.chat.allowed_usernames.is_empty());
        assert!(settings.voice.auto_transcribe);
        assert!(settings.voice.upload_transcripts);
    }

    #[test]
    fn connectors_start_connected_and_disconnect_is_a_plain_switch() {
        let settings = AppSettings::default();
        assert!(settings.connectors.wechat_enabled);
        assert!(settings.connectors.waitlist.is_empty());

        // Disconnecting must remain a valid stored state: keys and the decrypted snapshot
        // stay on disk, only the read gate flips.
        let mut disconnected = AppSettings::default();
        disconnected.connectors.wechat_enabled = false;
        assert!(validate(&disconnected).is_ok());
    }

    #[test]
    fn validation_rejects_invalid_schedule_and_interval() {
        let mut settings = AppSettings::default();
        settings.chat.sync_interval_seconds = 0;
        assert!(validate(&settings).is_err());

        settings.chat.sync_interval_seconds = 1800;
        settings.automation.daily_time = "25:00".to_string();
        assert!(validate(&settings).is_err());

        // The stepper works in whole minutes/hours, so a sub-minute stored value must be
        // rejected rather than shown as a rounded 1 minute.
        settings.automation.daily_time = "08:30".to_string();
        settings.chat.sync_interval_seconds = 30;
        assert!(validate(&settings).is_err());
        settings.chat.sync_interval_seconds = 3_596_401;
        assert!(validate(&settings).is_err());
    }

    #[test]
    fn sync_defaults_are_off_and_cursors_are_bounded() {
        let mut settings = AppSettings::default();
        assert!(!settings.sync.enabled);
        assert!(settings.sync.cursors.is_empty());
        assert!(settings.sync.pending_voice.is_empty());

        settings.sync.enabled = true;
        settings.sync.cursors.insert(
            "account".into(),
            HashMap::from([(
                "group".to_string(),
                SyncCursor {
                    boundary_count: None,
                    time: 1_700_000_000,
                    local_id: 42,
                },
            )]),
        );
        assert!(validate(&settings).is_ok());

        settings.sync.cursors.get_mut("account").unwrap().insert(
                "bad".into(),
                SyncCursor {
                    boundary_count: None,
                    time: -1,
                    local_id: 0,
                },
            );
        assert!(validate(&settings).is_err());
    }

    #[test]
    fn sync_cursor_covers_messages_at_or_before_its_position() {
        let cursor = SyncCursor {
            boundary_count: None,
            time: 100,
            local_id: 7,
        };
        assert!(cursor.covers((99, 900)));
        assert!(cursor.covers((100, 7)));
        assert!(!cursor.covers((100, 8)));
        assert!(!cursor.covers((101, 1)));
        assert!(!cursor.covers_boundary((100, 7), 1));
        let cursor = SyncCursor { boundary_count: Some(2), ..cursor };
        assert!(cursor.covers_boundary((100, 7), 2));
        assert!(!cursor.covers_boundary((100, 7), 3));
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
