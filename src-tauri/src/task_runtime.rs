//! The single coordinator for durable automatic backups.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

mod wechat_recovery;

pub(super) use wechat_recovery::{
    resume_wechat_backup, verify_wechat_backup_read, wechat_backup_needs_keys,
    WechatBackupReadError, WechatBackupRecovery, WECHAT_BACKUP_RECHECK,
};

#[derive(Debug, PartialEq)]
pub(super) enum RetryRemoteAction {
    ResumeArchive,
    RetryParsing,
    Reconcile,
}

/// A frozen selection can become unavailable before any create request was sent.
/// Only retire a provably local automatic plan; sent/unknown and manual jobs must
/// retain their original identity and recovery rules. Cursors advance on server
/// completion only, so a fresh bounded scan cannot skip the abandoned selection.
pub(super) fn retire_stale_plan(
    value: &mut settings::AppSettings,
    expected: &settings::UploadJob,
    owner: &settings::UploadOwner,
) -> bool {
    if !config_runtime::backup_enabled(value) || !config_runtime::ready(value, owner) {
        return false;
    }
    let Some(job) = value.upload_queue.iter_mut().find(|job| job.id == expected.id) else {
        return false;
    };
    if job.status != "failed"
        || job.message != "部分待上传消息已变化，请重新读取。"
        || job.owner.as_ref() != Some(owner)
        || job.account_id != owner.account_id
        || job.space_id != owner.space_id
        || job.scope != expected.scope
        || job.continuation_manual != Some(false)
        || job.initial_chunks.is_empty()
        || job.batch_id.is_some()
        || job.create_started != Some(false)
        || job.create_key.is_some()
        || job.package_digest.is_some()
    {
        return false;
    }
    job.status = "cancelled".into();
    job.message = "本机消息已变化，旧任务未发送；将重新读取后自动备份。".into();
    job.updated_at = Utc::now().to_rfc3339();
    let source = value.runtime.sources.get_mut(&owner.account_id).unwrap();
    source.scan_index = 0;
    source.scan_remaining = value.chat.allowed_usernames.get(&owner.account_id).map_or(0, Vec::len);
    source.retry_after = None;
    source.failures = 0;
    source.last_error = None;
    true
}

pub(super) fn retry_remote_action(status: &str) -> Result<RetryRemoteAction, String> {
    match status {
        "created" | "uploading" => Ok(RetryRemoteAction::ResumeArchive),
        "failed" => Ok(RetryRemoteAction::RetryParsing),
        "processing" | "completed" => Ok(RetryRemoteAction::Reconcile),
        "cancelled" => Err("服务端已取消该批次，请重新配置备份。".into()),
        _ => Err("无法确认服务端批次状态，请稍后重试。".into()),
    }
}

pub(super) async fn reconcile_current_jobs(app: &AppHandle) {
    let data = app_data(app);
    let Ok(s) = settings::load(&data) else {
        return;
    };
    let mut accounts: Vec<_> = s
        .upload_queue
        .iter()
        .filter(|j| j.owner.is_some() && j.batch_id.is_some() && upload_job_needs_reconciliation(j))
        .map(|j| j.account_id.clone())
        .collect();
    accounts.sort();
    accounts.dedup();
    // Rotate a bounded account window so additional sources cannot starve.
    static ROUND: AtomicUsize = AtomicUsize::new(0);
    let count = accounts.len();
    let start = ROUND.fetch_add(2, Ordering::Relaxed) % count.max(1);
    for account in accounts.iter().cycle().skip(start).take(count.min(2)) {
        let Ok(owner) = config_runtime::local_owner(&data, &s, &account) else {
            continue;
        };
        let Ok(token) = devify_access_token_async(&data, &owner.base_url).await else {
            continue;
        };
        let jobs: Vec<_> = s
            .upload_queue
            .iter()
            .filter(|j| {
                j.owner.as_ref() == Some(&owner)
                    && j.batch_id.is_some()
                    && upload_job_needs_reconciliation(j)
            })
            .cloned()
            .collect();
        static LAST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
        reconcile_owned_round(
            &data,
            &owner,
            &token,
            &jobs,
            &LAST,
            tokio::time::Instant::now() + RECONCILE_TOTAL_BUDGET,
        )
        .await;
    }
}

