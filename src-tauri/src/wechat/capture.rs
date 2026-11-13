//! In-place WeChat passphrase capture on macOS.
//!
//! This is a Rust port of the capture flow used by the reference project
//! `~/Workspace/AI/WeChatDataAnalysis` (`wechat_decrypt_tool/macos_db_key_capture.py`,
//! `macos_inplace_capture.py`, `macos_clone_capture.py`). The stage order, guards and
//! error semantics are kept the same on purpose:
//!
//! 1. verify `/Applications/WeChat.app` is the genuine Tencent build (Team `5A4RE8SF68`,
//!    identifier `com.tencent.xinWeChat`, hardened runtime, not ad-hoc);
//! 2. require that the current user can write both the bundle and its parent directory, so a
//!    crashed capture can always be restored without another authorization prompt;
//! 3. keep a verified `ditto` ZIP of the original bundle, then clone it on the same volume
//!    (`clonefile`) as the restore source;
//! 4. probe first, then re-sign only the *clone* ad-hoc: Tencent's entitlements are kept when
//!    accepted; otherwise the clone and nested components are signed without entitlements. The
//!    latter runs without the original sandbox and may use a different data directory. The clone
//!    is atomically exchanged into `/Applications/WeChat.app`;
//! 5. the user opens WeChat themselves and logs in, so a macOS security prompt is never
//!    bypassed on their behalf;
//! 6. preflight the `CCKeyDerivationPBKDF` breakpoint, ask the user to log out, then attach
//!    LLDB as administrator. A candidate is only accepted when it unlocks **both** the message
//!    and the session database, matching the reference's two-role validation;
//! 7. quit the debug build and atomically swap the untouched official clone back, verifying
//!    Team identifier, version and CDHash before declaring success;
//! 8. save the verified passphrase with owner-only permissions so the existing import flow can
//!    store it per database.
//!
//! Nothing here touches WeChat unless every guard passes. A failure after the swap always
//! attempts a restore; when the restore cannot be proven safe the recovery assets are kept and
//! the error says so (`external_install_conflict`, `in_place_debug_identity_unknown`, ...).

use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::fs;
use std::io::{self, Write};
use std::os::raw::{c_char, c_int, c_uint};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const WECHAT_APP: &str = "/Applications/WeChat.app";
pub const TENANT_TEAM: &str = "5A4RE8SF68";
pub const TENANT_IDENTIFIER: &str = "com.tencent.xinWeChat";
pub const LOGIN_HELPER: &str = "Contents/MacOS/WeChatAppEx.app";
pub const LOGIN_HELPER_IDENTIFIER: &str = "com.tencent.flue.WeChatAppEx";
/// Fixed same-volume slot next to the app, exactly like the reference implementation.
pub const RESTORE_SLOT: &str = ".WeChat.wedata-official-restore.app";
pub const STATE_NAME: &str = "prepared-in-place-capture.json";
pub const READY_NAME: &str = "native-capture-ready.json";
pub const RESULT_NAME: &str = "capture-result.json";
pub const PREFLIGHT_NAME: &str = "breakpoint-preflight.json";
pub const LOCK_NAME: &str = "installation.lock";
const STATE_SCHEMA: u32 = 2;

#[derive(Debug, Clone, Serialize)]
pub struct CaptureError {
    pub code: String,
    pub message: String,
    /// True when `/Applications/WeChat.app` may no longer be the Tencent build.
    pub wechat_modified: bool,
}

impl CaptureError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            wechat_modified: false,
        }
    }

    fn modified(mut self) -> Self {
        self.wechat_modified = true;
        self
    }

    fn with_code(mut self, code: &str) -> Self {
        self.code = code.to_string();
        self
    }
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

pub type CaptureResult<T> = Result<T, CaptureError>;

/* ── signature identity ───────────────────────────────────────────────────── */

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Signature {
    pub valid: bool,
    pub ad_hoc: bool,
    pub hardened_runtime: bool,
    pub team_identifier: String,
    pub identifier: String,
    pub cdhash: String,
}

impl Signature {
    pub fn is_tencent_official(&self) -> bool {
        self.valid
            && !self.ad_hoc
            && self.team_identifier == TENANT_TEAM
            && self.identifier == TENANT_IDENTIFIER
    }

    /// LLDB cannot attach to a hardened binary, so the temporary build must be ad-hoc
    /// **without** the hardened runtime flag.
    pub fn is_debuggable_ad_hoc(&self) -> bool {
        self.valid && self.ad_hoc && !self.hardened_runtime
    }
}

/// The exact bundle this transaction created: identity plus the on-disk inode, exactly like
/// the reference, so a restore never treats an unrelated ad-hoc build as its own.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleIdentity {
    pub version: String,
    pub build: String,
    pub cdhash: String,
    pub device: u64,
    pub inode: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleVersion {
    pub short: String,
    pub build: String,
}

/// Recovery facts. Written before anything in the installed bundle changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureState {
    pub schema_version: u32,
    pub stage: String,
    pub created_at: u64,
    pub transaction_id: String,
    pub owner_pid: i64,
    pub wechat_app_path: String,
    pub backup_path: String,
    pub backup_root: String,
    pub backup_created: bool,
    pub version: String,
    pub build: String,
    pub official_cdhash: String,
    /// Older state files predate the macOS 15 fallback and must keep meaning "preserved".
    #[serde(default)]
    pub entitlement_mode: EntitlementMode,
    pub debug_identity: Option<BundleIdentity>,
    pub debug_pid: Option<i64>,
    pub capture_phase: Option<String>,
    pub capture_backend: Option<String>,
    pub preflight_pid: Option<i64>,
    pub recovery_error_code: Option<String>,
}

impl CaptureState {
    fn version(&self) -> BundleVersion {
        BundleVersion {
            short: self.version.clone(),
            build: self.build.clone(),
        }
    }
}

/* ── process helpers ──────────────────────────────────────────────────────── */

pub fn collapse(text: &str, limit: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = joined.chars().count();
    if count <= limit {
        return joined;
    }
    joined.chars().skip(count - limit).collect()
}

fn run(args: &[&str]) -> CaptureResult<(bool, String, String)> {
    let output = Command::new(args[0]).args(&args[1..]).output().map_err(|error| {
        CaptureError::new(
            "command_failed",
            format!("无法执行 {}：{error}", Path::new(args[0]).file_name().unwrap_or_default().to_string_lossy()),
        )
    })?;
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    ))
}

fn shell_quote_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'\''"#))
}

fn shell_quote(path: &Path) -> String {
    shell_quote_text(&path.to_string_lossy())
}

/// `do shell script … with administrator privileges` is the same mechanism the reference tool
/// uses: the password is typed into the macOS dialog and never reaches this process.
fn run_as_administrator(script: &str, timeout: Duration) -> CaptureResult<String> {
    let child = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg("on run argv\ndo shell script (item 1 of argv) with administrator privileges\nend run")
        .arg("--")
        .arg(script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| CaptureError::new("administrator_failed", format!("无法请求管理员授权：{error}")))?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let output = match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return Err(CaptureError::new(
                "administrator_failed",
                format!("管理员授权进程执行失败：{error}"),
            ))
        }
        Err(_) => {
            // Do not leave the authorization helper behind; the privileged shell has its own
            // watchdog (see `lldb_command`) so LLDB does not outlive this timeout either.
            let _ = Command::new("/bin/kill").arg("-TERM").arg(pid.to_string()).status();
            return Err(CaptureError::new(
                "administrator_timeout",
                "管理员授权或密钥捕获等待超时。",
            ));
        }
    };
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        if stderr.contains("-128") || stderr.to_lowercase().contains("user canceled") {
            return Err(CaptureError::new("administrator_cancelled", "已取消管理员授权。"));
        }
        return Err(CaptureError::new(
            "administrator_failed",
            format!("管理员操作失败：{}", collapse(&stderr, 400)),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn plist_string(app: &Path, key: &str) -> Option<String> {
    let plist = app.join("Contents").join("Info.plist");
    let (ok, stdout, _) = run(&[
        "/usr/libexec/PlistBuddy",
        "-c",
        &format!("Print :{key}"),
        &plist.to_string_lossy(),
    ])
    .ok()?;
    if !ok {
        return None;
    }
    let value = stdout.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/* ── identity ─────────────────────────────────────────────────────────────── */

pub fn normalize_app(path: Option<&Path>) -> CaptureResult<PathBuf> {
    let mut candidate = path.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(WECHAT_APP));
    if candidate.file_name().map(|name| name == "WeChat").unwrap_or(false)
        && candidate
            .parent()
            .and_then(|parent| parent.file_name())
            .map(|name| name == "MacOS")
            .unwrap_or(false)
    {
        if let Some(bundle) = candidate.ancestors().nth(3) {
            candidate = bundle.to_path_buf();
        }
    }
    if !candidate.is_dir() || candidate.extension().map(|ext| ext != "app").unwrap_or(true) {
        return Err(CaptureError::new(
            "invalid_wechat_app",
            format!("微信应用不存在或不是 .app：{}", candidate.display()),
        ));
    }
    let identifier = plist_string(&candidate, "CFBundleIdentifier").unwrap_or_default();
    if identifier != TENANT_IDENTIFIER || !candidate.join("Contents/MacOS/WeChat").is_file() {
        return Err(CaptureError::new(
            "invalid_wechat_app",
            format!("应用身份不是 {TENANT_IDENTIFIER}：{}", candidate.display()),
        ));
    }
    Ok(candidate)
}

/// Parsed `codesign -dvvv` output, kept separate so it can be unit-tested.
pub fn parse_codesign_output(text: &str) -> Signature {
    let value = |prefix: &str| -> String {
        text.lines()
            .find_map(|item| item.trim().strip_prefix(prefix).map(|value| value.trim().to_string()))
            .unwrap_or_default()
    };
    let team = value("TeamIdentifier=")
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    let flags_line = text
        .lines()
        .find(|item| item.trim_start().starts_with("flags="))
        .unwrap_or("")
        .to_string();
    Signature {
        // Set by the caller from `codesign --verify`.
        valid: true,
        ad_hoc: text.contains("Signature=adhoc") || flags_line.contains("adhoc"),
        hardened_runtime: flags_line.contains("runtime"),
        team_identifier: if team == "not" { String::new() } else { team },
        identifier: value("Identifier="),
        cdhash: value("CDHash=").to_lowercase(),
    }
}

/// `deep` runs the full `codesign --verify --deep --strict` check (slow but authoritative).
/// Status polling uses the fast path so the UI is not blocked by a deep verification.
pub fn inspect_signature(app: &Path, deep: bool) -> CaptureResult<Signature> {
    let (_, stdout, stderr) = run(&["/usr/bin/codesign", "-dvvv", &app.to_string_lossy()])?;
    let mut signature = parse_codesign_output(&format!("{stdout}\n{stderr}"));
    if deep {
        let (valid, ..) = run(&[
            "/usr/bin/codesign",
            "--verify",
            "--deep",
            "--strict",
            &app.to_string_lossy(),
        ])?;
        signature.valid = valid;
    }
    Ok(signature)
}

/// Gatekeeper only evaluates bundles that still carry `com.apple.quarantine`. A DMG-installed
/// WeChat keeps the attribute, and `clonefile` + `RENAME_SWAP` carry it into the resigned copy.
pub fn is_quarantined(app: &Path) -> bool {
    run(&[
        "/usr/bin/xattr",
        "-p",
        "com.apple.quarantine",
        &app.to_string_lossy(),
    ])
    .map(|(ok, stdout, _)| ok && !stdout.trim().is_empty())
    .unwrap_or(false)
}

/// Ask Gatekeeper for its own verdict. `None` means the tool could not answer, which must not be
/// reported as "accepted". `spctl --assess` still works on Sequoia; only its policy mutation flags
/// were removed.
pub fn gatekeeper_assessment(app: &Path) -> (Option<bool>, String) {
    match run(&[
        "/usr/bin/spctl",
        "--assess",
        "--type",
        "execute",
        "--verbose=4",
        &app.to_string_lossy(),
    ]) {
        Ok((ok, stdout, stderr)) => (
            Some(ok),
            collapse(&format!("{} {}", stdout.trim(), stderr.trim()), 200),
        ),
        Err(_) => (None, String::new()),
    }
}

pub fn bundle_version(app: &Path) -> CaptureResult<BundleVersion> {
    let short = plist_string(app, "CFBundleShortVersionString")
        .ok_or_else(|| CaptureError::new("invalid_wechat_app", "读不到微信版本号"))?;
    let build = plist_string(app, "CFBundleVersion").unwrap_or_else(|| "unknown".to_string());
    Ok(BundleVersion {
        short: sanitize_version(&short),
        build: sanitize_version(&build),
    })
}

fn sanitize_version(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

fn bundle_identity(app: &Path, signature: &Signature) -> CaptureResult<BundleIdentity> {
    let version = bundle_version(app)?;
    let meta = fs::metadata(app)
        .map_err(|error| CaptureError::new("invalid_wechat_app", format!("无法读取微信属性：{error}")))?;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(BundleIdentity {
        version: version.short,
        build: version.build,
        cdhash: signature.cdhash.to_lowercase(),
        device: meta.dev(),
        inode: meta.ino(),
    })
}

fn debug_identity_matches(app: &Path, expected: Option<&BundleIdentity>) -> bool {
    let Some(expected) = expected else { return false };
    if expected.cdhash.is_empty() {
        return false;
    }
    if app.symlink_metadata().map(|meta| meta.file_type().is_symlink()).unwrap_or(true)
        || !app.is_dir()
    {
        return false;
    }
    match inspect_signature(app, false) {
        Ok(signature) => {
            signature.is_debuggable_ad_hoc()
                && bundle_identity(app, &signature).map(|value| &value == expected).unwrap_or(false)
        }
        Err(_) => false,
    }
}

fn official_identity_matches(
    app: &Path,
    signature: &Signature,
    expected_version: Option<&BundleVersion>,
    expected_cdhash: Option<&str>,
) -> bool {
    if !signature.is_tencent_official() {
        return false;
    }
    if let Some(version) = expected_version {
        if bundle_version(app).map(|value| &value != version).unwrap_or(true) {
            return false;
        }
    }
    match expected_cdhash {
        Some(cdhash) if !cdhash.is_empty() => signature.cdhash.eq_ignore_ascii_case(cdhash),
        _ => true,
    }
}

/* ── entitlements ─────────────────────────────────────────────────────────── */

/// Which entitlement set the temporary signature was built with.
///
/// The reference flow carries Tencent's entitlements over verbatim, which is what macOS 14 and
/// earlier need. macOS 15 validates more strictly: an ad-hoc signature that still claims the
/// team-bound entitlements is rejected before the process reaches `main`
/// (`Launchd job spawn failed`, launchd errno 153), so those copies have to be signed *without*
/// the team-bound claims instead of being handed over as a WeChat that cannot start.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntitlementMode {
    #[default]
    Preserved,
    Sanitized,
    Deep,
}

impl EntitlementMode {
    pub fn as_str(self) -> &'static str {
        match self {
            EntitlementMode::Preserved => "preserved",
            EntitlementMode::Sanitized => "sanitized",
            EntitlementMode::Deep => "deep",
        }
    }
}

/// Claims that only a Developer ID signature naming the right team can satisfy. An ad-hoc
/// signature has no team, so these can never be honoured.
pub const RESTRICTED_ENTITLEMENTS: &[&str] = &[
    "com.apple.application-identifier",
    "com.apple.developer.team-identifier",
    "com.apple.security.application-groups",
];

/// `temporary-exception.*` claims are granted by a provisioning profile, so an ad-hoc signature
/// cannot carry them either.
const RESTRICTED_ENTITLEMENT_PREFIX: &str = "com.apple.security.temporary-exception.";

pub fn is_restricted_entitlement(key: &str) -> bool {
    RESTRICTED_ENTITLEMENTS.contains(&key) || key.starts_with(RESTRICTED_ENTITLEMENT_PREFIX)
}

/// Drop every claim that names the signing team while keeping the sandbox and the data-access
/// claims WeChat needs. Returns the filtered dictionary plus the removed keys (sorted, so the
/// persisted report is stable).
pub fn sanitize_entitlements(entitlements: &serde_json::Value) -> (serde_json::Value, Vec<String>) {
    let Some(dictionary) = entitlements.as_object() else {
        return (entitlements.clone(), Vec::new());
    };
    let mut kept = serde_json::Map::new();
    let mut dropped = Vec::new();
    for (key, value) in dictionary {
        if is_restricted_entitlement(key) {
            dropped.push(key.clone());
        } else {
            kept.insert(key.clone(), value.clone());
        }
    }
    dropped.sort();
    (serde_json::Value::Object(kept), dropped)
}

/// Throwaway directory outside the app bundle, removed even when the flow fails.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> CaptureResult<Self> {
        // A per-call counter keeps two commands in the same process from sharing (and deleting)
        // each other's directory.
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "wechatify-{tag}-{}-{serial}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).map_err(|error| {
            CaptureError::new(
                "in_place_probe_unavailable",
                format!("无法创建临时目录（权限探针需要它）：{error}"),
            )
        })?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn plutil_convert(input: &Path, output: &Path, format: &str) -> CaptureResult<()> {
    let (ok, _, stderr) = run(&[
        "/usr/bin/plutil",
        "-convert",
        format,
        "-o",
        &output.to_string_lossy(),
        &input.to_string_lossy(),
    ])?;
    if ok {
        return Ok(());
    }
    Err(CaptureError::new(
        "in_place_entitlements_unreadable",
        format!("无法转换微信的权限列表：{}", collapse(&stderr, 200)),
    ))
}

