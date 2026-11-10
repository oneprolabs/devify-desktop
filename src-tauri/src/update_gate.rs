use std::{collections::HashSet, sync::Mutex};

#[derive(Default)]
struct GateState { installing: bool, accounts: HashSet<String> }

#[derive(Default)]
pub struct UpdateGate(Mutex<GateState>);

pub struct UploadGuard<'a> { gate: &'a UpdateGate, account: String }

impl Drop for UploadGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.gate.0.lock() { state.accounts.remove(&self.account); }
    }
}

impl UpdateGate {
    pub fn begin_upload(&self, account: &str) -> Result<UploadGuard<'_>, String> {
        let mut state = self.0.lock().map_err(|_| "无法检查上传状态。")?;
        if state.installing { return Err("正在安装更新，请稍后重试。".into()); }
        if !state.accounts.insert(account.to_string()) { return Err("该账号正在上传，请稍后重试。".into()); }
        Ok(UploadGuard { gate: self, account: account.to_string() })
    }

    pub fn begin_install(&self) -> Result<(), String> {
        let mut state = self.0.lock().map_err(|_| "无法检查上传状态。")?;
        if state.installing { return Err("正在安装更新。".into()); }
        if !state.accounts.is_empty() { return Err("正在上传，请等待上传完成后再安装更新。".into()); }
        state.installing = true;
        Ok(())
    }

    pub fn cancel_install(&self) {
        if let Ok(mut state) = self.0.lock() { state.installing = false; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installing_and_uploading_cannot_overlap() {
        let gate = UpdateGate::default();
        let upload = gate.begin_upload("first").unwrap();
        assert!(gate.begin_install().is_err());
        drop(upload);
        gate.begin_install().unwrap();
        assert!(gate.begin_upload("first").is_err());
        gate.cancel_install();
        assert!(gate.begin_upload("first").is_ok());
    }
    #[test]
    fn account_uploads_are_unique_and_release_on_drop() {
        let gate = UpdateGate::default();
        let upload = gate.begin_upload("first").unwrap();
        assert!(gate.begin_upload("first").is_err());
        assert!(gate.begin_upload("second").is_ok());
        drop(upload);
        assert!(gate.begin_upload("first").is_ok());
    }
}