#[tauri::command]
pub(super) async fn task_runtime_retry(app: AppHandle, job_id: String) -> Result<(), String> {
    let state = app.state::<UploadState>();
    let _retry = begin_retry(&state, &job_id)?;
    let data = app_data(&app);
    let s = settings::load(&data)?;
    if s.runtime.paused {
        return Err("请先在任务中继续备份。".into());
    }
    let job = s
        .upload_queue
        .iter()
        .find(|j| j.id == job_id)
        .ok_or("找不到任务。")?;
    if job.status != "failed" {
        return Err("任务状态已变化，请查看最新状态。".into());
    }
    let owner = config_runtime::local_owner(&data, &s, &job.account_id)?;
    if job.owner.as_ref() != Some(&owner) {
        return Err("任务属于其他登录账号。".into());
    }
    let token = devify_access_token_async(&data, &owner.base_url).await?;
    let mut next_status = "queued";
    if let Some(id) = job.batch_id {
        let remote = fixed_identity_get(
            &data,
            &owner.base_url,
            &token,
            &format!("/api/v1/chat/uploads/{id}"),
        )
        .await?;
        validate_remote_owner(&owner, id, &remote, job.create_key.as_deref())?;
        match retry_remote_action(remote["status"].as_str().unwrap_or_default())? {
            RetryRemoteAction::Reconcile => next_status = "processing",
            action => {
                let current = settings::load(&data)?;
                let scope = job.scope.as_ref().ok_or("任务缺少原始范围。")?;
                validate_live_owned_job(&data, &job_id, &owner, &token, scope)?;
                if !current.sync.enabled || !config_runtime::ready(&current, &owner) {
                    return Err("请先开启自动备份并完成设置同步。".into());
                }
                if action == RetryRemoteAction::ResumeArchive {
                    if !upload_archive::exists(&data, job.package_digest.as_deref()) {
                        return Err("找不到原始上传包，请保留任务并重新检查备份。".into());
                    }
                } else {
                    // Freeze the service and credentials checked above. A login/service
                    // switch must never redirect this old batch's retry to a new account.
                    let response = http_client::strict()
                        .post(format!("{}/api/v1/chat/uploads/{id}/retry", owner.base_url))
                        .bearer_auth(&token)
                        .send(&data).await.map_err(|e| devify_network_message("重试失败", &e))?;
                    if !response.status().is_success() {
                        return Err(format!("重试失败：HTTP {}", response.status()));
                    }
                    next_status = "processing";
                }
            }
        }
    }
    settings::update(&data, |s| {
        validate_owner_state(s, &owner, &token)?;
        if desktop_credentials::load(&data, &owner.base_url)?.access != token {
            return Err("登录状态已改变。".into());
        }
        if retire_stale_plan(s, job, &owner) {
            return Ok(());
        }
        let live = s
            .upload_queue
            .iter_mut()
            .find(|j| j.id == job_id)
            .ok_or("任务已移除。")?;
        if live.owner.as_ref() != Some(&owner) || live.status != "failed" {
            return Err("任务状态已变化。".into());
        }
        let collision = legacy_create_key_collision(live);
        live.status = if live.batch_id.is_none() && live.create_started == Some(true) {
            "failed"
        } else {
            next_status
        }
        .into();
        live.message = if collision { "正在核对旧备份标识冲突（409），请稍候。" } else { "已安排重试，等待后台继续。" }.into();
        live.legacy_failure_confirmed = false;
        if let Some(source) = s.runtime.sources.get_mut(&owner.account_id) {
            source.retry_after = None;
            source.failures = 0;
            source.last_error = None;
        }
        Ok(())
    })?;
    TASK_WAKE.notify_one();
    emit_task_runtime(&app);
    Ok(())
}

/// One coordinator, one preparation/upload at a time. Sources and chats rotate;
/// each batch contains at most 200 messages from eight conversations.
pub(super) async fn backup_tick(
    app: &AppHandle,
    wake_pending: &mut Vec<(settings::UploadOwner, backup_wake::WakeScan)>,
) {
    let data = app_data(app);
    let Ok(value) = settings::load(&data) else {
        return;
    };
    let woke = backup_wake::take();
    if !config_runtime::backup_enabled(&value) || value.backup.mode == "manual" {
        wake_pending.clear();
        return;
    }
    let accounts: Vec<_> = value.runtime.sources.keys().cloned().collect();
    if woke {
        let previous = std::mem::take(wake_pending);
        *wake_pending = accounts
            .iter()
            .filter_map(|account| config_runtime::local_owner(&data, &value, account).ok())
            .map(|owner| {
                let mut scan = previous
                    .iter()
                    .find_map(|(pending, scan)| (*pending == owner).then_some(*scan))
                    .unwrap_or_else(backup_wake::WakeScan::pending);
                scan.rearm();
                (owner, scan)
            })
            .collect();
    }
    wake_pending.retain(|(_, scan)| scan.keep(Instant::now()));
    for account in accounts {
        let Ok(value) = settings::load(&data) else {
            return;
        };
        let Ok(owner) = config_runtime::local_owner(&data, &value, &account) else {
            continue;
        };
        if !config_runtime::ready(&value, &owner) {
            continue;
        }
        let source = &value.runtime.sources[&account];
        if source
            .retry_after
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .is_some_and(|t| t > Utc::now())
        {
            continue;
        }
        if !value.sync.enabled
            || value.backup.mode == "manual"
            || !network::allow_upload(value.backup.wifi_only)
            || entitlements::require(&data, &value.devify.base_url, capabilities::INGEST_AUTO)
                .is_err()
        {
            continue;
        }
        let wake_scan = wake_pending
            .iter_mut()
            .find_map(|(pending, scan)| (*pending == owner).then_some(scan));
        let wake_due = wake_scan
            .as_ref()
            .is_some_and(|scan| scan.is_due(Instant::now()));
        if !source_needs_execution(&value, &owner, Local::now(), wake_due) {
            continue;
        }
        let result = run_source_tick(app, &owner, wake_scan).await;
        let _ = record_source_result(
            &data,
            &owner,
            value.user_settings_revision,
            source.key_recovery_pending,
            &result,
        );
    }
    emit_task_runtime(app);
}

