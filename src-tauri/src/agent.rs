//! Local Agent discovery and invocation.
//!
//! The app itself never talks to a model provider. Instead it detects agent CLIs that the
//! user already has installed and authenticated (Claude Code, Codex, OpenCode, ...), and
//! runs the user-selected one in non-interactive mode with the summary prompt on stdin or
//! argv. Credentials, model routing and quotas stay inside each agent's own configuration.

use crate::settings::AgentSettings;
use crate::wechat::query::SummaryInput;
use serde::Serialize;
use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const VERSION_TIMEOUT: Duration = Duration::from_secs(6);
const MIN_TIMEOUT_SECONDS: u64 = 30;
const MAX_TIMEOUT_SECONDS: u64 = 900;
const TEST_PROMPT: &str = "这是一次本地连通性测试。请只回复两个字母：OK";
const STDERR_TAIL_CHARS: usize = 900;
const MODELS_TIMEOUT: Duration = Duration::from_secs(30);
const AUTH_HINT: &str =
    "该 Agent 的默认模型凭据不可用。请在「模型」里填写它已经配置好的模型，或先在该 Agent 内重新登录后重试。";

/// CLI auth failures across these agents are textual, not structured. Matching a few
/// distinctive phrases is enough to add an actionable hint instead of echoing a raw 401.
fn looks_like_auth_failure(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "api key has expired",
        "failed to authenticate",
        "invalid api key",
        "user is disabled",
        "not logged in",
        "run `opencode auth login`",
        "please run /login",
        "missing api key",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

#[derive(Clone, Copy)]
enum PromptArg {
    /// Prompt is written to stdin. The optional marker is appended to argv.
    Stdin(Option<&'static str>),
    /// Prompt is passed as the value of a flag, e.g. `-p <prompt>`.
    Flag(&'static str),
    /// Prompt is a positional argument, e.g. `opencode run <prompt>`.
    Positional,
}

struct Spec {
    id: &'static str,
    name: &'static str,
    bins: &'static [&'static str],
    prefix_args: &'static [&'static str],
    prompt: PromptArg,
    model_flag: Option<&'static str>,
    last_message_flag: Option<&'static str>,
    /// Extra argv that prints the models this CLI can talk to, when it supports listing.
    models_args: Option<&'static [&'static str]>,
}

/// Agents that can be invoked non-interactively. Order is the display order: the ones
/// most people have installed come first.
const SPECS: &[Spec] = &[
    Spec {
        id: "claude-code",
        name: "Claude Code",
        bins: &["claude"],
        prefix_args: &["-p", "--output-format", "json"],
        prompt: PromptArg::Stdin(None),
        model_flag: Some("--model"),
        last_message_flag: None,
        models_args: None,
    },
    Spec {
        id: "codex",
        name: "Codex CLI",
        bins: &["codex"],
        prefix_args: &["exec", "--sandbox", "read-only", "--skip-git-repo-check", "--ephemeral"],
        prompt: PromptArg::Stdin(Some("-")),
        model_flag: Some("-m"),
        last_message_flag: Some("-o"),
        models_args: None,
    },
    Spec {
        id: "opencode",
        name: "OpenCode",
        bins: &["opencode"],
        prefix_args: &["run"],
        prompt: PromptArg::Positional,
        model_flag: Some("--model"),
        last_message_flag: None,
        models_args: Some(&["models"]),
    },
    Spec {
        id: "gemini-cli",
        name: "Gemini CLI",
        bins: &["gemini"],
        prefix_args: &[],
        prompt: PromptArg::Flag("-p"),
        model_flag: Some("-m"),
        last_message_flag: None,
        models_args: None,
    },
    Spec {
        id: "qwen-code",
        name: "Qwen Code",
        bins: &["qwen"],
        prefix_args: &[],
        prompt: PromptArg::Flag("-p"),
        model_flag: Some("--model"),
        last_message_flag: None,
        models_args: None,
    },
    Spec {
        id: "cursor-agent",
        name: "Cursor Agent",
        bins: &["cursor-agent"],
        prefix_args: &[],
        prompt: PromptArg::Flag("-p"),
        model_flag: Some("--model"),
        last_message_flag: None,
        models_args: None,
    },
];

