//! macOS Full Disk Access: status probing, the System Settings deep link, and revealing
//! the app in Finder.
//!
//! macOS does not let an app request this permission — there is no consent alert, and the
//! `kTCCServiceSystemPolicyAllFiles` grant can only be switched on by the user in
//! System Settings. What an app *can* do is (a) tell whether it already holds the grant,
//! (b) deep-link to the exact pane, and (c) point Finder at itself so it can be dragged
//! into the list. That is the whole of the "request" available here.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize)]
pub struct DiskAccess {
    /// True only when a protected read actually succeeded — never inferred from a setting.
    pub granted: bool,
    /// granted | denied | unknown
    pub state: String,
    /// The `.app` bundle when running bundled, otherwise the executable.
    pub app_path: Option<String>,
    pub bundled: bool,
    /// Running from a read-only DMG mount: grants made there do not stick.
    pub translocated: bool,
    /// Where the app lives: applications | volumes | translocated | build | other.
    pub location: String,
    /// True when the bundle sits in /Applications, the only place a grant survives.
    pub installed: bool,
    /// WeChat's container, when this process can see it.
    pub wechat_dir: Option<String>,
    /// Human-readable reason for the current state.
    pub detail: String,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The `.app` bundle containing `exe`, if any.
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let app = contents.parent()?;
    if app.extension().map(|ext| ext == "app").unwrap_or(false) {
        Some(app.to_path_buf())
    } else {
        None
    }
}

pub fn is_translocated(path: &Path) -> bool {
    path.to_string_lossy().contains("/AppTranslocation/")
}

/// TCC grants bind to the app's location: a copy launched from a mounted DMG (or from a
/// translocated quarantine path) cannot keep the switch turned on, so the location decides
/// which instructions the UI shows before anything else.
pub fn location_of(path: &Path) -> &'static str {
    let text = path.to_string_lossy();
    if text.contains("/AppTranslocation/") {
        "translocated"
    } else if text.starts_with("/Volumes/") {
        "volumes"
    } else if text.starts_with("/Applications/") {
        "applications"
    } else if text.contains("/target/release/") || text.contains("/target/debug/") {
        "build"
    } else {
        "other"
    }
}

/// WeChat 4.x keeps its databases inside its own sandbox container, which macOS protects
/// with TCC. Reading the container's top level can succeed without the grant, so the probe
/// descends to the directory that actually holds the databases.
fn wechat_containers() -> Vec<PathBuf> {
    let Some(home) = home() else { return Vec::new() };
    let Ok(entries) = fs::read_dir(home.join("Library/Containers")) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            name.contains("wechat") || name.contains("weixin")
        })
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

/// The one path that decides the question: where WeChat 4.x keeps its accounts.
///
/// Nothing else is probed. The container root and `Data` are listable without any grant, and
/// the FileProvider/Share containers that also match "wechat" are not protected at all — probing
/// either would report "granted" for an app that cannot read a single chat database.
fn data_path(container: &Path) -> PathBuf {
    container.join("Data/Documents/xwechat_files")
}

enum Probe {
    Granted,
    Denied(PathBuf),
    Missing,
}

fn read_probe(path: &Path) -> Probe {
    match fs::read_dir(path) {
        Ok(_) => Probe::Granted,
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
            Probe::Denied(path.to_path_buf())
        }
        Err(_) => Probe::Missing,
    }
}

/// Only paths macOS actually gates behind Full Disk Access are probed, so a "granted" answer
/// means the app can read data it needs.
fn probe(paths: &[PathBuf]) -> Probe {
    let mut denial: Option<PathBuf> = None;
    for path in paths {
        match read_probe(path) {
            Probe::Granted => return Probe::Granted,
            Probe::Denied(path) => denial.get_or_insert(path),
            Probe::Missing => continue,
        };
    }
    match denial {
        Some(path) => Probe::Denied(path),
        None => Probe::Missing,
    }
}

pub fn status() -> DiskAccess {
    let exe = std::env::current_exe().ok();
    let bundle = exe.as_deref().and_then(bundle_of);
    let app_path = bundle.clone().or_else(|| exe.clone());
    let translocated = exe.as_deref().map(is_translocated).unwrap_or(false);

    let containers = wechat_containers();
    // Only a container that actually holds `xwechat_files` answers the question. A missing one
    // means it belongs to an extension or a share target, so it is skipped instead of being
    // mistaken for readable WeChat data.
    let mut wechat_dir: Option<PathBuf> = None;
    let mut result = Probe::Missing;
    for container in &containers {
        match read_probe(&data_path(container)) {
            Probe::Missing => continue,
            other => {
                wechat_dir = Some(container.clone());
                result = other;
                break;
            }
        }
    }

    // WeChat is not installed yet: a gated directory still tells us whether the switch is on.
    if matches!(result, Probe::Missing) {
        if let Some(home) = home() {
            result = probe(&[home.join("Library/Safari")]);
        }
    }

    let (state, detail) = match &result {
        Probe::Granted => ("granted".to_string(), "已能读取受保护的目录".to_string()),
        Probe::Denied(path) => (
            "denied".to_string(),
            format!("读取 {} 时被 macOS 拒绝", path.to_string_lossy()),
        ),
        Probe::Missing => {
            let detail = if containers.is_empty() {
                "没有找到微信的沙盒容器，无法据此判定权限".to_string()
            } else {
                "微信容器存在，但没有找到可读取的数据库目录".to_string()
            };
            ("unknown".to_string(), detail)
        }
    };

    DiskAccess {
        granted: matches!(result, Probe::Granted),
        state,
        app_path: app_path.as_ref().map(|path| path.to_string_lossy().to_string()),
        bundled: bundle.is_some(),
        translocated,
        location: app_path
            .as_deref()
            .map(location_of)
            .unwrap_or("other")
            .to_string(),
        installed: app_path
            .as_deref()
            .map(|path| location_of(path) == "applications")
            .unwrap_or(false),
        wechat_dir: wechat_dir.map(|path| path.to_string_lossy().to_string()),
        detail,
    }
}

