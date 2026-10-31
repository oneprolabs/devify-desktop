//! Local desktop credentials. Never log or include this file in diagnostics.

use std::fs;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "devify-credentials.json";

#[derive(Deserialize, Serialize)]
pub struct Credentials {
    pub base_url: String,
    pub access: String,
    pub refresh: String,
}

pub fn load(app_data: &Path, base_url: &str) -> Result<Credentials, String> {
    let raw = fs::read(app_data.join(FILE_NAME))
        .map_err(|_| "未找到 Devify 登录凭据，请重新登录。".to_string())?;
    let credentials: Credentials = serde_json::from_slice(&raw)
        .map_err(|_| "Devify 登录凭据格式无效，请重新登录。".to_string())?;
    if credentials.base_url != base_url.trim_end_matches('/') {
        return Err("Devify 服务地址已更改，请重新登录。".to_string());
    }
    Ok(credentials)
}

pub fn save(app_data: &Path, credentials: &Credentials) -> Result<(), String> {
    fs::create_dir_all(app_data).map_err(|_| "无法创建客户端数据目录。".to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(app_data)
        .map_err(|_| "无法创建 Devify 凭据文件。".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| "无法限制 Devify 凭据文件权限。".to_string())?;
    }
    serde_json::to_writer(&mut temp, credentials)
        .map_err(|_| "无法写入 Devify 登录凭据。".to_string())?;
    temp.flush()
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|_| "无法保存 Devify 登录凭据。".to_string())?;
    temp.persist(app_data.join(FILE_NAME))
        .map_err(|_| "无法更新 Devify 登录凭据。".to_string())?;
    Ok(())
}
