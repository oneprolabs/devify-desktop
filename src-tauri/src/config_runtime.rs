//! Durable, ordered configuration outbox. Bodies are immutable once enqueued.
use crate::{
    desktop_credentials,
    settings::{AppSettings, UploadOwner},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Runtime {
    pub active_profile: Option<String>,
    pub profiles: BTreeMap<String, Profile>,
    pub paused: bool,
    pub logout_pending: bool,
    pub sources: BTreeMap<String, Source>,
    pub pending: Vec<Mutation>,
    pub last_error: Option<String>,
    pub last_cleanup: Option<String>,
}
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Source {
    pub owner: Option<UploadOwner>,
    pub draft: bool,
    pub backup_enabled: Option<bool>,
    pub completed: bool,
    pub wizard_step: Option<String>,
    pub acknowledged: Option<serde_json::Value>,
    pub active: Option<serde_json::Value>,
    pub last_run_at: Option<String>,
    pub scan_index: usize,
    pub scan_remaining: usize,
    pub retry_after: Option<String>,
    pub failures: u32,
    pub last_error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mutation {
    pub owner: UploadOwner,
    pub body: serde_json::Value,
}

/// Legacy clients stored two switches. Reading either disabled value must keep
/// backup off; only an explicit user edit enables it and repairs both fields.
pub fn backup_enabled(value: &AppSettings) -> bool {
    value.sync.enabled && !value.runtime.paused
}

pub fn set_backup_enabled(value: &mut AppSettings, enabled: bool) {
    value.sync.enabled = enabled;
    value.runtime.paused = !enabled;
}

pub fn enqueue_current_sources(data: &Path, value: &mut AppSettings) -> Result<(), String> {
    let accounts: std::collections::BTreeSet<_> = value.chat.allowed_usernames.keys()
        .chain(value.runtime.sources.keys()).cloned().collect();
    for account in accounts {
        if let Ok(owner) = local_owner(data, value, &account) {
            if value.runtime.sources.get(&account).and_then(|s| s.owner.as_ref())
                .is_none_or(|old| same_principal(old, &owner)) {
                enqueue(value, owner)?;
            }
        }
    }
    Ok(())
}

pub fn payload(value: &AppSettings, account: &str, draft: bool) -> serde_json::Value {
    let names = value
        .chat
        .allowed_usernames
        .get(account)
        .cloned()
        .unwrap_or_default();
    let ranges: BTreeMap<_, _> = value
        .chat
        .initial_ranges
        .get(account)
        .into_iter()
        .flat_map(|r| r.iter())
        .filter(|(name, _)| names.contains(name))
        .collect();
    let enabled = value
        .runtime
        .sources
        .get(account)
        .is_none_or(|s| s.backup_enabled != Some(false))
        && backup_enabled(value)
        && if crate::whatsapp::is_account(account) {
            value.connectors.whatsapp_enabled
        } else {
            value.connectors.wechat_enabled
        };
    serde_json::json!({"setup_state":if draft {"draft"} else {"completed"}, "enabled":enabled,
        "usernames":names,"initial_ranges":ranges,"sync_interval_seconds":value.chat.sync_interval_seconds,"backup":value.backup})
}

pub fn local_owner(data: &Path, value: &AppSettings, account: &str) -> Result<UploadOwner, String> {
    let base_url = value.devify.resolved_base_url()?;
    let credentials = desktop_credentials::load(data, &base_url)?;
    let user_id = crate::access_token_claim(&credentials.access, "user_id")
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .ok_or("请先登录。")?;
    let device_id = crate::cloud_retry_device_id(&value.devify.device_id, &credentials.access)?;
    Ok(UploadOwner {
        base_url,
        user_id,
        device_id,
        space_id: value
            .devify
            .active_space_id
            .clone()
            .ok_or("请先选择空间。")?,
        account_id: account.into(),
    })
}

pub fn same_principal(a: &UploadOwner, b: &UploadOwner) -> bool {
    a.user_id == b.user_id
        && a.device_id == b.device_id
        && a.space_id == b.space_id
        && a.account_id == b.account_id
        && crate::settings::same_service_url(&a.base_url, &b.base_url)
}

pub fn enqueue(value: &mut AppSettings, owner: UploadOwner) -> Result<(), String> {
    let account = owner.account_id.clone();
    let source = value.runtime.sources.entry(account.clone()).or_default();
    if source
        .owner
        .as_ref()
        .is_some_and(|old| !same_principal(old, &owner))
    {
        return Err("该来源属于另一个登录账号，请重新完成连接向导。".into());
    }
    source.owner = Some(owner.clone());
    // A legacy source needs an explicit completed wizard unless it already has a
    // verified progress owner. Merely viewing settings cannot activate uploads.
    if !source.completed && !source.draft {
        source.completed = value
            .sync
            .owners
            .get(&account)
            .is_some_and(|old| same_principal(old, &owner))
            || value.upload_queue.iter().any(|j| {
                j.owner
                    .as_ref()
                    .is_some_and(|old| same_principal(old, &owner))
            });
        source.draft = !source.completed;
    }
    let draft = source.draft;
    let desired = payload(value, &account, draft);
    let last = value
        .runtime
        .pending
        .iter()
        .rev()
        .find(|m| same_principal(&m.owner, &owner))
        .map(|m| &m.body["payload"])
        .or_else(|| {
            value
                .runtime
                .sources
                .get(&account)
                .and_then(|s| s.acknowledged.as_ref())
        });
    if last == Some(&desired) {
        return Ok(());
    }
    if value.runtime.pending.len() >= 128 {
        return Err("待提交的设置较多，请恢复连接后重试。".into());
    }
    let mut nonce = [0u8; 16];
    getrandom::getrandom(&mut nonce).map_err(|_| "无法生成配置提交标识。")?;
    let mutation_id = hex::encode(nonce);
    let body = serde_json::json!({"schema_version":1,"device_id":owner.device_id,"mutation_id":mutation_id,
        "local_revision":value.settings_revision.saturating_add(1),"target":{"platform":crate::expected_source_type(&account),"source_account_id":account},"payload":desired});
    value.runtime.pending.push(Mutation { owner, body });
    Ok(())
}

pub fn ready(value: &AppSettings, owner: &UploadOwner) -> bool {
    !value.runtime.paused
        && value
            .runtime
            .sources
            .get(&owner.account_id)
            .is_some_and(|source| {
                source.completed
                    && !source.draft
                    && source.backup_enabled != Some(false)
                    && source
                        .owner
                        .as_ref()
                        .is_some_and(|old| same_principal(old, owner))
                    && source.acknowledged.is_some()
            })
        && !value
            .runtime
            .pending
            .iter()
            .any(|m| same_principal(&m.owner, owner))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owner() -> UploadOwner {
        UploadOwner {
            base_url: "http://localhost".into(),
            user_id: 1,
            device_id: "d".into(),
            space_id: "s".into(),
            account_id: "a".into(),
        }
    }
    #[test]
    fn draft_completion_has_new_key_and_frozen_body() {
        let mut v = AppSettings::default();
        v.runtime.sources.insert(
            "a".into(),
            Source {
                draft: true,
                ..Default::default()
            },
        );
        enqueue(&mut v, owner()).unwrap();
        let original = v.runtime.pending[0].body.clone();
        enqueue(&mut v, owner()).unwrap();
        assert_eq!(v.runtime.pending.len(), 1);
        let s = v.runtime.sources.get_mut("a").unwrap();
        s.draft = false;
        s.completed = true;
        enqueue(&mut v, owner()).unwrap();
        assert_eq!(v.runtime.pending.len(), 2);
        assert_eq!(v.runtime.pending[0].body, original);
        assert_ne!(
            v.runtime.pending[0].body["mutation_id"],
            v.runtime.pending[1].body["mutation_id"]
        );
        assert!(!ready(&v, &owner()));
        let mut other = owner();
        other.user_id = 2;
        assert!(enqueue(&mut v, other).is_err());
    }
}

/// User/source settings are a local profile, while task evidence remains globally
/// stored with its immutable owner. A new login never inherits another user's plan.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    active_account_id: String,
    chat: crate::settings::ChatPolicy,
    connectors: crate::settings::ConnectorSettings,
    keys: crate::settings::KeysSettings,
    sync: crate::settings::SyncSettings,
    backup: crate::settings::BackupSettings,
    sources: BTreeMap<String, Source>,
    paused: bool,
}
impl Profile {
    fn capture(s: &AppSettings) -> Self {
        Self {
            active_account_id: s.active_account_id.clone(),
            chat: s.chat.clone(),
            connectors: s.connectors.clone(),
            keys: s.keys.clone(),
            sync: s.sync.clone(),
            backup: s.backup.clone(),
            sources: s.runtime.sources.clone(),
            paused: s.runtime.paused,
        }
    }
    fn apply(self, s: &mut AppSettings) {
        s.active_account_id = self.active_account_id;
        s.chat = self.chat;
        s.connectors = self.connectors;
        s.keys = self.keys;
        s.sync = self.sync;
        s.backup = self.backup;
        s.runtime.sources = self.sources;
        s.runtime.paused = self.paused;
    }
}
fn profile_identity(data: &Path, s: &AppSettings) -> Result<(String, Option<i64>), String> {
    use sha2::Digest;
    let base = s.devify.resolved_base_url()?;
    let user = desktop_credentials::load(data, &base)
        .ok()
        .and_then(|c| crate::access_token_claim(&c.access, "user_id"))
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|u| *u > 0);
    let key = hex::encode(sha2::Sha256::digest(
        format!(
            "{base}\0{user:?}\0{}",
            s.devify.active_space_id.as_deref().unwrap_or("")
        )
        .as_bytes(),
    ));
    Ok((key, user))
}