#[derive(Serialize, Clone, Debug)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub command: Option<String>,
    pub version: Option<String>,
    pub available: bool,
    pub note: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct AgentRun {
    pub output: String,
    pub elapsed_ms: u64,
    pub command: String,
}

fn spec(id: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|spec| spec.id == id)
}

/// Stable ids in display order, for callers that build their own target list.
pub fn ids() -> impl Iterator<Item = &'static str> {
    SPECS.iter().map(|spec| spec.id)
}

pub fn agent_name(id: &str) -> String {
    spec(id).map(|spec| spec.name.to_string()).unwrap_or_else(|| id.to_string())
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

/// Directories that matter for GUI apps, whose PATH often only contains
/// `/usr/bin:/bin:/usr/sbin:/sbin` when launched from Finder.
pub(crate) fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = env::var_os("PATH") {
        dirs.extend(env::split_paths(&path));
    }
    if let Some(home) = home_dir() {
        for rel in [
            ".local/bin",
            ".opencode/bin",
            ".bun/bin",
            ".cargo/bin",
            ".npm-global/bin",
            ".volta/bin",
            ".claude/local",
            "bin",
        ] {
            dirs.push(home.join(rel));
        }
    }
    for dir in [
        "/opt/homebrew/bin",
        "/opt/homebrew/sbin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
    ] {
        dirs.push(PathBuf::from(dir));
    }
    let mut seen = HashMap::new();
    dirs.retain(|dir| seen.insert(dir.clone(), ()).is_none());
    dirs
}

#[cfg(unix)]
pub(crate) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub(crate) fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Resolve executables through PATH/search dirs first, then through the user's login
/// shell so nvm/asdf/mise style setups keep working for a Finder-launched app.
pub(crate) fn resolve_bins(bins: &[&str]) -> HashMap<String, PathBuf> {
    let mut found = HashMap::new();
    let dirs = search_dirs();
    for bin in bins {
        for dir in &dirs {
            let candidate = dir.join(bin);
            if is_executable(&candidate) {
                found.insert(bin.to_string(), candidate);
                break;
            }
        }
    }
    if found.len() == bins.len() {
        return found;
    }
    if let Some(map) = login_shell_lookup(bins) {
        for (bin, path) in map {
            found.entry(bin).or_insert(path);
        }
    }
    found
}

fn login_shell_lookup(bins: &[&str]) -> Option<HashMap<String, PathBuf>> {
    let shell = env::var("SHELL").ok().filter(|value| !value.trim().is_empty())?;
    // Fixed, compile-time binary names only: no user input reaches the shell. Each line is
    // `name=path` so a missing binary cannot shift the mapping for the next one.
    let script = bins
        .iter()
        .map(|bin| format!("p=$(command -v {bin} 2>/dev/null); [ -n \"$p\" ] && printf '{bin}=%s\\n' \"$p\""))
        .collect::<Vec<_>>()
        .join("; ");
    let output = with_timeout(Duration::from_secs(4), move || {
        Command::new(shell)
            .arg("-lc")
            .arg(script)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
    })??;
    let mut map = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((name, path)) = line.trim().split_once('=') else {
            continue;
        };
        let path = PathBuf::from(path);
        if bins.contains(&name) && is_executable(&path) {
            map.insert(name.to_string(), path);
        }
    }
    Some(map)
}

fn resolve_command(provider_id: &str, override_command: &str) -> Option<PathBuf> {
    let spec = spec(provider_id)?;
    let override_path = override_command.trim();
    if !override_path.is_empty() {
        let path = PathBuf::from(override_path);
        return is_executable(&path).then_some(path);
    }
    let found = resolve_bins(spec.bins);
    spec.bins.iter().find_map(|bin| found.get(*bin).cloned())
}

/// Resolve the executable for the configured agent without running it.
pub fn resolve(settings: &AgentSettings) -> Option<PathBuf> {
    resolve_command(&settings.provider_id, &settings.command)
}

/// Resolve one known agent's executable without probing `--version` (the access page only
/// needs to know whether the host is installed).
pub fn detect(provider_id: &str) -> Option<PathBuf> {
    resolve_command(provider_id, "")
}

fn with_timeout<T: Send + 'static>(duration: Duration, task: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(task());
    });
    rx.recv_timeout(duration).ok()
}

