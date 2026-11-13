use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

use crate::{desktop_credentials, diagnostics, settings};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Screenshot {
    pub data: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub app_version: String,
    pub platform: String,
    pub arch: String,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            app_version: env!("CARGO_PKG_VERSION").into(),
            platform: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub client_id: String,
    #[serde(default)]
    pub environment: Environment,
    #[serde(default)]
    pub related_batch_ids: Vec<u64>,
    pub receiver: String,
    pub owner_id: Option<i64>,
    pub category: String,
    pub description: String,
    pub contact: String,
    pub page: String,
    pub screenshots: Vec<Screenshot>,
    pub diagnostics: Option<serde_json::Value>,
    /// Once sent, an ambiguous retry must use these exact bytes.
    #[serde(default)]
    pub attempted: bool,
}

fn new_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| "无法创建反馈编号。")?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let value = hex::encode(bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &value[..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..]
    ))
}

fn identity(root: &Path) -> (String, Option<i64>) {
    let base = settings::load(root)
        .ok()
        .and_then(|s| s.devify.resolved_base_url().ok())
        .unwrap_or_default();
    let user = desktop_credentials::load(root, &base).ok().and_then(|c| {
        let payload = c
            .access
            .split('.')
            .nth(1)
            .and_then(|s| URL_SAFE_NO_PAD.decode(s).ok())?;
        let value: serde_json::Value = serde_json::from_slice(&payload).ok()?;
        value["user_id"].as_i64().filter(|id| *id > 0)
    });
    (base, user)
}

fn draft_path(root: &Path) -> PathBuf {
    let (base, user) = identity(root);
    let key = hex::encode(Sha256::digest(format!("{base}\n{user:?}").as_bytes()));
    root.join("feedback-drafts").join(format!("{key}.json"))
}

fn validate(draft: &Draft) -> Result<(), String> {
    if !matches!(
        draft.category.as_str(),
        "wechat" | "backup" | "upload" | "login" | "interface" | "other"
    ) || !matches!(
        draft.page.as_str(),
        "chat" | "events" | "topics" | "reports" | "ask" | "tasks" | "settings" | "login" | "app"
    ) || draft.environment.app_version.len() > 32
        || draft.environment.arch.len() > 32
        || !matches!(
            draft.environment.platform.as_str(),
            "macos" | "windows" | "linux"
        )
        || draft.related_batch_ids.len() > 20
        || draft.related_batch_ids.iter().any(|id| *id == 0)
        || draft.description.chars().count() > 5000
        || draft.contact.chars().count() > 200
        || draft.client_id.len() != 36
        || !draft
            .client_id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b'-')
    {
        return Err("反馈内容格式无效。".into());
    }
    image_bytes(draft)?;
    if let Some(value) = &draft.diagnostics {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Snapshot {
            schema_version: u32,
            generated_at: String,
            events: Vec<diagnostics::Event>,
        }
        let parsed: Snapshot =
            serde_json::from_value(value.clone()).map_err(|_| "诊断内容格式无效。")?;
        if parsed.schema_version != 1
            || chrono::DateTime::parse_from_rfc3339(&parsed.generated_at).is_err()
            || parsed.events.len() > 1000
            || value.to_string().len() > 400 * 1024
        {
            return Err("诊断内容过大或无效。".into());
        }
    }
    Ok(())
}

fn image_bytes(draft: &Draft) -> Result<Vec<Vec<u8>>, String> {
    if draft.screenshots.len() > 3 {
        return Err("最多添加 3 张截图。".into());
    }
    draft
        .screenshots
        .iter()
        .map(|s| {
            if s.data.len() > 4 * 1024 * 1024 {
                return Err("每张截图不得超过 3 MB。".into());
            }
            let bytes = STANDARD.decode(&s.data).map_err(|_| "截图数据无效。")?;
            if bytes.len() > 3 * 1024 * 1024
                || !(bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.starts_with(b"\xff\xd8\xff"))
            {
                return Err("仅支持 PNG/JPEG 截图。".into());
            }
            Ok(bytes)
        })
        .collect()
}

