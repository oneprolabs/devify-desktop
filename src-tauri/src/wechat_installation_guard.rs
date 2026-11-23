//! Only the WeChat installation-operation lock is shared between app variants.
//! Keys, recovery assets and all application state remain in the owning app directory.
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

pub struct Guard {
    _file: File,
}

pub fn acquire(data: &Path) -> Result<Guard, String> {
    #[cfg(target_os = "macos")]
    if crate::build_flavor::DEVELOPMENT && production_running()? {
        return Err(
            "连接微信前请先退出正式版 AIMyChats，完成连接和恢复后可重新打开。读取聊天不受影响。"
                .into(),
        );
    }
    acquire_at(data, crate::build_flavor::IDENTIFIER)
}

/// Map the command's private recovery directory to its owning app directory.
pub fn for_capture(root: &Path) -> Result<Guard, crate::wechat::capture::CaptureError> {
    let owner = root.parent().ok_or_else(|| {
        crate::wechat::capture::CaptureError::new("installation_busy", "无法确定连接目录。")
    })?;
    acquire(owner)
        .map_err(|message| crate::wechat::capture::CaptureError::new("installation_busy", message))
}

fn acquire_at(data: &Path, identifier: &str) -> Result<Guard, String> {
    let parent = data.parent().ok_or("无法确定应用数据目录。")?;
    let shared = parent.join(".aimychats-wechat-operation");
    std::fs::create_dir_all(&shared).map_err(|e| format!("无法创建微信操作锁：{e}"))?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(shared.join("installation.lock"))
        .map_err(|e| format!("无法打开微信操作锁：{e}"))?;
    file.try_lock()
        .map_err(|_| "另一个版本正在连接或恢复微信，请等待操作结束。".to_string())?;
    let other = if identifier == crate::build_flavor::DEVELOPMENT_ID {
        crate::build_flavor::PRODUCTION_ID
    } else {
        crate::build_flavor::DEVELOPMENT_ID
    };
    // The capture flow spans several user-driven commands. A surviving recovery
    // record retains ownership between commands and across process death.
    if parent
        .join(other)
        .join("key-capture")
        .join(crate::wechat::capture::STATE_NAME)
        .exists()
    {
        return Err("另一个版本还有未完成的微信连接，请先回到那个版本恢复微信，再继续。".into());
    }
    Ok(Guard { _file: file })
}

#[cfg(target_os = "macos")]
fn production_running() -> Result<bool, String> {
    // Bundle identity remains stable when the user renames or moves the app.
    let script = r#"ObjC.import("AppKit"); $.NSWorkspace.sharedWorkspace.runningApplications.js.some(function(app) { return ObjC.unwrap(app.bundleIdentifier) === "local.wechat.devifybridge"; })"#;
    let output = std::process::Command::new("/usr/bin/osascript")
        .args(["-l", "JavaScript", "-e", script])
        .output()
        .map_err(|e| format!("无法检查正式版是否运行，已停止连接微信：{e}"))?;
    if !output.status.success() {
        return Err("无法检查正式版是否运行，已停止连接微信。".into());
    }
    match String::from_utf8_lossy(&output.stdout).trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("无法确认正式版是否运行，已停止连接微信。".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn variants_share_only_the_installation_lock_and_pending_transaction_check() {
        let temp = tempfile::tempdir().unwrap();
        let prod = temp.path().join(crate::build_flavor::PRODUCTION_ID);
        let dev = temp.path().join(crate::build_flavor::DEVELOPMENT_ID);
        let first = acquire_at(&prod, crate::build_flavor::PRODUCTION_ID).unwrap();
        assert!(acquire_at(&dev, crate::build_flavor::DEVELOPMENT_ID).is_err());
        drop(first);
        assert!(acquire_at(&dev, crate::build_flavor::DEVELOPMENT_ID).is_ok());
        let state = prod
            .join("key-capture")
            .join(crate::wechat::capture::STATE_NAME);
        std::fs::create_dir_all(state.parent().unwrap()).unwrap();
        std::fs::write(&state, "pending recovery").unwrap();
        assert!(acquire_at(&dev, crate::build_flavor::DEVELOPMENT_ID).is_err());
        assert!(acquire_at(&prod, crate::build_flavor::PRODUCTION_ID).is_ok());
        std::fs::remove_file(state).unwrap();
        assert!(acquire_at(&dev, crate::build_flavor::DEVELOPMENT_ID).is_ok());
    }
}