/// Check the local schedule before network identity verification or the activity
/// guard. A server-processing batch is reconciled separately without a busy ring.
pub(super) fn source_needs_execution(
    s: &settings::AppSettings,
    owner: &settings::UploadOwner,
    now: chrono::DateTime<Local>,
    wake_pending: bool,
) -> bool {
    let jobs: Vec<_> = s
        .upload_queue
        .iter()
        .filter(|j| j.owner.as_ref() == Some(owner))
        .collect();
    if jobs.iter().any(|j| {
        j.batch_id.is_some()
            && (j.status == "processing" && !j.message.starts_with("上传中断")
                || j.status == "failed"
                    && j.legacy_failure_confirmed
                    && upload_job_has_pending_sync(j))
    }) {
        return false;
    }
    if jobs.iter().any(|j| {
        !j.legacy_failure_confirmed
            && matches!(
                j.status.as_str(),
                "queued" | "running" | "uploading" | "failed"
            )
    }) {
        return true;
    }
    let Some(source) = s.runtime.sources.get(&owner.account_id) else {
        return false;
    };
    let continuing = s
        .sync
        .initial_progress
        .get(&owner.account_id)
        .is_some_and(|v| !v.is_empty())
        || s.sync
            .continuation
            .get(&owner.account_id)
            .is_some_and(|v| !v.is_empty());
    source.scan_remaining > 0 || continuing || source_scan_due(s, source, now, wake_pending)
}

fn source_scan_due(
    value: &settings::AppSettings,
    source: &config_runtime::Source,
    now: chrono::DateTime<Local>,
    wake_pending: bool,
) -> bool {
    if wake_pending || source.key_recovery_pending || source.wechat_key_failure.is_some() {
        return backup::automatic_allowed_now(&value.backup, now);
    }
    let last_run = source
        .last_run_at
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|date| date.with_timezone(&Local));
    backup::due(
        &value.backup,
        value.chat.sync_interval_seconds,
        last_run,
        now,
    )
}

pub(super) enum SourceTickError {
    Backup(String),
    WakeFollowUp(String),
    WechatKey(String),
}

impl From<String> for SourceTickError {
    fn from(error: String) -> Self {
        Self::Backup(error)
    }
}

impl From<&str> for SourceTickError {
    fn from(error: &str) -> Self {
        Self::Backup(error.into())
    }
}