fn probe_version(path: &Path) -> Option<String> {
    let path = path.to_path_buf();
    with_timeout(VERSION_TIMEOUT, move || {
        let output = Command::new(path)
            .arg("--version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
        Some(line.chars().take(80).collect::<String>())
    })?
}

/// Detect every supported agent. `override_id`/`override_command` come from the saved
/// settings so a user-pinned path is reflected in the list without a second pass.
pub fn scan(override_id: &str, override_command: &str) -> Vec<AgentInfo> {
    SPECS
        .iter()
        .map(|spec| {
            let overridden = spec.id == override_id && !override_command.trim().is_empty();
            let command = resolve_command(spec.id, if overridden { override_command } else { "" });
            match command {
                Some(path) => {
                    let version = probe_version(&path);
                    let mut note = "已检测到，可用于总结任务".to_string();
                    if overridden {
                        note = "使用自定义路径".to_string();
                    }
                    if version.is_none() {
                        note = "可执行文件存在，但 --version 没有响应".to_string();
                    }
                    AgentInfo {
                        id: spec.id.to_string(),
                        name: spec.name.to_string(),
                        command: Some(path.to_string_lossy().to_string()),
                        version,
                        available: true,
                        note,
                    }
                }
                None => AgentInfo {
                    id: spec.id.to_string(),
                    name: spec.name.to_string(),
                    command: None,
                    version: None,
                    available: false,
                    note: if overridden {
                        "自定义路径不是可执行文件".to_string()
                    } else {
                        "未检测到可执行文件".to_string()
                    },
                },
            }
        })
        .collect()
}

fn timeout_of(settings: &AgentSettings) -> Duration {
    Duration::from_secs(settings.timeout_seconds.clamp(MIN_TIMEOUT_SECONDS, MAX_TIMEOUT_SECONDS))
}

/// Strip ANSI escape sequences so agent output can be rendered as plain text.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                for next in chars.by_ref() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {
                chars.next();
            }
        }
    }
    out
}

fn tail(text: &str, limit: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return text.trim().to_string();
    }
    chars[chars.len() - limit..].iter().collect::<String>().trim().to_string()
}

/// Extract `(is_error, result)` from Claude Code's `--output-format json` payload.
/// Returns `None` when the payload is not the expected object, so an older CLI that
/// prints plain text still works.
fn parse_claude_result(raw: &str) -> Option<(bool, String)> {
    let value: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    let result = value.get("result")?.as_str()?.to_string();
    let is_error = value.get("is_error").and_then(|flag| flag.as_bool()).unwrap_or(false);
    Some((is_error, result))
}

struct RunFiles {
    dir: PathBuf,
    stdout: PathBuf,
    stderr: PathBuf,
    last_message: PathBuf,
}

impl RunFiles {
    fn create(app_data: &Path) -> Result<Self, String> {
        let dir = app_data
            .join("agent-runs")
            .join(format!("{}-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"), std::process::id()));
        fs::create_dir_all(&dir).map_err(|e| format!("创建 Agent 运行目录失败：{e}"))?;
        Ok(Self {
            stdout: dir.join("stdout.txt"),
            stderr: dir.join("stderr.txt"),
            last_message: dir.join("last-message.txt"),
            dir,
        })
    }

    fn read(&self, path: &Path) -> String {
        fs::read(path)
            .map(|bytes| strip_ansi(&String::from_utf8_lossy(&bytes)))
            .unwrap_or_default()
    }

    fn cleanup(&self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Result<(bool, u64), String> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok((false, start.elapsed().as_millis() as u64)),
            Ok(None) => {}
            Err(error) => return Err(format!("等待 Agent 进程失败：{error}")),
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Ok((true, start.elapsed().as_millis() as u64));
        }
        std::thread::sleep(Duration::from_millis(120));
    }
}

