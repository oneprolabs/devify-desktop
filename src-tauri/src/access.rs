//! Agent access kit.
//!
//! The desktop app serves the read-only `wxq` CLI and three `SKILL.md` documents from its
//! loopback listener. The Agent 集成 page turns that into one prompt: the user pastes it
//! into an agent, and the agent downloads both artifacts into the directories the user
//! chose. No repository checkout, MCP client, or manual `cargo build` is involved.

use crate::agent;
use serde::Serialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const CLI_NAME: &str = "wxq";

#[derive(Clone, Copy, Debug)]
pub struct SkillDoc {
    pub name: &'static str,
    pub content: &'static str,
}

pub const SKILLS: &[SkillDoc] = &[
    SkillDoc {
        name: "wechat-chat-records",
        content: include_str!("../../skills/wechat-chat-records/SKILL.md"),
    },
    SkillDoc {
        name: "wechat-daily-summary",
        content: include_str!("../../skills/wechat-daily-summary/SKILL.md"),
    },
    SkillDoc {
        name: "wechat-work-exploration",
        content: include_str!("../../skills/wechat-work-exploration/SKILL.md"),
    },
];

#[derive(Serialize, Clone, Debug)]
pub struct AccessTarget {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub command: Option<String>,
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
        "opencode" => Some(".config/opencode/skills"),
        "gemini-cli" => Some(".gemini/skills"),
        "qwen-code" => Some(".qwen/skills"),
        "cursor-agent" => Some(".cursor/skills"),
        "workbuddy" => Some(".workbuddy/skills"),
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
    let mut targets: Vec<AccessTarget> = agent::ids()
        .filter_map(|id| {
            let rel = skills_rel(id)?;
            let command = agent::detect(id);
            let dir = home.as_ref().map(|home| home.join(rel));
            Some(AccessTarget {
                id: id.to_string(),
                name: agent::agent_name(id),
                available: command.is_some(),
                command: command.map(|path| path.to_string_lossy().to_string()),
                skills_dir: dir
                    .as_ref()
                    .map(|path| path.to_string_lossy().to_string())
                    .unwrap_or_default(),
                installed: dir.as_deref().map(installed_skills).unwrap_or_default(),
            })
        })
        .collect();

    if let Some(home) = &home {
        let workbuddy = home.join(".workbuddy");
        if workbuddy.is_dir() {
            let dir = workbuddy.join("skills");
            targets.push(AccessTarget {
                id: "workbuddy".to_string(),
                name: "WorkBuddy".to_string(),
                available: true,
                command: None,
                skills_dir: dir.to_string_lossy().to_string(),
                installed: installed_skills(&dir),
            });
        }
    }

    let custom = default_skills_dir();
    targets.push(AccessTarget {
        id: "custom".to_string(),
        name: "其他 Agent（自定义目录）".to_string(),
        available: true,
        command: None,
        skills_dir: custom.to_string_lossy().to_string(),
        installed: installed_skills(&custom),
    });
    targets
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

/// Linux/macOS binary in a GUI app: the sibling built next to the app wins, then the
/// installed copy, then the usual build outputs.
pub fn resolve_wxq(app_data: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(CLI_NAME));
        }
    }
    candidates.push(app_data.join("bin").join(CLI_NAME));
    if let Some(home) = home_dir() {
        candidates.push(home.join(".local/bin").join(CLI_NAME));
    }
    for dir in agent::search_dirs() {
        candidates.push(dir.join(CLI_NAME));
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    candidates.push(manifest.join("target/release").join(CLI_NAME));
    candidates.push(manifest.join("target/debug").join(CLI_NAME));
    candidates.into_iter().find(|path| agent::is_executable(path))
}