/// A task may finish after logout, reconfiguration, or a space switch. It may
/// update only the unchanged source that started it, never a replacement profile.
pub(super) fn record_source_result(
    data: &Path,
    owner: &settings::UploadOwner,
    revision: u64,
    key_recovery_pending: bool,
    result: &Result<(), SourceTickError>,
) -> Result<(), String> {
    settings::update(data, |s| {
        if s.user_settings_revision != revision
            || !config_runtime::ready(s, owner)
            || config_runtime::local_owner(data, s, &owner.account_id)
                .is_ok_and(|live| config_runtime::same_principal(&live, owner))
                == false
        {
            return Ok(());
        }
        let source = s
            .runtime
            .sources
            .get_mut(&owner.account_id)
            .ok_or("来源已移除。")?;
        // A scan started before recovery must not restore the previous backoff.
        if source.key_recovery_pending && !key_recovery_pending {
            return Ok(());
        }
        if let Err(SourceTickError::Backup(_) | SourceTickError::WakeFollowUp(_)) = result {
            // Temporary failures interrupt initial confirmation. A confirmed failure
            // keeps its recovery action until the required databases are readable.
            if let Some(failure) = source
                .wechat_key_failure
                .as_mut()
                .filter(|failure| !failure.confirmed)
            {
                failure.interrupted = true;
            }
        }
        match result {
            Err(SourceTickError::WechatKey(database)) => {
                let confirmed = source.wechat_key_failure.as_ref().is_some_and(|previous| {
                    previous.confirmed || (!previous.interrupted && previous.database == *database)
                });
                source.wechat_key_failure = Some(config_runtime::WechatKeyFailure {
                    database: database.clone(),
                    confirmed,
                    interrupted: false,
                });
                source.failures = if confirmed {
                    source.failures.saturating_add(1)
                } else {
                    0
                };
                let delay = (30_i64 << source.failures.min(6)).min(1800);
                source.retry_after =
                    Some((Utc::now() + chrono::Duration::seconds(delay)).to_rfc3339());
                source.last_error = Some(WECHAT_BACKUP_RECHECK.into());
            }
            Err(SourceTickError::Backup(error)) => {
                source.failures = source.failures.saturating_add(1);
                let delay = if error == WECHAT_ACCOUNT_UNAVAILABLE {
                    30
                } else {
                    (30_i64.saturating_mul(1_i64 << source.failures.min(6))).min(1800)
                };
                source.retry_after =
                    Some((Utc::now() + chrono::Duration::seconds(delay)).to_rfc3339());
                source.last_error = Some(error.clone());
            }
            Err(SourceTickError::WakeFollowUp(error)) => {
                // The next scheduler tick can recheck; this failure must not delay the regular run.
                source.last_error = Some(error.clone());
            }
            Ok(()) => {
                source.failures = 0;
                source.retry_after = None;
                source.last_error = None;
            }
        }
        Ok(())
    })?;
    Ok(())
}

/// Finish the first selected-account pass before prioritising its partial
/// chunks. Once every selected chat has had a turn, do not keep scanning the
/// already-complete chats between each bounded continuation page.
pub(super) fn unfinished_scan_names(value: &settings::AppSettings, owner: &settings::UploadOwner, names: &[String]) -> Vec<String> {
    if value.runtime.sources.get(&owner.account_id).is_none_or(|s| s.scan_remaining != 0) {
        return Vec::new();
    }
    let Some(progress) = value.sync.initial_progress.get(&owner.account_id) else { return Vec::new(); };
    names.iter().filter(|name| progress.get(*name).is_some_and(|chunk| {
        chunk.space_id == owner.space_id && settings::same_service_url(&chunk.base_url, &owner.base_url)
    })).cloned().collect()
}

