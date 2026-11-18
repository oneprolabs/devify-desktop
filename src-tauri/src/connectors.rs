//! Local install detection for connectors that read a desktop app's own data.
//!
//! Every connector kind must answer `detect`; the `match` in [`detect`] is exhaustive, so
//! adding a kind without a detector does not compile. The install gate is reserved for
//! operations that execute or modify the vendor bundle; local read-only data paths can use
//! already verified keys without requiring the installed app to pass this check.

use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::wechat::capture;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectState {
    Installed,
    NotInstalled,
    /// Present, but not the vendor's signed build.
    Untrusted,
    Unsupported,
    /// The connector does not depend on a local app.
    NotRequired,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectorDetect {
    pub state: DetectState,
    pub app_path: Option<String>,
    pub version: Option<String>,
    pub message: String,
}

impl ConnectorDetect {
    pub fn usable(&self) -> bool {
        matches!(self.state, DetectState::Installed | DetectState::NotRequired)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorKind {
    Wechat,
    Whatsapp,
}

impl ConnectorKind {
    pub const ALL: &'static [ConnectorKind] = &[ConnectorKind::Wechat, ConnectorKind::Whatsapp];

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.id() == value)
    }

    pub fn id(self) -> &'static str {
        match self {
            ConnectorKind::Wechat => "wechat",
            ConnectorKind::Whatsapp => "whatsapp",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ConnectorKind::Wechat => "微信",
            ConnectorKind::Whatsapp => "WhatsApp",
        }
    }
}

/// App state the detectors need but cannot read themselves.
#[derive(Debug, Clone, Copy, Default)]
pub struct Environment {
    /// An in-app key capture is running, so `/Applications/WeChat.app` is temporarily our
    /// ad-hoc build and must not be reported as untrusted.
    pub capture_pending: bool,
}

pub fn detect(kind: ConnectorKind, env: &Environment) -> ConnectorDetect {
    match kind {
        ConnectorKind::Wechat => detect_wechat(env),
        ConnectorKind::Whatsapp => detect_whatsapp(),
    }
}

/// Gate for every command that reads new data from the local app.
pub fn require_installed(kind: ConnectorKind, env: &Environment) -> Result<(), String> {
    let result = detect(kind, env);
    if result.usable() {
        return Ok(());
    }
    Err(blocked_message(kind, &result))
}

pub fn blocked_message(kind: ConnectorKind, result: &ConnectorDetect) -> String {
    let title = match result.state {
        DetectState::NotInstalled => format!("未检测到{}", kind.label()),
        DetectState::Untrusted => format!("检测到的{}不是官方版本", kind.label()),
        DetectState::Unsupported => "当前系统暂不支持".to_string(),
        DetectState::Installed | DetectState::NotRequired => String::new(),
    };
    format!("{title}，已停止读取新的聊天数据。{}", result.message)
}

fn not_installed(message: impl Into<String>) -> ConnectorDetect {
    ConnectorDetect {
        state: DetectState::NotInstalled,
        app_path: None,
        version: None,
        message: message.into(),
    }
}

#[cfg(target_os = "macos")]
fn detect_wechat(env: &Environment) -> ConnectorDetect {
    let mut candidates = vec![PathBuf::from(capture::WECHAT_APP)];
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join("Applications/WeChat.app"));
    }
    classify_macos_wechat(
        &candidates,
        env.capture_pending,
        |app| capture::inspect_signature(app, false).ok(),
        |app| capture::bundle_version(app).ok().map(|value| value.short),
    )
}

