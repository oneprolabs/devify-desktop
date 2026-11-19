//! The OS owns this lock; force quit/power loss cannot leave a stale lock behind.
use std::{
    fs::{File, OpenOptions},
    path::Path,
};
pub struct Guard {
    _file: File,
}
pub fn acquire(data: &Path) -> Result<Guard, String> {
    std::fs::create_dir_all(data).map_err(|e| e.to_string())?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(data.join("client-runtime.lock"))
        .map_err(|e| format!("无法打开运行锁：{e}"))?;
    file.try_lock()
        .map_err(|_| "已有一个 AIMyChats 正在使用这个数据目录，请回到已打开的窗口。".to_string())?;
    Ok(Guard { _file: file })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lock_is_released_with_the_process_handle() {
        let temp = tempfile::tempdir().unwrap();
        let one = acquire(temp.path()).unwrap();
        assert!(acquire(temp.path()).is_err());
        drop(one);
        assert!(acquire(temp.path()).is_ok());
    }
}