#[cfg(target_os = "macos")]
fn open_settings_pane() -> Result<String, String> {
    use std::process::Command;
    // macOS 13+ (Ventura and later) uses the Settings app's extension URL; earlier systems
    // use the Security & Privacy pane. Try the new one first, then the legacy one.
    let urls = [
        "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles",
        "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
    ];
    let mut last = String::from("无法打开系统设置");
    for url in urls {
        match Command::new("open").arg(url).status() {
            Ok(status) if status.success() => return Ok(url.to_string()),
            Ok(status) => last = format!("open 退出码 {status}"),
            Err(err) => last = format!("无法启动 open：{err}"),
        }
    }
    Err(last)
}

#[cfg(not(target_os = "macos"))]
fn open_settings_pane() -> Result<String, String> {
    Err("只有 macOS 需要完全磁盘访问权限".into())
}

#[cfg(target_os = "macos")]
fn reveal(path: &Path) -> Result<(), String> {
    use std::process::Command;
    Command::new("open")
        .arg("-R")
        .arg(path)
        .status()
        .map_err(|err| format!("无法在 Finder 中显示：{err}"))
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(format!("open -R 退出码 {status}"))
            }
        })
}

#[cfg(not(target_os = "macos"))]
fn reveal(_path: &Path) -> Result<(), String> {
    Err("只有 macOS 需要完全磁盘访问权限".into())
}

pub fn open_settings() -> Result<String, String> {
    open_settings_pane()
}

/// Reveal the `.app` bundle (or the bare executable in development) so it can be dragged
/// straight into the Full Disk Access list.
pub fn reveal_app() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|err| format!("无法定位可执行文件：{err}"))?;
    let target = bundle_of(&exe).unwrap_or(exe);
    reveal(&target)?;
    Ok(target.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_is_found_from_the_macos_directory() {
        let exe = Path::new("/Applications/Wechatify.app/Contents/MacOS/wechat-devify-bridge");
        assert_eq!(
            bundle_of(exe),
            Some(PathBuf::from("/Applications/Wechatify.app"))
        );
    }

    #[test]
    fn bare_binaries_have_no_bundle() {
        assert_eq!(bundle_of(Path::new("/tmp/target/debug/wechat-devify-bridge")), None);
        assert_eq!(bundle_of(Path::new("/tmp/Wechatify.app/wechat-devify-bridge")), None);
    }

    #[test]
    fn location_distinguishes_installed_mounted_and_build_copies() {
        assert_eq!(
            location_of(Path::new("/Applications/Wechatify.app/Contents/MacOS/wechat-devify-bridge")),
            "applications"
        );
        assert_eq!(
            location_of(Path::new("/Volumes/Wechatify/Wechatify.app/Contents/MacOS/wechat-devify-bridge")),
            "volumes"
        );
        assert_eq!(
            location_of(Path::new("/Users/me/Downloads/Wechatify.app/Contents/MacOS/wechat-devify-bridge")),
            "other"
        );
        assert_eq!(
            location_of(Path::new("/repo/src-tauri/target/release/wechat-devify-bridge")),
            "build"
        );
    }

    #[test]
    fn translocation_is_detected_from_the_mount_path() {
        let path = Path::new(
            "/private/var/folders/x/AppTranslocation/ABC-123/d/Wechatify.app/Contents/MacOS/wechat-devify-bridge",
        );
        assert!(is_translocated(path));
        assert!(!is_translocated(Path::new(
            "/Applications/Wechatify.app/Contents/MacOS/wechat-devify-bridge"
        )));
    }

    #[test]
    fn only_the_account_directory_decides_the_answer() {
        // Listing a container root, its `Data`, or the un-protected FileProvider/Share
        // containers all succeed without a grant, so only this path may be probed.
        let container = Path::new("/Users/me/Library/Containers/com.tencent.xinWeChat");
        assert_eq!(
            data_path(container),
            PathBuf::from("/Users/me/Library/Containers/com.tencent.xinWeChat/Data/Documents/xwechat_files")
        );
    }

    #[test]
    fn status_is_consistent_with_its_state() {
        let access = status();
        match access.state.as_str() {
            "granted" => assert!(access.granted),
            "denied" | "unknown" => assert!(!access.granted),
            other => panic!("unexpected state {other}"),
        }
        assert!(!access.detail.is_empty());
    }
}
