//! Verify local WeChat backup reads and resume the affected source.

use crate::{config_runtime, discover, settings, whatsapp, DecryptOutcome};
use serde::Serialize;
use std::path::Path;

pub(crate) const WECHAT_BACKUP_RECHECK: &str = "暂时无法读取微信备份数据，将自动重试。";

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub(crate) enum WechatBackupReadError {
    Key(String),
    Read(String),
}

pub(crate) fn verify_wechat_backup_read(
    account: &str,
    expected_database: Option<&str>,
    outcomes: &[DecryptOutcome],
) -> Result<(), WechatBackupReadError> {
    // Avatars are best-effort upload metadata, not part of message coverage.
    let required = || {
        outcomes
            .iter()
            .filter(|o| !o.rel.starts_with("head_image/"))
    };
    let readable = |o: &DecryptOutcome| matches!(o.status.as_str(), "ok" | "cached");
    if outcomes.iter().any(|o| o.account != account) {
        return Err(WechatBackupReadError::Read(WECHAT_BACKUP_RECHECK.into()));
    }
    if let Some(failure) = required().find(|o| !readable(o) && o.status != "no-key") {
        return Err(WechatBackupReadError::Read(
            failure
                .error
                .clone()
                .unwrap_or_else(|| WECHAT_BACKUP_RECHECK.into()),
        ));
    }
    let has_message_shard = required().any(|o| discover::is_message_shard(&o.rel));
    if expected_database.is_some_and(|database| !required().any(|o| o.rel == database)) {
        return Err(WechatBackupReadError::Read(
            "上次读取失败的微信数据库暂未找到，将自动重试。".into(),
        ));
    }
    if !has_message_shard || !required().any(|o| o.rel == "session/session.db") {
        return Err(WechatBackupReadError::Read(
            "暂时未找到微信备份所需的会话或消息数据库，将自动重试。".into(),
        ));
    }
    if let Some(failure) = required().find(|o| o.status == "no-key") {
        return Err(WechatBackupReadError::Key(failure.rel.clone()));
    }
    Ok(())
}

pub(crate) fn wechat_backup_needs_keys(account: &str, source: &config_runtime::Source) -> bool {
    !whatsapp::is_account(account)
        && source
            .wechat_key_failure
            .as_ref()
            .is_some_and(|failure| failure.confirmed)
}

pub(crate) enum WechatBackupRecovery {
    Unchanged,
    Resumed,
    Blocked(WechatBackupReadError),
}

fn pending_wechat_key_failure<'a>(
    data: &Path,
    value: &'a settings::AppSettings,
    owner: &settings::UploadOwner,
) -> Option<&'a config_runtime::WechatKeyFailure> {
    if whatsapp::is_account(&owner.account_id)
        || !config_runtime::local_owner(data, value, &owner.account_id)
            .is_ok_and(|live| config_runtime::same_principal(&live, owner))
    {
        return None;
    }
    let source = value.runtime.sources.get(&owner.account_id)?;
    source
        .owner
        .as_ref()
        .filter(|old| config_runtime::same_principal(old, owner))?;
    source.wechat_key_failure.as_ref()
}

pub(crate) fn resume_wechat_backup(
    data: &Path,
    owner: &settings::UploadOwner,
    outcomes: &[DecryptOutcome],
) -> Result<WechatBackupRecovery, String> {
    let snapshot = settings::load(data)?;
    let Some(failure) = pending_wechat_key_failure(data, &snapshot, owner) else {
        return Ok(WechatBackupRecovery::Unchanged);
    };
    // A failed poll must not write settings or bump their revision.
    if let Err(error) =
        verify_wechat_backup_read(&owner.account_id, Some(&failure.database), outcomes)
    {
        return Ok(WechatBackupRecovery::Blocked(error));
    }
    let mut recovery = WechatBackupRecovery::Unchanged;
    settings::update(data, |value| {
        let Some(failure) = pending_wechat_key_failure(data, value, owner) else {
            return Ok(());
        };
        if let Err(error) =
            verify_wechat_backup_read(&owner.account_id, Some(&failure.database), outcomes)
        {
            recovery = WechatBackupRecovery::Blocked(error);
            return Ok(());
        }
        let source = value
            .runtime
            .sources
            .get_mut(&owner.account_id)
            .ok_or("来源已移除。")?;
        source.wechat_key_failure = None;
        source.key_recovery_pending = true;
        source.retry_after = None;
        source.failures = 0;
        source.last_error = None;
        recovery = WechatBackupRecovery::Resumed;
        Ok(())
    })?;
    Ok(recovery)
}