fn save(root: &Path, draft: &Draft) -> Result<(), String> {
    validate(draft)?;
    let (base, user) = identity(root);
    if draft.receiver != base || draft.owner_id != user {
        return Err("服务端或账号已改变，请重新打开反馈。".into());
    }
    let path = draft_path(root);
    fs::create_dir_all(path.parent().unwrap()).map_err(|_| "无法创建反馈草稿目录。")?;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())
        .map_err(|_| "无法保存反馈草稿。")?;
    serde_json::to_writer(&mut file, draft).map_err(|_| "无法保存反馈草稿。")?;
    file.as_file()
        .sync_all()
        .map_err(|_| "无法保存反馈草稿。")?;
    file.persist(path).map_err(|_| "无法保存反馈草稿。")?;
    Ok(())
}

#[tauri::command]
pub fn feedback_draft_get(app: AppHandle) -> Result<Draft, String> {
    let root = crate::app_data(&app);
    let path = draft_path(&root);
    if path.exists() {
        let bytes = fs::read(path).map_err(|_| "无法读取反馈草稿。")?;
        let draft: Draft = serde_json::from_slice(&bytes).map_err(|_| "反馈草稿损坏。")?;
        validate(&draft)?;
        return Ok(draft);
    }
    let (receiver, owner_id) = identity(&root);
    Ok(Draft {
        client_id: new_id()?,
        environment: Environment::default(),
        related_batch_ids: vec![],
        receiver,
        owner_id,
        category: "other".into(),
        description: String::new(),
        contact: String::new(),
        page: "app".into(),
        screenshots: vec![],
        diagnostics: None,
        attempted: false,
    })
}

#[tauri::command]
pub fn feedback_draft_save(app: AppHandle, draft: Draft) -> Result<(), String> {
    save_edit(&crate::app_data(&app), &draft)
}

fn save_edit(root: &Path, draft: &Draft) -> Result<(), String> {
    if let Ok(previous) = fs::read(draft_path(root))
        .and_then(|v| serde_json::from_slice::<Draft>(&v).map_err(std::io::Error::other))
    {
        if previous.attempted
            && previous.client_id == draft.client_id
            && serde_json::to_value(&previous).ok() != serde_json::to_value(&draft).ok()
        {
            return Err("该反馈已尝试提交，请重试原反馈或新建反馈。".into());
        }
    }
    save(root, draft)
}

#[tauri::command]
pub fn feedback_draft_discard(app: AppHandle) -> Result<(), String> {
    let path = draft_path(&crate::app_data(&app));
    if path.exists() {
        fs::remove_file(path).map_err(|_| "无法删除草稿。")?;
    }
    Ok(())
}

#[tauri::command]
pub fn feedback_diagnostics(app: AppHandle, hours: u32) -> serde_json::Value {
    diagnostics::snapshot(&crate::app_data(&app), hours)
}

#[tauri::command]
pub fn diagnostic_record(app: AppHandle, event: diagnostics::EventInput) -> Result<(), String> {
    diagnostics::record(&crate::app_data(&app), event)
}

fn payload(draft: &Draft) -> serde_json::Value {
    let mut value = serde_json::json!({ "client_id": draft.client_id, "category": draft.category, "description": draft.description, "contact": draft.contact,
        "context": { "app_version": draft.environment.app_version, "platform": draft.environment.platform, "arch": draft.environment.arch, "page": draft.page, "related_batch_ids": draft.related_batch_ids } });
    if let Some(diagnostics) = &draft.diagnostics {
        value["diagnostics"] = diagnostics.clone();
    }
    value
}

