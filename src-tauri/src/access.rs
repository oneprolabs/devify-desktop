//! Agent access kit.
//!
//! The desktop app serves the read-only `aimychats-cli` CLI and the unified `SKILL.md` document from its
//! loopback listener. The Agent 集成 page turns that into one prompt: the user pastes it
//! into an agent, and the agent downloads both artifacts into the directories the user
//! chose. No repository checkout, MCP client, or manual `cargo build` is involved.

use crate::agent;
pub mod install;
pub mod prompt;
mod discovery;
use serde::Serialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const CLI_NAME: &str = "aimychats-cli";
pub const CLI_DOWNLOAD: &str = "/agent/aimychats-cli";
pub const LEGACY_CLI_NAME: &str = "wxq";
pub const LEGACY_CLI_DOWNLOAD: &str = "/agent/wxq";

#[derive(Clone, Copy, Debug)]
pub struct SkillDoc {
    pub name: &'static str,
    pub legacy_names: &'static [&'static str],
    pub content: &'static str,
}

pub const SKILLS: &[SkillDoc] = &[SkillDoc {
    name: crate::build_flavor::SKILL_NAME,
    legacy_names: &["wechat-chat-records", "wechat-daily-summary", "wechat-work-exploration",
        "aimychats-chat-records", "aimychats-daily-summary", "aimychats-work-exploration"],
    content: include_str!(concat!(env!("OUT_DIR"), "/agent-skill.md")),
}];

pub fn skill_by_name(name: &str) -> Option<&'static SkillDoc> {
    SKILLS.iter().find(|skill| skill.name == name || skill.legacy_names.contains(&name))
}

