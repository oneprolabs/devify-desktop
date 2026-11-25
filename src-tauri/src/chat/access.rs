//! Shared account, source and session access checks for chat operations.
use crate::wechat::{decrypt, discover, query};
use crate::{app_data, config_runtime, settings, whatsapp};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

pub(crate) fn validate_account_id(account_id: &str) -> Result<(), String> {
    let mut components = Path::new(account_id).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err("微信账号标识无效。".into());
    }
    Ok(())
}

/// Shown whenever a read is blocked because the user disconnected WeChat.
const WECHAT_DISCONNECTED: &str = "微信连接已断开，重新连接后才能读取聊天。已上传的记录不受影响。";

pub(crate) fn allowed_usernames(
    app: &AppHandle,
    account_id: &str,
) -> Result<HashSet<String>, String> {
    validate_account_id(account_id)?;
    let settings = settings::load(&app_data(app))?;
    if !settings.connectors.wechat_enabled {
        return Err(WECHAT_DISCONNECTED.into());
    }
    Ok(config_runtime::browse_names(&settings, account_id)
        .into_iter()
        .collect())
}

/// Whitelist for a WhatsApp account. Same policy shape as WeChat, separate connect switch.
pub(crate) fn whatsapp_allowed_usernames(
    app: &AppHandle,
    account_id: &str,
) -> Result<HashSet<String>, String> {
    validate_account_id(account_id)?;
    let settings = settings::load(&app_data(app))?;
    if !settings.connectors.whatsapp_enabled {
        return Err("WhatsApp 已断开，已停止读取本机消息。".into());
    }
    if whatsapp::account().is_none_or(|a| a.id != account_id) {
        return Err("WhatsApp 登录账号已变化，请重新完成连接向导。".into());
    }
    Ok(config_runtime::browse_names(&settings, account_id)
        .into_iter()
        .collect())
}

pub(crate) fn require_whatsapp_attachment_session(
    app: &AppHandle,
    account_id: &str,
    username: &str,
) -> Result<settings::UploadOwner, String> {
    if !whatsapp_allowed_usernames(app, account_id)?.contains(username) {
        return Err("该会话不在当前策略白名单中。".into());
    }
    let data = app_data(app);
    let value = settings::load(&data)?;
    let principal = config_runtime::local_owner(&data, &value, account_id)?;
    let owner = value
        .runtime
        .sources
        .get(account_id)
        .and_then(|source| source.owner.as_ref())
        .or_else(|| value.sync.owners.get(account_id));
    let authorized = match owner {
        Some(owner) => config_runtime::same_principal(owner, &principal),
        None => config_runtime::has_current_profile(&data, &value)?,
    };
    if !authorized {
        return Err("该会话不在当前策略白名单中。".into());
    }
    Ok(principal)
}

/// Whitelist + visibility guard shared by the media and voice commands.
pub(crate) fn require_visible_session(
    app: &AppHandle,
    account_id: &str,
    username: &str,
) -> Result<(), String> {
    let cache = decrypt::cache_dir(&app_data(app), account_id);
    if !query::is_chat_session(&cache, username) {
        return Err("公众号、服务号和系统会话不在查看范围内。".into());
    }
    if !allowed_usernames(app, account_id)?.contains(username) {
        return Err("该会话不在当前策略白名单中。".into());
    }
    Ok(())
}

pub(crate) fn account_root(account_id: &str) -> Result<PathBuf, String> {
    discover::scan()
        .accounts
        .into_iter()
        .find(|account| account.id == account_id)
        .map(|account| PathBuf::from(account.dir))
        .ok_or_else(|| format!("找不到账号 {account_id} 的本机数据目录。"))
}