#[cfg(target_os = "windows")]
fn detect_wechat(_env: &Environment) -> ConnectorDetect {
    let mut candidates = Vec::new();
    for key in WINDOWS_REGISTRY_KEYS {
        let output = std::process::Command::new("reg")
            .args(["query", key, "/v", "InstallPath"])
            .output();
        if let Ok(output) = output {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(dir) = parse_reg_install_path(&text) {
                candidates.push(PathBuf::from(&dir).join("Weixin.exe"));
                candidates.push(PathBuf::from(&dir).join("WeChat.exe"));
            }
        }
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(var) {
            let base = PathBuf::from(base);
            candidates.push(base.join("Tencent\\Weixin\\Weixin.exe"));
            candidates.push(base.join("Tencent\\WeChat\\WeChat.exe"));
        }
    }
    classify_windows_wechat(&candidates)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn detect_wechat(_env: &Environment) -> ConnectorDetect {
    ConnectorDetect {
        state: DetectState::Unsupported,
        app_path: None,
        version: None,
        message: "当前系统没有微信桌面版的检测方式，暂不支持读取本机微信。".into(),
    }
}

/// WhatsApp's Developer ID identity. The Mac app stores its store in plaintext, so the
/// vendor signature is the only signal that the binary was not swapped for something else.
const WHATSAPP_TEAM: &str = "57T9237FN3";
const WHATSAPP_IDENTIFIER: &str = "net.whatsapp.WhatsApp";

#[cfg(target_os = "macos")]
fn detect_whatsapp() -> ConnectorDetect {
    let app = PathBuf::from(crate::whatsapp::APP_BUNDLE);
    let app = app.is_dir().then_some(app);
    let has_data = crate::whatsapp::data_dir()
        .map(|dir| dir.join("ChatStorage.sqlite"))
        .is_some_and(|path| path.is_file());
    classify_macos_whatsapp(
        app.as_deref(),
        has_data,
        |path| capture::inspect_signature(path, false).ok(),
        |path| capture::bundle_version(path).ok().map(|value| value.short),
        |_| crate::whatsapp::bundle_version(),
    )
}

#[cfg(not(target_os = "macos"))]
fn detect_whatsapp() -> ConnectorDetect {
    ConnectorDetect {
        state: DetectState::Unsupported,
        app_path: None,
        version: None,
        message: "WhatsApp 接入当前只支持 macOS 版桌面应用。".into(),
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn classify_macos_whatsapp(
    app: Option<&Path>,
    has_data: bool,
    inspect: impl Fn(&Path) -> Option<capture::Signature>,
    version: impl Fn(&Path) -> Option<String>,
    fallback_version: impl Fn(&Path) -> Option<String>,
) -> ConnectorDetect {
    let Some(app) = app else {
        if has_data {
            return ConnectorDetect {
                state: DetectState::Installed,
                app_path: None,
                version: None,
                message: "已找到本机 WhatsApp 数据，可以直接读取。".into(),
            };
        }
        return not_installed(
            "没有在「应用程序」中找到 WhatsApp。请先安装 WhatsApp Mac 版并登录一次，再回来点「重新检查」。",
        );
    };
    let found = |state: DetectState, message: &str| ConnectorDetect {
        state,
        app_path: Some(app.to_string_lossy().to_string()),
        version: version(app).or_else(|| fallback_version(app)),
        message: message.to_string(),
    };
    let official = inspect(app).is_some_and(|signature| {
        signature.valid
            && !signature.ad_hoc
            && signature.team_identifier == WHATSAPP_TEAM
            && signature.identifier == WHATSAPP_IDENTIFIER
    });
    if !official {
        return found(
            DetectState::Untrusted,
            "这个 WhatsApp.app 不是 WhatsApp Inc. 官方签名，可能被修改或来自非官方渠道。为保护聊天数据，请从 WhatsApp 官网重新安装。",
        );
    }
    if !has_data {
        return found(
            DetectState::Installed,
            "已检测到 WhatsApp，但还没有可读取的聊天数据。请先登录一次并打开聊天列表。",
        );
    }
    found(DetectState::Installed, "已检测到官方签名的 WhatsApp 和本机聊天数据。")
}


#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const WINDOWS_REGISTRY_KEYS: &[&str] = &[
    "HKCU\\Software\\Tencent\\Weixin",
    "HKCU\\Software\\Tencent\\WeChat",
    "HKLM\\SOFTWARE\\WOW6432Node\\Tencent\\WeChat",
];

/// Pick the first Tencent-signed bundle; fall back to reporting the first present one.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn classify_macos_wechat(
    candidates: &[PathBuf],
    capture_pending: bool,
    inspect: impl Fn(&Path) -> Option<capture::Signature>,
    version: impl Fn(&Path) -> Option<String>,
) -> ConnectorDetect {
    let present: Vec<&PathBuf> = candidates.iter().filter(|path| path.is_dir()).collect();
    if present.is_empty() {
        return not_installed(
            "没有在「应用程序」中找到微信。请先安装微信 Mac 版 4.x 并登录一次，再回来点「重新检查」。",
        );
    }
    let found = |path: &Path, state: DetectState, message: &str| ConnectorDetect {
        state,
        app_path: Some(path.to_string_lossy().to_string()),
        version: version(path),
        message: message.to_string(),
    };
    for path in &present {
        if inspect(path).is_some_and(|signature| signature.is_tencent_official()) {
            return found(path, DetectState::Installed, "已检测到腾讯官方签名的微信。");
        }
    }
    let first = present[0];
    if capture_pending && first == Path::new(capture::WECHAT_APP) {
        return found(
            first,
            DetectState::Installed,
            "正在获取密钥，微信临时使用调试签名；完成或取消后会恢复腾讯原版。",
        );
    }
    found(
        first,
        DetectState::Untrusted,
        "这个 WeChat.app 不是腾讯官方签名，可能被修改或来自非官方渠道。为保护聊天数据，请从微信官网重新安装。",
    )
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn classify_windows_wechat(candidates: &[PathBuf]) -> ConnectorDetect {
    match candidates.iter().find(|path| path.is_file()) {
        Some(path) => ConnectorDetect {
            state: DetectState::Installed,
            app_path: Some(path.to_string_lossy().to_string()),
            version: None,
            message: "已检测到微信。".into(),
        },
        None => not_installed(
            "没有找到微信安装目录。请先安装微信 Windows 版 4.x 并登录一次，再回来点「重新检查」。",
        ),
    }
}

/// `reg query <key> /v InstallPath` prints `    InstallPath    REG_SZ    C:\...`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn parse_reg_install_path(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("InstallPath")?;
        let value = rest.trim_start().strip_prefix("REG_SZ")?.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn official() -> capture::Signature {
        capture::Signature {
            valid: true,
            ad_hoc: false,
            hardened_runtime: true,
            team_identifier: capture::TENANT_TEAM.into(),
            identifier: capture::TENANT_IDENTIFIER.into(),
            cdhash: "abc".into(),
        }
    }

    fn ad_hoc() -> capture::Signature {
        capture::Signature {
            ad_hoc: true,
            team_identifier: String::new(),
            ..official()
        }
    }

    fn app_dir(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn every_kind_round_trips_and_has_a_detector() {
        for kind in ConnectorKind::ALL {
            assert_eq!(ConnectorKind::parse(kind.id()), Some(*kind));
            let result = detect(*kind, &Environment::default());
            assert!(!result.message.is_empty(), "{} has no message", kind.id());
        }
        assert_eq!(ConnectorKind::parse("telegram"), None);
    }

    fn whatsapp_signature(team: &str, identifier: &str, ad_hoc: bool) -> capture::Signature {
        capture::Signature {
            valid: true,
            ad_hoc,
            hardened_runtime: true,
            team_identifier: team.into(),
            identifier: identifier.into(),
            cdhash: "abc".into(),
        }
    }

    #[test]
    fn missing_whatsapp_is_not_installed() {
        let result = classify_macos_whatsapp(None, false, |_| None, |_| None, |_| None);
        assert_eq!(result.state, DetectState::NotInstalled);
        assert!(!result.usable());
    }

    #[test]
    fn whatsapp_data_without_the_bundle_is_still_usable() {
        let result = classify_macos_whatsapp(None, true, |_| None, |_| None, |_| None);
        assert_eq!(result.state, DetectState::Installed);
        assert!(result.usable());
    }

    #[test]
    fn only_the_whatsapp_inc_signature_counts() {
        let root = tempfile::tempdir().unwrap();
        let app = app_dir(root.path(), "WhatsApp.app");
        let official = classify_macos_whatsapp(
            Some(&app),
            true,
            |_| Some(whatsapp_signature(WHATSAPP_TEAM, WHATSAPP_IDENTIFIER, false)),
            |_| Some("26.45.21".into()),
            |_| None,
        );
        assert_eq!(official.state, DetectState::Installed);
        assert_eq!(official.version.as_deref(), Some("26.45.21"));

        for signature in [
            whatsapp_signature(WHATSAPP_TEAM, WHATSAPP_IDENTIFIER, true),
            whatsapp_signature("AAAAAAAAAA", WHATSAPP_IDENTIFIER, false),
            whatsapp_signature(WHATSAPP_TEAM, "net.whatsapp.Impostor", false),
        ] {
            let result = classify_macos_whatsapp(Some(&app), true, |_| Some(signature.clone()), |_| None, |_| None);
            assert_eq!(result.state, DetectState::Untrusted);
            assert!(!result.usable());
        }
    }

    #[test]
    fn signed_whatsapp_without_data_is_installed_but_asks_for_login() {
        let root = tempfile::tempdir().unwrap();
        let app = app_dir(root.path(), "WhatsApp.app");
        let result = classify_macos_whatsapp(
            Some(&app),
            false,
            |_| Some(whatsapp_signature(WHATSAPP_TEAM, WHATSAPP_IDENTIFIER, false)),
            |_| None,
            |_| None,
        );
        assert_eq!(result.state, DetectState::Installed);
        assert!(result.message.contains("登录"));
    }

    #[test]
    fn missing_bundle_is_not_installed() {
        let root = tempfile::tempdir().unwrap();
        let result = classify_macos_wechat(
            &[root.path().join("WeChat.app")],
            false,
            |_| Some(official()),
            |_| None,
        );
        assert_eq!(result.state, DetectState::NotInstalled);
        assert!(!result.usable());
        assert!(result.app_path.is_none());
    }

    #[test]
    fn official_bundle_is_installed_with_version() {
        let root = tempfile::tempdir().unwrap();
        let app = app_dir(root.path(), "WeChat.app");
        let result = classify_macos_wechat(
            &[app.clone()],
            false,
            |_| Some(official()),
            |_| Some("4.1.2".into()),
        );
        assert_eq!(result.state, DetectState::Installed);
        assert_eq!(result.version.as_deref(), Some("4.1.2"));
        assert_eq!(result.app_path.as_deref(), Some(app.to_string_lossy().as_ref()));
    }

    #[test]
    fn resigned_or_unreadable_bundle_is_untrusted() {
        let root = tempfile::tempdir().unwrap();
        let app = app_dir(root.path(), "WeChat.app");
        for signature in [Some(ad_hoc()), None] {
            let result =
                classify_macos_wechat(&[app.clone()], false, |_| signature.clone(), |_| None);
            assert_eq!(result.state, DetectState::Untrusted);
            assert!(!result.usable());
        }
    }

    #[test]
    fn pending_capture_only_trusts_the_capture_location() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = app_dir(root.path(), "WeChat.app");
        let result = classify_macos_wechat(&[elsewhere], true, |_| Some(ad_hoc()), |_| None);
        assert_eq!(result.state, DetectState::Untrusted);
    }

    #[test]
    fn official_copy_wins_over_an_earlier_untrusted_one() {
        let root = tempfile::tempdir().unwrap();
        let tampered = app_dir(root.path(), "a/WeChat.app");
        let genuine = app_dir(root.path(), "b/WeChat.app");
        let genuine_for_check = genuine.clone();
        let result = classify_macos_wechat(
            &[tampered, genuine.clone()],
            false,
            move |path| Some(if path == genuine_for_check { official() } else { ad_hoc() }),
            |_| None,
        );
        assert_eq!(result.state, DetectState::Installed);
        assert_eq!(result.app_path.as_deref(), Some(genuine.to_string_lossy().as_ref()));
    }

    #[test]
    fn windows_detection_needs_the_executable() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("Weixin.exe");
        assert_eq!(classify_windows_wechat(&[exe.clone()]).state, DetectState::NotInstalled);
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(classify_windows_wechat(&[exe]).state, DetectState::Installed);
    }

    #[test]
    fn reg_output_is_parsed() {
        let text = "\r\nHKEY_CURRENT_USER\\Software\\Tencent\\Weixin\r\n    InstallPath    REG_SZ    C:\\Program Files\\Tencent\\Weixin\r\n";
        assert_eq!(
            parse_reg_install_path(text).as_deref(),
            Some("C:\\Program Files\\Tencent\\Weixin")
        );
        assert_eq!(parse_reg_install_path("ERROR: not found"), None);
    }

    #[test]
    fn gate_message_names_the_state() {
        let blocked = not_installed("请先安装。");
        let message = blocked_message(ConnectorKind::Wechat, &blocked);
        assert!(message.starts_with("未检测到微信，已停止读取新的聊天数据。"));
        assert!(message.ends_with("请先安装。"));
    }
}