fn run_agent(settings: &AgentSettings, prompt: &str, app_data: &Path, timeout: Duration) -> Result<AgentRun, String> {
    let Some(spec) = spec(&settings.provider_id) else {
        return Err("尚未选择可用的本地 Agent，请先在「Agent 总结」页面完成配置。".into());
    };
    let Some(command_path) = resolve_command(&settings.provider_id, &settings.command) else {
        return Err(format!(
            "没有找到 {} 的可执行文件。可以在「Agent 总结」页面填写自定义路径。",
            spec.name
        ));
    };

    let files = RunFiles::create(app_data)?;
    let stdout = File::create(&files.stdout).map_err(|e| format!("创建输出文件失败：{e}"))?;
    let stderr = File::create(&files.stderr).map_err(|e| format!("创建日志文件失败：{e}"))?;

    let model = settings.model.trim();
    let mut args: Vec<String> = spec.prefix_args.iter().map(|arg| arg.to_string()).collect();
    if !model.is_empty() {
        if let Some(flag) = spec.model_flag {
            args.push(flag.to_string());
            args.push(model.to_string());
        }
    }
    if let Some(flag) = spec.last_message_flag {
        args.push(flag.to_string());
        args.push(files.last_message.to_string_lossy().to_string());
    }
    match spec.prompt {
        PromptArg::Stdin(marker) => {
            if let Some(marker) = marker {
                args.push(marker.to_string());
            }
        }
        PromptArg::Flag(flag) => {
            args.push(flag.to_string());
            args.push(prompt.to_string());
        }
        PromptArg::Positional => args.push(prompt.to_string()),
    }

    let wants_stdin = matches!(spec.prompt, PromptArg::Stdin(_));
    let mut child = Command::new(&command_path)
        .args(&args)
        // Run inside the app data directory: writable, out of the way of the user's
        // repositories, and never a place the agent is expected to edit code.
        .current_dir(app_data)
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(if wants_stdin { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|e| {
            files.cleanup();
            format!("启动 {} 失败：{e}", spec.name)
        })?;

    let writer = child.stdin.take().map(|mut stdin| {
        let payload = prompt.to_string();
        std::thread::spawn(move || {
            let _ = stdin.write_all(payload.as_bytes());
        })
    });

    let (timed_out, elapsed_ms) = match wait_with_timeout(&mut child, timeout) {
        Ok(result) => result,
        Err(error) => {
            files.cleanup();
            return Err(error);
        }
    };
    if let Some(writer) = writer {
        let _ = writer.join();
    }

    let stdout_text = files.read(&files.stdout);
    let stderr_text = files.read(&files.stderr);
    let last_message = files.read(&files.last_message);
    let code = child.try_wait().ok().flatten().and_then(|status| status.code());
    files.cleanup();

    if timed_out {
        return Err(format!(
            "{} 在 {} 秒内没有返回结果，已终止。可以调大超时时间，或换一个 Agent 再试。",
            spec.name,
            timeout.as_secs()
        ));
    }

    let output = if last_message.trim().is_empty() { stdout_text } else { last_message };
    let mut output = output.trim().to_string();

    // Claude Code exits 0 even when authentication or the API call failed; the JSON result
    // carries `is_error` and the human-readable reason.
    if spec.id == "claude-code" {
        if let Some((is_error, result)) = parse_claude_result(&output) {
            if is_error {
                let mut message = format!("{} 返回错误：{}", spec.name, result.trim());
                if looks_like_auth_failure(&result) {
                    message.push_str("\n\n");
                    message.push_str(AUTH_HINT);
                }
                return Err(message);
            }
            output = result.trim().to_string();
        }
    }
    if output.is_empty() {
        let mut detail = tail(&stderr_text, STDERR_TAIL_CHARS);
        let reason = match code {
            Some(code) => format!("退出码 {code}"),
            None => "没有输出".to_string(),
        };
        if looks_like_auth_failure(&detail) {
            if !detail.is_empty() {
                detail.push_str("\n\n");
            }
            detail.push_str(AUTH_HINT);
        }
        return Err(if detail.is_empty() {
            format!("{} 没有返回内容（{reason}）。请先在终端运行一次该 Agent，确认已登录。", spec.name)
        } else {
            format!("{} 没有返回内容（{reason}）：\n{detail}", spec.name)
        });
    }

    Ok(AgentRun {
        output,
        elapsed_ms,
        command: command_path.to_string_lossy().to_string(),
    })
}

pub fn test(settings: &AgentSettings, app_data: &Path) -> Result<AgentRun, String> {
    if settings.provider_id.trim().is_empty() {
        return Err("请先选择一个本地 Agent。".into());
    }
    let timeout = timeout_of(settings).min(Duration::from_secs(90));
    run_agent(settings, TEST_PROMPT, app_data, timeout)
}

/// Models the selected CLI reports as usable. Only agents with a listing command return
/// anything; the rest return an empty list so the UI falls back to free text.
pub fn list_models(settings: &AgentSettings, app_data: &Path) -> Result<Vec<String>, String> {
    let Some(spec) = spec(&settings.provider_id) else {
        return Err("尚未选择本地 Agent。".into());
    };
    let Some(args) = spec.models_args else {
        return Ok(Vec::new());
    };
    let Some(command_path) = resolve_command(&settings.provider_id, &settings.command) else {
        return Err(format!("没有找到 {} 的可执行文件。", spec.name));
    };

    let files = RunFiles::create(app_data)?;
    let stdout = File::create(&files.stdout).map_err(|e| format!("创建输出文件失败：{e}"))?;
    let stderr = File::create(&files.stderr).map_err(|e| format!("创建日志文件失败：{e}"))?;
    let mut child = Command::new(&command_path)
        .args(args)
        .current_dir(app_data)
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|e| {
            files.cleanup();
            format!("启动 {} 失败：{e}", spec.name)
        })?;

    let timed_out = match wait_with_timeout(&mut child, MODELS_TIMEOUT) {
        Ok((timed_out, _)) => timed_out,
        Err(error) => {
            files.cleanup();
            return Err(error);
        }
    };
    let stdout_text = files.read(&files.stdout);
    let stderr_text = files.read(&files.stderr);
    files.cleanup();

    if timed_out {
        return Err(format!(
            "{} 在 {} 秒内没有返回模型列表，已终止。",
            spec.name,
            MODELS_TIMEOUT.as_secs()
        ));
    }

    let mut models: Vec<String> = Vec::new();
    for line in stdout_text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('>') {
            continue;
        }
        if !models.iter().any(|model| model == line) {
            models.push(line.to_string());
        }
    }
    if models.is_empty() {
        let detail = tail(&stderr_text, STDERR_TAIL_CHARS);
        return Err(if detail.is_empty() {
            format!("{} 没有返回模型列表，可以直接手填模型名。", spec.name)
        } else {
            format!("{} 没有返回模型列表：\n{detail}", spec.name)
        });
    }
    Ok(models)
}