/// Where the app can rebuild `wxq` when the current install did not ship it.
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
    token_required: bool,
) -> AccessStatus {
    let base_url = client_base(bind);
    let targets = targets();
    let default_id = default_target(&targets, configured_agent);
    let default_skills = target_by_id(&targets, &default_id)
        .map(|target| target.skills_dir)
        .unwrap_or_else(|| default_skills_dir().to_string_lossy().to_string());
    let wxq = resolve_wxq(app_data).map(|path| WxqAsset {
        size: fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0),
        path: path.to_string_lossy().to_string(),
        download_url: format!("{base_url}/agent/wxq"),
    });
    AccessStatus {
        running,
        base_url: base_url.clone(),
        token_required,
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
        default_bin_dir: default_bin_dir().to_string_lossy().to_string(),
        default_skills_dir: default_skills,
        manifest_url: format!("{base_url}/agent/manifest"),
        install_url: format!("{base_url}/agent/install.sh"),
    }
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

/// The one prompt the page asks the user to copy. It carries the download URLs, the exact
/// target paths and the verification steps, so the receiving agent does not need to know
/// anything about this repository.
pub fn build_prompt(
    target: &AccessTarget,
    bin_dir: &Path,
    skills_dir: &Path,
    base_url: &str,
    token_required: bool,
    running: bool,
    wxq_available: bool,
) -> String {
    let bin = display_path(bin_dir);
    let skills = display_path(skills_dir);
    let wxq_path = format!("{}/{}", bin.trim_end_matches('/'), CLI_NAME);
    let auth_line = if token_required {
        "需要 Bearer Token：从用户的 `WECHAT_AGENT_TOKEN` 环境变量读取（旧名 `WECHAT_MCP_TOKEN` 也兼容），不要回显、不要写入文件；下面所有 curl 都带 `-H \"Authorization: Bearer $WECHAT_AGENT_TOKEN\"`。"
    } else {
        "仅本机访问，不需要 Token。"
    };
    let curl_auth = if token_required {
        " -H \"Authorization: Bearer $WECHAT_AGENT_TOKEN\""
    } else {
        ""
    };
    let mut prompt = String::new();
    prompt.push_str("你正在为 AImyChats 安装只读的 Agent 工具与 Skills。请按顺序执行，完成后汇报结果。不要修改微信数据，不要把聊天内容或 Token 写入日志、文件或外部服务。\n\n");
    prompt.push_str("## 目标环境\n");
    prompt.push_str(&format!("- 当前工具：{}\n", target.name));
    if let Some(command) = &target.command {
        prompt.push_str(&format!("- 工具可执行文件：{command}\n"));
    }
    prompt.push_str(&format!("- CLI 安装目录：{bin}（目标文件 {wxq_path}）\n"));
    prompt.push_str(&format!("- Skills 目录：{skills}\n"));
    prompt.push_str(&format!("- 本机服务：{base_url}\n"));
    prompt.push_str(&format!("- 认证：{auth_line}\n"));
    if !running {
        prompt.push_str("- 注意：桌面应用当前没有在监听该地址。请先让用户打开 AImyChats，再重试下载。\n");
    }
    prompt.push('\n');

    prompt.push_str("## 第 1 步：下载只读 CLI\n");
    if wxq_available {
        prompt.push_str("wxq 由 AImyChats 的本机服务直接提供，不要从 GitHub、包管理器或其它来源另找版本：\n\n");
        prompt.push_str(&format!("```bash\nmkdir -p {dir}\n", dir = sh_quote(&bin)));
        prompt.push_str(&format!(
            "curl -fsSL{auth} '{url}/agent/wxq' -o {tmp}\nchmod 755 {tmp}\nmv -f {tmp} {dst}\n```\n\n",
            auth = curl_auth,
            url = base_url,
            tmp = sh_quote(&format!("{wxq_path}.download")),
            dst = sh_quote(&wxq_path)
        ));
        prompt.push_str(&format!(
            "如果 `{wxq_path}` 已有同名的非 AImyChats 程序，停止并让用户确认，不要覆盖。\n\n"
        ));
    } else {
        prompt.push_str("本机暂时没有可下载的 wxq。请停止，不要从网络自行下载未知二进制；告诉用户在 AImyChats 的「Agent 集成 → 工具接入」确认 CLI 可用后重新复制提示词。\n\n");
    }

    prompt.push_str("## 第 2 步：下载 Skills 描述\n");
    prompt.push_str("下面是完整清单；`/agent/skills` 是同一清单的 JSON 形式，可用于核对：\n\n");
    prompt.push_str(&format!("```bash\ncurl -fsSL{auth} '{url}/agent/skills'\n```\n\n", auth = curl_auth, url = base_url));
    for skill in SKILLS {
        let dir = format!("{}/{}", skills.trim_end_matches('/'), skill.name);
        prompt.push_str(&format!("### {}\n", skill.name));
        prompt.push_str(&format!("- 目标：`{dir}/SKILL.md`\n"));
        prompt.push_str(&format!(
            "```bash\nmkdir -p {dir}\ncurl -fsSL{auth} '{url}/agent/skills/{name}' -o {dst}\n```\n",
            dir = sh_quote(&dir),
            auth = curl_auth,
            url = base_url,
            name = skill.name,
            dst = sh_quote(&format!("{dir}/SKILL.md"))
        ));
        prompt.push_str("- 如果目标文件已存在，先备份为 `SKILL.md.bak-<日期>`；不要删除其它 Skill。\n\n");
    }

    prompt.push_str("## 第 3 步：验证\n");
    prompt.push_str(&format!("```bash\n{}\n{}\n```\n\n", sh_quote(&wxq_path) + " health", sh_quote(&wxq_path) + " policy"));
    prompt.push_str("health 应返回 `ok · wechat-devify-bridge`；policy 应返回当前账号和白名单会话。若服务不可用，让用户打开 AImyChats 后重试；不要自行解密数据库或启动其它服务。白名单为空时，提示用户在应用的「聊天白名单」页配置。若服务需要 Token 而当前 shell 没有 `WECHAT_AGENT_TOKEN`，让用户自己导出后再运行验证，不要把 Token 写进命令或文件。\n\n");
    prompt.push_str("## 第 4 步：汇报\n");
    prompt.push_str("只汇报：wxq 的最终路径、skills 目录、三个 `SKILL.md` 是否写入、health/policy 的结果摘要、失败原因。完成后提示用户重新打开或刷新当前工具的 Skills 列表。\n");
    prompt
}