async fn run_source_tick(
    app: &AppHandle,
    expected: &settings::UploadOwner,
    wake_scan: Option<&mut backup_wake::WakeScan>,
) -> Result<(), SourceTickError> {
    let data = app_data(app);
    let account = &expected.account_id;
    if whatsapp::is_account(account) && whatsapp::account().is_none_or(|a| a.id != *account) {
        return Err("WhatsApp 登录账号已变化，请重新配置连接。".into());
    }
    let (owner, token) = verified_upload_identity(&data, account, &expected.space_id).await?;
    if !config_runtime::same_principal(&owner, expected) {
        return Err("来源的登录账号已改变。".into());
    }
    let state = app.state::<UploadState>();
    let _guard = begin_account_upload(&state, account)?;
    emit_task_runtime(app);
    let mut value = settings::load(&data)?;
    if !config_runtime::ready(&value, &owner) {
        return Ok(());
    }
    let names = value
        .chat
        .allowed_usernames
        .get(account)
        .cloned()
        .unwrap_or_default();
    if names.is_empty() {
        return Ok(());
    }
    value = settings::update(&data, |s| {
        s.sync.activate_owner(&owner);
        Ok(())
    })?;
    let pending: Vec<_> = value
        .upload_queue
        .iter()
        .filter(|j| {
            j.owner.as_ref() == Some(&owner)
                && upload_job_needs_reconciliation(j)
                && j.batch_id.is_some()
        })
        .cloned()
        .take(2)
        .collect();
    let deadline = tokio::time::Instant::now() + RECONCILE_TOTAL_BUDGET;
    for job in pending {
        if upload_archive::exists(&data, job.package_digest.as_deref())
            && job.status != "completed"
            && (job.status != "processing" || job.message.starts_with("上传中断"))
        {
            run_automatic_job(app, &owner, &token, &job).await?;
        }
        let fresh = settings::load(&data)?
            .upload_queue
            .into_iter()
            .find(|j| j.id == job.id)
            .ok_or("找不到任务。")?;
        reconcile_owned_job_until(&data, &owner, &token, &fresh, deadline).await?;
    }
    if value.upload_queue.iter().any(|j| {
        j.owner.as_ref() == Some(&owner)
            && j.can_recover_create()
            && (!upload_archive::exists(&data, j.package_digest.as_deref())
                || legacy_create_key_collision(j))
    }) {
        reconcile_unlinked_creates(&data, &owner, &token, &names).await;
    }
    value = settings::load(&data)?;
    if value.upload_queue.iter().any(|j| {
        j.owner.as_ref() == Some(&owner)
            && j.batch_id.is_some()
            && j.status == "failed"
            && upload_job_has_pending_sync(j)
    }) {
        return Err("有一批记录未处理成功，请在「任务」查看原因并重试。".into());
    }
    if value.upload_queue.iter().any(|j| {
        j.owner.as_ref() == Some(&owner)
            && j.batch_id.is_some()
            && upload_job_needs_reconciliation(j)
    }) {
        return Ok(());
    }
    if let Some(job) = value
        .upload_queue
        .iter()
        .find(|j| {
            j.owner.as_ref() == Some(&owner)
                && j.batch_id.is_none()
                && !j.legacy_failure_confirmed
                && matches!(
                    j.status.as_str(),
                    "queued" | "running" | "failed" | "uploading"
                )
        })
        .cloned()
    {
        if job.create_started == Some(true)
            && !upload_archive::exists(&data, job.package_digest.as_deref())
        {
            return Ok(());
        }
        let scope = job.scope.as_ref().ok_or("任务缺少范围，请重新配置。")?;
        if validate_upload_scope(&value, account, &owner.space_id, &scope.usernames).is_err() {
            settings::update(&data, |s| {
                if let Some(j) = s.upload_queue.iter_mut().find(|j| j.id == job.id) {
                    j.status = "cancelled".into();
                    j.message = "备份范围已更改，未发送的任务已取消。".into();
                }
                Ok(())
            })?;
            return Ok(());
        }
        return run_automatic_job(app, &owner, &token, &job)
            .await
            .map_err(Into::into);
    }
    let source = value
        .runtime
        .sources
        .get(account)
        .ok_or("找不到来源配置。")?;
    let continuing = value
        .sync
        .initial_progress
        .get(account)
        .is_some_and(|v| !v.is_empty())
        || value
            .sync
            .continuation
            .get(account)
            .is_some_and(|v| !v.is_empty());
    let now = Instant::now();
    let mut next_wake_scan = wake_scan.as_deref().copied();
    let wake_due = next_wake_scan.is_some_and(|scan| scan.is_due(now));
    let local_now = Local::now();
    let regular_due = source_scan_due(&value, source, local_now, false);
    let wake_allowed = wake_due && backup::automatic_allowed_now(&value.backup, local_now);
    if source.scan_remaining == 0 && !continuing && !regular_due && !wake_allowed {
        return Ok(());
    }
    let unfinished = unfinished_scan_names(&value, &owner, &names);
    let finishing = !unfinished.is_empty();
    let scan_names = if finishing { &unfinished } else { &names };
    let epoch = upload_epoch();
    let starts_scan = source.scan_remaining == 0 && !finishing;
    if starts_scan && (regular_due || wake_allowed) {
        if let Some(scan) = &mut next_wake_scan {
            scan.start(now, regular_due);
        }
    }
    let updates_regular_schedule =
        next_wake_scan.is_none_or(|scan| scan.updates_regular_schedule());
    let scan_error = |error| {
        if !finishing && !regular_due && !updates_regular_schedule {
            SourceTickError::WakeFollowUp(error)
        } else {
            SourceTickError::Backup(error)
        }
    };
    // A new scan refreshes changed WeChat databases before planning its first batch.
    if starts_scan && !whatsapp::is_account(account) {
        let scan = discover::scan();
        let local = scan
            .accounts
            .into_iter()
            .find(|a| a.id == *account)
            .ok_or_else(|| scan_error(WECHAT_ACCOUNT_UNAVAILABLE.into()))?;
        let store = keys::load(&keys::store_path(&data));
        let app = app.clone();
        let copy = data.clone();
        let outcomes = tauri::async_runtime::spawn_blocking(move || {
            read_new_wechat_data(&app, &copy, &local, &store, true, true)
        })
        .await
        .map_err(|error| scan_error(error.to_string()))?
        .map_err(scan_error)?;
        let expected = source
            .wechat_key_failure
            .as_ref()
            .map(|failure| failure.database.as_str());
        verify_wechat_backup_read(account, expected, &outcomes).map_err(|error| match error {
            WechatBackupReadError::Key(database) => SourceTickError::WechatKey(database),
            WechatBackupReadError::Read(message) => scan_error(message),
        })?;
    }
    let start = source.scan_index % scan_names.len();
    let remaining = if source.scan_remaining == 0 {
        scan_names.len()
    } else {
        source.scan_remaining.min(scan_names.len())
    };
    let scan_count = remaining.min(8);
    let scan_complete = !finishing && remaining == scan_count;
    let selected: Vec<_> = (0..scan_count)
        .map(|i| scan_names[(start + i) % scan_names.len()].clone())
        .collect();
    let snapshot = value.clone();
    let owner_copy = owner.clone();
    let copy = data.clone();
    let mut job = tauri::async_runtime::spawn_blocking(move || {
        plan_automatic_batch(&copy, &snapshot, &owner_copy, &selected)
    })
    .await
    .map_err(|error| scan_error(error.to_string()))?
    .map_err(scan_error)?;
    if upload_aborted(epoch) {
        return Ok(());
    }
    if let Some(j) = job.as_mut() {
        j.status = "queued".into();
        j.message = "等待后台备份".into();
    }
    settings::update(&data, |s| {
        if !config_runtime::ready(s, &owner)
            || desktop_credentials::load(&data, &owner.base_url)?.access != token
        {
            return Err("配置或登录状态已改变。".into());
        }
        // Do not commit a plan prepared against changed ranges or checkpoints.
        if account_backup_changed(&value, s, account)
            || serde_json::to_value((s.sync.cursors.get(account), s.sync.initial_progress.get(account))).ok()
                != serde_json::to_value((value.sync.cursors.get(account), value.sync.initial_progress.get(account))).ok()
        {
            return Err("备份范围或进度已改变，稍后重新读取。".into());
        }
        let source = s.runtime.sources.get_mut(account).ok_or("来源已移除。")?;
        if starts_scan && !whatsapp::is_account(account) {
            source.wechat_key_failure = None;
        }
        source.scan_index = (start + scan_count) % scan_names.len();
        source.scan_remaining = if finishing { 0 } else { remaining - scan_count };
        // An empty first wake scan establishes the interval; later checks leave it alone.
        if scan_complete && updates_regular_schedule {
            source.last_run_at = Some(Utc::now().to_rfc3339());
        }
        if scan_complete {
            source.key_recovery_pending = false;
        }
        source.retry_after = None;
        source.failures = 0;
        source.last_error = None;
        if let Some(job) = job.clone() {
            s.upload_queue.push(job);
        }
        Ok(())
    })?;
    let scanned_after_wake = !finishing && next_wake_scan.is_some_and(|scan| scan.is_scanning());
    if let (Some(scan), Some(mut next)) = (wake_scan, next_wake_scan) {
        if !finishing {
            next.commit_chunk(Instant::now(), scan_complete, job.is_some());
        }
        *scan = next;
    }
    if let Some(job) = job {
        if scanned_after_wake {
            if let Some(scope) = &job.scope {
                let usernames: Vec<_> = scope.message_selection.keys().collect();
                let _ = app.emit_to(
                    "main",
                    "wake-chat-messages-ready",
                    serde_json::json!({ "account_id": account, "usernames": usernames }),
                );
            }
        }
        emit_task_runtime(app);
        run_automatic_job(app, &owner, &token, &job).await?;
    }
    Ok(())
}