#[derive(Serialize, Clone, Debug)]
pub struct AccessTarget {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub command: Option<String>,
    pub desktop_app: Option<String>,
    pub config_dir: Option<String>,
    pub skills_dir: String,
    pub installed: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct SkillAsset {
    pub name: String,
    pub description: String,
    pub download_url: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct WxqAsset {
    pub path: String,
    pub size: u64,
    pub download_url: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct AccessStatus {
    pub running: bool,
    pub base_url: String,
    pub token_required: bool,
    /// Why the listener is not running when `running` is false and it is not an error.
    pub reason: Option<String>,
    pub wxq: Option<WxqAsset>,
    pub skills: Vec<SkillAsset>,
    pub targets: Vec<AccessTarget>,
    pub default_target: String,
    pub default_bin_dir: String,
    pub default_skills_dir: String,
    pub manifest_url: String,
    pub install_url: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct PromptOut {
    pub prompt: String,
    pub bin_dir: String,
    pub skills_dir: String,
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

fn default_bin_dir() -> PathBuf {
    home_dir().map(|home| home.join(".local/bin")).unwrap_or_else(|| PathBuf::from("/usr/local/bin"))
}

fn default_skills_dir() -> PathBuf {
    home_dir().map(|home| home.join(".agents/skills")).unwrap_or_else(|| PathBuf::from(".agents/skills"))
}

/// User-level skills directory each host scans. Only verified paths are listed; anything
/// else is handled by the "custom" target where the user types the directory.
fn skills_rel(id: &str) -> Option<&'static str> {
    match id {
        "claude-code" => Some(".claude/skills"),
        "codex" => Some(".codex/skills"),
        "chatgpt-desktop" => Some(".codex/skills"),
        "opencode" => Some(".config/opencode/skills"),
        "gemini-cli" => Some(".gemini/skills"),
        "qwen-code" => Some(".qwen/skills"),
        "cursor-agent" => Some(".cursor/skills"),
        "workbuddy" => Some(".workbuddy-ai/skills"),
        "deepseek-harness" => Some(".dsh/skills"),
        _ => None,
    }
}

/// Parse the one-line description out of a skill's YAML frontmatter.
pub fn skill_description(content: &str) -> String {
    for line in content.lines() {
        let Some(value) = line.trim().strip_prefix("description:") else {
            continue;
        };
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|value| value.strip_suffix('\'')))
            .unwrap_or(value);
        return value.to_string();
    }
    String::new()
}

fn installed_skills(skills_dir: &Path) -> Vec<String> {
    SKILLS
        .iter()
        .filter(|skill| skills_dir.join(skill.name).join("SKILL.md").is_file())
        .map(|skill| skill.name.to_string())
        .collect()
}

/// Every host the app knows how to install into, plus a custom entry.
pub fn targets() -> Vec<AccessTarget> {
    let home = home_dir();
    let commands = agent::detected_commands();
    let mut applications = vec![PathBuf::from("/Applications")];
    if let Some(home) = &home { applications.push(home.join("Applications")); }
    let dsh_home = env::var_os("DSH_HOME").map(PathBuf::from).filter(|path| path.is_absolute());
    discovery::targets(home.as_deref(), &commands, &applications, dsh_home.as_deref())
}

pub fn default_target(targets: &[AccessTarget], configured: &str) -> String {
    let configured = configured.trim();
    if !configured.is_empty() && targets.iter().any(|target| target.id == configured) {
        return configured.to_string();
    }
    targets
        .iter()
        .find(|target| target.available && target.id != "custom")
        .map(|target| target.id.clone())
        .unwrap_or_else(|| "custom".to_string())
}

pub fn target_by_id(targets: &[AccessTarget], id: &str) -> Option<AccessTarget> {
    targets.iter().find(|target| target.id == id).cloned()
}

/// Installing files does not require a detected CLI or desktop host. A custom
/// destination must be explicit, and never changes the selected summary provider.
pub fn installation_target(targets: &[AccessTarget], id: &str, directory: Option<&str>) -> Result<AccessTarget, String> {
    let mut target = target_by_id(targets, id).ok_or("不支持的智能体。")?;
    if id == "custom" && directory.is_none() {
        return Err("请先填写智能体的 Skills 目录。".into());
    }
    if let Some(directory) = directory {
        let path = expand_path(directory).ok_or("Skills 目录必须是绝对路径或以 ~/ 开头。")?;
        target.skills_dir = path.to_string_lossy().into();
    }
    Ok(target)
}

/// Expand `~`, `~/…` and absolute paths. Relative paths and control characters are
/// rejected so they cannot rewrite the generated prompt or shell script.
pub fn expand_path(input: &str) -> Option<PathBuf> {
    let value = input.trim();
    if value.is_empty() || value.contains('\n') || value.contains('\r') || value.contains('\0') {
        return None;
    }
    if value == "~" {
        return home_dir();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return home_dir().map(|home| home.join(rest));
    }
    let path = PathBuf::from(value);
    path.is_absolute().then_some(path)
}

/// Prefer the owning app's packaged resource, then its installed copy.
/// Never serve an unrelated executable from PATH.
pub fn resolve_cli(app_data: &Path) -> Option<PathBuf> {
    let bundled = env::current_exe().ok().and_then(|exe| {
        bundled_cli(exe.parent()?).into_iter().find(|path| agent::is_executable(path))
    });
    bundled.or_else(|| {
        let bin = app_data.join("bin");
        let path = bin.join(CLI_NAME);
        (fs::symlink_metadata(bin).is_ok_and(|meta| meta.is_dir())
            && fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file())
            && agent::is_executable(&path)).then_some(path)
    }).or_else(|| {
        if crate::build_flavor::DEVELOPMENT {
            return None;
        }
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        ["target/release", "target/debug"].into_iter().map(|dir| manifest.join(dir).join(CLI_NAME))
            .find(|path| agent::is_executable(path) && source_cli_matches_app(path))
    })
}

/// Shared Cargo output can be replaced by either app's build.
fn source_cli_matches_app(path: &Path) -> bool {
    std::process::Command::new(path).arg("build-info").output().ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok())
        .is_some_and(|info| info.get("identifier").and_then(|value| value.as_str()) == Some(crate::build_flavor::IDENTIFIER))
}