/// Also used inside the settings transaction when switching server or space.
/// The outgoing profile is archived before any incoming source can be enqueued.
pub fn activate_settings_profile(data: &Path, s: &mut AppSettings) -> Result<(), String> {
    let (key, user) = profile_identity(data, s)?;
    if s.runtime.active_profile.as_ref() == Some(&key) {
        return Ok(());
    }
    crate::abort_in_flight_uploads();
    let snapshot = Profile::capture(s);
    if let Some(old) = s.runtime.active_profile.clone() {
        s.runtime.profiles.insert(old, snapshot);
    } else {
        let known = user.is_some_and(|user| {
            s.sync
                .owners
                .values()
                .chain(s.upload_queue.iter().filter_map(|j| j.owner.as_ref()))
                .any(|o| {
                    o.user_id == user
                        && Some(&o.space_id) == s.devify.active_space_id.as_ref()
                        && crate::settings::same_service_url(&o.base_url, &s.devify.base_url)
                })
        });
        s.runtime.profiles.insert(
            if known {
                key.clone()
            } else {
                "legacy-unclaimed".into()
            },
            snapshot,
        );
    }
    let selected = s
        .runtime
        .profiles
        .get(&key)
        .cloned()
        .unwrap_or_else(|| Profile::capture(&AppSettings::default()));
    selected.apply(s);
    s.runtime.active_profile = Some(key);
    s.runtime.last_error = None;
    Ok(())
}

pub fn activate_profile(data: &Path) -> Result<(), String> {
    let current = crate::settings::load(data)?;
    let (key, user) = profile_identity(data, &current)?;
    // Only a verified JWT identity can adopt a login profile. Explicit settings
    // context switches handle the signed-out profile separately.
    if user.is_none() || current.devify.active_space_id.is_none() {
        return Ok(());
    }
    if current.runtime.active_profile.as_ref() == Some(&key) {
        return Ok(());
    }
    crate::settings::update(data, |s| activate_settings_profile(data, s))?;
    Ok(())
}

/// Browsing keeps using the last completed configuration while a new draft or
/// an unacknowledged save is being edited. The scheduler remains stopped for it.
pub fn browse_names(value: &AppSettings, account: &str) -> Vec<String> {
    if let Some(source) = value.runtime.sources.get(account) {
        if let Some(active) = source.active.as_ref() {
            return active["usernames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
        }
        if source.draft {
            return Vec::new();
        }
    }
    value
        .chat
        .allowed_usernames
        .get(account)
        .cloned()
        .unwrap_or_default()
}