async fn authenticated(
    root: &Path,
) -> Result<(String, String, tokio::sync::RwLockReadGuard<'static, ()>), String> {
    let (base, owner) = identity(root);
    let token = crate::devify_access_token_async(root, &base)
        .await
        .map_err(|_| "请先登录当前服务端，或保存诊断包后交给支持人员。")?;
    let guard = crate::UPLOAD_IDENTITY_LOCK.read().await;
    let (live_base, live_owner) = identity(root);
    if live_base != base || live_owner != owner || owner.is_none() {
        return Err("当前账号或服务端已改变，请重新打开反馈。".into());
    }
    Ok((base, token, guard))
}

async fn response_data(response: reqwest::Response) -> Result<serde_json::Value, String> {
    let status = response.status();
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 | 403 => "登录已失效或没有反馈权限，请重新登录。",
            404 => "当前服务端尚未开放问题反馈，请更新 Devify 或保存诊断包。",
            409 => "提交编号对应的内容已改变，请查看我的反馈或新建反馈。",
            413 => "截图或诊断内容过大，请缩小后新建反馈。",
            429 => "提交过于频繁，请稍后重试。",
            _ => "反馈服务暂不可用或拒绝了反馈，请保存草稿后重试。",
        }
        .into());
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|_| "反馈服务返回了无效响应。")?;
    Ok(body.get("data").cloned().unwrap_or(body))
}

#[tauri::command]
pub async fn feedback_submit(
    app: AppHandle,
    mut draft: Draft,
) -> Result<serde_json::Value, String> {
    validate(&draft)?;
    if draft.description.trim().is_empty() {
        return Err("请描述遇到的问题。".into());
    }
    let root = crate::app_data(&app);
    let (base, token, _guard) = authenticated(&root).await?;
    feedback_draft_save(app.clone(), draft.clone())?;
    draft.attempted = true;
    save(&root, &draft)?;
    let mut form = reqwest::multipart::Form::new().text("payload", payload(&draft).to_string());
    for (index, bytes) in image_bytes(&draft)?.into_iter().enumerate() {
        let mime = if bytes.starts_with(b"\x89PNG") {
            "image/png"
        } else {
            "image/jpeg"
        };
        form = form.part(
            "screenshots",
            reqwest::multipart::Part::bytes(bytes)
                .file_name(format!("screenshot-{index}"))
                .mime_str(mime)
                .map_err(|_| "无法准备截图。")?,
        );
    }
    let response = crate::devify_upload_client()?
        .post(format!("{base}/api/v1/feedbacks"))
        .bearer_auth(token)
        .multipart(form)
        .send()
        .await
        .map_err(|e| crate::devify_network_message("提交反馈失败，草稿已保留", &e))?;
    let result = response_data(response).await?;
    if result.get("id").and_then(|v| v.as_str()).is_none() {
        return Err("反馈响应缺少编号，草稿已保留。".into());
    }
    fs::remove_file(draft_path(&root))
        .map_err(|_| "反馈已提交，但无法删除本地草稿；可安全重试。")?;
    Ok(result)
}

