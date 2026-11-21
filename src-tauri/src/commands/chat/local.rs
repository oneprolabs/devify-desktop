//! Bounded local conversation and message reads for WeChat and WhatsApp.
use crate::chat::access::{allowed_usernames, validate_account_id, whatsapp_allowed_usernames};
use crate::chat::LOCAL_READ_BUDGET;
use crate::wechat::{
    self, decrypt,
    query::{self, DigestReport, MessageItem, SearchReport, SessionItem},
};
use crate::{app_data, config_runtime, settings, task_store, whatsapp};
use serde::Serialize;
use sha2::Digest;
use std::collections::HashSet;
use tauri::AppHandle;

#[tauri::command]
pub(crate) async fn chat_sessions(
    app: AppHandle,
    account_id: String,
    limit: Option<usize>,
    offset: Option<usize>,
    focus_username: Option<String>,
    focus_usernames: Option<Vec<String>>,
) -> Result<Vec<SessionItem>, String> {
    let _read_budget = LOCAL_READ_BUDGET
        .acquire()
        .await
        .map_err(|e| e.to_string())?;
    validate_account_id(&account_id)?;
    let data = app_data(&app);
    let value = settings::load(&data)?;
    let mut allowed: HashSet<String> = config_runtime::browse_names(&value, &account_id)
        .into_iter()
        .collect();
    let principal = config_runtime::local_owner(&data, &value, &account_id)?;
    if value
        .runtime
        .sources
        .get(&account_id)
        .and_then(|s| s.owner.as_ref())
        .or_else(|| value.sync.owners.get(&account_id))
        .is_some_and(|old| !config_runtime::same_principal(old, &principal))
    {
        return Ok(Vec::new());
    }
    if let Some(name) = focus_username {
        allowed.retain(|v| v == &name);
    }
    if let Some(names) = focus_usernames {
        if names.len() > 40 {
            return Err("一次最多读取 40 个会话名称。".into());
        }
        let names: HashSet<_> = names.into_iter().collect();
        allowed.retain(|v| names.contains(v));
    }
    let wa = whatsapp::is_account(&account_id);
    let cache = decrypt::cache_dir(&data, &account_id);
    let cache_key = hex::encode(sha2::Sha256::digest(
        serde_json::to_vec(&(
            "sessions-v1",
            &principal,
            limit.unwrap_or(50),
            offset.unwrap_or(0),
            &{
                let mut v: Vec<_> = allowed.iter().cloned().collect();
                v.sort();
                v
            },
        ))
        .map_err(|e| e.to_string())?,
    ));
    let read_data = data.clone();
    let read_key = cache_key.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if !wa
            && allowed
                .iter()
                .any(|name| name.ends_with("@qy_g") || name.ends_with("@qy_u"))
        {
            let _ = decrypt::prepare_business_cache(&data, &account_id, false);
        }
        let result = if wa {
            whatsapp::sessions_page(
                &data,
                &allowed,
                limit.unwrap_or(50).clamp(1, 100),
                offset.unwrap_or(0).min(1000),
            )
        } else {
            query::sessions_page(
                &cache,
                Some(&allowed),
                Some(limit.unwrap_or(50).clamp(1, 100)),
                offset.unwrap_or(0).min(1000),
            )
        };
        match result {
            Ok(mut list) => {
                if let Some(old) = task_store::cache_get(&read_data, &read_key)
                    .and_then(|v| serde_json::from_value::<Vec<SessionItem>>(v).ok())
                {
                    for item in old {
                        if allowed.contains(&item.username)
                            && !list.iter().any(|s| s.username == item.username)
                        {
                            list.push(item);
                        }
                    }
                    list.sort_by(|a, b| b.last_time.cmp(&a.last_time));
                    list.truncate(limit.unwrap_or(50).clamp(1, 100));
                }
                if !list.is_empty() {
                    if let Ok(body) = serde_json::to_value(&list) {
                        let _ = task_store::cache_put(&read_data, &read_key, &body);
                    }
                }
                Ok(list)
            }
            Err(error) if !wa && !cache.join("session/session.db").exists() => {
                task_store::cache_get(&read_data, &read_key)
                    .and_then(|v| serde_json::from_value(v).ok())
                    .ok_or(error)
            }
            Err(error) => Err(error),
        }
    })
    .await
    .map_err(|e| format!("读取会话失败：{e}"))?
}

#[derive(Serialize)]
pub(crate) struct AvatarItem {
    username: String,
    data_url: String,
}

#[derive(Serialize)]
pub(crate) struct WechatAccountProfile {
    nickname: Option<String>,
    alias: Option<String>,
    avatar: Option<String>,
}

#[tauri::command]
pub(crate) async fn wechat_account_profile(
    app: AppHandle,
    account_id: String,
) -> Option<WechatAccountProfile> {
    if whatsapp::is_account(&account_id) {
        let account = whatsapp::account()?;
        return Some(WechatAccountProfile {
            nickname: account.name.or_else(|| Some(account.phone)),
            alias: Some(account.id),
            avatar: None,
        });
    }
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    let profile = query::account_profile(&cache, &account_id)?;
    let avatar = wechat::avatar::avatars(&cache, &[profile.username])
        .into_iter()
        .next()
        .map(|item| item.data_url);
    Some(WechatAccountProfile {
        nickname: profile.nickname,
        alias: profile.alias,
        avatar,
    })
}