/// Convenience installer served at `/agent/install.sh`. It is the same operation as the
/// prompt, in one shell script, with an auth header when the listener needs one.
pub fn build_install_script(bin_dir: &Path, skills_dir: &Path, base_url: &str) -> String {
    let bin = display_path(bin_dir);
    let skills = display_path(skills_dir);
    let wxq_path = format!("{}/{}", bin.trim_end_matches('/'), CLI_NAME);
    let mut script = String::new();
    script.push_str("#!/bin/sh\nset -eu\n\n");
    script.push_str(&format!("BASE={}\n", sh_quote(base_url)));
    script.push_str(&format!("BIN_DIR={}\n", sh_quote(&bin)));
    script.push_str(&format!("SKILLS_DIR={}\n", sh_quote(&skills)));
    script.push_str(&format!("WXQ={}\n", sh_quote(&wxq_path)));
    script.push_str("AUTH=''\n");
    script.push_str("if [ -n \"${WECHAT_AGENT_TOKEN:-}\" ]; then AUTH=\"Authorization: Bearer $WECHAT_AGENT_TOKEN\"; elif [ -n \"${WECHAT_MCP_TOKEN:-}\" ]; then AUTH=\"Authorization: Bearer $WECHAT_MCP_TOKEN\"; fi\n");
    script.push_str("fetch() {\n");
    script.push_str("  if [ -n \"$AUTH\" ]; then curl -fsSL -H \"$AUTH\" \"$1\" -o \"$2\"; else curl -fsSL \"$1\" -o \"$2\"; fi\n");
    script.push_str("}\n\n");
    script.push_str("mkdir -p \"$BIN_DIR\" \"$SKILLS_DIR\"\n");
    script.push_str("tmp=\"$WXQ.download.$$\"\n");
    script.push_str("fetch \"$BASE/agent/wxq\" \"$tmp\"\n");
    script.push_str("chmod 755 \"$tmp\"\n");
    script.push_str("mv -f \"$tmp\" \"$WXQ\"\n");
    script.push_str("echo \"installed $WXQ\"\n");
    for skill in SKILLS {
        script.push_str(&format!(
            "mkdir -p \"$SKILLS_DIR/{name}\"\ndst=\"$SKILLS_DIR/{name}/SKILL.md\"\nif [ -e \"$dst\" ]; then cp \"$dst\" \"$dst.bak-$(date +%Y%m%d-%H%M%S)\"; fi\nfetch \"$BASE/agent/skills/{name}\" \"$dst\"\necho \"installed $dst\"\n",
            name = skill.name
        ));
    }
    script.push_str("\"$WXQ\" health\n");
    script
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> AccessTarget {
        AccessTarget {
            id: "claude-code".into(),
            name: "Claude Code".into(),
            available: true,
            command: Some("/usr/local/bin/claude".into()),
            skills_dir: "/Users/me/.claude/skills".into(),
            installed: Vec::new(),
        }
    }

    #[test]
    fn embeds_three_skills_with_descriptions() {
        assert_eq!(SKILLS.len(), 3);
        for skill in SKILLS {
            assert!(skill.content.starts_with("---"), "{} has frontmatter", skill.name);
            assert!(skill.content.contains(&format!("name: {}", skill.name)));
            assert!(!skill_description(skill.content).is_empty());
        }
    }

    #[test]
    fn prompt_carries_download_urls_paths_and_verification() {
        let prompt = build_prompt(
            &target(),
            Path::new("/Users/me/.local/bin"),
            Path::new("/Users/me/.claude/skills"),
            "http://127.0.0.1:18765",
            false,
            true,
            true,
        );
        assert!(prompt.contains("Claude Code"));
        assert!(prompt.contains("http://127.0.0.1:18765/agent/wxq"));
        assert!(prompt.contains("/Users/me/.local/bin/wxq"));
        assert!(prompt.contains("wechat-chat-records"));
        assert!(prompt.contains("wechat-daily-summary"));
        assert!(prompt.contains("wechat-work-exploration"));
        assert!(prompt.contains("health"));
        assert!(prompt.contains("policy"));
    }

    #[test]
    fn prompt_tells_the_agent_when_the_listener_is_down() {
        let prompt = build_prompt(
            &target(),
            Path::new("/tmp/bin"),
            Path::new("/tmp/skills"),
            "http://127.0.0.1:18765",
            true,
            false,
            false,
        );
        assert!(prompt.contains("没有在监听"));
        assert!(prompt.contains("WECHAT_AGENT_TOKEN"));
        assert!(prompt.contains("暂时没有可下载的 wxq"));
    }

    #[test]
    fn install_script_quotes_paths_and_verifies() {
        let script = build_install_script(
            Path::new("/Users/me/it's bin"),
            Path::new("/Users/me/skills"),
            "http://127.0.0.1:18765",
        );
        assert!(script.contains("#!/bin/sh"));
        assert!(script.contains("it'\\''s bin"));
        assert!(script.contains("/agent/wxq"));
        assert!(script.contains("/agent/skills/wechat-chat-records"));
        assert!(script.contains("\"$WXQ\" health"));
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
    }

    #[test]
    fn wildcard_bind_is_reported_as_loopback() {
        assert_eq!(client_base("127.0.0.1:18765"), "http://127.0.0.1:18765");
        assert_eq!(client_base("0.0.0.0:18765"), "http://127.0.0.1:18765");
        assert_eq!(client_base("[::]:18765"), "http://[::]:18765");
    }
}
