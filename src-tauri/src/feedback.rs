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

use crate::devify_http::RequestTrace;
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
    /// Best-effort OS release, e.g. `macOS 14.5 (23F79)`; empty when it cannot be read.
    #[serde(default)]
    pub os_version: String,
    /// Installed WeChat version; empty when WeChat is missing or the version is unknown.
    #[serde(default)]
    pub wechat_version: String,
}

/// Longest value the server accepts for a `context` string field.
const ENVIRONMENT_FIELD_MAX: usize = 64;

impl Default for Environment {
    fn default() -> Self {
        Self {
            app_version: env!("CARGO_PKG_VERSION").into(),
            platform: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            os_version: limited(os_version()),
            wechat_version: limited(wechat_version()),
        }
    }
}

/// The server accepts at most 64 characters per environment field; keep the report valid
/// instead of dropping the whole submission because a host string is too long.
fn limited(mut value: String) -> String {
    if value.chars().count() > ENVIRONMENT_FIELD_MAX {
        value = value.chars().take(ENVIRONMENT_FIELD_MAX).collect();
    }
    value
}

fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// OS release of the machine that produced the report, for example `macOS 14.5 (23F79)`.
fn os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        let product = command_stdout("/usr/bin/sw_vers", &["-productVersion"]).unwrap_or_default();
        let build = command_stdout("/usr/bin/sw_vers", &["-buildVersion"]).unwrap_or_default();
        return match (product.is_empty(), build.is_empty()) {
            (true, _) => String::new(),
            (false, true) => format!("macOS {product}"),
            (false, false) => format!("macOS {product} ({build})"),
        };
    }
    #[cfg(target_os = "windows")]
    {
        const KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        let read = |name: &str| {
            command_stdout("reg", &["query", KEY, "/v", name]).and_then(|text| reg_value(&text, name))
        };
        let product = read("ProductName").unwrap_or_default();
        let release = read("DisplayVersion").unwrap_or_default();
        let build = read("CurrentBuild").unwrap_or_default();
        let name = [product, release].iter().filter(|part| !part.is_empty()).cloned().collect::<Vec<_>>().join(" ");
        return match (name.is_empty(), build.is_empty()) {
            (true, true) => String::new(),
            (true, false) => format!("Windows (build {build})"),
            (false, true) => name,
            (false, false) => format!("{name} ({build})"),
        };
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let name = std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|text| {
                text.lines().find_map(|line| {
                    line.strip_prefix("PRETTY_NAME=")
                        .map(|value| value.trim().trim_matches('"').to_string())
                })
            })
            .unwrap_or_default();
        let kernel = command_stdout("uname", &["-r"]).unwrap_or_default();
        return match (name.is_empty(), kernel.is_empty()) {
            (true, true) => String::new(),
            (true, false) => format!("Linux {kernel}"),
            (false, true) => name,
            (false, false) => format!("{name} (Linux {kernel})"),
        };
    }
}

