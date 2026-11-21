//! Selected-conversation date and message ranges for collection views.
use crate::chat::access::{allowed_usernames, validate_account_id, whatsapp_allowed_usernames};
use crate::wechat::{
    decrypt,
    query::{self, MessageItem},
};
use crate::{app_data, collect_upload_messages, whatsapp};
use std::collections::BTreeMap;
use tauri::AppHandle;

#[tauri::command]
pub(crate) async fn wechat_collection_dates(
    app: AppHandle,
    account_id: String,
    usernames: Vec<String>,
) -> Result<BTreeMap<String, Vec<(String, usize)>>, String> {
    if whatsapp::is_account(&account_id) {
        let allowed = whatsapp_allowed_usernames(&app, &account_id)?;
        if usernames.iter().any(|name| !allowed.contains(name)) {
            return Err("会话不在已选范围中。".into());
        }
        let data = app_data(&app);
        return tauri::async_runtime::spawn_blocking(move || {
            usernames
                .into_iter()
                .map(|name| whatsapp::message_dates(&data, &name).map(|dates| (name, dates)))
                .collect()
        })
        .await
        .map_err(|e| format!("读取日期失败：{e}"))?;
    }
    let allowed = allowed_usernames(&app, &account_id)?;
    if usernames.iter().any(|name| !allowed.contains(name)) {
        return Err("会话不在已选范围中。".into());
    }
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    tauri::async_runtime::spawn_blocking(move || {
        usernames
            .into_iter()
            .map(|name| query::message_dates(&cache, &name).map(|dates| (name, dates)))
            .collect()
    })
    .await
    .map_err(|e| format!("读取日期失败：{e}"))?
}

#[tauri::command]
pub(crate) async fn wechat_catalog_dates(
    app: AppHandle,
    account_id: String,
    username: String,
) -> Result<Vec<(String, usize)>, String> {
    validate_account_id(&account_id)?;
    if whatsapp::is_account(&account_id) {
        let data = app_data(&app);
        return tauri::async_runtime::spawn_blocking(move || {
            whatsapp::message_dates(&data, &username)
        })
        .await
        .map_err(|error| format!("读取日期失败：{error}"))?;
    }
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    tauri::async_runtime::spawn_blocking(move || {
        if !query::is_chat_session(&cache, &username) {
            return Err("该会话不可用。".into());
        }
        query::message_dates(&cache, &username)
    })
    .await
    .map_err(|e| format!("读取日期失败：{e}"))?
}

#[tauri::command]
pub(crate) async fn wechat_collection_messages(
    app: AppHandle,
    account_id: String,
    username: String,
    since: Option<i64>,
    until: Option<i64>,
) -> Result<Vec<MessageItem>, String> {
    if since.zip(until).is_some_and(|(start, end)| start >= end) {
        return Err("日期范围无效。".into());
    }
    if whatsapp::is_account(&account_id) {
        let allowed = whatsapp_allowed_usernames(&app, &account_id)?;
        if !allowed.contains(&username) {
            return Err("该会话不在白名单中。".into());
        }
        let data = app_data(&app);
        return tauri::async_runtime::spawn_blocking(move || {
            whatsapp::messages_between(&data, &username, since, until, 5000)
        })
        .await
        .map_err(|error| format!("读取消息失败：{error}"))?;
    }
    let allowed = allowed_usernames(&app, &account_id)?;
    if !allowed.contains(&username) {
        return Err("该会话不在白名单中。".into());
    }
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    if !query::is_chat_session(&cache, &username) {
        return Err("该会话不在可采集范围中。".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        collect_upload_messages(&cache, &username, since, until, Some(5000))
    })
    .await
    .map_err(|e| format!("读取消息失败：{e}"))?
}
