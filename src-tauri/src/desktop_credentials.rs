//! Local desktop credentials. Never log or include this file in diagnostics.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

// Serialize all credential reads and mutations in this process. Refreshes use CAS so
// a response obtained for an older login cannot replace or remove a newer login.
static CREDENTIALS_LOCK: Mutex<u64> = Mutex::new(0);

const FILE_NAME: &str = "devify-credentials.json";

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Credentials {
    pub base_url: String,
    pub access: String,
    pub refresh: String,
}

pub fn load(app_data: &Path, base_url: &str) -> Result<Credentials, String> {
    let _guard = CREDENTIALS_LOCK.lock().map_err(|_| "无法读取服务端凭据。")?;
    load_unlocked(app_data, base_url)
}

fn load_unlocked(app_data: &Path, base_url: &str) -> Result<Credentials, String> {
    let raw = fs::read(app_data.join(FILE_NAME))
        .map_err(|_| "未找到服务端登录凭据，请重新登录。".to_string())?;
    let credentials: Credentials = serde_json::from_slice(&raw)
        .map_err(|_| "服务端登录凭据格式无效，请重新登录。".to_string())?;
    if credentials.base_url != base_url.trim_end_matches('/') {
        return Err("服务端地址已更改，请重新登录。".to_string());
    }
    Ok(credentials)
}

pub fn clear(app_data: &Path) -> Result<(), String> {
    let _identity = crate::UPLOAD_IDENTITY_LOCK.try_write().map_err(|_| "上传期间无法更改登录状态，请稍后重试。")?;
    let mut guard = CREDENTIALS_LOCK.lock().map_err(|_| "无法清除服务端凭据。")?;
    clear_unlocked(app_data)?;
    *guard = guard.wrapping_add(1);
    Ok(())
}

fn clear_unlocked(app_data: &Path) -> Result<(), String> {
    match fs::remove_file(app_data.join(FILE_NAME)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("无法清除服务端登录凭据。".to_string()),
    }
}

pub fn save(app_data: &Path, credentials: &Credentials) -> Result<(), String> {
    let _identity = crate::UPLOAD_IDENTITY_LOCK.try_write().map_err(|_| "上传期间无法更改登录状态，请稍后重试。")?;
    let mut guard = CREDENTIALS_LOCK.lock().map_err(|_| "无法保存服务端凭据。")?;
    save_unlocked(app_data, credentials)?;
    *guard = guard.wrapping_add(1);
    Ok(())
}

pub fn generation() -> Result<u64, String> {
    CREDENTIALS_LOCK.lock().map(|guard| *guard).map_err(|_| "无法读取服务端登录状态。".into())
}

/// Check both facts under one credential lock; errors are not treated as absence.
pub(crate) fn absent_at_generation(app_data: &Path, generation: u64) -> Result<bool, String> {
    let guard = CREDENTIALS_LOCK.lock().map_err(|_| "无法读取服务端登录状态。")?;
    if *guard != generation { return Ok(false); }
    match fs::metadata(app_data.join(FILE_NAME)) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(_) => Err("无法检查服务端登录凭据。".into()),
    }
}

/// Caller holds the identity write lease. Also invalidates in-flight auth requests.
pub(crate) fn advance_generation_locked() -> Result<u64, String> {
    let mut guard = CREDENTIALS_LOCK.lock().map_err(|_| "无法读取服务端登录状态。")?;
    *guard = guard.wrapping_add(1);
    Ok(*guard)
}

pub fn begin_auth_attempt() -> Result<u64, String> {
    let _identity = crate::UPLOAD_IDENTITY_LOCK.try_write().map_err(|_| "上传期间无法更改登录状态，请稍后重试。")?;
    advance_generation_locked()
}

pub fn load_with_generation(app_data: &Path, base_url: &str) -> Result<(Credentials, u64), String> {
    let guard = CREDENTIALS_LOCK.lock().map_err(|_| "无法读取服务端凭据。")?;
    Ok((load_unlocked(app_data, base_url)?, *guard))
}

/// Caller holds both the settings transaction and identity write lease, in that order.
pub(crate) fn save_if_generation_locked(app_data: &Path, generation: u64, credentials: &Credentials) -> Result<bool, String> {
    let mut guard = CREDENTIALS_LOCK.lock().map_err(|_| "无法保存服务端凭据。")?;
    if *guard != generation { return Ok(false); }
    save_unlocked(app_data, credentials)?;
    *guard = guard.wrapping_add(1);
    Ok(true)
}

pub fn save_if_current(app_data: &Path, generation: u64, expected: &Credentials, replacement: &Credentials) -> Result<bool, String> {
    let _identity = crate::UPLOAD_IDENTITY_LOCK.try_write().map_err(|_| "上传期间无法更改登录状态，请稍后重试。")?;
    let mut guard = CREDENTIALS_LOCK.lock().map_err(|_| "无法保存服务端凭据。")?;
    if *guard != generation || load_unlocked(app_data, &expected.base_url).ok().as_ref() != Some(expected) { return Ok(false); }
    save_unlocked(app_data, replacement)?;
    *guard = guard.wrapping_add(1);
    Ok(true)
}

pub fn clear_if_current(app_data: &Path, generation: u64, expected: &Credentials) -> Result<bool, String> {
    let _identity = crate::UPLOAD_IDENTITY_LOCK.try_write().map_err(|_| "上传期间无法更改登录状态，请稍后重试。")?;
    let mut guard = CREDENTIALS_LOCK.lock().map_err(|_| "无法清除服务端凭据。")?;
    if *guard != generation || load_unlocked(app_data, &expected.base_url).ok().as_ref() != Some(expected) { return Ok(false); }
    clear_unlocked(app_data)?;
    *guard = guard.wrapping_add(1);
    Ok(true)
}

pub fn clear_if_access(app_data: &Path, base_url: &str, access: &str, generation: u64) -> Result<bool, String> {
    let _identity = crate::UPLOAD_IDENTITY_LOCK.try_write().map_err(|_| "上传期间无法更改登录状态，请稍后重试。")?;
    clear_if_access_locked(app_data, base_url, access, generation)
}

pub(crate) fn clear_if_access_locked(app_data: &Path, base_url: &str, access: &str, generation: u64) -> Result<bool, String> {
    let mut guard = CREDENTIALS_LOCK.lock().map_err(|_| "无法清除服务端凭据。")?;
    if *guard != generation || !load_unlocked(app_data, base_url).ok().is_some_and(|current| current.access == access) { return Ok(false); }
    clear_unlocked(app_data)?;
    *guard = guard.wrapping_add(1);
    Ok(true)
}

fn save_unlocked(app_data: &Path, credentials: &Credentials) -> Result<(), String> {
    fs::create_dir_all(app_data).map_err(|_| "无法创建客户端数据目录。".to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(app_data)
        .map_err(|_| "无法创建服务端凭据文件。".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| "无法限制服务端凭据文件权限。".to_string())?;
    }
    serde_json::to_writer(&mut temp, credentials)
        .map_err(|_| "无法写入服务端登录凭据。".to_string())?;
    temp.flush()
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|_| "无法保存服务端登录凭据。".to_string())?;
    temp.persist(app_data.join(FILE_NAME))
        .map_err(|_| "无法更新服务端登录凭据。".to_string())?;
    Ok(())
}