/// `reg query <key> /v <name>` prints `    Name    REG_SZ    value`.
#[cfg(target_os = "windows")]
fn reg_value(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.trim().strip_prefix(name)?;
        let value = rest
            .trim_start()
            .strip_prefix("REG_SZ")
            .or_else(|| rest.trim_start().strip_prefix("REG_DWORD"))?
            .trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Installed WeChat version. Detection is the same one that gates reading new data, so the
/// report matches what the client can actually see on this machine.
fn wechat_version() -> String {
    let detected = crate::connectors::detect(
        crate::connectors::ConnectorKind::Wechat,
        &crate::connectors::Environment::default(),
    );
    if let Some(version) = detected.version {
        return version;
    }
    #[cfg(target_os = "windows")]
    if let Some(path) = detected.app_path {
        let quoted = path.replace('\'', "''");
        if let Some(version) = command_stdout(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                &format!("(Get-Item -LiteralPath '{quoted}').VersionInfo.ProductVersion"),
            ],
        ) {
            return version;
        }
    }
    String::new()
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
    /// Legacy free-form contact field. The page no longer asks for it — the authenticated
    /// account identifies the reporter — but it stays readable for drafts saved earlier.
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
        || draft.environment.os_version.chars().count() > ENVIRONMENT_FIELD_MAX
        || draft.environment.wechat_version.chars().count() > ENVIRONMENT_FIELD_MAX
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
        let mut draft: Draft = serde_json::from_slice(&bytes).map_err(|_| "反馈草稿损坏。")?;
        validate(&draft)?;
        // The environment is frozen when the report is first sent, so an editable draft
        // follows the current machine and fills fields missing from drafts saved earlier.
        if !draft.attempted {
            draft.environment = Environment::default();
        }
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

/// Request body for `POST /api/v1/feedbacks` and the `feedback.json` inside the ZIP bundle.
///
/// `contact` is intentionally absent: the authenticated account is the reporter. Newer
/// clients also send the automatically detected OS and WeChat versions; `with_environment`
/// is false only when retrying against a server that predates those context fields.
fn payload(draft: &Draft, with_environment: bool) -> serde_json::Value {
    let mut context = serde_json::json!({
        "app_version": draft.environment.app_version,
        "platform": draft.environment.platform,
        "arch": draft.environment.arch,
        "page": draft.page,
        "related_batch_ids": draft.related_batch_ids,
    });
    if with_environment {
        context["os_version"] = draft.environment.os_version.clone().into();
        context["wechat_version"] = draft.environment.wechat_version.clone().into();
    }
    let mut value = serde_json::json!({
        "client_id": draft.client_id,
        "category": draft.category,
        "description": draft.description,
        "context": context,
    });
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
    let body = response
        .text()
        .await
        .map_err(|_| "反馈服务返回了无效响应。".to_string())?;
    feedback_data(status, &body)
}

fn feedback_data(status: reqwest::StatusCode, body: &str) -> Result<serde_json::Value, String> {
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 | 403 => "登录已失效或没有反馈权限，请重新登录。",
            404 => "当前服务端尚未开放问题反馈，请更新客户端或保存诊断包。",
            409 => "提交编号对应的内容已改变，请查看我的反馈或新建反馈。",
            413 => "截图或诊断内容过大，请缩小后新建反馈。",
            429 => "提交过于频繁，请稍后重试。",
            _ => "反馈服务暂不可用或拒绝了反馈，请保存草稿后重试。",
        }
        .into());
    }
    let parsed: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "反馈服务返回了无效响应。")?;
    Ok(parsed.get("data").cloned().unwrap_or(parsed))
}

fn feedback_form(draft: &Draft, with_environment: bool) -> Result<reqwest::multipart::Form, String> {
    let mut form =
        reqwest::multipart::Form::new().text("payload", payload(draft, with_environment).to_string());
    for (index, bytes) in image_bytes(draft)?.into_iter().enumerate() {
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
    Ok(form)
}

async fn post_feedback(
    root: &Path,
    base: &str,
    token: &str,
    form: reqwest::multipart::Form,
) -> Result<(reqwest::StatusCode, String), String> {
    let response = crate::http_client::upload()
        .post(format!("{base}/api/v1/feedbacks"))
        .bearer_auth(token)
        .multipart(form)
        .send_traced(root, diagnostics::Module::Feedback)
        .await
        .map_err(|e| crate::devify_network_message("提交反馈失败，草稿已保留", &e))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|_| "反馈服务返回了无效响应。".to_string())?;
    Ok((status, body))
}

/// Send one draft, falling back to the legacy body when the server rejects the newer
/// environment fields. Split from [`feedback_submit`] so tests can drive the real request
/// bytes without inventing a Tauri app handle.
async fn send_feedback(
    root: &Path,
    base: &str,
    token: &str,
    draft: &Draft,
) -> Result<serde_json::Value, String> {
    let (status, body) = post_feedback(root, base, token, feedback_form(draft, true)?).await?;
    // A server that predates the host environment fields rejects unknown context keys.
    // Resend the legacy body so the report still arrives instead of failing outright.
    let (status, body) = if status.as_u16() == 400 && body.contains("不支持的诊断字段") {
        post_feedback(root, base, token, feedback_form(draft, false)?).await?
    } else {
        (status, body)
    };
    let result = feedback_data(status, &body)?;
    if result.get("id").and_then(|v| v.as_str()).is_none() {
        return Err("反馈响应缺少编号，草稿已保留。".into());
    }
    Ok(result)
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
    let result = send_feedback(&root, &base, &token, &draft).await?;
    fs::remove_file(draft_path(&root))
        .map_err(|_| "反馈已提交，但无法删除本地草稿；可安全重试。")?;
    Ok(result)
}