/// The packaged CLI is a plain resource (`bundle.resources`), so it is not signed as
/// an auxiliary executable of the app. macOS keeps resources under
/// `Contents/Resources`; other platforms place them next to the binary.
fn bundled_cli(bin_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let resources = if crate::build_flavor::DEVELOPMENT { "binaries-dev" } else { "binaries" };
    if let Some(contents) = bin_dir.parent() {
        dirs.push(contents.join("Resources").join(resources));
    }
    dirs.push(bin_dir.join("resources").join(resources));
    dirs.push(bin_dir.join(resources));
    dirs.into_iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flat_map(|entries| entries.flatten().map(|entry| entry.path()))
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name == CLI_NAME
                            || name
                                .strip_prefix(CLI_NAME)
                                .is_some_and(|suffix| suffix.starts_with('-'))
                    })
        })
        .collect()
}

/// Where the app can rebuild `aimychats-cli` when the current install did not ship it.
pub fn source_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    (dir.join("Cargo.toml").is_file()).then_some(dir)
}

/// Client-facing origin for the listener. A wildcard bind is reachable at loopback.
pub fn client_base(bind: &str) -> String {
    let (host, port) = bind.rsplit_once(':').unwrap_or((bind, "18765"));
    let host = host.trim_matches(['[', ']']);
    let host = match host {
        "0.0.0.0" | "" => "127.0.0.1",
        other => other,
    };
    if host.contains(':') {
        format!("http://[{host}]:{port}")
    } else {
        format!("http://{host}:{port}")
    }
}

pub fn status(
    app_data: &Path,
    configured_agent: &str,
    bind: &str,
    running: bool,
    reason: Option<String>,
) -> AccessStatus {
    let base_url = client_base(bind);
    let targets = targets();
    let default_id = default_target(&targets, configured_agent);
    let default_skills = target_by_id(&targets, &default_id)
        .map(|target| target.skills_dir)
        .unwrap_or_else(|| default_skills_dir().to_string_lossy().to_string());
    let wxq = resolve_cli(app_data).map(|path| WxqAsset {
        size: fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0),
        path: path.to_string_lossy().to_string(),
        download_url: format!("{base_url}{LEGACY_CLI_DOWNLOAD}"),
    });
    AccessStatus {
        running,
        base_url: base_url.clone(),
        token_required: true,
        reason,
        wxq,
        skills: SKILLS
            .iter()
            .map(|skill| SkillAsset {
                name: skill.name.to_string(),
                description: skill_description(skill.content),
                download_url: format!("{base_url}/agent/skills/{}", skill.name),
            })
            .collect(),
        targets,
        default_target: default_id,
        default_bin_dir: if crate::build_flavor::DEVELOPMENT { app_data.join("bin") } else { default_bin_dir() }.to_string_lossy().to_string(),
        default_skills_dir: default_skills,
        manifest_url: format!("{base_url}/agent/manifest"),
        install_url: format!("{base_url}/agent/install.sh"),
    }
}