/// Avatars for the given usernames, as `data:` URLs. Missing names are simply absent, so the
/// caller keeps its letter avatars; nothing here is an error worth interrupting a chat for.
#[tauri::command]
pub(crate) async fn wechat_avatars(
    app: AppHandle,
    account_id: String,
    usernames: Vec<String>,
) -> Vec<AvatarItem> {
    if whatsapp::is_account(&account_id) {
        return Vec::new();
    }
    let Ok(_budget) = LOCAL_READ_BUDGET.acquire().await else {
        return Vec::new();
    };
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    tauri::async_runtime::spawn_blocking(move || {
        wechat::avatar::avatars(&cache, &usernames.into_iter().take(100).collect::<Vec<_>>())
            .into_iter()
            .map(|avatar| AvatarItem {
                username: avatar.username,
                data_url: avatar.data_url,
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

#[tauri::command]
pub(crate) async fn wechat_session_catalog(
    app: AppHandle,
    account_id: String,
) -> Result<Vec<SessionItem>, String> {
    if whatsapp::is_account(&account_id) {
        validate_account_id(&account_id)?;
        let data = app_data(&app);
        return tauri::async_runtime::spawn_blocking(move || whatsapp::sessions(&data))
            .await
            .map_err(|error| format!("读取 WhatsApp 会话失败：{error}"))?;
    }
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    query::sessions(&cache)
}

#[tauri::command]
pub(crate) async fn wechat_messages(
    app: AppHandle,
    account_id: String,
    username: String,
    limit: Option<usize>,
    before: Option<i64>,
    before_id: Option<i64>,
    before_shard: Option<String>,
) -> Result<Vec<MessageItem>, String> {
    let _read_budget = LOCAL_READ_BUDGET
        .acquire()
        .await
        .map_err(|e| e.to_string())?;
    if whatsapp::is_account(&account_id) {
        let allowed = whatsapp_allowed_usernames(&app, &account_id)?;
        if !allowed.contains(&username) {
            return Err("该会话不在当前策略白名单中。".into());
        }
        let cursor = before.map(|time| match before_id {
            Some(id) => query::MessageCursor {
                time,
                local_id: id,
                source_shard: before_shard.unwrap_or_default(),
            },
            None => query::MessageCursor::before_time(time),
        });
        let data = app_data(&app);
        return tauri::async_runtime::spawn_blocking(move || {
            let rows =
                whatsapp::messages(&data, &username, limit.unwrap_or(100).clamp(1, 100), cursor)?;
            if rows.is_empty() && before.is_none() {
                return Err("LOCAL_CHAT_MISSING: 本地没有这段聊天记录。".into());
            }
            Ok(rows)
        })
        .await
        .map_err(|error| format!("读取 WhatsApp 消息失败：{error}"))?;
    }
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    let allowed = allowed_usernames(&app, &account_id)?;
    if !allowed.contains(&username) {
        return Err("该会话不在当前策略白名单中。".into());
    }
    // Both halves of the cursor come from the last row already on screen; without the id
    // this degrades to the old timestamp-only paging, which cannot see past a shared second.
    let cursor = before.map(|time| match before_id {
        Some(id) => query::MessageCursor {
            time,
            local_id: id,
            source_shard: before_shard.unwrap_or_default(),
        },
        None => query::MessageCursor::before_time(time),
    });
    if !query::is_visible_chat_username(&username) {
        return Err("该会话不在查看范围内。".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        if username.ends_with("@qy_g") || username.ends_with("@qy_u") {
            decrypt::prepare_business_cache(&app_data(&app), &account_id, true)?;
        }
        if !query::chat_present(&cache, &username)? {
            return Err("LOCAL_CHAT_MISSING: 本地没有这段聊天记录。".into());
        }
        let rows = query::messages(
            &cache,
            &username,
            limit.unwrap_or(100).clamp(1, 100),
            cursor,
        )?;
        if rows.is_empty() && before.is_none() {
            return Err("LOCAL_CHAT_MISSING: 本地没有这段聊天记录。".into());
        }
        Ok(rows)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Bounded keyword search over decrypted message content.
///
/// `max_sessions` caps how many conversations are scanned; the report tells the UI how
/// many were actually scanned and how many were skipped, so results are never presented
/// as exhaustive when they are not.
#[tauri::command]
pub(crate) async fn wechat_search(
    app: AppHandle,
    account_id: String,
    keyword: String,
    groups_only: Option<bool>,
    max_sessions: Option<usize>,
) -> Result<SearchReport, String> {
    if whatsapp::is_account(&account_id) {
        let allowed = whatsapp_allowed_usernames(&app, &account_id)?;
        let data = app_data(&app);
        return tauri::async_runtime::spawn_blocking(move || {
            let mut report =
                whatsapp::search(&data, &keyword, max_sessions.unwrap_or(200), Some(&allowed))?;
            // Rust enforces the whitelist even though the search only ever returns names.
            report.hits.retain(|hit| allowed.contains(&hit.username));
            if !groups_only.unwrap_or(false) {
                return Ok(report);
            }
            report.hits.retain(|hit| hit.is_group);
            Ok(report)
        })
        .await
        .map_err(|error| format!("搜索失败：{error}"))?;
    }
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    let allowed = allowed_usernames(&app, &account_id)?;
    query::search_filtered(
        &cache,
        &keyword,
        groups_only.unwrap_or(false),
        max_sessions.unwrap_or(200),
        Some(&allowed),
    )
}

#[tauri::command]
pub(crate) async fn wechat_digest(
    app: AppHandle,
    account_id: String,
    usernames: Vec<String>,
    since: Option<i64>,
    until: Option<i64>,
) -> Result<DigestReport, String> {
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    let allowed = allowed_usernames(&app, &account_id)?;
    if usernames.iter().any(|username| !allowed.contains(username)) {
        return Err("整理内容包含不在当前策略白名单中的会话。".into());
    }
    query::digest_filtered(&cache, &usernames, since, until, Some(&allowed))
}