#[tauri::command]
pub async fn feedback_list(app: AppHandle, page: u32) -> Result<serde_json::Value, String> {
    let root = crate::app_data(&app);
    let (base, token, _guard) = authenticated(&root).await?;
    let response = crate::http_client::upload()
        .get(format!(
            "{base}/api/v1/feedbacks?page={}",
            page.clamp(1, 10000)
        ))
        .bearer_auth(token)
        .send_traced_retrying(&root, diagnostics::Module::Feedback, crate::devify_http::RetryPolicy::READ)
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
    zip.write_all(payload(&draft, true).to_string().as_bytes())
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

    fn draft_with(environment: Environment) -> Draft {
        Draft {
            client_id: new_id().unwrap(),
            environment,
            related_batch_ids: vec![],
            receiver: String::new(),
            owner_id: None,
            category: "other".into(),
            description: "bug".into(),
            contact: "old@example.com".into(),
            page: "app".into(),
            screenshots: vec![],
            diagnostics: None,
            attempted: false,
        }
    }

    #[test]
    fn payload_drops_contact_and_adds_host_versions() {
        let draft = draft_with(Environment::default());
        let value = payload(&draft, true);
        assert!(value.get("contact").is_none());
        assert_eq!(value["context"]["os_version"], draft.environment.os_version);
        assert_eq!(value["context"]["wechat_version"], draft.environment.wechat_version);
        assert!(value["context"]["os_version"].as_str().unwrap().chars().count() <= ENVIRONMENT_FIELD_MAX);
        assert!(validate(&draft).is_ok());
    }

    #[test]
    fn legacy_payload_keeps_only_the_fields_older_servers_accept() {
        let draft = draft_with(Environment::default());
        let context = payload(&draft, false)["context"].clone();
        assert!(context.get("os_version").is_none());
        assert!(context.get("wechat_version").is_none());
        assert!(context.get("app_version").is_some());
    }

    #[test]
    fn drafts_saved_before_host_versions_still_load() {
        let json = r#"{
            "client_id": "77d31076-8e6c-4afe-80ce-7fe010cb6296",
            "environment": { "app_version": "0.1.0", "platform": "macos", "arch": "aarch64" },
            "related_batch_ids": [], "receiver": "", "owner_id": null,
            "category": "other", "description": "bug", "contact": "", "page": "app",
            "screenshots": [], "diagnostics": null, "attempted": false
        }"#;
        let draft: Draft = serde_json::from_str(json).unwrap();
        assert!(draft.environment.os_version.is_empty());
        assert!(draft.environment.wechat_version.is_empty());
        assert!(validate(&draft).is_ok());
    }

    #[test]
    fn oversized_host_versions_are_rejected() {
        let mut draft = draft_with(Environment::default());
        draft.environment.os_version = "x".repeat(ENVIRONMENT_FIELD_MAX + 1);
        assert!(validate(&draft).is_err());
        draft.environment.os_version = "x".repeat(ENVIRONMENT_FIELD_MAX);
        assert!(validate(&draft).is_ok());
    }

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
        assert_eq!(payload(&recovered, true), payload(&draft, true));
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
                request_id: Some(
                    crate::devify_http::RequestId::try_from("desktop-123".to_string()).unwrap(),
                ),
                server_request_id: Some(
                    crate::devify_http::RequestId::try_from("server-456".to_string()).unwrap(),
                ),
                http_status: Some(503),
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
        assert_eq!(payload(&draft, true)["diagnostics"]["events"][0]["batch_id"], 42);
        assert_eq!(
            payload(&draft, true)["diagnostics"]["events"][0]["request_id"],
            "desktop-123"
        );
    }

    /// Serve `responses` in order and return the request line, headers and body of each
    /// request. Records the real bytes the client sends instead of re-deriving the form.
    fn serve(
        listener: std::net::TcpListener,
        responses: Vec<(u16, String)>,
    ) -> std::thread::JoinHandle<Vec<(String, String, Vec<u8>)>> {
        use std::io::{Read, Write};
        std::thread::spawn(move || {
            let mut captured = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_nonblocking(false).unwrap();
                let mut data = Vec::new();
                let mut chunk = [0u8; 4096];
                while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                    let len = stream.read(&mut chunk).unwrap();
                    assert!(len > 0, "connection closed before headers");
                    data.extend_from_slice(&chunk[..len]);
                }
                let head_end = data.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                let head = String::from_utf8(data[..head_end].to_vec()).unwrap();
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or_default();
                while data.len() < head_end + length {
                    let len = stream.read(&mut chunk).unwrap();
                    assert!(len > 0, "connection closed before the body");
                    data.extend_from_slice(&chunk[..len]);
                }
                captured.push((
                    head.lines().next().unwrap().to_string(),
                    head.clone(),
                    data[head_end..head_end + length].to_vec(),
                ));
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                stream.flush().unwrap();
            }
            captured
        })
    }

    fn reproducible_draft() -> Draft {
        Draft {
            screenshots: vec![Screenshot {
                // A real 10x10 PNG: the server decodes uploads with Pillow.
                data: "iVBORw0KGgoAAAANSUhEUgAAAAoAAAAKCAIAAAACUFjqAAAAFklEQVR4nGP8//8/A27AhEeOYeRKAwCl4wMRx3ocVQAAAABJRU5ErkJggg==".into(),
            }],
            diagnostics: None,
            ..draft_with(Environment {
                app_version: "0.1.0".into(),
                platform: "macos".into(),
                arch: "aarch64".into(),
                os_version: "macOS 27.2.1 (26B131)".into(),
                wechat_version: "4.2.0".into(),
            })
        }
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn submission_posts_multipart_with_bearer_auth_and_environment() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = serve(
            listener,
            vec![(
                200,
                r#"{"code":0,"data":{"id":"4d1c9f1e-0000-4000-8000-000000000001","status":"open"}}"#
                    .into(),
            )],
        );
        let root = tempfile::tempdir().unwrap();
        let draft = reproducible_draft();
        let result = runtime().block_on(send_feedback(
            root.path(),
            &format!("http://{address}"),
            "secret-token",
            &draft,
        ));
        let value = result.unwrap();
        assert_eq!(value["id"], "4d1c9f1e-0000-4000-8000-000000000001");

        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 1);
        let (line, head, body) = &requests[0];
        assert_eq!(line, "POST /api/v1/feedbacks HTTP/1.1");
        let head = head.to_ascii_lowercase();
        assert!(head.contains("authorization: bearer secret-token"));
        assert!(head.contains("content-type: multipart/form-data; boundary="));
        let body_text = String::from_utf8_lossy(body);
        assert!(body_text.contains("name=\"payload\""));
        assert!(body_text.contains(r#""os_version":"macOS 27.2.1 (26B131)""#));
        assert!(body_text.contains(r#""wechat_version":"4.2.0""#));
        assert!(body_text.contains(r#""page":"app""#));
        assert!(!body_text.contains("contact"));
        assert!(!body_text.contains("old@example.com"));
        assert!(body_text.contains("name=\"screenshots\"; filename=\"screenshot-0\""));
        assert!(body_text.to_ascii_lowercase().contains("content-type: image/png"));
        assert!(body.windows(8).any(|w| w == b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn older_servers_are_retried_once_without_the_new_context_fields() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = serve(
            listener,
            vec![
                (400, r#"{"code":40001,"message":"不支持的诊断字段: os_version"}"#.into()),
                (200, r#"{"data":{"id":"4d1c9f1e-0000-4000-8000-000000000002"}}"#.into()),
            ],
        );
        let root = tempfile::tempdir().unwrap();
        let draft = reproducible_draft();
        let result = runtime().block_on(send_feedback(
            root.path(),
            &format!("http://{address}"),
            "secret-token",
            &draft,
        ));
        assert_eq!(result.unwrap()["id"], "4d1c9f1e-0000-4000-8000-000000000002");

        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        let first = String::from_utf8_lossy(&requests[0].2).into_owned();
        let second = String::from_utf8_lossy(&requests[1].2).into_owned();
        assert_eq!(requests[1].0, "POST /api/v1/feedbacks HTTP/1.1");
        assert!(first.contains(r#""os_version""#));
        assert!(!second.contains(r#""os_version""#));
        assert!(!second.contains(r#""wechat_version""#));
        assert!(second.contains(r#""app_version":"0.1.0""#));
        // The retry must reuse the same client id so the server can deduplicate.
        assert!(second.contains(&draft.client_id));
    }

    #[test]
    fn unsupported_or_unauthenticated_servers_report_actionable_errors() {
        let not_open = feedback_data(reqwest::StatusCode::NOT_FOUND, "").unwrap_err();
        assert!(not_open.contains("尚未开放"), "{not_open}");
        let expired = feedback_data(reqwest::StatusCode::UNAUTHORIZED, "").unwrap_err();
        assert!(expired.contains("登录已失效"), "{expired}");
        let conflict = feedback_data(reqwest::StatusCode::CONFLICT, "").unwrap_err();
        assert!(conflict.contains("已改变"), "{conflict}");
        let malformed = feedback_data(reqwest::StatusCode::OK, "<html>not json</html>").unwrap_err();
        assert!(malformed.contains("无效响应"), "{malformed}");
    }
}