/// Read the current signature's entitlements as JSON. `plutil` parses the XML so this never
/// depends on a hand-rolled plist reader.
pub fn read_entitlements(app: &Path) -> CaptureResult<serde_json::Value> {
    let (_, stdout, stderr) = run(&[
        "/usr/bin/codesign",
        "-d",
        "--entitlements",
        ":-",
        "--xml",
        &app.to_string_lossy(),
    ])?;
    let text = if stdout.contains("<plist") { stdout } else { stderr };
    let start = text
        .find("<plist")
        .or_else(|| text.find("<?xml"))
        .ok_or_else(|| {
            CaptureError::new(
                "in_place_entitlements_unreadable",
                "读不到微信的权限列表，已停止临时重签。",
            )
        })?;
    let scratch = Scratch::new("entitlements")?;
    let source = scratch.path().join("entitlements.plist");
    fs::write(&source, &text[start..]).map_err(|error| {
        CaptureError::new(
            "in_place_entitlements_unreadable",
            format!("无法写入临时权限文件：{error}"),
        )
    })?;
    let json = scratch.path().join("entitlements.json");
    plutil_convert(&source, &json, "json")?;
    let body = fs::read(&json).map_err(|error| {
        CaptureError::new(
            "in_place_entitlements_unreadable",
            format!("无法读取转换后的权限列表：{error}"),
        )
    })?;
    serde_json::from_slice(&body).map_err(|error| {
        CaptureError::new(
            "in_place_entitlements_unreadable",
            format!("无法解析微信的权限列表：{error}"),
        )
    })
}

fn plist_from_json(
    value: &serde_json::Value,
    scratch: &Scratch,
    name: &str,
) -> CaptureResult<String> {
    let json_path = scratch.path().join(format!("{name}.json"));
    let plist_path = scratch.path().join(format!("{name}.plist"));
    let body = serde_json::to_vec(value).map_err(|error| {
        CaptureError::new(
            "in_place_entitlements_unreadable",
            format!("无法序列化权限列表：{error}"),
        )
    })?;
    fs::write(&json_path, body).map_err(|error| {
        CaptureError::new(
            "in_place_entitlements_unreadable",
            format!("无法写入临时权限文件：{error}"),
        )
    })?;
    plutil_convert(&json_path, &plist_path, "xml1")?;
    fs::read_to_string(&plist_path).map_err(|error| {
        CaptureError::new(
            "in_place_entitlements_unreadable",
            format!("无法读取临时权限文件：{error}"),
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AmfiVerdict {
    Accepted,
    Rejected,
    /// The probe itself could not be built or started; never treated as a verdict.
    Unknown,
}

/// Bundle identifier of the throwaway probe. Fixed so its sandbox container is predictable and
/// can be cleaned up instead of accumulating one per run.
const PROBE_BUNDLE_ID: &str = "com.wechatify.amfi-probe";

const PROBE_INFO_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key>
	<string>com.wechatify.amfi-probe</string>
	<key>CFBundleExecutable</key>
	<string>amfi-probe</string>
	<key>CFBundleName</key>
	<string>WeChatify Probe</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>1.0</string>
	<key>CFBundleVersion</key>
	<string>1</string>
</dict>
</plist>
"#;

/// Start a throwaway sandboxed bundle signed ad-hoc with exactly these entitlements.
///
/// This checks a minimal non-Apple executable, not whether the complete WeChat bundle will launch.
fn amfi_probe(entitlements_plist: &str, scratch: &Scratch, name: &str) -> AmfiVerdict {
    let bundle = scratch.path().join(format!("{name}.app"));
    let macos = bundle.join("Contents").join("MacOS");
    let executable = macos.join("amfi-probe");
    let plist_path = scratch.path().join(format!("{name}.plist"));

    let prepared = fs::create_dir_all(&macos).is_ok()
        && fs::write(&plist_path, entitlements_plist).is_ok()
        && fs::write(bundle.join("Contents").join("Info.plist"), PROBE_INFO_PLIST).is_ok()
        && write_probe_executable(&executable).is_ok();
    if !prepared {
        return AmfiVerdict::Unknown;
    }
    let signed = run(&[
        "/usr/bin/codesign",
        "--force",
        "--sign",
        "-",
        "--entitlements",
        &plist_path.to_string_lossy(),
        &bundle.to_string_lossy(),
    ])
    .map(|(ok, ..)| ok)
    .unwrap_or(false);
    if !signed {
        return AmfiVerdict::Unknown;
    }

    let outcome = Command::new(&executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = fs::remove_dir_all(&bundle);
    // The sandbox creates a container for the probe; leave nothing behind.
    if let Some(home) = std::env::var_os("HOME") {
        let container = Path::new(&home)
            .join("Library/Containers")
            .join(PROBE_BUNDLE_ID);
        let _ = fs::remove_dir_all(container);
    }
    match outcome {
        // SIGKILL suggests rejection; other failures do not establish the cause.
        Ok(status) if status.success() => AmfiVerdict::Accepted,
        #[cfg(target_os = "macos")]
        Ok(status) if std::os::unix::process::ExitStatusExt::signal(&status) == Some(9) => AmfiVerdict::Rejected,
        Ok(_) | Err(_) => AmfiVerdict::Unknown,
    }
}

#[cfg(target_os = "macos")]
fn write_probe_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, include_bytes!(concat!(env!("OUT_DIR"), "/amfi-probe")))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(target_os = "macos"))]
fn write_probe_executable(_: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "macOS only"))
}

/// `Some(true)`/`Some(false)` only when the probe actually ran; `None` means "unknown".
fn amfi_accepts_entitlements(entitlements: &serde_json::Value) -> Option<bool> {
    let scratch = Scratch::new("amfi-check").ok()?;
    let plist = plist_from_json(entitlements, &scratch, "check").ok()?;
    match amfi_probe(&plist, &scratch, "check") {
        AmfiVerdict::Accepted => Some(true),
        AmfiVerdict::Rejected => Some(false),
        AmfiVerdict::Unknown => None,
    }
}

fn amfi_rejected_error() -> CaptureError {
    CaptureError::new(
        "in_place_amfi_rejected",
        "签名权限探针在测试时被系统终止，无法确认临时微信是否能启动。\
         为避免留下无法启动的微信，本次没有重签，/Applications/WeChat.app 保持原样。请改用「导入密钥」。",
    )
}

fn amfi_probe_failed_error() -> CaptureError {
    CaptureError::new(
        "in_place_amfi_probe_failed",
        "无法验证 ad-hoc 签名能否被这台 Mac 接受（权限探针没有跑起来）。\
         为避免把微信重签成无法启动的状态，本次没有重签，/Applications/WeChat.app 保持原样。",
    )
}

/// What the temporary signature should be built from, decided by the probe before the installed
/// bundle is touched.
#[derive(Debug, Clone)]
struct EntitlementPlan {
    mode: EntitlementMode,
    /// XML plist handed to `codesign --entitlements`; only set for `Sanitized`.
    plist: Option<String>,
    dropped: Vec<String>,
}

fn plan_entitlements(app: &Path) -> CaptureResult<EntitlementPlan> {
    let entitlements = read_entitlements(app)?;
    let scratch = Scratch::new("entitlements-plan")?;

    let preserved = plist_from_json(&entitlements, &scratch, "preserved")?;
    match amfi_probe(&preserved, &scratch, "preserved") {
        AmfiVerdict::Accepted => {
            return Ok(EntitlementPlan {
                mode: EntitlementMode::Preserved,
                plist: None,
                dropped: Vec::new(),
            })
        }
        AmfiVerdict::Unknown => return Err(amfi_probe_failed_error()),
        AmfiVerdict::Rejected => {}
    }

    let dropped = entitlements.as_object().map(|v| v.keys().cloned().collect()).unwrap_or_default();
    let plist = plist_from_json(&serde_json::json!({}), &scratch, "deep")?;
    match amfi_probe(&plist, &scratch, "sanitized") {
        AmfiVerdict::Accepted => Ok(EntitlementPlan {
            mode: EntitlementMode::Deep,
            plist: None,
            dropped,
        }),
        AmfiVerdict::Rejected => Err(amfi_rejected_error()),
        AmfiVerdict::Unknown => Err(amfi_probe_failed_error()),
    }
}

/// The entitlements must still express the sandbox WeChat expects, and the login helper must
/// still carry Tencent's signature; otherwise the QR/login controls silently stop working.
fn entitlements_text_is_compatible(text: &str, mode: EntitlementMode) -> bool {
    let has = |key: &str| text.contains(&format!("<key>{key}</key>"));
    if !has("com.apple.security.app-sandbox") || !has("com.apple.security.network.client") {
        return false;
    }
    match mode {
        EntitlementMode::Preserved => {
            has("com.apple.application-identifier")
                && has("com.apple.security.application-groups")
                && text.contains(&format!("<string>{TENANT_TEAM}.{TENANT_IDENTIFIER}</string>"))
        }
        // A sanitized signature must not keep anything macOS 15 refuses to honour.
        EntitlementMode::Sanitized => {
            !RESTRICTED_ENTITLEMENTS.iter().any(|key| has(key))
                && !text.contains(&format!("<key>{RESTRICTED_ENTITLEMENT_PREFIX}"))
        }
        EntitlementMode::Deep => false,
    }
}

pub fn in_place_signature_is_compatible(app: &Path, mode: EntitlementMode) -> bool {
    if mode == EntitlementMode::Deep {
        let helper = app.join(LOGIN_HELPER);
        let no_entitlements = run(&["/usr/bin/codesign", "-d", "--entitlements", ":-", "--xml", &app.to_string_lossy()])
            .map(|(ok, stdout, stderr)| ok && !stdout.contains("<plist") && !stderr.contains("<plist"))
            .unwrap_or(false);
        return inspect_signature(app, true).map(|s| s.is_debuggable_ad_hoc()).unwrap_or(false)
            && no_entitlements
            && inspect_signature(&helper, false).map(|s| s.is_debuggable_ad_hoc() && s.identifier == LOGIN_HELPER_IDENTIFIER).unwrap_or(false);
    }
    let (_, stdout, stderr) = run(&[
        "/usr/bin/codesign",
        "-d",
        "--entitlements",
        ":-",
        "--xml",
        &app.to_string_lossy(),
    ])
    .unwrap_or((false, String::new(), String::new()));
    let text = if stdout.contains("<plist") { stdout } else { stderr };
    if !entitlements_text_is_compatible(&text, mode) {
        return false;
    }
    let helper = app.join(LOGIN_HELPER);
    if !helper.is_dir() {
        return false;
    }
    inspect_signature(&helper, false)
        .map(|signature| {
            signature.valid
                && !signature.ad_hoc
                && signature.team_identifier == TENANT_TEAM
                && signature.identifier == LOGIN_HELPER_IDENTIFIER
        })
        .unwrap_or(false)
}

/* ── backup / clone / swap ────────────────────────────────────────────────── */

pub fn backup_path(backup_root: &Path, version: &BundleVersion) -> PathBuf {
    backup_root.join(format!("WeChat-{}-{}-original.zip", version.short, version.build))
}

/// `ditto -c -k --sequesterRsrc` keeps resource forks and signature metadata, which a plain
/// copy of an app bundle does not. An existing archive is reused only after a self-test.
pub fn backup_original(app: &Path, backup_root: &Path) -> CaptureResult<(PathBuf, bool)> {
    let version = bundle_version(app)?;
    fs::create_dir_all(backup_root)
        .map_err(|error| CaptureError::new("backup_failed", format!("无法创建备份目录：{error}")))?;
    let final_path = backup_path(backup_root, &version);
    if final_path.is_file() {
        let (ok, ..) = run(&["/usr/bin/unzip", "-tqq", &final_path.to_string_lossy()])?;
        if ok && fs::metadata(&final_path).map(|meta| meta.len() > 0).unwrap_or(false) {
            return Ok((final_path, false));
        }
        return Err(CaptureError::new(
            "backup_invalid",
            format!("已有的微信原版备份不完整，请人工检查：{}", final_path.display()),
        ));
    }
    let name = final_path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let staging = backup_root.join(format!(".{name}.copying"));
    let _ = fs::remove_file(&staging);
    let (ok, _, stderr) = run(&[
        "/usr/bin/ditto",
        "-c",
        "-k",
        "--sequesterRsrc",
        "--keepParent",
        &app.to_string_lossy(),
        &staging.to_string_lossy(),
    ])?;
    if !ok {
        let _ = fs::remove_file(&staging);
        return Err(CaptureError::new(
            "backup_failed",
            format!("无法备份微信原版：{}", collapse(&stderr, 400)),
        ));
    }
    let (verified, ..) = run(&["/usr/bin/unzip", "-tqq", &staging.to_string_lossy()])?;
    if !verified {
        let _ = fs::remove_file(&staging);
        return Err(CaptureError::new("backup_invalid", "备份压缩包自检失败，已停止。"));
    }
    fs::rename(&staging, &final_path)
        .map_err(|error| CaptureError::new("backup_failed", format!("无法保存备份：{error}")))?;
    Ok((final_path, true))
}

/// Extract the archive and prove it still carries the original Tencent identity and version.
pub fn verify_backup(
    archive: &Path,
    expected: Option<&BundleVersion>,
    work_root: &Path,
) -> CaptureResult<(Signature, BundleVersion)> {
    if !archive.is_file() || fs::metadata(archive).map(|meta| meta.len() == 0).unwrap_or(true) {
        return Err(CaptureError::new(
            "backup_missing",
            format!("找不到微信原版备份：{}", archive.display()),
        ));
    }
    let (ok, ..) = run(&["/usr/bin/unzip", "-tqq", &archive.to_string_lossy()])?;
    if !ok {
        return Err(CaptureError::new("backup_invalid", "原版备份压缩包自检失败。"));
    }
    fs::create_dir_all(work_root)
        .map_err(|error| CaptureError::new("backup_failed", format!("无法创建校验目录：{error}")))?;
    let extract_root = unique_dir(work_root, "verify")?;
    let (ok, _, stderr) = run(&[
        "/usr/bin/ditto",
        "-x",
        "-k",
        &archive.to_string_lossy(),
        &extract_root.to_string_lossy(),
    ])?;
    let extracted = extract_root.join("WeChat.app");
    let result = (|| {
        if !ok {
            return Err(CaptureError::new(
                "official_backup_untrusted",
                format!("无法校验原版备份：{}", collapse(&stderr, 400)),
            ));
        }
        let signature = inspect_signature(&extracted, true)?;
        if !signature.is_tencent_official() {
            return Err(CaptureError::new(
                "official_backup_untrusted",
                "原版备份不是有效的腾讯签名版本，已停止临时重签。",
            ));
        }
        let version = bundle_version(&extracted)?;
        if let Some(expected) = expected {
            if &version != expected {
                return Err(CaptureError::new(
                    "official_backup_version_mismatch",
                    "原版备份版本与当前安装不一致，已停止临时重签。",
                ));
            }
        }
        Ok((signature, version))
    })();
    let _ = fs::remove_dir_all(&extract_root);
    result
}

fn unique_dir(root: &Path, prefix: &str) -> CaptureResult<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let path = root.join(format!("{prefix}-{stamp}-{}", std::process::id()));
    fs::create_dir_all(&path)
        .map_err(|error| CaptureError::new("work_dir_failed", format!("无法创建临时目录：{error}")))?;
    Ok(path)
}

fn stamp_dir(root: &Path, prefix: &str) -> CaptureResult<PathBuf> {
    fs::create_dir_all(root)
        .map_err(|error| CaptureError::new("work_dir_failed", format!("无法创建目录：{error}")))?;
    let path = root.join(format!(
        "{prefix}-{}-{}",
        now_secs(),
        std::process::id()
    ));
    fs::create_dir_all(&path)
        .map_err(|error| CaptureError::new("work_dir_failed", format!("无法创建目录：{error}")))?;
    Ok(path)
}