pub(super) fn plan_automatic_batch(
    data: &Path,
    value: &settings::AppSettings,
    owner: &settings::UploadOwner,
    names: &[String],
) -> Result<Option<settings::UploadJob>, String> {
    let cache = decrypt::cache_dir(data, &owner.account_id);
    let wa = whatsapp::is_account(&owner.account_id);
    let mut selection = BTreeMap::new();
    let mut chunks = BTreeMap::new();
    let per_chat = 200 / names.len().max(1);
    for name in names {
        // Session lists may outlive a deleted local transcript. It has nothing
        // to upload, but must not prevent the other selected chats from backing up.
        // The presence probe still propagates unreadable/corrupt database errors.
        if !wa && !query::chat_present(&cache, name)? {
            continue;
        }
        let cursor = value
            .sync
            .cursors
            .get(&owner.account_id)
            .and_then(|v| v.get(name))
            .copied();
        let progress = value
            .sync
            .initial_progress
            .get(&owner.account_id)
            .and_then(|v| v.get(name));
        let range = value
            .chat
            .initial_ranges
            .get(&owner.account_id)
            .and_then(|v| v.get(name));
        let since = progress
            .and_then(|p| p.since)
            .or_else(|| cursor.map(|c| c.time))
            .or_else(|| range.and_then(|r| r.since))
            .unwrap_or(Utc::now().timestamp() - 7 * 86400);
        let latest = if wa {
            whatsapp::latest_position(data, name)?
        } else {
            query::latest_position(&cache, name)?
        };
        let Some((latest_time, latest_id)) = latest else {
            continue;
        };
        let mut baseline = progress
            .map(|p| p.baseline)
            .unwrap_or(settings::SyncCursor {
                time: latest_time,
                local_id: latest_id,
                boundary_count: None,
            });
        if cursor.is_none() && baseline.boundary_count.is_none() {
            baseline.boundary_count = Some(if wa {
                whatsapp::count_at_time(data, name, baseline.time)?
            } else {
                query::count_at_time(&cache, name, baseline.time)?
            });
        }
        let until = if cursor.is_none() {
            Some(
                progress
                    .and_then(|p| p.until)
                    .or_else(|| range.and_then(|r| r.until))
                    .unwrap_or(baseline.time.saturating_add(1)),
            )
        } else {
            None
        };
        let position = progress.map(|p| p.position).or(cursor);
        // Read a bounded oldest-first page; do not build a full-history inventory.
        let after = None;
        let since = position.map_or(since, |p| p.time);
        let read_limit =
            (per_chat + position.and_then(|p| p.boundary_count).unwrap_or(0) + 1).min(1001);
        let rows = if wa {
            whatsapp::upload_page(data, name, since, until, after, read_limit)?
        } else {
            query::upload_page(&cache, name, since, until, after, read_limit)?
        };
        if rows.is_empty() {
            continue;
        }
        let refs: Vec<_> = rows
            .iter()
            .map(|m| settings::MessageRef {
                time: m.time,
                local_id: m.local_id,
                source_shard: m.source_shard.clone(),
            })
            .collect();
        let plan = plan_sync_batch(&refs, position, per_chat)?;
        if plan.messages.is_empty() {
            continue;
        }
        let last = plan.messages.last().ok_or("空备份批次。")?;
        let pos = settings::SyncCursor {
            time: last.time,
            local_id: last.local_id,
            boundary_count: Some(plan.messages.iter().filter(|r| r.time == last.time).count()),
        };
        chunks.insert(
            name.clone(),
            settings::InitialChunk {
                since: Some(since),
                until,
                baseline: if cursor.is_none() { baseline } else { pos },
                position: pos,
                final_chunk: !plan.has_more && rows.len() < read_limit,
                space_id: owner.space_id.clone(),
                base_url: owner.base_url.clone(),
            },
        );
        selection.insert(name.clone(), plan.messages);
    }
    if selection.is_empty() {
        return Ok(None);
    }
    let scope = settings::UploadScope {
        usernames: selection.keys().cloned().collect(),
        since: None,
        until: None,
        base_url: owner.base_url.clone(),
        message_selection: selection,
    };
    let mut job = sync_job(owner, scope)?;
    job.initial_chunks = chunks;
    job.continue_sync = false;
    job.continuation_manual = Some(false);
    Ok(Some(job))
}