/// Apply saved per-tool paths without changing the legacy HTTP manifest contract.
pub fn configured_status(mut status: AccessStatus, settings: &crate::settings::AgentSettings) -> AccessStatus {
    for target in &mut status.targets {
        let selected = settings.for_provider(&target.id);
        if !selected.command.trim().is_empty() {
            if let Some(path) = agent::resolve(&selected) {
                target.available = true;
                target.command = Some(path.to_string_lossy().to_string());
            }
        }
        if let Some(profile) = settings.profiles.get(&target.id) {
            if let Some(dir) = expand_path(&profile.skills_dir) {
                target.skills_dir = dir.to_string_lossy().to_string();
                target.installed = installed_skills(&dir);
            }
        }
    }
    status
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

/// Legacy IPC prompt. Use the same installer as the download route instead of
/// asking the receiving agent to overwrite CLI and Skill files itself.
pub fn build_prompt(
    target: &AccessTarget,
    bin_dir: &Path,
    skills_dir: &Path,
    base_url: &str,
    token_path: &Path,
    running: bool,
    cli_available: bool,
) -> String {
    let cli = bin_dir.join(CLI_NAME);
    let mut prompt = format!(
        "为 {} 安装 AIMyChats 只读工具和 Skill。\n- CLI：{}\n- Skills：{}\n- 本机服务：{base_url}\n- Token 由应用写入 {}（0600）；不要复制、回显或记录。\n",
        target.name, cli.display(), skills_dir.display(), token_path.display()
    );
    if !running {
        prompt.push_str("桌面应用当前没有在监听该地址，请先打开 AIMyChats。\n");
    }
    if !cli_available {
        prompt.push_str("本机暂时没有可下载的 aimychats-cli。请在 AIMyChats 中确认 CLI 可用，不要从其他来源下载。\n");
        return prompt;
    }
    prompt.push_str(&format!(
        "\n从本机服务下载并执行安装脚本（CLI 来源：{base_url}{CLI_DOWNLOAD}，Skill 文档：/agent/skills/{}）。若目标已有不同的同名程序，请停止，不要覆盖。\n\n```sh\n(\nset -eu\nscript=$(mktemp)\ntrap 'rm -f \"$script\"' EXIT\ncurl -fsSL -H \"Authorization: Bearer $(cat {})\" --get --data-urlencode {} --data-urlencode {} {} -o \"$script\"\n/bin/sh \"$script\"\n)\n```\n\n安装脚本会运行 health。可再运行 `{} policy` 检查访问范围。只汇报安装路径和结果，不要记录聊天内容。\n",
        SKILLS[0].name,
        sh_quote(&display_path(token_path)),
        sh_quote(&format!("bin={}", bin_dir.display())),
        sh_quote(&format!("skills={}", skills_dir.display())),
        sh_quote(&format!("{base_url}/agent/install.sh")),
        sh_quote(&display_path(&cli)),
    ));
    prompt
}

/// Shell downloads may use a user-owned PATH directory. Never replace a different
/// executable there; the desktop installer handles upgrades in its app-owned bin.
pub fn build_install_script(
    bin_dir: &Path,
    skills_dir: &Path,
    base_url: &str,
    token_path: &Path,
) -> Result<String, String> {
    let cli = bin_dir.join(CLI_NAME);
    let data = token_path.parent().ok_or("本机 Token 路径无效。")?;
    let skill = &SKILLS[0];
    let doc = install::bound_skill(skill, data, &cli, base_url)?;
    let skill_dir = skills_dir.join(skill.name);
    Ok(format!(r#"#!/bin/sh
set -eu
BIN_DIR={bin}
CLI={cli}
LEGACY={legacy}
SKILL_DIR={skills}
TOKEN_FILE={token}
TOKEN="${{WECHAT_AGENT_TOKEN:-${{WECHAT_MCP_TOKEN:-}}}}"
if [ -z "$TOKEN" ] && [ -r "$TOKEN_FILE" ]; then TOKEN=$(cat "$TOKEN_FILE"); fi
if [ -z "$TOKEN" ]; then echo 'missing agent token: reopen AIMyChats' >&2; exit 1; fi

mkdir -p "$BIN_DIR"
work=$(mktemp -d "$BIN_DIR/.aimychats-XXXXXXXX")
kit=''
trap 'rm -f "$work/$cli_name"; rmdir "$work"; if [ -n "$kit" ]; then rm -f "$kit/SKILL.md"; rmdir "$kit"; fi' EXIT
cli_name={cli_name}
tmp="$work/$cli_name"
curl -fsSL -H "Authorization: Bearer $TOKEN" {url} -o "$tmp"
chmod 755 "$tmp"
if [ -e "$CLI" ] || [ -L "$CLI" ]; then
    if [ -L "$CLI" ] || [ ! -f "$CLI" ] || [ ! -x "$CLI" ] || ! cmp -s "$tmp" "$CLI"; then
        echo "Not overwriting $CLI; use AIMyChats to install or update its app-owned CLI." >&2
        exit 1
    fi
else
    ln -n "$tmp" "$CLI"
fi
if [ ! -e "$LEGACY" ] && [ ! -L "$LEGACY" ]; then
    ln -n "$CLI" "$LEGACY"
fi

if [ -L "$SKILL_DIR" ]; then echo "Not writing through $SKILL_DIR" >&2; exit 1; fi
mkdir -p "$SKILL_DIR"
kit=$(mktemp -d "$SKILL_DIR/.aimychats-XXXXXXXX")
dst="$SKILL_DIR/SKILL.md"
printf '%s' {doc} > "$kit/SKILL.md"
if [ -L "$dst" ] || {{ [ -e "$dst" ] && [ ! -f "$dst" ]; }}; then
    echo "Not overwriting $dst" >&2
    exit 1
fi
if ! cmp -s "$kit/SKILL.md" "$dst"; then
    if [ -f "$dst" ]; then cp "$dst" "$(mktemp "$dst.bak-XXXXXXXX")"; fi
    mv -f "$kit/SKILL.md" "$dst"
fi
"$CLI" --app-data {data} --endpoint {base} health
"#,
        bin = sh_quote(&display_path(bin_dir)),
        cli = sh_quote(&display_path(&cli)),
        legacy = sh_quote(&display_path(&bin_dir.join(LEGACY_CLI_NAME))),
        skills = sh_quote(&display_path(&skill_dir)),
        token = sh_quote(&display_path(token_path)),
        cli_name = CLI_NAME,
        url = sh_quote(&format!("{base_url}{CLI_DOWNLOAD}")),
        doc = sh_quote(&doc),
        data = sh_quote(&display_path(data)),
        base = sh_quote(base_url),
    ))
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn source_cli_fallback_rejects_the_other_app_build() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let cli = root.path().join("wxq");
        let other = if crate::build_flavor::DEVELOPMENT { crate::build_flavor::PRODUCTION_ID } else { crate::build_flavor::DEVELOPMENT_ID };
        for (identifier, expected) in [(crate::build_flavor::IDENTIFIER, true), (other, false)] {
            std::fs::write(&cli, format!("#!/bin/sh\nprintf '%s' '{{\"identifier\":\"{identifier}\"}}'\n")).unwrap();
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(super::source_cli_matches_app(&cli), expected);
        }
        std::fs::write(&cli, "#!/bin/sh\nprintf 'old CLI without build-info'\n").unwrap();
        assert!(!super::source_cli_matches_app(&cli));
    }

    #[test]
    fn packaged_cli_discovery_uses_only_its_channel_resources() {
        let root = tempfile::tempdir().unwrap();
        let contents = root.path().join("Contents");
        let own = if crate::build_flavor::DEVELOPMENT { "binaries-dev" } else { "binaries" };
        let other = if crate::build_flavor::DEVELOPMENT { "binaries" } else { "binaries-dev" };
        for name in [own, other] {
            let dir = contents.join("Resources").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("aimychats-cli-test"), "fixture").unwrap();
        }
        assert_eq!(super::bundled_cli(&contents.join("MacOS")),
            vec![contents.join("Resources").join(own).join("aimychats-cli-test")]);
    }

    use super::*;

    fn target() -> AccessTarget {
        AccessTarget {
            id: "claude-code".into(),
            name: "Claude Code".into(),
            available: true,
            command: Some("/usr/local/bin/claude".into()),
            desktop_app: None, config_dir: None,
            skills_dir: "/Users/me/.claude/skills".into(),
            installed: Vec::new(),
        }
    }

    #[test]
    fn embeds_the_unified_skill_with_legacy_aliases() {
        assert_eq!(SKILLS.len(), 1);
        for skill in SKILLS {
            assert!(skill.content.starts_with("---"), "{} has frontmatter", skill.name);
            assert!(skill.content.contains(&format!("name: {}", skill.name)));
            assert!(!skill_description(skill.content).is_empty());
            for name in skill.legacy_names { assert_eq!(skill_by_name(name).unwrap().name, skill.name); }
        }
    }

    #[test]
    fn prompt_carries_download_urls_paths_and_verification() {
        let prompt = build_prompt(
            &target(),
            Path::new("/Users/me/.local/bin"),
            Path::new("/Users/me/.claude/skills"),
            "http://127.0.0.1:18765",
            Path::new("/Users/me/Library/Application Support/local.wechat.devifybridge/agent-token"),
            true,
            true,
        );
        assert!(prompt.contains("Claude Code"));
        assert!(prompt.contains("http://127.0.0.1:18765/agent/aimychats-cli"));
        assert!(prompt.contains("/Users/me/.local/bin/aimychats-cli"));
        assert!(!prompt.contains("aimychats-chat-records"));
        assert!(!prompt.contains("aimychats-daily-summary"));
        assert!(!prompt.contains("aimychats-work-exploration"));
        assert!(prompt.contains("/agent/skills/aimychats"));
        assert!(prompt.contains("health"));
        assert!(prompt.contains("policy"));
        assert!(prompt.contains("agent-token"));
        assert!(prompt.contains("$(cat"));
    }

    #[test]
    fn prompt_tells_the_agent_when_the_listener_is_down() {
        let prompt = build_prompt(
            &target(),
            Path::new("/tmp/bin"),
            Path::new("/tmp/skills"),
            "http://127.0.0.1:18765",
            Path::new("/tmp/agent-token"),
            false,
            false,
        );
        assert!(prompt.contains("没有在监听"));
        assert!(prompt.contains("Token"));
        assert!(prompt.contains("暂时没有可下载的 aimychats-cli"));
    }

    #[test]
    fn install_script_quotes_paths_and_verifies() {
        let script = build_install_script(
            Path::new("/Users/me/it's bin"),
            Path::new("/Users/me/skills"),
            "http://127.0.0.1:18765",
            Path::new("/Users/me/agent-token"),
        ).unwrap();
        assert!(script.contains("#!/bin/sh"));
        assert!(script.contains("it'\\''s bin"));
        assert!(script.contains(CLI_DOWNLOAD));
        assert!(script.contains(&format!("name: {}", SKILLS[0].name)));
        assert!(script.contains("\"$CLI\" --app-data"));
        assert!(script.contains("TOKEN_FILE='/Users/me/agent-token'"));
        assert!(script.contains("missing agent token"));
    }

    #[cfg(unix)]
    #[test]
    fn download_installer_binds_skill_and_preserves_path_collisions() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("app's data");
        let bin = root.path().join("user bin");
        let skills = root.path().join("skills");
        let mock = root.path().join("curl");
        let token = data.join("agent-token");
        let script_path = root.path().join("install.sh");
        let cli = bin.join(CLI_NAME);
        let legacy = bin.join(LEGACY_CLI_NAME);
        let endpoint = "http://127.0.0.1:18765";
        fs::create_dir(&data).unwrap();
        fs::create_dir(&bin).unwrap();
        fs::write(&token, "fixture-token").unwrap();
        fs::write(&legacy, "existing legacy").unwrap();
        fs::write(&mock, r#"#!/bin/sh
set -eu
[ "$3" = 'Authorization: Bearer fixture-token' ]
[ "$4" = 'http://127.0.0.1:18765/agent/aimychats-cli' ]
cat > "$6" <<'FIXTURE'
#!/bin/sh
[ "$1" = --app-data ] && [ "$3" = --endpoint ] && [ "$4" = http://127.0.0.1:18765 ] && [ "$5" = health ]
FIXTURE
"#).unwrap();
        fs::set_permissions(&mock, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&script_path, build_install_script(&bin, &skills, endpoint, &token).unwrap()).unwrap();
        let run = || Command::new("/bin/sh").arg(&script_path)
            .env_clear().env("PATH", format!("{}:/usr/bin:/bin", root.path().display()))
            .output().unwrap();
        fs::write(&cli, "unrelated CLI").unwrap();
        let refusal = run();
        assert!(!refusal.status.success());
        assert!(String::from_utf8_lossy(&refusal.stderr).contains("Not overwriting"));
        assert_eq!(fs::read_to_string(&cli).unwrap(), "unrelated CLI");
        assert!(!skills.exists());
        fs::remove_file(&cli).unwrap();
        std::os::unix::fs::symlink(&legacy, &cli).unwrap();
        assert!(!run().status.success());
        assert_eq!(fs::read_link(&cli).unwrap(), legacy);
        fs::remove_file(&cli).unwrap();
        let skill_dir = skills.join(SKILLS[0].name);
        fs::create_dir_all(&skill_dir).unwrap();
        let skill = skill_dir.join("SKILL.md");
        fs::write(&skill, "old Skill").unwrap();
        for _ in 0..2 {
            let output = run();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        }
        let doc = fs::read_to_string(&skill).unwrap();
        assert!(doc.contains(&sh_quote(&display_path(&cli))));
        assert!(doc.contains(&sh_quote(&display_path(&data))));
        assert!(!doc.contains("fixture-token"));
        assert_eq!(fs::read_dir(skill_dir).unwrap().count(), 2);
        assert_eq!(fs::read_to_string(&legacy).unwrap(), "existing legacy");
        fs::remove_file(&legacy).unwrap();
        assert!(run().status.success());
        assert_eq!(fs::read(&legacy).unwrap(), fs::read(&cli).unwrap());
    }

    #[test]
    fn paths_must_be_absolute_or_home_relative() {
        assert_eq!(expand_path("~/skills"), home_dir().map(|home| home.join("skills")));
        assert!(expand_path("/tmp/skills").is_some());
        assert!(expand_path("relative/skills").is_none());
        assert!(expand_path("/tmp/bad\npath").is_none());
        assert!(expand_path("   ").is_none());
    }

    #[test]
    fn known_hosts_have_skill_directories() {
        for id in ["claude-code", "codex", "opencode", "gemini-cli", "qwen-code", "cursor-agent"] {
            assert!(skills_rel(id).is_some(), "{id}");
        }
        assert!(skills_rel("custom").is_none());
        assert_eq!(skills_rel("workbuddy"), Some(".workbuddy-ai/skills"));
    }

    #[test]
    fn custom_installation_is_explicit_and_independent_of_host_detection() {
        let mut custom = target();
        custom.id = "custom".into();
        custom.available = false;
        let targets = vec![custom];
        assert!(installation_target(&targets, "custom", None).is_err());
        assert!(installation_target(&targets, "custom", Some("relative/skills")).is_err());
        assert!(installation_target(&targets, "custom", Some("/tmp/skills\nother")).is_err());
        let installed = installation_target(&targets, "custom", Some("/tmp/my-harness/skills")).unwrap();
        assert_eq!(installed.skills_dir, "/tmp/my-harness/skills");
        assert!(!installed.available);
        assert_eq!(targets[0].skills_dir, "/Users/me/.claude/skills");
    }

    #[test]
    fn wildcard_bind_is_reported_as_loopback() {
        assert_eq!(client_base("127.0.0.1:18765"), "http://127.0.0.1:18765");
        assert_eq!(client_base("0.0.0.0:18765"), "http://127.0.0.1:18765");
        assert_eq!(client_base("[::]:18765"), "http://[::]:18765");
    }
}