pub fn summarize(
    settings: &AgentSettings,
    input: &SummaryInput,
    language: &str,
    app_data: &Path,
) -> Result<AgentRun, String> {
    let prompt = build_prompt(input, &settings.instructions, language);
    run_agent(settings, &prompt, app_data, timeout_of(settings))
}

/// Prompt shared by the interactive summary and the daily automation. The structure is
/// deliberately explicit: these agents happily invent detail when asked to "summarise".
pub fn build_prompt(input: &SummaryInput, instructions: &str, language: &str) -> String {
    let extra = instructions.trim();
    let mut prompt = String::new();
    prompt.push_str("你是一名严谨的工作助理。请把下面这段微信聊天记录整理成一份可直接阅读的工作总结。\n\n");
    prompt.push_str(&format!("输出语言：{}\n\n", language_directive(language)));
    prompt.push_str("硬性要求：\n");
    prompt.push_str("- 只依据给出的聊天记录，不要引入外部知识，不要编造人名、数字、日期、结论或承诺。\n");
    prompt.push_str("- 记录没有提到的内容不要补；信息不足或存在歧义时明确写「待确认」。\n");
    prompt.push_str("- 保留关键人名、项目名、数字、日期和链接，不确定的表述保持原样。\n");
    prompt.push_str("- 不要输出分析过程，不要复述全部原文。\n");
    prompt.push_str("- 如果记录主要是闲聊或没有可归纳的工作内容，直接说明即可，不要硬凑。\n\n");
    prompt.push_str("输出格式（Markdown，没有内容的小节可以省略）：\n");
    prompt.push_str("## 概览\n（2-4 句话说明这段时间在推进什么、结果如何）\n");
    prompt.push_str("## 关键进展\n");
    prompt.push_str("## 待办与跟进\n（每条写清负责人和截止时间；原文没写就标注「未指定」）\n");
    prompt.push_str("## 决策与结论\n");
    prompt.push_str("## 风险与待确认\n");
    prompt.push_str("## 值得回看的原文\n（最多 5 条，格式：[时间] 说话人：原文摘要）\n\n");
    prompt.push_str("用户补充要求：");
    prompt.push_str(if extra.is_empty() { "无" } else { extra });
    prompt.push_str("\n\n");
    prompt.push_str(&format!(
        "统计：会话 {} 个，消息 {} 条，原文约 {} 字。\n",
        input.sessions, input.messages, input.characters
    ));
    if input.truncated {
        prompt.push_str("注意：原文超出单次处理上限，已按时间顺序截断，更早或更长的内容没有纳入。\n");
    }
    prompt.push_str("\n聊天记录如下：\n<transcript>\n");
    prompt.push_str(&input.transcript);
    if !input.transcript.ends_with('\n') {
        prompt.push('\n');
    }
    prompt.push_str("</transcript>\n");
    prompt
}