async fn resume_archive(
    app: &AppHandle,
    job: &settings::UploadJob,
    owner: &settings::UploadOwner,
    token: &str,
) -> Result<DevifyUploadResult, String> {
    let data = app_data(app);
    let scope = job.scope.as_ref().ok_or("缺少原任务范围。")?;
    let digest = job.package_digest.as_deref().ok_or("缺少原归档摘要。")?;
    let key = job.create_key.as_deref().ok_or("缺少原请求标识。")?;
    let body = upload_archive::load(&data, digest)?;
    let value = settings::load(&data)?;
    let epoch = upload_epoch();
    if value.runtime.paused {
        return Err("后台任务已暂停。".into());
    }
    validate_live_owned_job(&data, &job.id, owner, token, scope)?;
    let client = http_client::upload();
    let batch = if let Some(id) = job.batch_id {
        fixed_identity_get(
            &data,
            &owner.base_url,
            token,
            &format!("/api/v1/chat/uploads/{id}"),
        )
        .await?
    } else {
        let mut payload = upload_create_body(
            &owner.account_id,
            key,
            &owner.device_id,
            &settings::resolved_device_label(&value.devify),
            &settings::default_platform(),
            expected_source_type(&owner.account_id),
            UploadTrigger::Scheduled,
        );
        payload["archive_sha256"] = serde_json::json!(digest);
        payload["archive_size"] = serde_json::json!(body.len());
        let mut response = client
            .post(format!(
                "{}/api/v1/spaces/{}/chat/uploads",
                owner.base_url, owner.space_id
            ))
            .bearer_auth(token)
            .json(&payload)
            .send(&data)
            .await
            .map_err(|e| devify_network_message("恢复上传失败", &e))?;
        if !response.status().is_success() {
            return Err(format!("恢复上传失败：HTTP {}", response.status()));
        }
        let mut raw = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            append_retry_identity_body(&mut raw, &chunk)?;
        }
        let v: serde_json::Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
        v.get("data").cloned().unwrap_or(v)
    };
    let id = batch["id"]
        .as_i64()
        .filter(|v| *v > 0)
        .ok_or("恢复响应缺少批次标识。")?;
    validate_remote_owner(owner, id, &batch, Some(key))?;
    if batch
        .get("archive_sha256")
        .and_then(serde_json::Value::as_str)
        .filter(|v| !v.is_empty())
        .is_some_and(|remote| remote != digest)
    {
        return Err("云端归档摘要不同，请保留任务。".into());
    }
    settings::update(&data, |s| {
        let live = s
            .upload_queue
            .iter()
            .find(|j| j.id == job.id)
            .ok_or("任务已移除。")?;
        validate_owned_job(s, live, owner, token, scope)?;
        if desktop_credentials::load(&data, &owner.base_url)?.access != token {
            return Err("登录状态已改变。".into());
        }
        let live = s.upload_queue.iter_mut().find(|j| j.id == job.id).unwrap();
        live.batch_id = Some(id);
        live.create_started = Some(true);
        Ok(())
    })?;
    let status = batch["status"].as_str().unwrap_or("");
    if matches!(status, "failed" | "cancelled") {
        return Err("服务端未能处理该批记录，请在任务中查看原因。".into());
    }
    if !matches!(status, "processing" | "completed") {
        if upload_aborted(epoch) {
            return Err(UPLOAD_ABORTED_MESSAGE.into());
        }
        validate_live_owned_job(&data, &job.id, owner, token, scope)?;
        if batch
            .get("archive_received_at")
            .is_none_or(serde_json::Value::is_null)
        {
            let form = reqwest::multipart::Form::new().part(
                "archive",
                reqwest::multipart::Part::bytes(body)
                    .file_name("chat.zip")
                    .mime_str("application/zip")
                    .map_err(|e| e.to_string())?,
            );
            let response = client
                .put(format!(
                    "{}/api/v1/chat/uploads/{id}/archive",
                    owner.base_url
                ))
                .bearer_auth(token)
                .multipart(form)
                .send(&data)
                .await
                .map_err(|e| devify_network_message("恢复归档失败", &e))?;
            if !response.status().is_success() {
                return Err(format!("恢复归档失败：HTTP {}", response.status()));
            }
        }
        if upload_aborted(epoch) {
            return Err(UPLOAD_ABORTED_MESSAGE.into());
        }
        validate_live_owned_job(&data, &job.id, owner, token, scope)?;
        let response = client
            .post(format!(
                "{}/api/v1/chat/uploads/{id}/complete",
                owner.base_url
            ))
            .bearer_auth(token)
            .send(&data)
            .await
            .map_err(|e| devify_network_message("恢复提交失败", &e))?;
        if !response.status().is_success() {
            return Err(format!("恢复提交失败：HTTP {}", response.status()));
        }
    }
    Ok(DevifyUploadResult {
        job_id: job.id.clone(),
        batch_id: id,
        status: "processing".into(),
        message_count: scope.message_selection.values().map(Vec::len).sum(),
        conversations: scope.usernames.len(),
        latest_position: job.covered.values().map(|p| (p.time, p.local_id)).max(),
        covered: job.covered.clone(),
        voice_pending: job.voice_pending.clone(),
        voice_processed: job.voice_processed.clone(),
    })
}

