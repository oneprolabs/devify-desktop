//! Immutable archive evidence survives a lost create/upload response.
use sha2::Digest;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
static ARCHIVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub fn lock() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    ARCHIVE_LOCK.lock().map_err(|_| "无法锁定归档存储。".into())
}
fn path(data: &Path, digest: &str) -> Result<PathBuf, String> {
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("归档摘要无效。".into());
    }
    Ok(data.join("upload-archives").join(format!("{digest}.zip")))
}
pub fn exists(data: &Path, digest: Option<&str>) -> bool {
    digest
        .and_then(|d| path(data, d).ok())
        .is_some_and(|p| p.is_file())
}
pub fn save(data: &Path, digest: &str, body: &[u8]) -> Result<(), String> {
    let target = path(data, digest)?;
    if body.len() > 64 * 1024 * 1024 {
        return Err("归档超过本机单批预算，请缩小范围。".into());
    }
    if hex::encode(sha2::Sha256::digest(body)) != digest {
        return Err("归档校验失败。".into());
    }
    let dir = target.parent().unwrap();
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    if target.exists() {
        load(data, digest)?;
        return Ok(());
    }
    let mut total = 0u64;
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())?.take(2001) {
        let entry = entry.map_err(|e| e.to_string())?;
        total = total.saturating_add(entry.metadata().map_err(|e| e.to_string())?.len());
    }
    if total.saturating_add(body.len() as u64) > 512 * 1024 * 1024 {
        return Err("本机待确认的备份已达到存储预算，等待现有任务完成后会自动继续。".into());
    }
    let mut file = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    file.write_all(body)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    file.persist_noclobber(&target).map_err(|e| e.to_string())?;
    fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn load(data: &Path, digest: &str) -> Result<Vec<u8>, String> {
    let p = path(data, digest)?;
    let meta = fs::metadata(&p).map_err(|e| e.to_string())?;
    if meta.len() > 64 * 1024 * 1024 {
        return Err("原归档超过本机读取预算。".into());
    }
    let raw = fs::read(p).map_err(|e| e.to_string())?;
    if hex::encode(sha2::Sha256::digest(&raw)) != digest {
        return Err("原归档校验失败，请保留任务并反馈。".into());
    }
    Ok(raw)
}
