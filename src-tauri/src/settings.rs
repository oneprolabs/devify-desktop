use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::io::Write;
use std::sync::Mutex;
use url::Url;


/// Accept both the current UUID string and the legacy numeric value. Legacy numbers point
/// at spaces that no longer exist; they become ``None`` so login can select the new space.
fn deserialize_optional_space_id<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::String(value)) => {
            let value = value.trim().to_string();
            (!value.is_empty()).then_some(value)
        }
        _ => None,
    })
}

/// Legacy numeric space ids are unverifiable after the UUID migration, so they become
/// empty and the surrounding loader drops the dependent job or sync state.
fn deserialize_space_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::String(value)) if !value.trim().is_empty() => {
            value.trim().to_string()
        }
        _ => String::new(),
    })
}

const SETTINGS_FILE: &str = "app-settings.json";
static SETTINGS_LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct AppSettings {
    /// Optimistic concurrency generation, owned by the settings store.
    pub settings_revision: u64,
    /// Version of user-editable fields, unaffected by background queue/status writes.
    pub user_settings_revision: u64,
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
    /// Identity that owns each account's local sync state. Missing legacy ownership pauses sync.
    pub owners: HashMap<String, UploadOwner>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub saved_progress: Vec<SavedSyncProgress>,
    /// account id → conversation username → upload position.
    pub cursors: HashMap<String, HashMap<String, SyncCursor>>,
    pub initial_progress: HashMap<String, HashMap<String, InitialChunk>>,
    pub continuation: HashMap<String, HashMap<String, ContinuationScope>>,
    pub last_run_at: Option<String>,
    /// idle | syncing | success | failed
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub last_message_count: u64,
    /// account id → conversation username → voice messages without a successful transcript.
    /// These are retried once automatic transcription becomes available.
    pub pending_voice: HashMap<String, HashMap<String, Vec<VoiceRef>>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SavedSyncProgress {
    account_id: String,
    owner: Option<UploadOwner>,
    cursors: HashMap<String, SyncCursor>,
    initial_progress: HashMap<String, InitialChunk>,
    continuation: HashMap<String, ContinuationScope>,
    pending_voice: HashMap<String, Vec<VoiceRef>>,
}

impl SyncSettings {
    pub fn has_progress(&self, account: &str) -> bool {
        self.cursors.get(account).is_some_and(|v| !v.is_empty())
            || self.initial_progress.get(account).is_some_and(|v| !v.is_empty())
            || self.continuation.get(account).is_some_and(|v| !v.is_empty())
            || self.pending_voice.get(account).is_some_and(|v| !v.is_empty())
    }

    pub fn has_saved_progress(&self, owner: &UploadOwner) -> bool {
        self.saved_progress.iter().any(|saved| saved.owner.as_ref() == Some(owner))
    }

    pub fn cursors_for_owner(&self, owner: &UploadOwner) -> Option<&HashMap<String, SyncCursor>> {
        if self.owners.get(&owner.account_id) == Some(owner) {
            return self.cursors.get(&owner.account_id);
        }
        self.saved_progress.iter()
            .find(|saved| saved.owner.as_ref() == Some(owner))
            .map(|saved| &saved.cursors)
    }

    pub fn activate_owner(&mut self, owner: &UploadOwner) {
        let account = &owner.account_id;
        if self.owners.get(account) == Some(owner) {
            return;
        }
        let restored = self.saved_progress.iter().position(|saved|
            saved.owner.as_ref() == Some(owner)
        ).map(|index| self.saved_progress.remove(index));
        if self.owners.contains_key(account) || self.has_progress(account) {
            self.saved_progress.push(SavedSyncProgress {
                account_id: account.clone(),
                owner: self.owners.remove(account),
                cursors: self.cursors.remove(account).unwrap_or_default(),
                initial_progress: self.initial_progress.remove(account).unwrap_or_default(),
                continuation: self.continuation.remove(account).unwrap_or_default(),
                pending_voice: self.pending_voice.remove(account).unwrap_or_default(),
            });
            self.last_run_at = None;
            self.last_status = None;
            self.last_error = None;
            self.last_message_count = 0;
        }
        if let Some(saved) = restored {
            self.cursors.insert(account.clone(), saved.cursors);
            self.initial_progress.insert(account.clone(), saved.initial_progress);
            self.continuation.insert(account.clone(), saved.continuation);
            self.pending_voice.insert(account.clone(), saved.pending_voice);
        }
        self.owners.insert(account.clone(), owner.clone());
    }
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
            owners: HashMap::new(),
            saved_progress: Vec::new(),
            cursors: HashMap::new(),
            initial_progress: HashMap::new(),
            continuation: HashMap::new(),
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
    #[serde(default, deserialize_with = "deserialize_space_id")]
    pub space_id: String,
    pub account_id: String,
    pub batch_id: Option<i64>,
    /// Verified cloud identity at task creation; legacy tasks have no owner.
    pub owner: Option<UploadOwner>,
    /// Durable intent: an unknown response requires proof of the original package before replay.
    pub create_started: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_key: Option<String>,
    /// Digest of the exact ZIP, when known; legacy jobs cannot prove an identical archive.
    pub package_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_client_version: Option<String>,
    /// Snapshot digest recorded only after the completed batch's remote owner is verified.
    /// This is a last-confirmed state, not proof that the server has not since deleted it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_completion_digest: Option<String>,
    /// A failed job verified against the server to have no batch that can still ingest.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub legacy_failure_confirmed: bool,
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
    pub initial_chunks: BTreeMap<String, InitialChunk>,
    pub sync_advance: bool,
    pub continue_sync: bool,
    /// None means the source was not recorded by an older version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation_manual: Option<bool>,
    pub manual_group: Option<String>,
    pub manual_index: usize,
    pub manual_total: usize,
    pub manual_fingerprint: Option<String>,
    pub voice_processed: BTreeMap<String, Vec<VoiceRef>>,
    pub voice_pending: BTreeMap<String, Vec<VoiceRef>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct UploadOwner {
    pub base_url: String,
    #[serde(default, deserialize_with = "deserialize_space_id")]
    pub space_id: String,
    pub account_id: String,
    pub user_id: i64,
    pub device_id: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InitialChunk {
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub baseline: SyncCursor,
    pub position: SyncCursor,
    pub final_chunk: bool,
    #[serde(default, deserialize_with = "deserialize_space_id")]
    pub space_id: String,
    pub base_url: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ContinuationScope {
    #[serde(default, deserialize_with = "deserialize_space_id")]
    pub space_id: String,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct MessageRef {
    pub local_id: i64,
    pub time: i64,
    #[serde(default)]
    pub source_shard: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct UploadScope {
    pub usernames: Vec<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub base_url: String,
    #[serde(default)]
    pub message_selection: BTreeMap<String, Vec<MessageRef>>,
}

impl UploadJob {
    pub fn can_recover_create(&self) -> bool {
        !self.legacy_failure_confirmed && self.batch_id.is_none()
            && self.create_started == Some(true) && self.package_digest.is_some()
    }

    pub fn matches_create_package(&self, key: &str, digest: &str) -> bool {
        self.package_digest.as_deref() == Some(digest)
            && self.create_key.as_deref().map_or(true, |saved| saved == key)
    }

    pub fn retry_scope<'a>(&'a self, settings: &AppSettings) -> Result<&'a UploadScope, String> {
        if self.status != "failed" {
            return Err("只能重试失败的上传任务。".into());
        }
        let scope = self
            .scope
            .as_ref()
            .ok_or("此旧任务没有保存原始范围，请重新创建采集任务。")?;
        if !same_service_url(&scope.base_url, &settings.devify.base_url) {
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
            space_id: String::new(),
            account_id: String::new(),
            batch_id: None,
            owner: None,
            create_started: None,
            create_key: None,
            package_digest: None,
            package_client_version: None,
            verified_completion_digest: None,
            legacy_failure_confirmed: false,
            status: "queued".into(),
            message: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            scope: None,
            covered: BTreeMap::new(),
            initial_baselines: BTreeMap::new(),
            initial_chunks: BTreeMap::new(),
            sync_advance: false,
            continue_sync: false,
            continuation_manual: None,
            manual_group: None,
            manual_index: 0,
            manual_total: 0,
            manual_fingerprint: None,
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
    #[serde(default, deserialize_with = "deserialize_optional_space_id")]
    pub active_space_id: Option<String>,
    pub recipient: String,
    pub subject: String,
}

/// Official product hosts share one API origin. The homepage is not that API, so requests
/// always go to [`OFFICIAL_API_BASE`]; the stored address itself is never rewritten.
const OFFICIAL_API_BASE: &str = "https://app.aimychats.com";

/// Normalize a service address for request building.
///
/// Reject an empty address instead of sending a request to a relative path.
pub fn normalize_base_url(value: &str) -> Result<String, String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        return Err("请先在登录窗口填写服务端地址。".into());
    }
    Ok(value.to_string())
}

fn official_host(value: &str) -> bool {
    Url::parse(value)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| {
            matches!(
                host.as_str(),
                "aimychats.com" | "www.aimychats.com" | "app.aimychats.com"
            )
        })
}

/// Address used for HTTP, credentials, and browser authorize/register links.
pub fn canonical_api_base(value: &str) -> Result<String, String> {
    let normalized = normalize_base_url(value)?;
    if official_host(&normalized) {
        Ok(OFFICIAL_API_BASE.to_string())
    } else {
        Ok(normalized)
    }
}

/// Address kept in the settings file and shown in the login and settings UI.
///
/// It is exactly what the user entered, only trimmed: the official host is mapped onto the
/// API origin while requests are built, so saving `https://app.aimychats.com/` must keep that
/// value instead of replacing it with the product homepage.
pub fn stored_base_url(value: &str) -> String {
    value.trim().to_string()
}

pub fn same_service_url(left: &str, right: &str) -> bool {
    match (canonical_api_base(left), canonical_api_base(right)) {
        (Ok(left), Ok(right)) => left == right,
        (Err(_), Err(_)) => left.trim() == right.trim(),
        _ => false,
    }
}

impl DevifySettings {
    pub fn resolved_base_url(&self) -> Result<String, String> {
        canonical_api_base(&self.base_url)
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
    /// Server-side TopicSubscription that owns the daily report schedule.
    pub daily_topic_id: Option<i64>,
    /// Space that owns `daily_topic_id`; an unverified old topic requires manual review.
    #[serde(default, deserialize_with = "deserialize_optional_space_id")]
    pub daily_topic_space_id: Option<String>,
    /// Last successful (or attempted) sync of the schedule to Devify.
    pub last_schedule_sync_at: Option<String>,
    /// Signature of the schedule that was last synced, used to avoid polling.
    pub last_schedule_signature: Option<String>,
    pub last_schedule_error: Option<String>,
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
            settings_revision: 0,
            user_settings_revision: 0,
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
            base_url: "https://aimychats.com/".to_string(),
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
            daily_topic_id: None,
            daily_topic_space_id: None,
            last_schedule_sync_at: None,
            last_schedule_signature: None,
            last_schedule_error: None,
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
    settings.devify.base_url = stored_base_url(&settings.devify.base_url);
    settings.devify.active_space_id = settings
        .devify
        .active_space_id
        .filter(|space_id| !space_id.trim().is_empty());
    settings.automation.daily_topic_space_id = settings
        .automation
        .daily_topic_space_id
        .filter(|space_id| !space_id.trim().is_empty());
    settings
        .upload_queue
        .retain(|job| !job.space_id.trim().is_empty());
    settings
        .sync
        .owners
        .retain(|_, owner| !owner.space_id.trim().is_empty());
    settings.sync.initial_progress.retain(|_, chunks| {
        chunks.retain(|_, chunk| !chunk.space_id.trim().is_empty());
        !chunks.is_empty()
    });
    settings.sync.continuation.retain(|_, scopes| {
        scopes.retain(|_, scope| !scope.space_id.trim().is_empty());
        !scopes.is_empty()
    });
    Ok(settings)
}

pub fn save(app_data: &Path, settings: &AppSettings) -> Result<(), String> {
    validate(settings)?;
    let _guard = SETTINGS_LOCK.lock().map_err(|_| "无法锁定应用策略。")?;
    let current = load(app_data)?;
    if current.settings_revision != settings.settings_revision {
        return Err("应用策略已被其他操作修改，请重新加载后重试。".into());
    }
    let mut updated = settings.clone();
    store_base_url(&mut updated);
    let _identity = identity_write_guard(&current, &updated)?;
    assign_revisions(&current, &mut updated)?;
    save_unlocked(app_data, &updated)?;
    if _identity.is_some() { crate::desktop_credentials::advance_generation_locked()?; }
    Ok(())
}

/// Mutate only the caller's fields while holding the shared settings transaction lock.
pub fn update(app_data: &Path, mutate: impl FnOnce(&mut AppSettings) -> Result<(), String>) -> Result<AppSettings, String> {
    let _guard = SETTINGS_LOCK.lock().map_err(|_| "无法锁定应用策略。")?;
    let current = load(app_data)?;
    let mut updated = current.clone();
    mutate(&mut updated)?;
    store_base_url(&mut updated);
    validate(&updated)?;
    let _identity = identity_write_guard(&current, &updated)?;
    assign_revisions(&current, &mut updated)?;
    save_unlocked(app_data, &updated)?;
    if _identity.is_some() { crate::desktop_credentials::advance_generation_locked()?; }
    Ok(updated)
}

/// Identity transactions use the settings, identity, credentials lock order.
/// The callback must not call settings save/update or reacquire identity.
pub(crate) fn with_identity<T>(app_data: &Path, check: impl FnOnce(&AppSettings) -> Result<T, String>) -> Result<T, String> {
    let _settings = SETTINGS_LOCK.lock().map_err(|_| "无法锁定应用策略。")?;
    let _identity = crate::UPLOAD_IDENTITY_LOCK.try_write().map_err(|_| "上传期间无法更改登录状态，请稍后重试。")?;
    check(&load(app_data)?)
}

pub(crate) fn update_identity(app_data: &Path, mutate: impl FnOnce(&mut AppSettings) -> Result<(), String>) -> Result<AppSettings, String> {
    let _settings = SETTINGS_LOCK.lock().map_err(|_| "无法锁定应用策略。")?;
    let _identity = crate::UPLOAD_IDENTITY_LOCK.try_write().map_err(|_| "上传期间无法更改登录状态，请稍后重试。")?;
    let current = load(app_data)?;
    let mut updated = current.clone();
    mutate(&mut updated)?;
    store_base_url(&mut updated);
    validate(&updated)?;
    let changed = identity_changed(&current, &updated);
    assign_revisions(&current, &mut updated)?;
    save_unlocked(app_data, &updated)?;
    if changed { crate::desktop_credentials::advance_generation_locked()?; }
    Ok(updated)
}

fn identity_changed(current: &AppSettings, next: &AppSettings) -> bool {
    current.active_account_id != next.active_account_id
        || !same_service_url(&current.devify.base_url, &next.devify.base_url)
        || current.devify.active_space_id != next.devify.active_space_id
        || current.devify.device_id != next.devify.device_id
        || current.automation.enabled != next.automation.enabled
        || current.automation.daily_summary_enabled != next.automation.daily_summary_enabled
        || current.automation.daily_time != next.automation.daily_time
}

fn identity_write_guard<'a>(current: &AppSettings, next: &AppSettings) -> Result<Option<tokio::sync::RwLockWriteGuard<'a, ()>>, String> {
    if identity_changed(current, next) {
        Ok(Some(crate::UPLOAD_IDENTITY_LOCK.try_write().map_err(|_| "上传期间无法切换账号、服务、空间或设备，请稍后重试。")?))
    } else { Ok(None) }
}

fn user_settings_view(settings: &AppSettings) -> Result<serde_json::Value, String> {
    let mut value = serde_json::to_value(settings).map_err(|e| format!("序列化应用策略失败：{e}"))?;
    if let Some(root) = value.as_object_mut() {
        root.remove("settings_revision"); root.remove("user_settings_revision"); root.remove("upload_queue");
        if let Some(sync) = root.get_mut("sync").and_then(serde_json::Value::as_object_mut) {
            for name in ["owners", "cursors", "initial_progress", "continuation", "pending_voice", "last_run_at", "last_status", "last_error", "last_message_count"] { sync.remove(name); }
        }
        if let Some(automation) = root.get_mut("automation").and_then(serde_json::Value::as_object_mut) {
            for name in ["last_daily_run", "last_daily_status", "last_daily_error", "daily_topic_id", "daily_topic_space_id", "last_schedule_sync_at", "last_schedule_signature", "last_schedule_error"] { automation.remove(name); }
        }
    }
    Ok(value)
}

fn assign_revisions(current: &AppSettings, updated: &mut AppSettings) -> Result<(), String> {
    updated.settings_revision = current.settings_revision.checked_add(1).ok_or("应用策略版本已耗尽。")?;
    updated.user_settings_revision = if user_settings_view(current)? != user_settings_view(updated)? {
        current.user_settings_revision.checked_add(1).ok_or("用户设置版本已耗尽。")?
    } else { current.user_settings_revision };
    Ok(())
}

fn store_base_url(settings: &mut AppSettings) {
    settings.devify.base_url = stored_base_url(&settings.devify.base_url);
}

fn save_unlocked(app_data: &Path, updated: &AppSettings) -> Result<(), String> {
    fs::create_dir_all(app_data).map_err(|e| format!("创建应用数据目录失败：{e}"))?;
    let content = serde_json::to_vec_pretty(&updated).map_err(|e| format!("序列化应用策略失败：{e}"))?;
    let mut temp = tempfile::NamedTempFile::new_in(app_data).map_err(|e| format!("创建临时策略失败：{e}"))?;
    temp.write_all(&content).and_then(|_| temp.as_file().sync_all()).map_err(|e| format!("写入应用策略失败：{e}"))?;
    temp.persist(path(app_data)).map_err(|e| format!("保存应用策略失败：{e}"))?;
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
        return Err("服务端默认主题不能为空。".into());
    }
    if settings.devify.subject.len() > 200 || settings.devify.recipient.len() > 320 {
        return Err("服务端配置过长。".into());
    }
    if settings.chat.allowed_usernames.len() > 20 {
        return Err("最多支持 20 个微信账号的白名单配置。".into());
    }
    if settings.chat.initial_ranges.len() > 20 || settings.chat.initial_ranges.values().any(|ranges| ranges.len() > 1000 || ranges.iter().any(|(name, range)| name.is_empty() || name.len() > 512 || range.since.zip(range.until).is_some_and(|(start, end)| start >= end))) {
        return Err("首次上传日期范围无效。".into());
    }
    for usernames in settings.chat.allowed_usernames.values() {
        if usernames.len() > 1000 {
            return Err(format!("单个账号最多配置 1000 个会话，当前选择了 {} 个。", usernames.len()));
        }
        if usernames.iter().any(|username| username.is_empty() || username.len() > 512) {
            return Err("存在空的会话 ID 或超过 512 字节的会话 ID，请检查会话数据。".into());
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

pub fn is_valid_time(value: &str) -> bool {
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

    #[test]
    fn switching_cloud_identity_preserves_only_matching_progress() {
        let mut sync = super::SyncSettings::default();
        let first = super::UploadOwner {
            base_url: "https://first.example".into(),
            space_id: "first-space".into(),
            account_id: "wechat".into(),
            user_id: 4,
            device_id: "device".into(),
        };
        let second = super::UploadOwner {
            base_url: "https://other.example".into(),
            ..first.clone()
        };
        let cursor = super::SyncCursor { time: 1, local_id: 2, boundary_count: Some(1) };
        sync.owners.insert(first.account_id.clone(), first.clone());
        sync.cursors.entry(first.account_id.clone()).or_default().insert("first-chat".into(), cursor);

        sync.activate_owner(&second);
        assert!(sync.cursors_for_owner(&second).is_none());
        assert!(sync.has_saved_progress(&first));
        sync.cursors.entry(second.account_id.clone()).or_default().insert("second-chat".into(), cursor);
        let serialized = serde_json::to_string(&sync).unwrap();
        let mut sync: super::SyncSettings = serde_json::from_str(&serialized).unwrap();
        sync.activate_owner(&first);
        assert!(sync.cursors_for_owner(&first).unwrap().contains_key("first-chat"));
        assert!(sync.cursors_for_owner(&second).unwrap().contains_key("second-chat"));
        assert!(!sync.cursors_for_owner(&first).unwrap().contains_key("second-chat"));

        let mut unknown = super::SyncSettings::default();
        unknown.cursors.entry(first.account_id.clone()).or_default().insert("legacy-chat".into(), cursor);
        unknown.activate_owner(&first);
        assert!(unknown.cursors_for_owner(&first).is_none());
        assert_eq!(unknown.saved_progress.len(), 1);
        assert!(unknown.saved_progress[0].owner.is_none());
    }

    #[test]
    fn recovering_create_requires_original_digest_and_any_saved_key() {
        let mut job = super::UploadJob {
            create_started: Some(true),
            create_key: Some("original-key".into()),
            package_digest: Some("original-digest".into()),
            ..Default::default()
        };
        assert!(job.can_recover_create());
        assert!(job.matches_create_package("original-key", "original-digest"));
        assert!(!job.matches_create_package("another-key", "original-digest"));
        assert!(!job.matches_create_package("original-key", "another-digest"));
        job.create_key = None;
        assert!(job.matches_create_package("reconstructed-key", "original-digest"));
        job.package_digest = None;
        assert!(!job.can_recover_create());
    }
    use super::*;

    #[test]
    fn legacy_numeric_space_ids_are_dropped_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let content = r#"{
            "devify": {"active_space_id": 7},
            "automation": {"daily_topic_id": 123, "daily_topic_space_id": 8},
            "upload_queue": [{
                "id": "job", "space_id": 7, "status": "failed",
                "owner": {"base_url": "https://example.test", "space_id": 7,
                          "account_id": "a", "user_id": 1, "device_id": "d"}
            }],
            "sync": {
                "owners": {"a": {"base_url": "https://example.test", "space_id": 7,
                                 "account_id": "a", "user_id": 1, "device_id": "d"}},
                "continuation": {"a": {"chat": {"space_id": 7, "base_url": "https://example.test"}}},
                "initial_progress": {"a": {"chat": {
                    "space_id": 7, "base_url": "https://example.test",
                    "since": null, "until": null, "final_chunk": false,
                    "baseline": {"time": 0, "local_id": 0},
                    "position": {"time": 0, "local_id": 0}
                }}}
            }
        }"#;
        fs::write(dir.path().join(SETTINGS_FILE), content).unwrap();

        let settings = load(dir.path()).unwrap();

        assert_eq!(settings.devify.active_space_id, None);
        assert_eq!(settings.automation.daily_topic_space_id, None);
        assert_eq!(settings.automation.daily_topic_id, Some(123));
        assert!(settings.upload_queue.is_empty());
        assert!(settings.sync.owners.is_empty());
        assert!(settings.sync.continuation.is_empty());
        assert!(settings.sync.initial_progress.is_empty());
    }

    #[test]
    fn uuid_space_ids_round_trip() {
        let mut settings = AppSettings::default();
        settings.devify.active_space_id =
            Some("38c08ab4-799f-4389-bc5b-10018ddb0e4e".into());
        let restored: AppSettings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(
            restored.devify.active_space_id.as_deref(),
            Some("38c08ab4-799f-4389-bc5b-10018ddb0e4e")
        );
    }

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
            space_id: "7".into(),
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
    fn default_server_preserves_saved_server() {
        assert_eq!(DevifySettings::default().base_url, "https://aimychats.com/");
        let saved: DevifySettings = serde_json::from_str(r#"{"base_url":"https://custom.example"}"#).unwrap();
        assert_eq!(saved.base_url, "https://custom.example");
    }

    #[test]
    fn saved_server_address_is_never_rewritten() {
        let data = tempfile::tempdir().unwrap();
        let mut settings = AppSettings::default();
        settings.devify.base_url = "  https://app.aimychats.com/  ".into();
        save(data.path(), &settings).unwrap();

        let loaded = load(data.path()).unwrap();
        assert_eq!(loaded.devify.base_url, "https://app.aimychats.com/");
        assert_eq!(loaded.devify.resolved_base_url().unwrap(), "https://app.aimychats.com");

        let mut settings = loaded.clone();
        settings.devify.base_url = "https://aimychats.com/".into();
        save(data.path(), &settings).unwrap();

        let loaded = load(data.path()).unwrap();
        assert_eq!(loaded.devify.base_url, "https://aimychats.com/");
        assert_eq!(loaded.devify.resolved_base_url().unwrap(), "https://app.aimychats.com");
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
    fn official_hosts_resolve_to_the_app_api() {
        assert_eq!(
            canonical_api_base("https://aimychats.com/").unwrap(),
            "https://app.aimychats.com"
        );
        assert_eq!(
            canonical_api_base("https://www.aimychats.com").unwrap(),
            "https://app.aimychats.com"
        );
        assert_eq!(
            canonical_api_base("https://app.aimychats.com/").unwrap(),
            "https://app.aimychats.com"
        );
        assert_eq!(
            canonical_api_base("http://127.0.0.1:18080").unwrap(),
            "http://127.0.0.1:18080"
        );
        assert!(same_service_url(
            "https://aimychats.com/",
            "https://app.aimychats.com"
        ));
        assert_eq!(stored_base_url("  https://app.aimychats.com/  "), "https://app.aimychats.com/");
        assert_eq!(stored_base_url("https://aimychats.com/"), "https://aimychats.com/");
        assert_eq!(
            AppSettings::default().devify.resolved_base_url().unwrap(),
            "https://app.aimychats.com"
        );
        assert!(same_service_url("", ""));
        let data = tempfile::tempdir().unwrap();
        crate::desktop_credentials::save(
            data.path(),
            &crate::desktop_credentials::Credentials {
                base_url: "https://app.aimychats.com".into(),
                access: "access".into(),
                refresh: "refresh".into(),
                device_session: None,
                session_device_id: None,
            },
        )
        .unwrap();
        assert_eq!(
            crate::desktop_credentials::load(data.path(), "https://aimychats.com/")
                .unwrap()
                .access,
            "access"
        );
    }

    #[test]
    fn retry_treats_official_hosts_as_the_same_server() {
        let (mut settings, mut job) = retry_fixture();
        settings.devify.base_url = "https://aimychats.com/".into();
        job.scope.as_mut().unwrap().base_url = "https://app.aimychats.com".into();
        assert!(job.retry_scope(&settings).is_ok());
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
    fn conversation_validation_distinguishes_count_and_id() {
        let mut settings = AppSettings::default();
        settings.chat.allowed_usernames.insert("account".into(), (0..1001).map(|i| format!("chat{i}")).collect());
        assert!(validate(&settings).unwrap_err().contains("1001"));
        settings.chat.allowed_usernames.insert("account".into(), vec!["x".repeat(513)]);
        assert!(validate(&settings).unwrap_err().contains("512 字节"));
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