async fn run_automatic_job(
    app: &AppHandle,
    owner: &settings::UploadOwner,
    token: &str,
    job: &settings::UploadJob,
) -> Result<(), String> {
    let data = app_data(app);
    let scope = job.scope.clone().ok_or("任务缺少范围。")?;
    if !config_runtime::ready(&settings::load(&data)?, owner) {
        return Ok(());
    }
    let result = if job.create_started == Some(true)
        && upload_archive::exists(&data, job.package_digest.as_deref())
    {
        resume_archive(app, job, owner, token).await
    } else {
        devify_upload_wechat(
            app.clone(),
            job.id.clone(),
            owner.space_id.clone(),
            owner.account_id.clone(),
            scope.usernames,
            scope.since,
            scope.until,
            Some(scope.base_url),
            None,
            scope.message_selection,
            UploadTrigger::Scheduled,
            None,
        )
        .await
    };
    let mut retired = false;
    settings::update(&data, |s| {
        validate_owner_state(s, owner, token)?;
        if desktop_credentials::load(&data, &owner.base_url)?.access != token {
            return Err("登录状态已改变。".into());
        }
        let live = s
            .upload_queue
            .iter_mut()
            .find(|j| j.id == job.id)
            .ok_or("任务已移除。")?;
        apply_manual_upload_outcome(live, job, &result, "已提交，等待服务端处理")?;
        retired = retire_stale_plan(s, job, owner);
        Ok(())
    })?;
    emit_task_runtime(app);
    if retired { Ok(()) } else { result.map(|_| ()) }
}