extern "C" {
    fn clonefile(src: *const c_char, dst: *const c_char, flags: c_int) -> c_int;
    fn renameatx_np(fromfd: c_int, from: *const c_char, tofd: c_int, to: *const c_char, flags: c_uint) -> c_int;
    fn access(path: *const c_char, mode: c_int) -> c_int;
}

const AT_FDCWD: c_int = -2;
const RENAME_SWAP: c_uint = 0x0000_0002;

fn cstring(path: &Path) -> CaptureResult<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| CaptureError::new("invalid_path", format!("路径包含非法字符：{}", path.display())))
}

pub fn can_write(path: &Path) -> bool {
    match cstring(path) {
        Ok(value) => unsafe { access(value.as_ptr(), 2) == 0 },
        Err(_) => false,
    }
}

pub fn restore_slot(app: &Path) -> PathBuf {
    app.with_file_name(RESTORE_SLOT)
}

/// Same-volume APFS clone: the first recovery source, and the bundle that gets re-signed so
/// the installed path is only ever touched by an atomic exchange.
pub fn clone_staging(app: &Path, expected: &BundleVersion, expected_cdhash: &str) -> CaptureResult<PathBuf> {
    let staged = restore_slot(app);
    if staged.symlink_metadata().is_ok() {
        return Err(CaptureError::new(
            "official_restore_staging_conflict",
            "原版微信保护位置已有恢复资产，请先恢复一次微信。",
        )
        .modified());
    }
    let source = cstring(app)?;
    let target = cstring(&staged)?;
    if unsafe { clonefile(source.as_ptr(), target.as_ptr(), 0) } != 0 {
        let error = io::Error::last_os_error();
        let _ = fs::remove_dir_all(&staged);
        return Err(CaptureError::new(
            "local_restore_clone_failed",
            format!("无法创建原版微信写时复制保护：{error}"),
        ));
    }
    let signature = inspect_signature(&staged, true)?;
    if !signature.is_tencent_official()
        || bundle_version(&staged)? != *expected
        || signature.cdhash.to_lowercase() != expected_cdhash.to_lowercase()
    {
        let _ = fs::remove_dir_all(&staged);
        return Err(CaptureError::new(
            "local_restore_clone_invalid",
            "原版微信写时复制保护校验失败，已停止临时重签。",
        ));
    }
    Ok(staged)
}

pub fn atomic_swap(first: &Path, second: &Path) -> CaptureResult<()> {
    let a = cstring(first)?;
    let b = cstring(second)?;
    if unsafe { renameatx_np(AT_FDCWD, a.as_ptr(), AT_FDCWD, b.as_ptr(), RENAME_SWAP) } != 0 {
        let error = io::Error::last_os_error();
        return Err(CaptureError::new(
            "atomic_restore_failed",
            format!("无法原子交换微信原版与临时版本：{error}"),
        )
        .modified());
    }
    Ok(())
}

/// Move unexpected leftovers aside instead of deleting them: an unknown bundle in the recovery
/// slot may be the only copy of something the user cares about.
fn preserve_assets(app: &Path, state_root: &Path, reason: &str) -> CaptureResult<Option<PathBuf>> {
    let mut kept = None;
    let staged = restore_slot(app);
    if staged.symlink_metadata().is_ok() {
        let target = stamp_dir(&state_root.join("preserved"), reason)?.join("restore-slot.app");
        fs::rename(&staged, &target).map_err(|error| {
            CaptureError::new("preserve_failed", format!("无法保留原版保护位置的内容：{error}")).modified()
        })?;
        kept = Some(target);
    }
    Ok(kept)
}

/* ── processes ────────────────────────────────────────────────────────────── */

pub fn main_pid(app: &Path) -> Option<i64> {
    let executable = app.join("Contents/MacOS/WeChat");
    let (ok, stdout, _) = run(&[
        "/usr/bin/pgrep",
        "-f",
        &format!("^{}$", regex_escape(&executable.to_string_lossy())),
    ])
    .ok()?;
    if !ok {
        return None;
    }
    stdout.split_whitespace().next()?.parse::<i64>().ok()
}

fn regex_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if "\\^$.|?*+()[]{}".contains(ch) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn bundle_pids(app: &Path) -> Vec<i64> {
    let pattern = format!("^{}/", regex_escape(&app.join("Contents").to_string_lossy()));
    match run(&["/usr/bin/pgrep", "-f", &pattern]) {
        Ok((true, stdout, _)) => stdout
            .split_whitespace()
            .filter_map(|value| value.parse::<i64>().ok())
            .collect(),
        _ => Vec::new(),
    }
}

fn wait_until<F: Fn() -> bool>(limit: Duration, check: F) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        thread::sleep(Duration::from_millis(250));
    }
    check()
}

fn finalize_helpers(app: &Path) -> CaptureResult<()> {
    if wait_until(Duration::from_secs(10), || bundle_pids(app).is_empty()) {
        return Ok(());
    }
    for pid in bundle_pids(app) {
        let _ = run(&["/bin/kill", "-TERM", &pid.to_string()]);
    }
    if wait_until(Duration::from_secs(5), || bundle_pids(app).is_empty()) {
        return Ok(());
    }
    for pid in bundle_pids(app) {
        let _ = run(&["/bin/kill", "-KILL", &pid.to_string()]);
    }
    Ok(())
}

/// Quit WeChat the reference way: AppleEvent first, bounded SIGTERM after. `force` is only used
/// while restoring a verified official bundle.
pub fn quit_wechat(app: &Path, force: bool) -> CaptureResult<()> {
    if main_pid(app).is_none() {
        let remaining = bundle_pids(app);
        if remaining.is_empty() {
            return Ok(());
        }
        for pid in remaining {
            let _ = run(&["/bin/kill", "-TERM", &pid.to_string()]);
        }
        if wait_until(Duration::from_secs(10), || bundle_pids(app).is_empty()) {
            return Ok(());
        }
        for pid in bundle_pids(app) {
            let _ = run(&["/bin/kill", "-KILL", &pid.to_string()]);
        }
        return Ok(());
    }

    let _ = Command::new("/usr/bin/osascript")
        .args(["-e", "tell application id \"com.tencent.xinWeChat\" to quit"])
        .output();
    if wait_until(Duration::from_secs(15), || main_pid(app).is_none()) {
        return finalize_helpers(app);
    }
    if let Some(pid) = main_pid(app) {
        let _ = run(&["/bin/kill", "-TERM", &pid.to_string()]);
    }
    if wait_until(Duration::from_secs(15), || main_pid(app).is_none()) {
        return finalize_helpers(app);
    }
    if !force {
        return Err(CaptureError::new(
            "wechat_still_running",
            "微信没有正常退出，请手动退出微信后重试。",
        ));
    }
    if let Some(pid) = main_pid(app) {
        let _ = run(&["/bin/kill", "-CONT", &pid.to_string()]);
        let _ = run(&["/bin/kill", "-KILL", &pid.to_string()]);
    }
    if !wait_until(Duration::from_secs(6), || main_pid(app).is_none()) {
        return Err(CaptureError::new(
            "wechat_force_stop_failed",
            "无法结束临时调试微信，已停止替换应用。",
        ));
    }
    finalize_helpers(app)
}