/// Turns the 语言 page setting into a sentence for the prompt.
fn language_directive(language: &str) -> &'static str {
    match language {
        "English" => "使用英文书写全部内容。",
        "按原文语言" => "按聊天原文的主要语言书写；中英混排时以中文为主。",
        _ => "使用简体中文书写全部内容。",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_input() -> SummaryInput {
        SummaryInput {
            transcript: "【会话】项目群（群聊）（1 条）\n[10-27 09:12] 张三：周五前给出方案\n".to_string(),
            sessions: 1,
            messages: 1,
            characters: 42,
            truncated: false,
        }
    }

    #[test]
    fn specs_have_unique_ids_and_commands() {
        let mut ids: Vec<&str> = SPECS.iter().map(|spec| spec.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), SPECS.len());
        assert!(SPECS.iter().all(|spec| !spec.bins.is_empty()));
    }

    #[test]
    fn only_listing_agents_expose_models() {
        let opencode = spec("opencode").expect("opencode");
        assert_eq!(opencode.models_args, Some(&["models"][..]));
        assert!(spec("codex").expect("codex").models_args.is_none());
    }

    #[test]
    fn auth_failures_are_recognised_from_cli_text() {
        assert!(looks_like_auth_failure(
            "Error: This API key has expired. Create a new API key in OpenCode Console"
        ));
        assert!(looks_like_auth_failure("Failed to authenticate. API Error: 403"));
        assert!(!looks_like_auth_failure("network timeout while calling the model"));
    }

    #[test]
    fn prompt_carries_instructions_and_transcript() {
        let prompt = build_prompt(&sample_input(), "重点关注交付风险", "跟随界面");
        assert!(prompt.contains("重点关注交付风险"));
        assert!(prompt.contains("<transcript>"));
        assert!(prompt.contains("周五前给出方案"));
        assert!(prompt.contains("会话 1 个，消息 1 条"));
        assert!(!prompt.contains("已按时间顺序截断"));
    }

    #[test]
    fn prompt_defaults_instructions_to_none() {
        let prompt = build_prompt(&sample_input(), "   ", "跟随界面");
        assert!(prompt.contains("用户补充要求：无"));
    }

    #[test]
    fn truncated_input_is_disclosed() {
        let mut input = sample_input();
        input.truncated = true;
        assert!(build_prompt(&input, "", "跟随界面").contains("已按时间顺序截断"));
    }

    #[test]
    fn ansi_sequences_are_removed() {
        assert_eq!(strip_ansi("\u{1b}[31m红\u{1b}[0m色"), "红色");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn claude_json_result_is_parsed_and_errors_are_visible() {
        let ok = r#"{"type":"result","is_error":false,"result":"OK"}"#;
        assert_eq!(parse_claude_result(ok), Some((false, "OK".to_string())));

        let failed = r#"{"type":"result","is_error":true,"result":"Failed to authenticate."}"#;
        assert_eq!(parse_claude_result(failed), Some((true, "Failed to authenticate.".to_string())));

        assert_eq!(parse_claude_result("plain text"), None);
    }

    #[test]
    fn timeout_is_clamped_to_a_sane_range() {
        let mut settings = AgentSettings::default();
        settings.timeout_seconds = 1;
        assert_eq!(timeout_of(&settings).as_secs(), MIN_TIMEOUT_SECONDS);
        settings.timeout_seconds = 10_000;
        assert_eq!(timeout_of(&settings).as_secs(), MAX_TIMEOUT_SECONDS);
    }

    #[test]
    fn unknown_agent_is_not_resolvable() {
        assert!(resolve_command("not-an-agent", "").is_none());
        assert_eq!(agent_name("codex"), "Codex CLI");
    }
}
