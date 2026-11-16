//! Subscription entitlements: what the signed-in account may use, and the single guard the
//! Tauri commands call before doing paid work.
//!
//! The server is the authority. In its current state no entitlement endpoint exists, so an
//! unknown or unsupported response means "allow everything" — exactly the behaviour the app
//! had before this module. Once the server starts answering with `enforced: true`, the same
//! guard starts denying features a plan does not carry, without any further client change.
//!
//! Cache layout (`<app data>/devify-entitlements.json`):
//!
//! ```json
//! {
//!   "enforced": true,
//!   "features": ["ingest.upload", "cloud.events"],
//!   "limits": {"limits.ingest.conversations": 50},
//!   "quotas": {"quotas.cloud.ask_per_day": 100},
//!   "usage": {"ingest.messages_this_month": 1234},
//!   "plan": {"slug": "pro", "name": "Pro", "status": "active"},
//!   "base_url": "https://devify.example.com",
//!   "fetched_at": 1793932800,
//!   "source": "server"
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::capabilities;

const FILE: &str = "devify-entitlements.json";
/// How long a snapshot is trusted before the app tries to refresh it.
const REFRESH_SECONDS: i64 = 6 * 60 * 60;
/// How long a snapshot still grants its features while the server is unreachable.
const GRACE_SECONDS: i64 = 7 * 24 * 60 * 60;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Plan {
    pub slug: String,
    pub name: String,
    pub status: String,
    pub current_period_end: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Snapshot {
    pub plan: Option<Plan>,
    pub features: Vec<String>,
    /// Numeric ceilings keyed by `limits.*`.
    pub limits: BTreeMap<String, i64>,
    /// Periodically resetting counters keyed by `quotas.*`.
    pub quotas: BTreeMap<String, i64>,
    /// Current usage reported by the server, keyed like the quotas.
    pub usage: BTreeMap<String, i64>,
    /// `false` keeps the legacy "everything is allowed" behaviour. The server flips this on
    /// once a plan actually defines its features; until then the client must not guess.
    pub enforced: bool,
    /// Features granted after the snapshot goes stale beyond [`GRACE_SECONDS`].
    /// Empty means the local free baseline from [`capabilities::free_keys`].
    pub fallback_features: Vec<String>,
    /// The account holds an active subscription to the operator's internal plan.
    /// Gates features reserved for internal users, such as the agent access kit.
    pub internal: bool,
    /// The service address the snapshot belongs to. Credentials and plans are per server.
    pub base_url: String,
    /// Unix seconds when this snapshot was fetched.
    pub fetched_at: i64,
    /// `server`, `unsupported` (endpoint missing) or `unknown` (nothing cached).
    pub source: String,
    /// Server-side cache hint, kept for display only.
    pub expires_at: Option<String>,
}

impl Snapshot {
    fn fresh_age(&self, now: i64) -> i64 {
        now.saturating_sub(self.fetched_at)
    }

    pub fn stale(&self, now: i64) -> bool {
        self.fresh_age(now) > REFRESH_SECONDS
    }

    pub fn grace_expired(&self, now: i64) -> bool {
        self.fresh_age(now) > GRACE_SECONDS
    }
}

/// What the UI receives. Flattened so the frontend does not have to know about the cache.
#[derive(Serialize, Clone, Debug)]
pub struct View {
    pub plan: Option<Plan>,
    pub features: Vec<String>,
    pub limits: BTreeMap<String, i64>,
    pub quotas: BTreeMap<String, i64>,
    pub usage: BTreeMap<String, i64>,
    pub enforced: bool,
    pub source: String,
    pub base_url: String,
    pub fetched_at: Option<i64>,
    pub expires_at: Option<String>,
    pub stale: bool,
    pub grace_expired: bool,
    /// Mirrors the cache so the UI can explain what survives an expired snapshot.
    pub fallback_features: Vec<String>,
    /// Whether the account may use internal-only features.
    pub internal: bool,
    /// The local free baseline, sent so the frontend never hard-codes the same list twice.
    pub free_keys: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Remote {
    plan: Option<Plan>,
    features: Vec<String>,
    limits: BTreeMap<String, i64>,
    quotas: BTreeMap<String, i64>,
    usage: BTreeMap<String, i64>,
    enforced: bool,
    fallback_features: Vec<String>,
    internal: bool,
    expires_at: Option<String>,
}

pub fn load(app_data: &Path) -> Option<Snapshot> {
    let raw = fs::read_to_string(app_data.join(FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save(app_data: &Path, snapshot: &Snapshot) -> Result<(), String> {
    let path = app_data.join(FILE);
    let body = serde_json::to_string_pretty(snapshot)
        .map_err(|error| format!("序列化订阅权益失败：{error}"))?;
    let temp = app_data.join(format!("{FILE}.tmp"));
    fs::write(&temp, body).map_err(|error| format!("写入订阅权益失败：{error}"))?;
    fs::rename(&temp, &path).map_err(|error| format!("保存订阅权益失败：{error}"))?;
    Ok(())
}

/// The cached snapshot, but only when it belongs to the configured server.
pub fn effective(app_data: &Path, base_url: &str) -> Option<Snapshot> {
    let snapshot = load(app_data)?;
    if !crate::settings::same_service_url(&snapshot.base_url, base_url) {
        return None;
    }
    Some(snapshot)
}

fn now_seconds() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn view(app_data: &Path, base_url: &str) -> View {
    let now = now_seconds();
    match effective(app_data, base_url) {
        Some(snapshot) => View {
            plan: snapshot.plan.clone(),
            features: snapshot.features.clone(),
            limits: snapshot.limits.clone(),
            quotas: snapshot.quotas.clone(),
            usage: snapshot.usage.clone(),
            enforced: snapshot.enforced,
            source: snapshot.source.clone(),
            base_url: snapshot.base_url.clone(),
            fetched_at: Some(snapshot.fetched_at),
            expires_at: snapshot.expires_at.clone(),
            stale: snapshot.stale(now),
            grace_expired: snapshot.grace_expired(now),
            fallback_features: snapshot.fallback_features.clone(),
            internal: snapshot.internal,
            free_keys: capabilities::free_keys()
                .into_iter()
                .map(str::to_string)
                .collect(),
        },
        None => View {
            plan: None,
            features: Vec::new(),
            limits: BTreeMap::new(),
            quotas: BTreeMap::new(),
            usage: BTreeMap::new(),
            enforced: false,
            source: "unknown".into(),
            base_url: base_url.to_string(),
            fetched_at: None,
            expires_at: None,
            stale: false,
            grace_expired: false,
            fallback_features: Vec::new(),
            internal: false,
            free_keys: capabilities::free_keys()
                .into_iter()
                .map(str::to_string)
                .collect(),
        },
    }
}

fn wildcard_match(pattern: &str, key: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    match pattern.strip_suffix(".*") {
        Some(prefix) => key.starts_with(prefix) && key[prefix.len()..].starts_with('.'),
        None => pattern == key,
    }
}

fn granted(snapshot: &Snapshot, key: &str, now: i64) -> bool {
    if snapshot.grace_expired(now) {
        return if snapshot.fallback_features.is_empty() {
            capabilities::free_keys().contains(&key)
        } else {
            snapshot
                .fallback_features
                .iter()
                .any(|item| wildcard_match(item, key))
        };
    }
    snapshot.features.iter().any(|item| wildcard_match(item, key))
}

/// Guard for a paid capability. Returns `Ok(())` while the server has not turned enforcement
/// on, so adding the call never changes behaviour by itself.
pub fn require(app_data: &Path, base_url: &str, key: &str) -> Result<(), String> {
    let Some(snapshot) = effective(app_data, base_url) else {
        return Ok(());
    };
    if !snapshot.enforced {
        return Ok(());
    }
    if granted(&snapshot, key, now_seconds()) {
        return Ok(());
    }
    let label = capabilities::label(key);
    match snapshot.plan.as_ref().map(|plan| plan.name.trim()).filter(|name| !name.is_empty()) {
        Some(plan) => Err(format!(
            "「{label}」不在当前套餐（{plan}）内，请升级后重试。"
        )),
        None => Err(format!("「{label}」不在当前套餐内，请升级后重试。")),
    }
}

/// Whether the account may use internal-only features. A missing snapshot means "no":
/// the internal plan is the only way in, and it can only arrive from the server.
pub fn is_internal(app_data: &Path, base_url: &str) -> bool {
    effective(app_data, base_url).is_some_and(|snapshot| snapshot.internal)
}

/// Ask the server what this account may use. A server that has not shipped the endpoint yet
/// is a normal state: it is cached as `unsupported` and keeps the app wide open.
pub async fn refresh(app_data: &Path, base_url: &str, token: &str) -> Result<View, String> {
    use crate::devify_http::RequestTrace;
    let base_url = crate::settings::canonical_api_base(base_url)?;
    let response = crate::http_client::api()
        .get(format!("{base_url}/api/v1/me/entitlements"))
        .bearer_auth(token)
        .send_traced_retrying(
            app_data,
            crate::diagnostics::Module::App,
            crate::devify_http::RetryPolicy::READ,
        )
        .await
        .map_err(|error| format!("读取订阅权益失败：{error}"))?;
    let status = response.status();

    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::NOT_IMPLEMENTED {
        let snapshot = Snapshot {
            base_url: base_url.to_string(),
            fetched_at: now_seconds(),
            source: "unsupported".into(),
            ..Snapshot::default()
        };
        save(app_data, &snapshot)?;
        return Ok(view(app_data, &base_url));
    }
    if !status.is_success() {
        return Err(format!("读取订阅权益失败：服务端返回 {status}"));
    }

    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|error| format!("读取订阅权益失败：{error}"))?;
    let payload = payload.get("data").cloned().unwrap_or(payload);
    let remote: Remote = serde_json::from_value(payload)
        .map_err(|error| format!("解析订阅权益失败：{error}"))?;

    let snapshot = Snapshot {
        plan: remote.plan,
        features: remote.features,
        limits: remote.limits,
        quotas: remote.quotas,
        usage: remote.usage,
        enforced: remote.enforced,
        fallback_features: remote.fallback_features,
        internal: remote.internal,
        base_url: base_url.to_string(),
        fetched_at: now_seconds(),
        source: "server".into(),
        expires_at: remote.expires_at,
    };
    save(app_data, &snapshot)?;
    Ok(view(app_data, &base_url))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wcd-entitlements-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn snapshot(enforced: bool, features: &[&str]) -> Snapshot {
        Snapshot {
            enforced,
            features: features.iter().map(|item| item.to_string()).collect(),
            base_url: "https://devify.test".into(),
            fetched_at: now_seconds(),
            source: "server".into(),
            ..Snapshot::default()
        }
    }

    #[test]
    fn missing_snapshot_allows_everything() {
        let dir = temp_dir("missing");
        assert!(require(&dir, "https://devify.test", "cloud.ask").is_ok());
    }

    #[test]
    fn unenforced_snapshot_allows_everything() {
        let dir = temp_dir("unenforced");
        save(&dir, &snapshot(false, &[])).unwrap();
        assert!(require(&dir, "https://devify.test", "cloud.ask").is_ok());
    }

    #[test]
    fn enforced_snapshot_denies_missing_features() {
        let dir = temp_dir("enforced");
        save(&dir, &snapshot(true, &["cloud.events", "ingest.*"])).unwrap();
        assert!(require(&dir, "https://devify.test", "cloud.events").is_ok());
        assert!(require(&dir, "https://devify.test", "ingest.upload").is_ok());
        let error = require(&dir, "https://devify.test", "cloud.ask").unwrap_err();
        assert!(error.contains("问答"), "{error}");
    }

    #[test]
    fn a_different_server_does_not_reuse_the_cache() {
        let dir = temp_dir("other-server");
        save(&dir, &snapshot(true, &[])).unwrap();
        assert!(require(&dir, "https://other.test", "cloud.ask").is_ok());
        assert!(effective(&dir, "https://other.test").is_none());
    }

    #[test]
    fn expired_snapshot_falls_back_to_the_free_baseline() {
        let dir = temp_dir("expired");
        let mut value = snapshot(true, &["cloud.ask"]);
        value.fetched_at = now_seconds() - GRACE_SECONDS - 60;
        save(&dir, &value).unwrap();
        assert!(require(&dir, "https://devify.test", "cloud.ask").is_err());
        assert!(require(&dir, "https://devify.test", "local.wechat_read").is_ok());
        assert!(require(&dir, "https://devify.test", "ai.local_digest").is_ok());
    }

    #[test]
    fn expired_snapshot_honours_an_explicit_fallback_list() {
        let dir = temp_dir("expired-fallback");
        let mut value = snapshot(true, &[]);
        value.fetched_at = now_seconds() - GRACE_SECONDS - 60;
        value.fallback_features = vec!["cloud.records".into()];
        save(&dir, &value).unwrap();
        assert!(require(&dir, "https://devify.test", "cloud.records").is_ok());
        assert!(require(&dir, "https://devify.test", "cloud.ask").is_err());
        assert!(require(&dir, "https://devify.test", "local.wechat_read").is_err());
    }

    #[test]
    fn internal_only_comes_from_the_server_snapshot() {
        let dir = temp_dir("internal");
        assert!(!is_internal(&dir, "https://devify.test"));

        let mut value = snapshot(false, &[]);
        value.internal = true;
        save(&dir, &value).unwrap();

        assert!(is_internal(&dir, "https://devify.test"));
        assert!(!is_internal(&dir, "https://other.test"));
    }

    #[test]
    fn wildcards_only_match_a_whole_segment() {
        assert!(wildcard_match("ingest.*", "ingest.upload"));
        assert!(wildcard_match("cloud.*", "cloud.events.write"));
        assert!(!wildcard_match("cloud.*", "cloud"));
        assert!(!wildcard_match("ingest.*", "ingest"));
        assert!(!wildcard_match("cloud.*", "cloudy.events"));
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("cloud.ask", "cloud.ask"));
        assert!(!wildcard_match("cloud.ask", "cloud.asks"));
    }
}