/// The user opens WeChat themselves (the reference asks the same), so a system security prompt
/// can never be bypassed on their behalf. Only a *stable* process counts as launched.
/// Why a temporarily resigned copy may refuse to start. Gatekeeper evaluates anything that still
/// carries `com.apple.quarantine`, and replacing Tencent's Developer ID signature with an ad-hoc
/// one invalidates the notarisation ticket. macOS 15 removed the Control-click override and 15.1
/// can reject a Finder launch without showing any dialog, which looks like "double-click does
/// nothing".
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct LaunchDiagnosis {
    pub quarantined: bool,
    /// True when the installed bundle still looks like this transaction's debug copy.
    pub resigned: bool,
    pub gatekeeper_accepted: Option<bool>,
    pub gatekeeper_reason: String,
    pub open_error: String,
    /// The system refused to create the process at all, so no Gatekeeper override can help.
    /// Either the launch error says so or the installed signature provably cannot start here.
    pub amfi_rejected: bool,
    /// WeChat processes referencing this bundle; 0 means nothing was started at all.
    pub processes: usize,
    /// Recommended next action, ready for the UI to render verbatim.
    pub hint: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LaunchAttempt {
    pub launched: bool,
    pub error: String,
    pub diagnosis: LaunchDiagnosis,
}

/// `RBSRequestErrorDomain Code=5` plus `Launchd job spawn failed` (errno 153) is what macOS 15
/// prints when the signature is refused while the process is being created — long before
/// Gatekeeper's "unidentified developer" prompt could appear.
pub fn open_error_is_amfi_rejection(text: &str) -> bool {
    let text = text.to_lowercase();
    text.contains("launchd job spawn failed")
        || text.contains("code=153")
        || text.contains("unknown error: 153")
        || text.contains("rbsrequesterrordomain")
}

fn launch_hint(diagnosis: &LaunchDiagnosis) -> String {
    if diagnosis.processes > 0 {
        return "已经有微信进程在运行，但不是本次临时副本或还没有稳定。请先在微信菜单里完全退出，\
                再用「打开微信」重新启动。"
            .to_string();
    }
    if diagnosis.amfi_rejected {
        return "这不是 Gatekeeper：这台 Mac 在创建微信进程时就拒绝了本次临时签名（launchd 153），\
                点「仍要打开」或改权限都不会生效。请点「取消并恢复微信」换回腾讯原版；\
                之后改用「导入密钥」，或换一台 macOS 14 及更早的 Mac 获取密钥。"
            .to_string();
    }
    let blocked = diagnosis.gatekeeper_accepted == Some(false);
    if blocked || (diagnosis.quarantined && diagnosis.gatekeeper_accepted.is_none()) {
        return "这是 Gatekeeper 拦截，不是签名损坏：先在 Finder 里双击 /Applications/WeChat.app，\
                出现「无法打开」时点「完成」；然后在 1 小时内打开 系统设置 → 隐私与安全性 → 安全性，\
                点「仍要打开」→ 输入管理员密码 → 再点「打开」。macOS 15 起没有右键「打开」入口，\
                15.1 也可能完全不弹窗。"
            .to_string();
    }
    "系统没有拦下这次启动，但微信进程没有出现。请手动在 Finder 里双击 /Applications/WeChat.app；\
     仍然没有反应时，到 ~/Library/Logs/DiagnosticReports 查看微信崩溃报告，\
     或点「取消并恢复微信」结束本次捕获。"
        .to_string()
}

pub fn diagnose_launch(app: &Path, open_error: &str) -> LaunchDiagnosis {
    let (gatekeeper_accepted, gatekeeper_reason) = gatekeeper_assessment(app);
    let resigned = inspect_signature(app, false)
        .map(|signature| signature.is_debuggable_ad_hoc())
        .unwrap_or(false);
    // Ask the same probe the prepare step uses, so a Finder launch (which gives us no error
    // text at all) is still explained correctly.
    let probe_rejected = resigned
        && (read_entitlements(app)
            .ok()
            .and_then(|value| amfi_accepts_entitlements(&value))
            == Some(false));
    let mut diagnosis = LaunchDiagnosis {
        quarantined: is_quarantined(app),
        resigned,
        gatekeeper_accepted,
        gatekeeper_reason,
        open_error: open_error.to_string(),
        amfi_rejected: open_error_is_amfi_rejection(open_error) || probe_rejected,
        processes: bundle_pids(app).len(),
        hint: String::new(),
    };
    diagnosis.hint = launch_hint(&diagnosis);
    diagnosis
}

/// `open -n` returns 0 even when LaunchServices refuses the app, so wait briefly for a process
/// before reporting the attempt as successful. Never bypasses a system prompt.
pub fn open_wechat_for_user(app: &Path) -> LaunchAttempt {
    let (ok, _, stderr) = run(&["/usr/bin/open", "-n", &app.to_string_lossy()])
        .unwrap_or_else(|error| (false, String::new(), error.to_string()));
    let error = if ok { String::new() } else { collapse(&stderr, 400) };
    let mut diagnosis = diagnose_launch(app, &error);
    if ok && diagnosis.processes == 0 {
        for _ in 0..20 {
            thread::sleep(Duration::from_millis(150));
            if !bundle_pids(app).is_empty() {
                break;
            }
        }
        diagnosis = diagnose_launch(app, &error);
    }
    LaunchAttempt {
        launched: ok && diagnosis.processes > 0,
        error,
        diagnosis,
    }
}

pub fn launch_wechat(app: &Path) -> CaptureResult<i64> {
    let attempt = open_wechat_for_user(app);
    let deadline = Instant::now() + Duration::from_secs(45);
    while Instant::now() < deadline {
        if let Some(pid) = main_pid(app) {
            thread::sleep(Duration::from_secs(2));
            return match main_pid(app) {
                Some(stable) if stable == pid => Ok(pid),
                _ => Err(CaptureError::new(
                    "wechat_launch_exited",
                    "临时微信在启动检查期间退出或更换了进程，已停止并准备恢复官方原版。\
                     如果系统提示无法验证，请不要绕过安全检查。",
                )),
            };
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(CaptureError::new(
        "wechat_launch_failed",
        match attempt.error.is_empty() {
            true => format!("未能启动微信。{}", attempt.diagnosis.hint),
            false => format!("未能启动微信：{} {}", attempt.error, attempt.diagnosis.hint),
        },
    ))
}

/* ── state, lock, paths ───────────────────────────────────────────────────── */

pub fn state_path(root: &Path) -> PathBuf {
    root.join(STATE_NAME)
}

fn ready_path(root: &Path) -> PathBuf {
    root.join(READY_NAME)
}

fn result_path(root: &Path) -> PathBuf {
    root.join(RESULT_NAME)
}

fn preflight_path(root: &Path) -> PathBuf {
    root.join(PREFLIGHT_NAME)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

fn transaction_id() -> String {
    format!(
        "{:x}{:x}{:x}",
        now_secs(),
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.subsec_nanos())
            .unwrap_or(0)
    )
}

/// Owner-only, atomically replaced state file. Progress is advisory: a failed write must never
/// prevent recovery, so callers treat write errors as non-fatal where that is safe.
pub fn write_state(root: &Path, state: &CaptureState) -> CaptureResult<()> {
    fs::create_dir_all(root)
        .map_err(|error| CaptureError::new("state_write_failed", format!("无法创建状态目录：{error}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(root, fs::Permissions::from_mode(0o700));
    }
    let temp = root.join(format!(".{STATE_NAME}.{}.tmp", std::process::id()));
    let body = serde_json::to_vec_pretty(state)
        .map_err(|error| CaptureError::new("state_write_failed", format!("无法序列化状态：{error}")))?;
    let mut file = fs::File::create(&temp)
        .map_err(|error| CaptureError::new("state_write_failed", format!("无法写入状态：{error}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
    }
    file.write_all(&body)
        .and_then(|_| file.sync_all())
        .map_err(|error| CaptureError::new("state_write_failed", format!("无法写入状态：{error}")))?;
    drop(file);
    fs::rename(&temp, state_path(root))
        .map_err(|error| CaptureError::new("state_write_failed", format!("无法保存状态：{error}")))
}

pub fn read_state(root: &Path) -> Option<CaptureState> {
    let body = fs::read(state_path(root)).ok()?;
    let state: CaptureState = serde_json::from_slice(&body).ok()?;
    (state.schema_version == 1 || state.schema_version == STATE_SCHEMA).then_some(state)
}

fn remove_state(root: &Path) {
    let _ = fs::remove_file(state_path(root));
    let _ = fs::remove_file(ready_path(root));
    let _ = fs::remove_file(preflight_path(root));
}

fn set_phase(root: &Path, state: &mut CaptureState, phase: &str) {
    if state.stage != "capturing" {
        return;
    }
    state.capture_phase = Some(phase.to_string());
    let _ = write_state(root, state);
}

struct Lock {
    path: PathBuf,
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn process_alive(pid: i64) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// One capture transaction per installation, across processes, exactly like the reference's
/// installation lock. A stale lock from a dead process is reclaimed.
fn acquire_lock(root: &Path) -> CaptureResult<Lock> {
    fs::create_dir_all(root)
        .map_err(|error| CaptureError::new("state_write_failed", format!("无法创建状态目录：{error}")))?;
    let path = root.join(LOCK_NAME);
    for _ in 0..2 {
        match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let _ = file.write_all(format!("{}\n", std::process::id()).as_bytes());
                return Ok(Lock { path });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let owner = fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| text.trim().parse::<i64>().ok());
                if owner.map(process_alive).unwrap_or(false) {
                    return Err(CaptureError::new(
                        "in_place_capture_busy",
                        "另一个密钥捕获正在进行，请等它结束后再试。",
                    ));
                }
                let _ = fs::remove_file(&path);
            }
            Err(error) => {
                return Err(CaptureError::new(
                    "state_write_failed",
                    format!("无法创建捕获锁：{error}"),
                ))
            }
        }
    }
    Err(CaptureError::new("in_place_capture_busy", "无法独占微信安装，请稍后重试。"))
}

/* ── status ───────────────────────────────────────────────────────────────── */

#[derive(Debug, Clone, Serialize)]
pub struct CaptureStatus {
    pub pending: bool,
    pub stage: String,
    pub needs_cleanup: bool,
    pub monitor_ready: bool,
    pub capture_phase: Option<String>,
    pub resume_available: bool,
    pub prepared_process_exited: Option<bool>,
    pub transaction_id: Option<String>,
    pub recovery_error_code: Option<String>,
    /// `preserved` / `sanitized` while a transaction is pending, so the UI can explain the
    /// macOS 15 fallback even after a restart.
    pub entitlement_mode: Option<String>,
    pub platform_supported: bool,
    pub lldb_available: bool,
    pub wechat_present: bool,
    pub wechat_version: Option<String>,
    pub wechat_build: Option<String>,
    pub wechat_official: bool,
    pub wechat_ad_hoc: bool,
    /// The bundle carries `com.apple.quarantine`, so Gatekeeper will evaluate the resigned copy.
    pub wechat_quarantined: bool,
    pub user_can_write: bool,
    pub backup_root: String,
    pub restore_slot_present: bool,
    pub message: String,
}

/// Result of a capture that finished but was never processed (app killed, machine restarted).
/// Only a result tagged with the current transaction and carrying a full-length passphrase is
/// accepted; anything else has to go through the normal capture path again.
fn resumable_result(root: &Path, state: &CaptureState) -> Option<serde_json::Value> {
    let payload = read_json(&result_path(root))?;
    if payload.get("transaction_id").and_then(|value| value.as_str())
        != Some(state.transaction_id.as_str())
    {
        return None;
    }
    let passphrase = payload
        .get("passphrase")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    (passphrase.len() == 64 && passphrase.chars().all(|ch| ch.is_ascii_hexdigit()))
        .then_some(payload)
}

fn lldb_available() -> bool {
    Command::new("/usr/bin/lldb")
        .args(["--batch", "-o", "script import lldb; print('WEDATA_PYTHON_READY')"])
        .output()
        .map(|output| output.status.success() && String::from_utf8_lossy(&output.stdout).lines().any(|line| line.trim() == "WEDATA_PYTHON_READY"))
        .unwrap_or(false)
}

pub fn status(app: &Path, root: &Path, backup_root: &Path) -> CaptureStatus {
    let platform_supported = cfg!(target_os = "macos") && cfg!(target_arch = "aarch64");
    let signature = inspect_signature(app, false).ok();
    let official = signature.as_ref().map(|value| value.is_tencent_official()).unwrap_or(false);
    let ad_hoc = signature.as_ref().map(|value| value.ad_hoc).unwrap_or(false);
    let version = bundle_version(app).ok();
    let slot = restore_slot(app).symlink_metadata().is_ok();

    let mut status = CaptureStatus {
        pending: false,
        stage: "idle".to_string(),
        needs_cleanup: false,
        monitor_ready: false,
        capture_phase: None,
        resume_available: false,
        prepared_process_exited: None,
        transaction_id: None,
        recovery_error_code: None,
        entitlement_mode: None,
        platform_supported,
        lldb_available: lldb_available(),
        wechat_present: app.is_dir(),
        wechat_version: version.as_ref().map(|value| value.short.clone()),
        wechat_build: version.as_ref().map(|value| value.build.clone()),
        wechat_official: official,
        wechat_ad_hoc: ad_hoc,
        wechat_quarantined: app.is_dir() && is_quarantined(app),
        user_can_write: can_write(app)
            && app.parent().map(can_write).unwrap_or(false),
        backup_root: backup_root.to_string_lossy().to_string(),
        restore_slot_present: slot,
        message: String::new(),
    };

    if let Some(state) = read_state(root) {
        status.pending = true;
        status.needs_cleanup = true;
        status.stage = state.stage.clone();
        status.transaction_id = Some(state.transaction_id.clone());
        status.recovery_error_code = state.recovery_error_code.clone();
        status.entitlement_mode = Some(state.entitlement_mode.as_str().to_string());
        status.capture_phase = state.capture_phase.clone();
        if state.stage == "capturing" && resumable_result(root, &state).is_some() {
            // Only a stopped owner needs the recovery button; a live capture completes itself.
            status.resume_available = !process_alive(state.owner_pid);
            if !matches!(state.capture_phase.as_deref(), Some("validating" | "restoring")) {
                status.capture_phase = Some("captured".to_string());
            }
        }
        status.monitor_ready = read_json(&ready_path(root))
            .and_then(|value| value.get("status").and_then(|v| v.as_str()).map(str::to_string))
            .map(|value| value == "ready")
            .unwrap_or(false);
        if matches!(state.stage.as_str(), "launched" | "preflight_passed" | "capturing") {
            if let Some(expected) = state.debug_pid {
                status.prepared_process_exited = Some(main_pid(app) != Some(expected));
            }
        }
    }

    let message = match status.stage.as_str() {
        "backup_verified" | "debug_prepared" | "resigned" => {
            "微信已临时重签，等待你手动打开它并登录。".to_string()
        }
        "awaiting_manual_launch" => "微信已临时重签，等待你手动打开它并登录。".to_string(),
        "launched" => "等待断点预检。".to_string(),
        "preflight_passed" => "断点可用：请先在微信里退出账号，再启动监测。".to_string(),
        "capturing" => match status.capture_phase.as_deref() {
            Some("waiting_authorization") => "等待管理员授权，请在弹出的系统对话框中输入密码。".to_string(),
            Some("monitoring") => "监测已就绪：请重新登录要获取的账号。".to_string(),
            Some("captured") => "已捕获密钥，正在结束调试会话，随后校验并恢复腾讯原版微信。".to_string(),
            Some("validating") => "正在逐库校验捕获结果。".to_string(),
            Some("restoring") => "正在恢复腾讯原版微信。".to_string(),
            _ => "捕获进行中。".to_string(),
        },
        "external_install_conflict" => {
            "检测到微信安装被其他程序替换，已保留恢复资产并停止覆盖。".to_string()
        }
        "recovery_blocked" => "当前微信与本次恢复记录不匹配，已保留恢复资产并停止操作。".to_string(),
        "invalid" => "捕获状态异常，请先恢复微信。".to_string(),
        _ if status.pending => "存在未完成的捕获，请先恢复腾讯原版微信。".to_string(),
        _ if !platform_supported => "应用内捕获目前仅支持 Apple Silicon Mac。".to_string(),
        _ if !status.lldb_available => "LLDB 或其 Python 运行环境无法启动，请修复 Xcode Command Line Tools 后重试。".to_string(),
        _ if !status.wechat_present => "没有在「应用程序」中找到微信。".to_string(),
        _ if !status.wechat_official => {
            "当前微信不是腾讯原签名版本，已停止应用内捕获（可用备份手动恢复）。".to_string()
        }
        _ if !status.user_can_write => {
            "当前用户不能写入微信或其安装目录；为保证异常后仍能恢复原版，已停止。".to_string()
        }
        _ => "环境检查通过，可以开始。".to_string(),
    };
    status.message = if status.resume_available {
        "已捕获密钥（应用重启后仍保留），点下面的按钮校验、恢复原版微信并导入。".to_string()
    } else {
        message
    };
    status
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let body = fs::read(path).ok()?;
    serde_json::from_slice(&body).ok()
}

/* ── probes ───────────────────────────────────────────────────────────────── */

/// One account database used as an independent validation role. The reference requires the
/// message and session databases: a single hit is never accepted as the account passphrase.
#[derive(Debug, Clone, Serialize)]
pub struct Probe {
    pub role: String,
    pub path: String,
    pub page1_hex: String,
}

fn probe_page1(probe: &Probe) -> Option<Vec<u8>> {
    let hex = probe.page1_hex.trim();
    if hex.len() != 8192 {
        return None;
    }
    hex::decode(hex).ok()
}

fn validate_candidate(passphrase: &[u8], probes: &[Probe]) -> Option<String> {
    use crate::wechat::crypto;
    let mut modes: Vec<String> = Vec::new();
    for probe in probes {
        let page1 = probe_page1(probe)?;
        let raw = crypto::verify_key(&page1, passphrase);
        let derived = crypto::verify_passphrase(&page1, passphrase).is_some();
        match (raw, derived) {
            (true, _) => modes.push("raw_enc_key".to_string()),
            (false, true) => modes.push("sqlcipher_passphrase".to_string()),
            _ => return None,
        }
    }
    if modes.is_empty() {
        return None;
    }
    let unique: std::collections::BTreeSet<_> = modes.iter().collect();
    if unique.len() == 1 {
        Some(modes[0].clone())
    } else {
        Some("mixed".to_string())
    }
}

/* ── LLDB scripts ─────────────────────────────────────────────────────────── */

const PREFLIGHT_TEMPLATE: &str = r#"import json
import lldb
import os

RESULT_PATH = "@@RESULT@@"
TRANSACTION_ID = "@@TXN@@"


def _hand_over(fd, path):
    # LLDB runs as root behind `do shell script … with administrator privileges`, so a 0600 file
    # would be unreadable by the app. Give the payload to the owner of its directory (which is the
    # invoking user) instead of loosening the mode: the directory itself is 0700.
    try:
        stat = os.stat(os.path.dirname(path))
        if os.getuid() == 0:
            os.fchown(fd, stat.st_uid, stat.st_gid)
    except OSError:
        pass


def _write(payload):
    if TRANSACTION_ID:
        payload = dict(payload, transaction_id=TRANSACTION_ID)
    fd = os.open(RESULT_PATH, os.O_WRONLY | os.O_TRUNC | os.O_CREAT, 0o600)
    try:
        _hand_over(fd, RESULT_PATH)
        os.write(fd, json.dumps(payload).encode())
        os.fsync(fd)
    finally:
        os.close(fd)


def _resolved_locations(breakpoint, target):
    count = 0
    for index in range(breakpoint.GetNumLocations()):
        location = breakpoint.GetLocationAtIndex(index)
        address = location.GetAddress()
        if not location.IsResolved() or not address.IsValid():
            continue
        section = address.GetSection()
        if (
            section.IsValid()
            and section.GetPermissions() & lldb.ePermissionsExecutable
            and address.GetLoadAddress(target) != lldb.LLDB_INVALID_ADDRESS
        ):
            count += 1
    return count


def _preflight(debugger, command, result, internal_dict):
    target = debugger.GetSelectedTarget()
    breakpoint = target.BreakpointCreateByName("CCKeyDerivationPBKDF")
    locations = _resolved_locations(breakpoint, target)
    process = target.GetProcess()
    _write({"pbkdf_locations": int(locations), "pid": int(process.GetProcessID() or 0)})
    print("WEDATA_PREFLIGHT_LOCATIONS", locations, flush=True)
    process.Detach()


def __lldb_init_module(debugger, internal_dict):
    debugger.HandleCommand("command script add -f " + __name__ + "._preflight wedata_preflight")
"#;

const CAPTURE_TEMPLATE: &str = r#"import hashlib
import hmac
import json
import lldb
import os

RESULT_PATH = "@@RESULT@@"
READY_PATH = "@@READY@@"
TRANSACTION_ID = "@@TXN@@"
ACCOUNT_PROBE_PAGES = @@PAGES@@
DIAGNOSTICS = {
    "pbkdf_calls": 0,
    "pbkdf_shape_hits": 0,
    "pbkdf_salt_hits": 0,
    "candidate_rejections": 0,
}


def _hand_over(fd, path):
    # Same reason as the preflight script: root-owned payloads have to end up readable (and only
    # readable) by the user who owns the capture directory.
    try:
        stat = os.stat(os.path.dirname(path))
        if os.getuid() == 0:
            os.fchown(fd, stat.st_uid, stat.st_gid)
    except OSError:
        pass


def _write(path, payload):
    if TRANSACTION_ID:
        payload = dict(payload, transaction_id=TRANSACTION_ID)
    fd = os.open(path, os.O_WRONLY | os.O_TRUNC | os.O_CREAT, 0o600)
    try:
        _hand_over(fd, path)
        os.write(fd, json.dumps(payload).encode())
        os.fsync(fd)
    finally:
        os.close(fd)


def _record(name):
    DIAGNOSTICS[name] = int(DIAGNOSTICS.get(name, 0)) + 1


def _resolved_locations(breakpoint, target):
    count = 0
    for index in range(breakpoint.GetNumLocations()):
        location = breakpoint.GetLocationAtIndex(index)
        address = location.GetAddress()
        if not location.IsResolved() or not address.IsValid():
            continue
        section = address.GetSection()
        if (
            section.IsValid()
            and section.GetPermissions() & lldb.ePermissionsExecutable
            and address.GetLoadAddress(target) != lldb.LLDB_INVALID_ADDRESS
        ):
            count += 1
    return count


def _register(frame, name):
    return frame.FindRegister(name).GetValueAsUnsigned()


def _page_mode(candidate, page):
    if len(candidate) != 32 or len(page) != 4096 or page.startswith(b"SQLite format 3\x00"):
        return ""
    salt = page[:16]
    stored_hmac = page[4032:4096]
    for mode in ("raw_enc_key", "sqlcipher_passphrase"):
        enc_key = (
            candidate
            if mode == "raw_enc_key"
            else hashlib.pbkdf2_hmac("sha512", candidate, salt, 256000, 32)
        )
        mac_salt = bytes(value ^ 0x3A for value in salt)
        mac_key = hashlib.pbkdf2_hmac("sha512", enc_key, mac_salt, 2, 32)
        digest = hmac.new(mac_key, page[16:4032], hashlib.sha512)
        digest.update((1).to_bytes(4, "little"))
        if hmac.compare_digest(stored_hmac, digest.digest()):
            return mode
    return ""


def _save_valid_candidate(candidate, source, process):
    modes = {}
    for role, page in ACCOUNT_PROBE_PAGES.items():
        mode = _page_mode(candidate, page)
        if mode:
            modes[role] = mode
    if not modes or len(modes) != len(ACCOUNT_PROBE_PAGES):
        _record("candidate_rejections")
        return False
    unique = set(modes.values())
    _write(
        RESULT_PATH,
        {
            "passphrase": candidate.hex(),
            "source": source,
            "key_mode": next(iter(unique)) if len(unique) == 1 else "mixed",
            "validated_roles": sorted(modes),
            "diagnostics": dict(DIAGNOSTICS),
        },
    )
    print("WEDATA_MATCHED_VALIDATED_DATABASE_KEY", source, flush=True)
    try:
        _write(READY_PATH, {"status": "captured"})
    except Exception:
        pass
    process.Kill()
    os._exit(0)


def _pbkdf_callback(frame, bp_loc, internal_dict):
    process = frame.GetThread().GetProcess()
    _record("pbkdf_calls")
    password_ptr = _register(frame, "x1")
    password_len = _register(frame, "x2")
    salt_ptr = _register(frame, "x3")
    salt_len = _register(frame, "x4")
    if password_len not in (32, 64) or salt_len != 16:
        return False
    _record("pbkdf_shape_hits")
    error = lldb.SBError()
    salt = process.ReadMemory(salt_ptr, 16, error)
    if not error.Success() or len(salt) != 16:
        return False
    known = set()
    for page in ACCOUNT_PROBE_PAGES.values():
        known.add(page[:16])
        known.add(bytes(value ^ 0x3A for value in page[:16]))
    if salt not in known:
        return False
    _record("pbkdf_salt_hits")
    candidate = process.ReadMemory(password_ptr, password_len, error)
    if not error.Success() or len(candidate) != password_len:
        return False
    if len(candidate) == 64:
        try:
            candidate = bytes.fromhex(candidate.decode("ascii"))
        except (UnicodeDecodeError, ValueError):
            return False
    if len(candidate) != 32:
        return False
    _save_valid_candidate(candidate, "pbkdf", process)
    return False


def _setup(debugger, command, result, internal_dict):
    target = debugger.GetSelectedTarget()
    breakpoint = target.BreakpointCreateByName("CCKeyDerivationPBKDF")
    breakpoint.SetScriptCallbackFunction(__name__ + "._pbkdf_callback")
    breakpoint.SetAutoContinue(True)
    locations = _resolved_locations(breakpoint, target)
    process = target.GetProcess()
    _write(
        READY_PATH,
        {
            "status": "ready",
            "method": "macos_lldb_stub",
            "pid": int(process.GetProcessID() or 0),
            "pbkdf_locations": int(locations),
        },
    )
    print("WEDATA_KEY_MONITOR_READY", locations, flush=True)
    if locations <= 0:
        process.Detach()
        os._exit(24)


def __lldb_init_module(debugger, internal_dict):
    debugger.HandleCommand("command script add -f " + __name__ + "._setup wedata_capture")
"#;

fn write_script(root: &Path, name: &str, body: &str) -> CaptureResult<PathBuf> {
    let path = root.join(name);
    fs::write(&path, body)
        .map_err(|error| CaptureError::new("capture_script_failed", format!("无法写入 LLDB 脚本：{error}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

fn python_pages(probes: &[Probe]) -> String {
    let mut entries: Vec<String> = Vec::new();
    for probe in probes {
        entries.push(format!(
            "{}: bytes.fromhex(\"{}\")",
            python_string(&probe.role),
            probe.page1_hex
        ));
    }
    format!("{{{}}}", entries.join(", "))
}

fn python_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

/// LLDB paths containing spaces need quoting when used inside its command files.
/// LLDB splits `command script import` on whitespace and the capture directory lives under
/// `…/Application Support/…`, so the callback path must be quoted or the import silently fails and
/// the script never defines its command (which looks like "no usable breakpoints").
fn lldb_quote(path: &Path) -> String {
    let value = path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!("\"{value}\"")
}

#[cfg(test)]
fn lldb_command(script_path: &Path, keepalive_secs: u64) -> String {
    lldb_command_with_executable(script_path, keepalive_secs, Path::new("/usr/bin/lldb"))
}

#[cfg(test)]
fn lldb_command_with_executable(script_path: &Path, keepalive_secs: u64, executable: &Path) -> String {
    lldb_command_inner(script_path, keepalive_secs, executable, None)
}

fn lldb_command_inner(script_path: &Path, keepalive_secs: u64, executable: &Path, captured: Option<(&Path, &str, i64)>) -> String {
    let script_arg = shell_quote(script_path);
    let keepalive = keepalive_secs.max(1);
    let (ready_path, transaction, target_pid) = captured.unwrap_or((Path::new(""), "", 0));
    let shell = format!(
        r#"capture_dir=$(/usr/bin/mktemp -d /tmp/wedata-lldb.XXXXXX)
lldb_pid=""
watchdog_pid=""
cleanup() {{
  trap '' HUP INT TERM
  if [ -n "$watchdog_pid" ]; then /bin/kill "$watchdog_pid" 2>/dev/null || true; fi
  if [ -n "$lldb_pid" ]; then /bin/kill "$lldb_pid" 2>/dev/null || true; fi
  if [ -n "$watchdog_pid" ]; then wait "$watchdog_pid" 2>/dev/null || true; fi
  if [ -n "$lldb_pid" ]; then wait "$lldb_pid" 2>/dev/null || true; fi
  /bin/rm -rf "$capture_dir"
}}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
/usr/bin/env TERM=dumb {executable} --batch --source {script} < /dev/null > "$capture_dir/output" 2>&1 &
lldb_pid=$!
/usr/bin/env LC_ALL=C LANG=C /usr/bin/perl -MJSON::PP -e '
  my ($timeout, $debugger, $ready, $transaction, $target) = @ARGV;
  my $deadline = time + $timeout;
  while (time < $deadline) {{
    if ($target && open(my $file, "<", $ready)) {{
      local $/; my $data = eval {{ decode_json(<$file>) }}; close($file);
      if (ref($data) eq "HASH" && ($data->{{status}} // "") eq "captured" &&
          ($data->{{transaction_id}} // "") eq $transaction && !kill(0, $target)) {{
        kill "TERM", $debugger; exit 0;
      }}
    }}
    select(undef, undef, undef, 0.2);
  }}
  kill "TERM", $debugger;
' {keepalive} "$lldb_pid" {ready} {transaction} {target} </dev/null >/dev/null 2>&1 &
watchdog_pid=$!
wait "$lldb_pid"
lldb_status=$?
lldb_pid=""
/bin/kill "$watchdog_pid" 2>/dev/null || true
wait "$watchdog_pid" 2>/dev/null || true
watchdog_pid=""
/bin/cat "$capture_dir/output"
echo "WEDATA_LLDB_EXIT=$lldb_status"
exit 0
"#,
        script = script_arg,
        executable = shell_quote(executable),
        keepalive = keepalive,
        ready = shell_quote(ready_path),
        transaction = shell_quote_text(transaction),
        target = target_pid,
    );
    format!("/bin/bash -c {}", shell_quote_text(&shell))
}

fn run_lldb(script_body: &Path, timeout_secs: u64) -> CaptureResult<String> {
    run_lldb_with_capture(script_body, timeout_secs, None)
}

fn run_lldb_with_capture(script_body: &Path, timeout_secs: u64, captured: Option<(&Path, &str, i64)>) -> CaptureResult<String> {
    if !lldb_available() {
        return Err(CaptureError::new(
            "lldb_missing",
            "LLDB 或其 Python 运行环境无法启动，请修复 Xcode Command Line Tools 后再试。",
        ));
    }
    let command = lldb_command_inner(script_body, timeout_secs, Path::new("/usr/bin/lldb"), captured);
    run_as_administrator(&command, Duration::from_secs(timeout_secs + 45))
}

/* ── prepare ──────────────────────────────────────────────────────────────── */

#[derive(Debug, Clone, Serialize)]
pub struct PrepareResult {
    pub transaction_id: String,
    pub version: String,
    pub build: String,
    pub backup_path: String,
    pub backup_created: bool,
    pub official_cdhash: String,
    /// `preserved` or `sanitized`; the UI explains the macOS 15 fallback when it is sanitized.
    pub entitlement_mode: String,
    /// Unused when the entitlements were preserved.
    pub dropped_entitlements: Vec<String>,
}

fn verify_official_for_resign(app: &Path) -> CaptureResult<Signature> {
    let signature = inspect_signature(app, true)?;
    if !signature.is_tencent_official() {
        return Err(CaptureError::new(
            "wechat_signature_untrusted",
            "当前 /Applications/WeChat.app 不是有效的腾讯正式签名版本，已停止临时重签。",
        )
        .modified());
    }
    if !can_write(app) || !app.parent().map(can_write).unwrap_or(false) {
        return Err(CaptureError::new(
            "in_place_restore_permissions_unsafe",
            "当前用户不能直接写入微信及其安装目录；为保证异常退出后仍能恢复原版，已停止临时重签。",
        ));
    }
    Ok(signature)
}

/// Sign the *clone* ad-hoc, then prove the result can be launched and attached to and still
/// carries the sandbox claims the chosen mode asks for.
fn sign_debug_clone(staged: &Path, plan: &EntitlementPlan) -> CaptureResult<Signature> {
    let scratch = Scratch::new("in-place-sign")?;
    let target = staged.to_string_lossy().to_string();
    let mut owned_args: Vec<String> = vec!["/usr/bin/codesign".to_string(), "--force".to_string()];
    if plan.mode == EntitlementMode::Deep {
        owned_args.push("--deep".to_string());
    } else { match plan.plist.as_deref() {
        Some(plist) => {
            let path = scratch.path().join("debug-entitlements.plist");
            fs::write(&path, plist).map_err(|error| {
                CaptureError::new(
                    "in_place_sign_failed",
                    format!("无法写入临时权限列表：{error}"),
                )
            })?;
            owned_args.push("--entitlements".to_string());
            owned_args.push(path.to_string_lossy().to_string());
        }
        // macOS 14 and earlier: keep Tencent's entitlement dictionary verbatim.
        None => owned_args.push("--preserve-metadata=entitlements".to_string()),
    }}
    owned_args.push("--sign".to_string());
    owned_args.push("-".to_string());
    owned_args.push(target.clone());
    let args: Vec<&str> = owned_args.iter().map(String::as_str).collect();

    let direct = run(&args)?;
    if !direct.0 {
        // Root-owned / managed installs need the administrator dialog; same fallback as the
        // reference, with every argument quoted exactly.
        let quoted: Vec<String> = owned_args[1..owned_args.len() - 1]
            .iter()
            .map(|value| shell_quote_text(value))
            .collect();
        let command = format!(
            "/usr/bin/codesign {} {}",
            quoted.join(" "),
            shell_quote(staged)
        );
        run_as_administrator(&command, Duration::from_secs(345))
            .map_err(|error| error.with_code("in_place_sign_failed"))?;
    }
    let signature = inspect_signature(staged, true)?;
    if !signature.is_debuggable_ad_hoc() {
        return Err(CaptureError::new(
            "in_place_sign_failed",
            "临时调试签名校验失败（需要不带强化运行时的 ad-hoc 签名）。",
        )
        .modified());
    }
    if !in_place_signature_is_compatible(staged, plan.mode) {
        return Err(CaptureError::new(
            "in_place_entitlements_unsafe",
            match plan.mode {
                EntitlementMode::Preserved => {
                    "临时签名没有保留沙箱与应用组权限，或登录组件签名被改动，已停止。"
                }
                EntitlementMode::Sanitized => {
                    "临时签名没有保留沙箱与网络权限，或仍带有腾讯团队的受限权限，已停止。"
                }
                EntitlementMode::Deep => "临时副本的深度签名或权限校验失败，已停止。",
            },
        )
        .modified());
    }
    Ok(signature)
}

/// Verify, back up, clone, sign the clone, record recovery facts and exchange it in.
pub fn prepare(
    app: &Path,
    root: &Path,
    backup_root: &Path,
    defer_launch: bool,
) -> CaptureResult<PrepareResult> {
    if !cfg!(target_os = "macos") {
        return Err(CaptureError::new("unsupported_platform", "密钥捕获仅支持 macOS。"));
    }
    let app = normalize_app(Some(app))?;
    if app != Path::new(WECHAT_APP) {
        return Err(CaptureError::new(
            "in_place_default_path_required",
            "临时重签只允许作用于 /Applications/WeChat.app，请先使用微信默认安装路径。",
        ));
    }
    let _lock = acquire_lock(root)?;

    // A previous transaction must be resolved (or superseded) before a new one starts.
    if let Some(previous) = read_state(root) {
        let signature = inspect_signature(&app, true)?;
        let same_installation = !signature.is_tencent_official()
            || (signature.cdhash.eq_ignore_ascii_case(&previous.official_cdhash)
                && previous.version == bundle_version(&app)?.short);
        if same_installation {
            restore_locked(&app, root)?;
        } else {
            // The user installed a different official build since the last transaction: keep
            // the old recovery assets but never overwrite the new installation.
            let target = stamp_dir(&root.join("preserved"), "superseded")?;
            if Path::new(&previous.backup_path).is_file() {
                let _ = fs::copy(&previous.backup_path, target.join("WeChat-original.zip"));
            }
            remove_state(root);
        }
    }

    let signature = verify_official_for_resign(&app)?;
    let version = bundle_version(&app)?;
    let original_cdhash = signature.cdhash.to_lowercase();
    if original_cdhash.is_empty() {
        return Err(CaptureError::new(
            "wechat_signature_untrusted",
            "无法读取微信的 CDHash，已停止临时重签。",
        ));
    }

    // Decide the signature shape *before* anything is touched: on macOS 15+ a preserved
    // entitlement dictionary makes the resigned copy unlaunchable, and finding that out after
    // the swap would mean handing the user a WeChat that cannot start.
    let plan = plan_entitlements(&app)?;

    let (backup, backup_created) = backup_original(&app, backup_root)?;
    let (backup_signature, backup_version) = verify_backup(&backup, Some(&version), root)?;
    if !backup_signature.cdhash.eq_ignore_ascii_case(&original_cdhash) {
        return Err(CaptureError::new(
            "official_backup_identity_mismatch",
            "原版备份虽有腾讯签名，但与当前安装的微信不是同一构建，已停止临时重签。",
        ));
    }

    let transaction = transaction_id();
    let mut state = CaptureState {
        schema_version: STATE_SCHEMA,
        stage: "backup_verified".to_string(),
        created_at: now_secs(),
        transaction_id: transaction.clone(),
        owner_pid: std::process::id() as i64,
        wechat_app_path: app.to_string_lossy().to_string(),
        backup_path: backup.to_string_lossy().to_string(),
        backup_root: backup_root.to_string_lossy().to_string(),
        backup_created,
        version: backup_version.short.clone(),
        build: backup_version.build.clone(),
        official_cdhash: original_cdhash.clone(),
        entitlement_mode: plan.mode,
        debug_identity: None,
        debug_pid: None,
        capture_phase: None,
        capture_backend: None,
        preflight_pid: None,
        recovery_error_code: None,
    };
    // Persist the recovery facts before anything in the installed bundle changes.
    write_state(root, &state)?;

    let outcome = (|| {
        quit_wechat(&app, false)?;
        // A leftover bundle in the recovery slot without a matching state file is moved
        // aside (never deleted) before the fixed slot is reused, like the reference.
        if restore_slot(&app).symlink_metadata().is_ok() {
            let _ = preserve_assets(&app, root, "leftover")?;
        }
        let staged = clone_staging(&app, &version, &original_cdhash)?;
        let debug_signature = sign_debug_clone(&staged, &plan)?;
        let debug_identity = bundle_identity(&staged, &debug_signature)?;
        state.debug_identity = Some(debug_identity.clone());
        state.stage = "debug_prepared".to_string();
        write_state(root, &state)?;

        // The installed bundle must still be the one we verified; otherwise another installer
        // won the race and we must not overwrite it.
        let current = inspect_signature(&app, true)?;
        if !official_identity_matches(&app, &current, Some(&version), Some(&original_cdhash)) {
            return Err(CaptureError::new(
                "external_install_conflict",
                "准备期间微信安装已变化；已保留当前应用和恢复资产，未覆盖。",
            ));
        }
        atomic_swap(&app, &staged)?;
        let installed = inspect_signature(&app, true)?;
        if !debug_identity_matches(&app, Some(&debug_identity))
            || !installed.is_debuggable_ad_hoc()
            || !in_place_signature_is_compatible(&app, plan.mode)
        {
            // Put the official bundle back before reporting the failure.
            if debug_identity_matches(&app, Some(&debug_identity)) {
                let _ = atomic_swap(&app, &staged);
            }
            return Err(CaptureError::new(
                "in_place_swap_verify_failed",
                "临时微信原子安装后的签名校验失败，已尝试回滚。",
            )
            .modified());
        }
        // The signature that actually landed must pass the same probe. If it does not, hand the
        // official bundle back instead of a WeChat that the system will refuse to start.
        {
            let verdict = if plan.mode == EntitlementMode::Deep {
                amfi_accepts_entitlements(&serde_json::json!({}))
            } else {
                read_entitlements(&app).ok().and_then(|entitlements| amfi_accepts_entitlements(&entitlements))
            };
            if verdict != Some(true) {
                if debug_identity_matches(&app, Some(&debug_identity)) {
                    let _ = atomic_swap(&app, &staged);
                }
                return Err(CaptureError::new(
                    "in_place_amfi_rejected",
                    "无法确认安装后的临时副本的签名权限可用，已尝试换回腾讯原版微信。\
                     请检查恢复状态，或改用「导入密钥」。",
                )
                .modified());
            }
        }
        // The recovery slot must still hold the untouched official bundle.
        let staged_signature = inspect_signature(&staged, true)?;
        if !official_identity_matches(&staged, &staged_signature, Some(&version), Some(&original_cdhash)) {
            return Err(CaptureError::new(
                "external_install_conflict",
                "交换期间检测到其他微信安装，已停止本次捕获并保留恢复资产。",
            ));
        }
        Ok(())
    })();

    match outcome {
        Ok(()) => {
            state.stage = "resigned".to_string();
            let pid = if defer_launch { None } else { Some(launch_wechat(&app)?) };
            state.stage = if defer_launch { "awaiting_manual_launch" } else { "launched" }.to_string();
            state.debug_pid = pid;
            write_state(root, &state)?;
            Ok(PrepareResult {
                transaction_id: transaction,
                version: state.version.clone(),
                build: state.build.clone(),
                backup_path: state.backup_path.clone(),
                backup_created,
                official_cdhash: original_cdhash,
                entitlement_mode: plan.mode.as_str().to_string(),
                dropped_entitlements: plan.dropped.clone(),
            })
        }
        Err(error) => {
            state.recovery_error_code = Some(error.code.clone());
            let _ = write_state(root, &state);
            match restore_locked(app.as_path(), root) {
                Ok(_) => Err(error),
                Err(restore_error) => {
                    if matches!(
                        restore_error.code.as_str(),
                        "external_install_conflict"
                            | "in_place_debug_identity_unknown"
                            | "official_restore_staging_conflict"
                            | "official_restore_staging_missing"
                    ) {
                        Err(restore_error)
                    } else {
                        Err(CaptureError::new(
                            "official_restore_failed",
                            format!("临时重签失败，且自动恢复腾讯原版微信失败：{restore_error}"),
                        )
                        .modified())
                    }
                }
            }
        }
    }
}

/* ── manual launch, preflight, capture ───────────────────────────────────── */

#[derive(Debug, Clone, Serialize)]
pub struct LaunchCheck {
    pub transaction_id: String,
    pub ready_for_preflight: bool,
    pub requires_manual_launch: bool,
    pub pid: Option<i64>,
    /// Present only when no stable process was found, so the UI can explain *why*.
    pub diagnosis: Option<LaunchDiagnosis>,
}

fn require_prepared(app: &Path, root: &Path, allow: &[&str]) -> CaptureResult<CaptureState> {
    let state = read_state(root).ok_or_else(|| {
        CaptureError::new(
            "in_place_capture_not_prepared",
            "没有找到可恢复的临时重签状态，请重新开始。",
        )
    })?;
    if Path::new(&state.wechat_app_path) != app {
        return Err(CaptureError::new(
            "in_place_capture_target_mismatch",
            "恢复状态对应的微信安装路径与当前路径不一致，已拒绝操作。",
        )
        .modified());
    }
    if !allow.contains(&state.stage.as_str()) {
        return Err(CaptureError::new(
            "in_place_capture_owner_changed",
            "捕获事务已结束或步骤顺序已变化，请先恢复微信后重新开始。",
        )
        .modified());
    }
    Ok(state)
}

fn verify_debug_installation(app: &Path, state: &CaptureState) -> CaptureResult<()> {
    let signature = inspect_signature(app, false)?;
    if !signature.is_debuggable_ad_hoc()
        || !in_place_signature_is_compatible(app, state.entitlement_mode)
        || !debug_identity_matches(app, state.debug_identity.as_ref())
    {
        return Err(CaptureError::new(
            "in_place_debug_identity_unknown",
            "当前微信不是本次准备的调试副本，已停止捕获并保留恢复资产。",
        )
        .modified());
    }
    Ok(())
}

/// Check an explicitly user-started app. Never launches or authorizes it on the user's behalf.
pub fn confirm_launch(app: &Path, root: &Path, transaction: &str) -> CaptureResult<LaunchCheck> {
    let app = normalize_app(Some(app))?;
    let mut state = require_prepared(&app, root, &["awaiting_manual_launch", "launched"])?;
    if state.transaction_id != transaction {
        return Err(CaptureError::new(
            "in_place_capture_owner_changed",
            "等待启动的事务已变化，请先恢复微信。",
        )
        .modified());
    }
    verify_debug_installation(&app, &state)?;
    let mut result = LaunchCheck {
        transaction_id: transaction.to_string(),
        ready_for_preflight: false,
        requires_manual_launch: true,
        pid: None,
        diagnosis: None,
    };
    let Some(pid) = main_pid(&app) else {
        result.diagnosis = Some(diagnose_launch(&app, ""));
        return Ok(result);
    };
    thread::sleep(Duration::from_secs(2));
    if main_pid(&app) != Some(pid) {
        result.diagnosis = Some(diagnose_launch(&app, ""));
        return Ok(result);
    }
    verify_debug_installation(&app, &state)?;
    state.debug_pid = Some(pid);
    state.stage = "launched".to_string();
    write_state(root, &state)?;
    result.ready_for_preflight = true;
    result.requires_manual_launch = false;
    result.pid = Some(pid);
    Ok(result)
}

#[derive(Debug, Clone, Serialize)]
pub struct PreflightResult {
    pub transaction_id: String,
    pub pid: i64,
    pub pbkdf_locations: i64,
    pub message: String,
}

/// Attach briefly, prove the PBKDF breakpoint resolves, and detach *before* the user logs out.
pub fn preflight(app: &Path, root: &Path) -> CaptureResult<PreflightResult> {
    let app = normalize_app(Some(app))?;
    let mut state = require_prepared(&app, root, &["launched", "preflight_passed"])?;
    verify_debug_installation(&app, &state)?;
    let pid = match (state.debug_pid, main_pid(&app)) {
        (Some(expected), Some(current)) if expected == current => current,
        _ => {
            return Err(CaptureError::new(
                "debug_wechat_not_running",
                "临时调试微信已经退出，将恢复腾讯原版微信。",
            )
            .modified())
        }
    };

    let script = PREFLIGHT_TEMPLATE
        .replace("@@RESULT@@", &preflight_path(root).to_string_lossy())
        .replace("@@TXN@@", &state.transaction_id);
    let callback = write_script(root, "preflight_callback.py", &script)?;
    let commands = format!(
        "settings set target.preload-symbols false\n\
         process attach -p {pid}\n\
         process handle SIGTRAP -n false -p false -s false\n\
         command script import {}\n\
         wedata_preflight\n\
         quit\n",
        lldb_quote(&callback)
    );
    let command_file = write_script(root, "preflight.lldb", &commands)?;
    let _ = fs::remove_file(preflight_path(root));
    let output = run_lldb(&command_file, 45)?;
    let _ = fs::remove_file(&callback);
    let _ = fs::remove_file(&command_file);

    let compact = output.to_lowercase();
    if compact.contains("library not loaded") || compact.contains("wedata_lldb_exit=134") {
        let _ = write_script(root, "last-preflight.log", &output);
        return Err(CaptureError::new(
            "lldb_runtime_unavailable",
            "LLDB 无法启动：缺少运行库。请修复 Xcode Command Line Tools 的 Python 运行环境；监测未启动，请不要退出账号。",
        ));
    }
    if compact.contains("attach failed") || compact.contains("not allowed to attach") {
        return Err(CaptureError::new(
            "lldb_attach_failed",
            "LLDB 无法附加临时调试微信，请确认管理员授权已完成。",
        ));
    }
    let payload = read_json(&preflight_path(root)).ok_or_else(|| {
        let _ = write_script(root, "last-preflight.log", &output);
        CaptureError::new("lldb_preflight_failed", "断点检查没有返回结果，请检查 LLDB 日志；监测未启动，请不要退出账号。")
    })?;
    let locations = payload
        .get("pbkdf_locations")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    if payload
        .get("transaction_id")
        .and_then(|value| value.as_str())
        .map(|value| value != state.transaction_id)
        .unwrap_or(false)
    {
        return Err(CaptureError::new(
            "capture_preflight_stale",
            "断点预检属于上一次操作，请重新开始本次获取。",
        ));
    }
    if locations <= 0 {
        // Keep the privileged session's output: without it a failed preflight is unreproducible
        // (the interesting part happens as root, outside this process's stderr).
        let _ = write_script(root, "last-preflight.log", &output);
        return Err(CaptureError::new(
            "capture_breakpoints_unavailable",
            "当前微信版本没有可用的密钥捕获断点；监测未启动，请不要退出账号。",
        ));
    }
    state.stage = "preflight_passed".to_string();
    state.capture_backend = Some("lldb".to_string());
    state.preflight_pid = Some(pid);
    write_state(root, &state)?;
    Ok(PreflightResult {
        transaction_id: state.transaction_id,
        pid,
        pbkdf_locations: locations,
        message: "断点可用。请先在微信里退出账号，再启动监测。".to_string(),
    })
}

#[derive(Debug, Clone)]
pub struct CaptureOutcome {
    /// Never serialized to the renderer; used only to import the verified key.
    pub passphrase_hex: String,
    pub key_mode: String,
    pub validated_roles: Vec<String>,
    pub saved_path: String,
    pub restored: bool,
    pub source: String,
}

fn save_passphrase_file(passphrase: &str) -> CaptureResult<PathBuf> {
    if passphrase.len() != 64 || !passphrase.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(CaptureError::new("invalid_passphrase", "捕获到的口令格式无效。"));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| CaptureError::new("invalid_passphrase", "无法定位当前用户目录。"))?;
    let target = home.join(".wcdb-key-tool").join("wechat-passphrase.json");
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| CaptureError::new("passphrase_not_saved", format!("无法创建口令目录：{error}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let temp = target.with_extension("tmp");
    let mut file = fs::File::create(&temp)
        .map_err(|error| CaptureError::new("passphrase_not_saved", format!("无法写入口令文件：{error}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
    }
    let body = serde_json::json!({ "passphrase": passphrase.to_lowercase() }).to_string();
    file.write_all(format!("{}\n", body).as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| CaptureError::new("passphrase_not_saved", format!("无法写入口令文件：{error}")))?;
    drop(file);
    fs::rename(&temp, &target)
        .map_err(|error| CaptureError::new("passphrase_not_saved", format!("无法保存口令文件：{error}")))?;
    Ok(target)
}

/// Attach after logout, capture on re-login, validate against both databases, then restore.
pub fn capture(app: &Path, root: &Path, probes: &[Probe], timeout_secs: u64) -> CaptureResult<CaptureOutcome> {
    let app = normalize_app(Some(app))?;
    let roles: Vec<&str> = probes.iter().map(|probe| probe.role.as_str()).collect();
    if !roles.contains(&"message") || !roles.contains(&"session") {
        return Err(CaptureError::new(
            "probe_database_required",
            "缺少消息库或会话库，无法完成双库校验，已停止捕获。",
        ));
    }
    if probes.iter().any(|probe| probe_page1(probe).is_none()) {
        return Err(CaptureError::new(
            "probe_database_invalid",
            "所选账号数据库不是有效的加密 WCDB（第一页无法读取）。",
        ));
    }

    // The privileged monitor is detached from this process: it can still finish (and validate)
    // the capture after the app is killed or rebuilt. Prefer that result over asking the user to
    // log out and in again.
    if let Some(mut state) = read_state(root).filter(|state| state.stage == "capturing") {
        if resumable_result(root, &state).is_some() {
            set_phase(root, &mut state, "captured");
            return complete_capture(&app, root, probes, state, Ok("resumed".to_string()));
        }
    }

    let mut state = require_prepared(&app, root, &["preflight_passed"])?;
    verify_debug_installation(&app, &state)?;
    // The preflight result must belong to this transaction: a stale file would let monitoring
    // start against breakpoints that were validated for a different process.
    let preflight = read_json(&preflight_path(root)).unwrap_or(serde_json::Value::Null);
    if preflight
        .get("transaction_id")
        .and_then(|value| value.as_str())
        .map(|value| value != state.transaction_id)
        .unwrap_or(true)
    {
        return Err(CaptureError::new(
            "capture_preflight_required",
            "请先完成断点预检，再启动登录监测。",
        )
        .modified());
    }
    let pid = match (state.preflight_pid, main_pid(&app)) {
        (Some(expected), Some(current)) if expected == current => current,
        _ => {
            return Err(CaptureError::new(
                "capture_target_changed",
                "预检后的微信进程已更换，请先恢复微信后重新开始。",
            )
            .modified())
        }
    };

    state.stage = "capturing".to_string();
    state.capture_phase = Some("waiting_authorization".to_string());
    write_state(root, &state)?;
    let _ = fs::remove_file(result_path(root));
    let _ = fs::remove_file(ready_path(root));

    let script = CAPTURE_TEMPLATE
        .replace("@@RESULT@@", &result_path(root).to_string_lossy())
        .replace("@@READY@@", &ready_path(root).to_string_lossy())
        .replace("@@TXN@@", &state.transaction_id)
        .replace("@@PAGES@@", &python_pages(probes));
    let callback = write_script(root, "capture_callback.py", &script)?;
    let commands = format!(
        "settings set target.preload-symbols false\n\
         process attach -p {pid}\n\
         process handle SIGTRAP -n false -p false -s false\n\
         command script import {}\n\
         wedata_capture\n\
         process continue\n",
        lldb_quote(&callback)
    );
    let command_file = write_script(root, "capture.lldb", &commands)?;

    // The privileged call blocks until LLDB exits; poll the advisory ready/result files from
    // this thread so the UI can tell the user when the monitor is armed.
    let (tx, rx) = mpsc::channel();
    let command_timeout = timeout_secs;
    {
        let command_file = command_file.clone();
        let capture_ready = ready_path(root);
        let transaction = state.transaction_id.clone();
        thread::spawn(move || {
            let _ = tx.send(run_lldb_with_capture(&command_file, command_timeout, Some((&capture_ready, &transaction, pid))));
        });
    }

    let deadline = Instant::now() + Duration::from_secs(timeout_secs + 20);
    let mut output = None;
    loop {
        if let Ok(value) = rx.try_recv() {
            output = Some(value);
        }
        if output.is_some() {
            break;
        }
        if read_json(&ready_path(root))
            .and_then(|value| value.get("status").and_then(|v| v.as_str()).map(str::to_string))
            .map(|value| value == "ready")
            .unwrap_or(false)
            && state.capture_phase.as_deref() == Some("waiting_authorization")
        {
            set_phase(root, &mut state, "monitoring");
        }
        if Instant::now() > deadline {
            // The privileged shell's watchdog stops LLDB at `timeout_secs`; give it a moment to
            // reap so the bundle is never swapped while LLDB is still attached.
            output = rx.recv_timeout(Duration::from_secs(30)).ok();
            if output.is_none() {
                set_phase(root, &mut state, "restoring");
                let _ = fs::remove_file(&callback);
                let _ = fs::remove_file(&command_file);
                return Err(after_capture_failure(
                    &app,
                    root,
                    CaptureError::new("capture_timeout", "等待登录捕获超时，已结束监测并恢复微信。"),
                    &mut state,
                ));
            }
            break;
        }
        thread::sleep(Duration::from_millis(300));
    }
    let _ = fs::remove_file(&callback);
    let _ = fs::remove_file(&command_file);

    let run_result = output.unwrap_or_else(|| {
        Err(CaptureError::new(
            "capture_timeout",
            "等待登录捕获超时，已结束监测并恢复微信。",
        ))
    });
    complete_capture(&app, root, probes, state, run_result)
}

/// Validate and persist the captured passphrase, restore the official bundle and report the
/// outcome. Split out of `capture()` because the LLDB monitor can finish after the app was
/// killed and restarted: the transaction-tagged result on disk is still valid, so completing it
/// must not cost the user another administrator prompt or another login.
fn complete_capture(
    app: &Path,
    root: &Path,
    probes: &[Probe],
    mut state: CaptureState,
    run_result: CaptureResult<String>,
) -> CaptureResult<CaptureOutcome> {
    let payload = read_json(&result_path(root)).unwrap_or(serde_json::Value::Null);
    let passphrase_hex = payload
        .get("passphrase")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_lowercase();
    if passphrase_hex.len() == 64
        && payload
            .get("transaction_id")
            .and_then(|value| value.as_str())
            .map(|value| value != state.transaction_id)
            .unwrap_or(true)
    {
        set_phase(root, &mut state, "restoring");
        return Err(after_capture_failure(
            &app,
            root,
            CaptureError::new(
                "capture_transaction_mismatch",
                "捕获结果不属于当前事务，已丢弃且未保存任何密钥。",
            ),
            &mut state,
        ));
    }

    if passphrase_hex.len() != 64 {
        let detail = match &run_result {
            Ok(text) => collapse(text, 500),
            Err(error) => error.to_string(),
        };
        let lower = detail.to_lowercase();
        let error = if lower.contains("attach failed") || lower.contains("not allowed to attach") {
            CaptureError::new(
                "lldb_attach_failed",
                "LLDB 无法附加临时调试微信，请确认管理员授权已完成。",
            )
        } else if matches!(
            run_result.as_ref().err().map(|error| error.code.as_str()),
            Some("administrator_timeout") | Some("capture_timeout")
        ) {
            CaptureError::new(
                "capture_timeout",
                "等待登录捕获超时，已结束监测并恢复微信。请重新开始，并在「监测已就绪」后再登录。",
            )
        } else if let Err(error) = run_result {
            error
        } else if lower.contains("wedata_key_monitor_ready 0") {
            CaptureError::new(
                "capture_breakpoints_unavailable",
                "监测没有找到可用的密钥断点，已结束并恢复微信。",
            )
        } else {
            CaptureError::new(
                "passphrase_not_captured",
                "没有捕获到口令。请在监测就绪后，于微信里退出账号并重新登录同一账号后再试。",
            )
        };
        set_phase(root, &mut state, "restoring");
        return Err(after_capture_failure(&app, root, error, &mut state));
    }

    // Independent Rust-side validation against the same databases; the LLDB script's result is
    // never trusted on its own.
    let key = hex::decode(&passphrase_hex)
        .map_err(|_| CaptureError::new("invalid_passphrase", "捕获到的口令格式无效。"))?;
    set_phase(root, &mut state, "validating");
    let key_mode = match validate_candidate(&key, probes) {
        Some(mode) => mode,
        None => {
            set_phase(root, &mut state, "restoring");
            return Err(after_capture_failure(
                &app,
                root,
                CaptureError::new(
                    "passphrase_database_mismatch",
                    "已捕获登录时的口令，但无法通过消息库与会话库校验。请确认登录的是同一账号。",
                ),
                &mut state,
            ));
        }
    };

    let saved = match save_passphrase_file(&passphrase_hex) {
        Ok(path) => path,
        Err(error) => {
            set_phase(root, &mut state, "restoring");
            return Err(after_capture_failure(&app, root, error, &mut state));
        }
    };

    set_phase(root, &mut state, "restoring");
    let restored = match restore(&app, root) {
        Ok(result) => result.restored,
        Err(error) => {
            return Err(CaptureError::new(
                "official_restore_failed",
                format!("已捕获并校验密钥，但自动恢复腾讯原版微信失败：{error}。恢复资产已保留。"),
            )
            .modified())
        }
    };

    Ok(CaptureOutcome {
        passphrase_hex,
        key_mode,
        validated_roles: vec!["message".to_string(), "session".to_string()],
        saved_path: saved.to_string_lossy().to_string(),
        restored,
        source: payload
            .get("source")
            .and_then(|value| value.as_str())
            .unwrap_or("pbkdf")
            .to_string(),
    })
}

fn after_capture_failure(
    app: &Path,
    root: &Path,
    error: CaptureError,
    state: &mut CaptureState,
) -> CaptureError {
    state.recovery_error_code = Some(error.code.clone());
    let _ = write_state(root, state);
    match restore(app, root) {
        Ok(_) => error,
        Err(restore_error) => {
            if matches!(
                restore_error.code.as_str(),
                "external_install_conflict"
                    | "in_place_debug_identity_unknown"
                    | "official_restore_staging_conflict"
                    | "official_restore_staging_missing"
            ) {
                restore_error
            } else {
                CaptureError::new(
                    "official_restore_failed",
                    format!("{} 另外，自动恢复腾讯原版微信失败：{restore_error}", error.message),
                )
                .modified()
            }
        }
    }
}

/* ── restore ──────────────────────────────────────────────────────────────── */

#[derive(Debug, Clone, Serialize)]
pub struct RestoreResult {
    pub restored: bool,
    pub version: String,
    pub build: String,
    pub cdhash: String,
    pub verified: bool,
}

fn safe_backup_from_state(state: &CaptureState, backup_root: &Path) -> CaptureResult<PathBuf> {
    let candidate = PathBuf::from(&state.backup_path);
    let root = fs::canonicalize(backup_root).unwrap_or_else(|_| backup_root.to_path_buf());
    let parent = candidate.parent().map(|value| {
        fs::canonicalize(value).unwrap_or_else(|_| value.to_path_buf())
    });
    let name = candidate.file_name().unwrap_or_default().to_string_lossy().to_string();
    let safe = parent.as_deref() == Some(root.as_path())
        && candidate.extension().map(|ext| ext == "zip").unwrap_or(false)
        && name.starts_with("WeChat-");
    if !safe {
        return Err(CaptureError::new(
            "official_backup_path_unsafe",
            "恢复状态中的备份路径不在当前备份目录，已拒绝覆盖微信。",
        )
        .modified());
    }
    Ok(candidate)
}

/// Restore the untouched Tencent bundle, verifying identity before and after the swap. Any
/// mismatch keeps the recovery assets and refuses to overwrite.
pub fn restore(app: &Path, root: &Path) -> CaptureResult<RestoreResult> {
    let _lock = acquire_lock(root)?;
    restore_locked(app, root)
}

/// Same as [`restore`], but for callers that already hold the installation lock.
fn restore_locked(app: &Path, root: &Path) -> CaptureResult<RestoreResult> {
    let app = normalize_app(Some(app))?;
    let Some(state) = read_state(root) else {
        let signature = inspect_signature(&app, true)?;
        if !signature.is_tencent_official() {
            return Err(CaptureError::new(
                "in_place_recovery_state_missing",
                "微信当前不是腾讯原签名，且没有可信恢复状态；为避免覆盖错误应用，已停止自动恢复。",
            )
            .modified());
        }
        return Ok(RestoreResult {
            restored: false,
            version: bundle_version(&app)?.short,
            build: bundle_version(&app)?.build,
            cdhash: signature.cdhash,
            verified: true,
        });
    };
    let expected_version = state.version();
    let expected_cdhash = state.official_cdhash.to_lowercase();
    if expected_cdhash.is_empty() {
        return Err(CaptureError::new(
            "in_place_capture_state_invalid",
            "恢复状态缺少原版构建身份，已保留并停止操作。",
        ));
    }
    let backup = safe_backup_from_state(&state, Path::new(&state.backup_root))?;

    let result = (|| -> CaptureResult<RestoreResult> {
        let current = inspect_signature(&app, true)?;
        if current.is_tencent_official() {
            if !official_identity_matches(&app, &current, Some(&expected_version), Some(&expected_cdhash)) {
                return Err(CaptureError::new(
                    "external_install_conflict",
                    "当前腾讯官方微信与本次备份不是同一构建；已保留应用、备份和恢复状态，未自动覆盖。",
                ));
            }
            let staged = restore_slot(&app);
            if debug_identity_matches(&staged, state.debug_identity.as_ref()) {
                let _ = fs::remove_dir_all(&staged);
            }
            return Ok(RestoreResult {
                restored: false,
                version: state.version.clone(),
                build: state.build.clone(),
                cdhash: state.official_cdhash.clone(),
                verified: true,
            });
        }

        if !debug_identity_matches(&app, state.debug_identity.as_ref()) {
            return Err(CaptureError::new(
                "in_place_debug_identity_unknown",
                "当前非官方微信无法确认为本次生成的调试副本；已保留恢复资产，拒绝自动覆盖。",
            )
            .modified());
        }

        // Prefer the same-volume clone; fall back to the verified ZIP if it is missing.
        let staged = restore_slot(&app);
        let mut staged_ok = false;
        if staged.symlink_metadata().is_ok() {
            if staged.symlink_metadata().map(|meta| meta.file_type().is_symlink()).unwrap_or(true)
                || !staged.is_dir()
            {
                return Err(CaptureError::new(
                    "local_restore_path_unsafe",
                    "原版微信保护路径类型异常，已停止自动覆盖。",
                )
                .modified());
            }
            staged_ok = inspect_signature(&staged, true)
                .map(|signature| {
                    official_identity_matches(&staged, &signature, Some(&expected_version), Some(&expected_cdhash))
                })
                .unwrap_or(false);
            if !staged_ok {
                if debug_identity_matches(&staged, state.debug_identity.as_ref()) {
                    let _ = fs::remove_dir_all(&staged);
                } else {
                    return Err(CaptureError::new(
                        "official_restore_staging_conflict",
                        "原版保护位置存在身份不匹配的应用；已保留，未覆盖。",
                    )
                    .modified());
                }
            }
        }
        if !staged_ok {
            let backup = verify_backup(&backup, Some(&expected_version), root)?;
            if !backup.0.cdhash.eq_ignore_ascii_case(&expected_cdhash) {
                return Err(CaptureError::new(
                    "official_backup_identity_mismatch",
                    "原版备份身份与恢复记录不一致，已停止恢复。",
                ));
            }
            let extract_root = unique_dir(root, "restore")?;
            let (ok, _, stderr) = run(&[
                "/usr/bin/ditto",
                "-x",
                "-k",
                &state.backup_path,
                &extract_root.to_string_lossy(),
            ])?;
            if !ok {
                let _ = fs::remove_dir_all(&extract_root);
                return Err(CaptureError::new(
                    "official_backup_untrusted",
                    format!("无法解压原版备份：{}", collapse(&stderr, 400)),
                ));
            }
            let extracted = extract_root.join("WeChat.app");
            // `extract_root` and `/Applications` may be on different volumes, so the app is
            // copied with `ditto` (the same tool the backup was made with) rather than renamed.
            let (copied, _, stderr) = run(&[
                "/usr/bin/ditto",
                &extracted.to_string_lossy(),
                &staged.to_string_lossy(),
            ])?;
            let _ = fs::remove_dir_all(&extract_root);
            if !copied {
                let _ = fs::remove_dir_all(&staged);
                return Err(CaptureError::new(
                    "official_restore_staging_invalid",
                    format!("无法暂存原版微信：{}", collapse(&stderr, 400)),
                ));
            }
            let signature = inspect_signature(&staged, true)?;
            if !official_identity_matches(&staged, &signature, Some(&expected_version), Some(&expected_cdhash)) {
                return Err(CaptureError::new(
                    "official_restore_staging_invalid",
                    "恢复暂存的腾讯微信签名校验失败，已停止覆盖。",
                )
                .modified());
            }
        }

        quit_wechat(&app, true)?;
        if !debug_identity_matches(&app, state.debug_identity.as_ref()) {
            return Err(CaptureError::new(
                "external_install_conflict",
                "关闭微信期间安装已变化，已停止恢复。",
            ));
        }
        if !official_identity_matches(
            &staged,
            &inspect_signature(&staged, true)?,
            Some(&expected_version),
            Some(&expected_cdhash),
        ) {
            return Err(CaptureError::new(
                "official_restore_staging_invalid",
                "恢复前原版保护身份已变化，已停止恢复。",
            )
            .modified());
        }
        atomic_swap(&app, &staged)?;
        let installed = inspect_signature(&app, true)?;
        if !official_identity_matches(&app, &installed, Some(&expected_version), Some(&expected_cdhash)) {
            // Only roll back when the destination is still ours.
            if official_identity_matches(&app, &installed, None, None) {
                let _ = atomic_swap(&app, &staged);
            }
            return Err(CaptureError::new(
                "official_restore_verify_failed",
                "恢复后的腾讯微信签名校验失败；恢复资产已保留。",
            )
            .modified());
        }
        if debug_identity_matches(&staged, state.debug_identity.as_ref()) {
            let _ = fs::remove_dir_all(&staged);
        }
        Ok(RestoreResult {
            restored: true,
            version: state.version.clone(),
            build: state.build.clone(),
            cdhash: installed.cdhash,
            verified: true,
        })
    })();

    match result {
        Ok(value) => {
            remove_state(root);
            let _ = fs::remove_file(result_path(root));
            Ok(value)
        }
        Err(error) => {
            let mut state = state;
            if error.code == "external_install_conflict" {
                state.stage = "external_install_conflict".to_string();
                state.recovery_error_code = Some(error.code.clone());
                let _ = write_state(root, &state);
            } else if matches!(
                error.code.as_str(),
                "in_place_debug_identity_unknown" | "official_restore_staging_conflict"
            ) {
                state.stage = "recovery_blocked".to_string();
                state.recovery_error_code = Some(error.code.clone());
                let _ = write_state(root, &state);
            }
            Err(error)
        }
    }
}

/// Cancel: stop a running monitor if possible, then restore the official bundle.
pub fn cancel(app: &Path, root: &Path) -> CaptureResult<RestoreResult> {
    terminate_stale_lldb(root);
    restore(app, root)
}

/// Best-effort cleanup of an LLDB preflight/capture process left behind by a crashed app.
fn terminate_stale_lldb(root: &Path) {
    let marker = root.to_string_lossy().to_string();
    let Ok((true, stdout, _)) = run(&["/bin/ps", "-axo", "pid=,command="]) else {
        return;
    };
    let mut pids: Vec<String> = Vec::new();
    for line in stdout.lines() {
        if !line.contains(&marker) {
            continue;
        }
        if !line.contains("lldb") && !line.contains("wedata-lldb") {
            continue;
        }
        if let Some(pid) = line.split_whitespace().next() {
            if pid.chars().all(|ch| ch.is_ascii_digit()) {
                pids.push(pid.to_string());
            }
        }
    }
    if pids.is_empty() {
        return;
    }
    for pid in &pids {
        let _ = run(&["/bin/kill", "-TERM", pid]);
    }
    // The monitor runs as root, so terminating it needs one authorization prompt; ignore
    // failures because the shell watchdog also stops it at the end of its lease.
    let _ = run_as_administrator(
        &format!("/bin/kill -TERM {} 2>/dev/null || true", pids.join(" ")),
        Duration::from_secs(30),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_matching_full_length_result_can_be_resumed() {
        let root = std::env::temp_dir().join(format!("wxc-resume-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let state = CaptureState {
            schema_version: STATE_SCHEMA,
            stage: "capturing".to_string(),
            created_at: 0,
            transaction_id: "txn".to_string(),
            owner_pid: 1,
            wechat_app_path: String::new(),
            backup_path: String::new(),
            backup_root: String::new(),
            backup_created: true,
            version: "4.1.15".to_string(),
            build: "270135".to_string(),
            official_cdhash: String::new(),
            entitlement_mode: EntitlementMode::Preserved,
            debug_identity: None,
            debug_pid: None,
            capture_phase: Some("monitoring".to_string()),
            capture_backend: None,
            preflight_pid: None,
            recovery_error_code: None,
        };

        // Truncated passphrase: the monitor had not finished, so this is not resumable.
        fs::write(result_path(&root), r#"{"passphrase":"aa","transaction_id":"txn"}"#).unwrap();
        assert!(resumable_result(&root, &state).is_none());

        // Full passphrase from a different transaction must never be adopted.
        fs::write(
            result_path(&root),
            format!(r#"{{"passphrase":"{}","transaction_id":"other"}}"#, "ab".repeat(32)),
        )
        .unwrap();
        assert!(resumable_result(&root, &state).is_none());

        fs::write(
            result_path(&root),
            format!(r#"{{"passphrase":"{}","transaction_id":"txn"}}"#, "ab".repeat(32)),
        )
        .unwrap();
        assert!(resumable_result(&root, &state).is_some());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parses_tencent_signature() {
        let output = "Executable=/Applications/WeChat.app/Contents/MacOS/WeChat\n\
                      Identifier=com.tencent.xinWeChat\n\
                      TeamIdentifier=5A4RE8SF68\n\
                      CDHash=ABCDEF0123456789\n\
                      flags=0x10000(runtime)\n";
        let signature = parse_codesign_output(output);
        assert!(signature.is_tencent_official());
        assert!(signature.hardened_runtime);
        assert_eq!(signature.cdhash, "abcdef0123456789");
    }

    #[test]
    fn ad_hoc_signature_is_not_official_but_is_debuggable() {
        let output = "Identifier=com.tencent.xinWeChat\n\
                      TeamIdentifier=not set\n\
                      CDHash=1111\n\
                      Signature=adhoc\n\
                      flags=0x2(adhoc)\n";
        let signature = parse_codesign_output(output);
        assert!(signature.team_identifier.is_empty());
        assert!(!signature.is_tencent_official());
        assert!(signature.is_debuggable_ad_hoc());
    }

    #[test]
    fn hardened_adhoc_signature_cannot_be_debugged() {
        let output = "TeamIdentifier=not set\nSignature=adhoc\nflags=0x10002(adhoc,runtime)\n";
        let signature = parse_codesign_output(output);
        assert!(!signature.is_debuggable_ad_hoc());
    }

    #[test]
    fn versions_are_sanitized_for_file_names() {
        assert_eq!(sanitize_version("4.1.7 beta/1"), "4.1.7_beta_1");
        assert_eq!(sanitize_version(""), "unknown");
    }

    #[test]
    fn backup_names_include_version_and_build() {
        let version = BundleVersion {
            short: "4.1.15".into(),
            build: "270135".into(),
        };
        let path = backup_path(Path::new("/tmp/backups"), &version);
        assert!(path.ends_with("WeChat-4.1.15-270135-original.zip"));
    }

    #[test]
    fn pgrep_patterns_escape_regex_metacharacters() {
        assert_eq!(
            regex_escape("/Applications/WeChat.app/Contents/MacOS/WeChat"),
            "/Applications/WeChat\\.app/Contents/MacOS/WeChat"
        );
    }

    #[test]
    fn state_round_trips_and_keeps_identity() {
        let root = std::env::temp_dir().join(format!("wxcap-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let state = CaptureState {
            schema_version: STATE_SCHEMA,
            stage: "preflight_passed".into(),
            created_at: 1,
            transaction_id: "abc".into(),
            owner_pid: 42,
            wechat_app_path: WECHAT_APP.into(),
            backup_path: "/tmp/WeChat-4.1.15-270135-original.zip".into(),
            backup_root: "/tmp".into(),
            backup_created: true,
            version: "4.1.15".into(),
            build: "270135".into(),
            official_cdhash: "aa".into(),
            entitlement_mode: EntitlementMode::Sanitized,
            debug_identity: Some(BundleIdentity {
                version: "4.1.15".into(),
                build: "270135".into(),
                cdhash: "bb".into(),
                device: 1,
                inode: 2,
            }),
            debug_pid: Some(7),
            capture_phase: Some("monitoring".into()),
            capture_backend: Some("lldb".into()),
            preflight_pid: Some(7),
            recovery_error_code: None,
        };
        write_state(&root, &state).expect("write state");
        let loaded = read_state(&root).expect("read state");
        assert_eq!(loaded.version().short, "4.1.15");
        assert_eq!(loaded.debug_identity.expect("identity").inode, 2);
        remove_state(&root);
        assert!(read_state(&root).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn shell_quoting_survives_single_quotes() {
        assert_eq!(shell_quote_text("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_quote_text("it's"), r#"'it'\''s'"#);
    }

    fn wechat_entitlements() -> serde_json::Value {
        serde_json::json!({
            "com.apple.application-identifier": "5A4RE8SF68.com.tencent.xinWeChat",
            "com.apple.security.app-sandbox": true,
            "com.apple.security.application-groups": ["5A4RE8SF68.com.tencent.xinWeChat"],
            "com.apple.security.network.client": true,
            "com.apple.security.device.camera": true,
            "com.apple.security.temporary-exception.sbpl": ["(allow network-outbound)"],
        })
    }

    #[test]
    fn sanitizing_keeps_the_sandbox_and_drops_every_team_bound_claim() {
        let (sanitized, dropped) = sanitize_entitlements(&wechat_entitlements());
        let keys: Vec<&str> = sanitized
            .as_object()
            .expect("dictionary")
            .keys()
            .map(String::as_str)
            .collect();
        assert!(keys.contains(&"com.apple.security.app-sandbox"));
        assert!(keys.contains(&"com.apple.security.network.client"));
        assert!(keys.contains(&"com.apple.security.device.camera"));
        assert!(!keys.contains(&"com.apple.application-identifier"));
        assert!(!keys.contains(&"com.apple.security.application-groups"));
        assert!(!keys.iter().any(|key| key.starts_with(RESTRICTED_ENTITLEMENT_PREFIX)));
        // Sorted, so the persisted report never reorders between runs.
        assert_eq!(
            dropped,
            vec![
                "com.apple.application-identifier".to_string(),
                "com.apple.security.application-groups".to_string(),
                "com.apple.security.temporary-exception.sbpl".to_string(),
            ]
        );
    }

    #[test]
    fn preserved_signatures_need_the_tencent_claims_but_sanitized_ones_must_not_have_them() {
        let preserved = r#"<key>com.apple.application-identifier</key><string>5A4RE8SF68.com.tencent.xinWeChat</string>
            <key>com.apple.security.app-sandbox</key><true/>
            <key>com.apple.security.application-groups</key><array/>
            <key>com.apple.security.network.client</key><true/>"#;
        assert!(entitlements_text_is_compatible(preserved, EntitlementMode::Preserved));
        assert!(!entitlements_text_is_compatible(preserved, EntitlementMode::Sanitized));

        let sanitized = r#"<key>com.apple.security.app-sandbox</key><true/>
            <key>com.apple.security.network.client</key><true/>"#;
        assert!(entitlements_text_is_compatible(sanitized, EntitlementMode::Sanitized));
        assert!(!entitlements_text_is_compatible(sanitized, EntitlementMode::Preserved));

        // Dropping the sandbox, or forgetting to strip one restricted claim, must still fail.
        let unsandboxed = r#"<key>com.apple.security.network.client</key><true/>"#;
        assert!(!entitlements_text_is_compatible(unsandboxed, EntitlementMode::Sanitized));
        let leaked = r#"<key>com.apple.security.app-sandbox</key><true/>
            <key>com.apple.security.network.client</key><true/>
            <key>com.apple.security.application-groups</key><array/>"#;
        assert!(!entitlements_text_is_compatible(leaked, EntitlementMode::Sanitized));
    }

    #[test]
    fn launchd_153_is_recognised_as_a_signature_refusal() {
        // The exact wording macOS 15.1 printed for the resigned WeChat.
        let error = "The application cannot be opened for an unexpected reason, error=Error Domain=\
                     RBSRequestErrorDomain Code=5 \"Launch failed.\" UserInfo={NSLocalizedFailureReason=\
                     Launch failed., NSUnderlyingError=0x6000e9b340fe {Error Domain=NSPOSIXErrorDomain \
                     Code=153 \"Unknown error: 153\" UserInfo={NSLocalizedDescription=Launchd job spawn failed}}}";
        assert!(open_error_is_amfi_rejection(error));
        assert!(!open_error_is_amfi_rejection(
            "The application cannot be opened because it is from an unidentified developer."
        ));
        assert!(!open_error_is_amfi_rejection(""));
    }

    #[test]
    fn signature_refusal_does_not_send_the_user_to_gatekeeper_settings() {
        let mut diagnosis = diagnosis_with(Some(false), true, 0);
        diagnosis.amfi_rejected = true;
        diagnosis.hint = launch_hint(&diagnosis);
        assert!(diagnosis.hint.contains("取消并恢复微信"));
        assert!(!diagnosis.hint.contains("隐私与安全性"));
    }

    #[test]
    fn legacy_state_without_an_entitlement_mode_is_read_as_preserved() {
        let root = std::env::temp_dir().join(format!("wxcap-legacy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        // Shape written by the previous release, before the macOS 15 fallback existed.
        let body = r#"{"schema_version":2,"stage":"resigned","created_at":1,"transaction_id":"abc",
            "owner_pid":1,"wechat_app_path":"/Applications/WeChat.app","backup_path":"/tmp/b.zip",
            "backup_root":"/tmp","backup_created":true,"version":"4.1.15","build":"270135",
            "official_cdhash":"aa","debug_identity":null,"debug_pid":null,"capture_phase":null,
            "capture_backend":null,"preflight_pid":null,"recovery_error_code":null}"#;
        fs::write(state_path(&root), body).unwrap();
        let loaded = read_state(&root).expect("legacy state must stay readable");
        assert_eq!(loaded.entitlement_mode, EntitlementMode::Preserved);
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires the separately prepared WeChat trial bundle"]
    fn deep_signed_trial_is_compatible() {
        let path = std::env::var_os("WECHAT_TRIAL_APP").expect("set WECHAT_TRIAL_APP");
        let app = Path::new(&path);
        assert!(in_place_signature_is_compatible(app, EntitlementMode::Deep));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn amfi_probe_bundle_uses_the_identifier_that_gets_cleaned_up() {
        assert!(PROBE_INFO_PLIST.contains(&format!("<string>{PROBE_BUNDLE_ID}</string>")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn amfi_probe_reports_a_verdict_instead_of_an_unknown() {
        // The probe must actually build, sign and start its bundle on the host, otherwise every
        // prepare would refuse to run. An unsandboxed, unrestricted dictionary always starts.
        let scratch = Scratch::new("probe-test").expect("scratch");
        let plist = plist_from_json(
            &serde_json::json!({"com.apple.security.get-task-allow": true}),
            &scratch,
            "probe",
        )
        .expect("plist");
        assert_eq!(amfi_probe(&plist, &scratch, "probe"), AmfiVerdict::Accepted);
    }

    /// Support check for the host this runs on:
    /// `cargo test --lib installed_wechat_entitlement_plan -- --ignored --nocapture`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires the official /Applications/WeChat.app on this Mac"]
    fn installed_wechat_entitlement_plan() {
        let app = Path::new(WECHAT_APP);
        let entitlements = read_entitlements(app).expect("entitlements");
        let (sanitized, dropped) = sanitize_entitlements(&entitlements);
        println!("preserved accepted: {:?}", amfi_accepts_entitlements(&entitlements));
        println!("sanitized accepted: {:?}", amfi_accepts_entitlements(&sanitized));
        println!("dropped: {dropped:?}");
        let plan = plan_entitlements(app).expect("plan");
        println!("chosen mode: {:?}", plan.mode);
        assert!(app.is_dir());
    }

    #[test]
    fn candidate_validation_requires_every_probe() {
        // Random pages cannot be unlocked by this passphrase, so validation must fail.
        let probe = Probe {
            role: "message".into(),
            path: "/tmp/msg.db".into(),
            page1_hex: "11".repeat(4096),
        };
        assert!(validate_candidate(&[0u8; 32], std::slice::from_ref(&probe)).is_none());
        let short = Probe {
            role: "message".into(),
            path: "/tmp/msg.db".into(),
            page1_hex: "11".repeat(10),
        };
        assert!(validate_candidate(&[0u8; 32], std::slice::from_ref(&short)).is_none());
    }

    #[test]
    fn lldb_command_quotes_the_script_and_keeps_a_watchdog() {
        let command = lldb_command(Path::new("/tmp/with space/capture.lldb"), 240);
        assert!(command.starts_with("/bin/bash -c "));
        assert!(command.contains("--batch --source"));
        assert!(command.contains(" 240 "));
        assert!(command.contains("capture.lldb"));
        // The script path is single-quoted inside the shell body.
        assert!(command.contains("'/tmp/with space/capture.lldb'"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn lldb_wrapper_executes_command_file_in_batch() {
        if !lldb_available() { return; }
        let scratch = Scratch::new("lldb-batch-test").unwrap();
        let script = scratch.path().join("commands with spaces.lldb");
        fs::write(&script, "script print('WEDATA_BATCH_TEST_OK')\n").unwrap();
        let output = Command::new("/bin/bash").arg("-c")
            .arg(lldb_command(&script, 10)).output().unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.lines().any(|line| line.trim() == "WEDATA_BATCH_TEST_OK"), "{text}");
        assert!(text.contains("WEDATA_LLDB_EXIT=0"), "{text}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn lldb_wrapper_finishes_promptly_after_debugger_exits() {
        if !lldb_available() { return; }
        let scratch = Scratch::new("lldb-exit-test").unwrap();
        let script = scratch.path().join("commands.lldb");
        fs::write(&script, "script import os; os._exit(0)\n").unwrap();
        let command = lldb_command(&script, 6);
        let started = Instant::now();
        let output = Command::new("/bin/bash").arg("-c").arg(command).output().unwrap();
        assert!(output.status.success());
        assert!(started.elapsed() < Duration::from_secs(4), "debugger wrapper waited for watchdog: {:?}", started.elapsed());
    }

    #[test]
    fn lldb_wrapper_does_not_wait_for_inherited_output_from_a_child() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("lldb-inherited-output-test").unwrap();
        let script = scratch.path().join("commands.lldb");
        let executable = scratch.path().join("fake-lldb");
        fs::write(&script, "\n").unwrap();
        fs::write(&executable, "#!/bin/sh\n/bin/sleep 6 &\nexit 0\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let command = lldb_command_with_executable(&script, 8, &executable);
        let started = Instant::now();
        let output = Command::new("/bin/bash").arg("-c").arg(command).output().unwrap();
        assert!(output.status.success());
        assert!(started.elapsed() < Duration::from_secs(4), "inherited output kept wrapper open: {:?}", started.elapsed());
    }

    #[test]
    fn lldb_wrapper_still_stops_a_stuck_debugger() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("lldb-timeout-test").unwrap();
        let script = scratch.path().join("commands.lldb");
        let executable = scratch.path().join("fake-lldb");
        fs::write(&script, "\n").unwrap();
        fs::write(&executable, "#!/bin/sh\nexec /bin/sleep 15\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let started = Instant::now();
        let output = Command::new("/bin/bash").arg("-c")
            .arg(lldb_command_with_executable(&script, 2, &executable)).output().unwrap();
        assert!(output.status.success());
        assert!(started.elapsed() < Duration::from_secs(8), "watchdog did not stop debugger");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("WEDATA_LLDB_EXIT=0"));
    }

    #[test]
    fn captured_result_stops_debugger_only_after_matching_target_exits() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("lldb-captured-test").unwrap();
        let script = scratch.path().join("commands.lldb");
        let executable = scratch.path().join("fake-lldb");
        let ready = scratch.path().join("ready.json");
        fs::write(&script, "\n").unwrap();
        fs::write(&executable, "#!/bin/sh\nexec /bin/sleep 30\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let mut target = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let command = lldb_command_inner(&script, 12, &executable, Some((&ready, "expected", target.id() as i64)));
        let mut wrapper = Command::new("/bin/bash").arg("-c").arg(command).stdout(Stdio::null()).spawn().unwrap();
        fs::write(&ready, r#"{"status":"captured","transaction_id":"expected"}"#).unwrap();
        thread::sleep(Duration::from_millis(700));
        let alive_before_exit = wrapper.try_wait().unwrap().is_none();
        fs::write(&ready, r#"{"status":"captured","transaction_id":"other"}"#).unwrap();
        target.kill().unwrap();
        target.wait().unwrap();
        thread::sleep(Duration::from_millis(700));
        let alive_for_wrong_transaction = wrapper.try_wait().unwrap().is_none();
        fs::write(&ready, r#"{"status":"captured","transaction_id":"expected"}"#).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while wrapper.try_wait().unwrap().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(100));
        }
        let stopped = wrapper.try_wait().unwrap().is_some();
        if !stopped { let _ = wrapper.kill(); let _ = wrapper.wait(); }
        assert!(alive_before_exit, "debugger stopped while target was still alive");
        assert!(alive_for_wrong_transaction, "debugger stopped for another transaction");
        assert!(stopped, "debugger did not stop after target exited");
    }

    #[test]
    fn lldb_import_path_is_quoted_for_spaces() {
        // `command script import` splits on whitespace; the capture directory contains
        // "Application Support", so an unquoted path makes the whole capture silently fail.
        assert_eq!(
            lldb_quote(Path::new("/Users/me/Library/Application Support/app/capture_callback.py")),
            "\"/Users/me/Library/Application Support/app/capture_callback.py\""
        );
        assert_eq!(lldb_quote(Path::new("/tmp/a\"b.py")), "\"/tmp/a\\\"b.py\"");
    }

    #[test]
    fn python_pages_literal_keeps_role_names() {
        let literal = python_pages(&[
            Probe {
                role: "message".into(),
                path: "/a".into(),
                page1_hex: "aa".repeat(16),
            },
            Probe {
                role: "session".into(),
                path: "/b".into(),
                page1_hex: "bb".repeat(16),
            },
        ]);
        assert!(literal.contains("\"message\": bytes.fromhex("));
        assert!(literal.contains("\"session\": bytes.fromhex("));
    }

    fn diagnosis_with(accepted: Option<bool>, quarantined: bool, processes: usize) -> LaunchDiagnosis {
        let mut diagnosis = LaunchDiagnosis {
            quarantined,
            resigned: true,
            gatekeeper_accepted: accepted,
            gatekeeper_reason: String::new(),
            open_error: String::new(),
            amfi_rejected: false,
            processes,
            hint: String::new(),
        };
        diagnosis.hint = launch_hint(&diagnosis);
        diagnosis
    }

    #[test]
    fn rejected_gatekeeper_assessment_points_to_open_anyway() {
        // macOS 15.1 can block a Finder launch without a dialog, so the hint has to name the exact
        // System Settings path instead of telling the user to "check any security prompt".
        let hint = diagnosis_with(Some(false), true, 0).hint;
        assert!(hint.contains("仍要打开"));
        assert!(hint.contains("隐私与安全性"));
        assert!(hint.contains("没有右键"));
    }

    #[test]
    fn unevaluable_quarantined_bundle_still_gets_the_gatekeeper_steps() {
        let hint = diagnosis_with(None, true, 0).hint;
        assert!(hint.contains("仍要打开"));
    }

    #[test]
    fn accepted_launch_without_a_process_falls_back_to_crash_logs() {
        let hint = diagnosis_with(Some(true), false, 0).hint;
        assert!(!hint.contains("仍要打开"));
        assert!(hint.contains("DiagnosticReports"));
    }

    #[test]
    fn running_processes_are_reported_as_a_stale_launch() {
        let hint = diagnosis_with(Some(false), true, 2).hint;
        assert!(hint.contains("完全退出"));
        assert!(!hint.contains("仍要打开"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn quarantine_flag_is_detected_from_the_extended_attribute() {
        let dir = std::env::temp_dir().join(format!("wxc-quarantine-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(!is_quarantined(&dir));
        let set = Command::new("/usr/bin/xattr")
            .args(["-w", "com.apple.quarantine", "0081;00000000;probe;"])
            .arg(&dir)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if set {
            assert!(is_quarantined(&dir));
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