#[tauri::command]
pub async fn feedback_list(app: AppHandle, page: u32) -> Result<serde_json::Value, String> {
    let root = crate::app_data(&app);
    let (base, token, _guard) = authenticated(&root).await?;
    let response = crate::devify_upload_client()?
        .get(format!(
            "{base}/api/v1/feedbacks?page={}",
            page.clamp(1, 10000)
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| crate::devify_network_message("读取反馈失败", &e))?;
    response_data(response).await
}

#[tauri::command]
pub fn feedback_export(app: AppHandle, draft: Draft) -> Result<String, String> {
    use tauri::Manager;
    validate(&draft)?;
    let dir = app
        .path()
        .download_dir()
        .map_err(|_| "无法找到下载目录。")?;
    fs::create_dir_all(&dir).map_err(|_| "无法创建下载目录。")?;
    let path = dir.join(format!("AIMyChats-feedback-{}.zip", new_id()?));
    let file = tempfile::NamedTempFile::new_in(&dir).map_err(|_| "无法保存诊断包。")?;
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("feedback.json", zip::write::SimpleFileOptions::default())
        .map_err(|_| "无法创建诊断包。")?;
    zip.write_all(payload(&draft).to_string().as_bytes())
        .map_err(|_| "无法写入诊断包。")?;
    for (index, bytes) in image_bytes(&draft)?.into_iter().enumerate() {
        let ext = if bytes.starts_with(b"\x89PNG") {
            "png"
        } else {
            "jpg"
        };
        zip.start_file(
            format!("screenshot-{}.{ext}", index + 1),
            zip::write::SimpleFileOptions::default(),
        )
        .map_err(|_| "无法创建诊断包。")?;
        zip.write_all(&bytes).map_err(|_| "无法写入诊断包。")?;
    }
    zip.finish()
        .map_err(|_| "无法保存诊断包。")?
        .persist_noclobber(&path)
        .map_err(|_| "无法保存诊断包。")?;
    Ok(path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_images_and_content_are_rejected_before_upload() {
        let mut draft = Draft {
            client_id: new_id().unwrap(),
            environment: Environment::default(),
            related_batch_ids: vec![],
            receiver: String::new(),
            owner_id: None,
            category: "other".into(),
            description: "bug".into(),
            contact: String::new(),
            page: "app".into(),
            screenshots: vec![Screenshot {
                data: STANDARD.encode(b"credentials"),
            }],
            diagnostics: None,
            attempted: false,
        };
        assert!(validate(&draft).is_err());
        draft.screenshots.clear();
        assert!(validate(&draft).is_ok());
        draft.diagnostics = Some(serde_json::json!({"token":"secret"}));
        assert!(validate(&draft).is_err());
    }

    #[test]
    fn attempted_draft_is_immutable_and_receivers_are_isolated() {
        let root = tempfile::tempdir().unwrap();
        let (receiver, owner_id) = identity(root.path());
        let draft = Draft {
            client_id: new_id().unwrap(),
            environment: Environment::default(),
            related_batch_ids: vec![42],
            receiver,
            owner_id,
            category: "upload".into(),
            description: "上传失败".into(),
            contact: String::new(),
            page: "chat".into(),
            screenshots: vec![],
            diagnostics: None,
            attempted: true,
        };
        save(root.path(), &draft).unwrap();
        assert!(save_edit(root.path(), &draft).is_ok());
        let mut changed = draft.clone();
        changed.description = "changed content".into();
        assert!(save_edit(root.path(), &changed).is_err());
        changed = draft.clone();
        changed.receiver = "https://other.example".into();
        assert!(save(root.path(), &changed).is_err());
        let raw = fs::read(draft_path(root.path())).unwrap();
        let recovered: Draft = serde_json::from_slice(&raw).unwrap();
        assert_eq!(recovered.description, draft.description);
        assert_eq!(payload(&recovered), payload(&draft));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(draft_path(root.path()))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn generated_diagnostics_can_be_saved_without_secrets() {
        let root = tempfile::tempdir().unwrap();
        diagnostics::record(
            root.path(),
            diagnostics::EventInput {
                module: diagnostics::Module::Upload,
                operation: diagnostics::Operation::Failure,
                code: Some(diagnostics::Code::CommandFailed),
                batch_id: Some(42),
                duration_ms: Some(7),
            },
        )
        .unwrap();
        let (receiver, owner_id) = identity(root.path());
        let draft = Draft {
            client_id: new_id().unwrap(),
            environment: Environment::default(),
            related_batch_ids: vec![],
            receiver,
            owner_id,
            category: "upload".into(),
            description: "failure".into(),
            contact: String::new(),
            page: "chat".into(),
            screenshots: vec![],
            diagnostics: Some(diagnostics::snapshot(root.path(), 0)),
            attempted: false,
        };
        assert!(save_edit(root.path(), &draft).is_ok());
        assert_eq!(payload(&draft)["diagnostics"]["events"][0]["batch_id"], 42);
    }
}
