//! Attachment manifests, lazy image decoding and local media export.
use crate::chat::access::{account_root, require_visible_session};
use crate::chat::LOCAL_READ_BUDGET;
use crate::wechat::{decrypt, media, query};
use crate::{app_data, capabilities, entitlements, local_scope, settings};
use std::path::{Path, PathBuf};
use tauri::AppHandle;

/// Per-message attachment manifest (images, videos, documents, voice).
#[tauri::command]
pub(crate) async fn wechat_message_media(
    app: AppHandle,
    account_id: String,
    username: String,
    since: i64,
    until: i64,
) -> Result<media::MediaReport, String> {
    require_visible_session(&app, &account_id, &username)?;
    let root = account_root(&account_id)?;
    let cache = decrypt::cache_dir(&app_data(&app), &account_id);
    let raw = query::raw_messages_between(&cache, &username, since, until, 20000)?;
    let keys = media::resolve_image_keys(&root, &query::md5_hex(&username));
    Ok(media::resolve(&root, &username, &raw, keys.as_ref()))
}

/// Build (and cache) a display preview for one image message.
#[tauri::command]
pub(crate) async fn wechat_media_preview(
    app: AppHandle,
    account_id: String,
    local_id: i64,
    paths: Vec<String>,
) -> Result<Option<String>, String> {
    let root = account_root(&account_id)?;
    let targets: Vec<PathBuf> = paths.iter().map(|path| root.join(path)).collect();
    let data = local_scope::root(&app_data(&app));
    let _budget = LOCAL_READ_BUDGET
        .acquire()
        .await
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        Ok(
            media::make_preview(&data, &account_id, local_id, &root, &targets)
                .ok()
                .map(|p| p.to_string_lossy().to_string()),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Decode one encrypted image offline for the in-app viewer.
#[tauri::command]
pub(crate) async fn wechat_media_open_image(
    app: AppHandle,
    account_id: String,
    paths: Vec<String>,
) -> Result<String, String> {
    let root = account_root(&account_id)?;
    let targets: Vec<PathBuf> = paths
        .iter()
        .map(|path| {
            if Path::new(path).is_absolute() {
                PathBuf::from(path)
            } else {
                root.join(path)
            }
        })
        .collect();
    let data = local_scope::root(&app_data(&app));
    let _budget = LOCAL_READ_BUDGET
        .acquire()
        .await
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        media::open_image(&data, &account_id, &root, &targets)
            .map(|out| out.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Reveal one attachment in Finder/Explorer. The path must stay inside the account folder.
#[tauri::command]
pub(crate) async fn wechat_media_reveal(account_id: String, path: String) -> Result<(), String> {
    let root = account_root(&account_id)?;
    let target = if Path::new(&path).is_absolute() {
        PathBuf::from(&path)
    } else {
        root.join(&path)
    };
    media::reveal(&root, &target)
}

/// Export transcript + manifest + plaintext attachments into `~/Downloads/AIMyChats/…`.
#[tauri::command]
pub(crate) async fn wechat_export_bundle(
    app: AppHandle,
    account_id: String,
    username: String,
    display: String,
    since: i64,
    until: i64,
) -> Result<media::ExportReport, String> {
    let data = app_data(&app);
    let settings = settings::load(&data)?;
    entitlements::require(
        &data,
        &settings.devify.base_url,
        capabilities::BRIDGE_EXPORT,
    )?;
    require_visible_session(&app, &account_id, &username)?;
    let root = account_root(&account_id)?;
    media::export_bundle(
        &app_data(&app),
        &root,
        &account_id,
        &username,
        &display,
        since,
        until,
    )
}
