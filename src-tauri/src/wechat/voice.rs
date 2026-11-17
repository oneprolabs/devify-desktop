//! WeChat voice messages: SILK v3 decode, WAV cache, and local transcription.
//!
//! WeChat 4.x keeps voice payloads in `message/media_0.db` (`VoiceInfo.svr_id` →
//! `voice_data`, a SILK v3 stream prefixed with one `0x02` byte). The blob is decoded with
//! the vendored Skype SILK reference decoder (`vendor/silk`), written as 24 kHz mono 16-bit
//! WAV under the app data directory, and then handed to a local ASR CLI.
//!
//! Nothing is uploaded. Transcription engines are external CLIs on purpose: the app must not
//! bundle a multi-gigabyte model, and the user already picks a local agent CLI elsewhere.

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::agent;

const SAMPLE_RATE: u32 = 24000;
const TRANSCRIPTS_NAME: &str = "voice-transcripts.json";
const SETTINGS_NAME: &str = "voice-settings.json";

#[cfg(unix)]
unsafe extern "C" {
    fn wechat_silk_decode(
        input: *const u8,
        input_len: i32,
        out_samples: *mut *mut i16,
        out_len: *mut i32,
        out_rate: *mut i32,
    ) -> i32;
    fn wechat_silk_free(samples: *mut i16);
}

#[cfg(not(unix))]
extern "C" {
    fn wechat_silk_decode(
        input: *const u8,
        input_len: i32,
        out_samples: *mut *mut i16,
        out_len: *mut i32,
        out_rate: *mut i32,
    ) -> i32;
    fn wechat_silk_free(samples: *mut i16);
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct VoiceSettings {
    /// Engine id, or empty for "first available".
    pub engine: String,
    /// Engine-specific model name or absolute path. Empty means "engine default".
    pub model: String,
    /// Spoken language passed to the ASR CLI.
    pub language: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct VoiceEngineInfo {
    pub id: String,
    pub label: String,
    pub path: Option<String>,
    pub model_hint: String,
    pub available: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct VoiceSettingsView {
    pub settings: VoiceSettings,
    pub engines: Vec<VoiceEngineInfo>,
    /// Engine that would be used right now, or `None` when nothing is installed.
    pub selected: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TranscriptEntry {
    pub text: String,
    pub engine: String,
    pub model: String,
    pub created_at: i64,
}

#[derive(Serialize, Clone, Debug)]
pub struct VoiceResult {
    pub server_id: i64,
    pub local_id: i64,
    pub wav: Option<String>,
    pub duration_secs: f64,
    pub text: Option<String>,
    pub cached: bool,
    pub error: Option<String>,
}

struct EngineSpec {
    id: &'static str,
    label: &'static str,
    bins: &'static [&'static str],
    model_hint: &'static str,
    default_model: &'static str,
}

const ENGINES: &[EngineSpec] = &[
    EngineSpec {
        id: "whisper-cli",
        label: "whisper.cpp（whisper-cli）",
        bins: &["whisper-cli", "whisper-cpp", "main"],
        model_hint: "需要 ggml 模型文件路径，例如 ~/.cache/whisper/ggml-large-v3-turbo.bin",
        default_model: "",
    },
    EngineSpec {
        id: "mlx-whisper",
        label: "mlx-whisper（Apple Silicon）",
        bins: &["mlx_whisper"],
        model_hint: "默认 mlx-community/whisper-large-v3-mlx，首次运行会下载模型",
        default_model: "mlx-community/whisper-large-v3-mlx",
    },
    EngineSpec {
        id: "whisper",
        label: "openai-whisper（whisper）",
        bins: &["whisper"],
        model_hint: "默认 small，可填 tiny/base/small/medium/large",
        default_model: "small",
    },
];

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub fn settings_path(app_data: &Path) -> PathBuf {
    app_data.join(SETTINGS_NAME)
}

pub fn transcripts_path(app_data: &Path) -> PathBuf {
    app_data.join(TRANSCRIPTS_NAME)
}

pub fn load_settings(app_data: &Path) -> VoiceSettings {
    fs::read_to_string(settings_path(app_data))
        .ok()
        .and_then(|text| serde_json::from_str::<VoiceSettings>(&text).ok())
        .map(|mut settings| {
            if settings.language.trim().is_empty() {
                settings.language = "zh".to_string();
            }
            settings
        })
        .unwrap_or_else(|| VoiceSettings {
            engine: String::new(),
            model: String::new(),
            language: "zh".to_string(),
        })
}

pub fn save_settings(app_data: &Path, settings: &VoiceSettings) -> Result<(), String> {
    let text = serde_json::to_string_pretty(settings).map_err(|e| format!("无法序列化语音设置：{e}"))?;
    write_private(&settings_path(app_data), text.as_bytes())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("无法创建目录：{e}"))?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = fs::File::create(&tmp).map_err(|e| format!("无法写入 {}：{e}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
        }
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("无法写入 {}：{e}", path.display()))?;
    }
    fs::rename(&tmp, path).map_err(|e| format!("无法保存 {}：{e}", path.display()))
}

/// Instantiate the decoder and return 24 kHz mono PCM.
pub fn decode_pcm(blob: &[u8]) -> Result<Vec<i16>, String> {
    if blob.is_empty() {
        return Err("语音数据为空。".into());
    }
    let mut out: *mut i16 = std::ptr::null_mut();
    let mut len: i32 = 0;
    let mut rate: i32 = 0;
    let code = unsafe {
        wechat_silk_decode(
            blob.as_ptr(),
            blob.len().min(i32::MAX as usize) as i32,
            &mut out,
            &mut len,
            &mut rate,
        )
    };
    if code != 0 || out.is_null() || len <= 0 {
        return Err(format!("SILK 解码失败（code {code}），可能不是微信 SILK v3 语音。"));
    }
    let pcm = unsafe { std::slice::from_raw_parts(out, len as usize) }.to_vec();
    unsafe { wechat_silk_free(out) };
    Ok(pcm)
}

/// Wrap 16-bit mono PCM in a WAV container.
pub fn pcm_to_wav(pcm: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in pcm {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

pub fn voice_dir(app_data: &Path, account: &str) -> PathBuf {
    app_data.join("voice-cache").join(account)
}

pub fn wav_path(app_data: &Path, account: &str, server_id: i64) -> PathBuf {
    voice_dir(app_data, account).join(format!("{server_id}.wav"))
}

/// Decode and cache one voice blob. Returns the WAV path and its duration in seconds.
pub fn ensure_wav(
    app_data: &Path,
    account: &str,
    server_id: i64,
    blob: &[u8],
) -> Result<(PathBuf, f64), String> {
    let path = wav_path(app_data, account, server_id);
    if path.is_file() {
        let samples = (fs::metadata(&path).map(|m| m.len()).unwrap_or(0).saturating_sub(44) / 2) as f64;
        return Ok((path, samples / SAMPLE_RATE as f64));
    }
    let pcm = decode_pcm(blob)?;
    let duration = pcm.len() as f64 / SAMPLE_RATE as f64;
    let wav = pcm_to_wav(&pcm, SAMPLE_RATE);
    write_private(&path, &wav)?;
    Ok((path, duration))
}

pub fn load_transcripts(app_data: &Path) -> BTreeMap<String, TranscriptEntry> {
    fs::read_to_string(transcripts_path(app_data))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save_transcripts(app_data: &Path, entries: &BTreeMap<String, TranscriptEntry>) -> Result<(), String> {
    let text = serde_json::to_string_pretty(entries).map_err(|e| format!("无法序列化转写缓存：{e}"))?;
    write_private(&transcripts_path(app_data), text.as_bytes())
}

pub fn transcript_key(account: &str, server_id: i64) -> String {
    format!("{account}|{server_id}")
}

fn engine_spec(id: &str) -> Option<&'static EngineSpec> {
    ENGINES.iter().find(|spec| spec.id == id)
}

fn engine_path(spec: &EngineSpec) -> Option<PathBuf> {
    let found = agent::resolve_bins(spec.bins);
    spec.bins.iter().find_map(|bin| found.get(*bin).cloned())
}

pub fn engines() -> Vec<VoiceEngineInfo> {
    ENGINES
        .iter()
        .map(|spec| {
            let path = engine_path(spec);
            VoiceEngineInfo {
                id: spec.id.to_string(),
                label: spec.label.to_string(),
                path: path.as_ref().map(|p| p.to_string_lossy().to_string()),
                model_hint: spec.model_hint.to_string(),
                available: path.is_some(),
            }
        })
        .collect()
}

pub fn view(app_data: &Path) -> VoiceSettingsView {
    let settings = load_settings(app_data);
    let engines = engines();
    let selected = pick_engine(&settings, &engines).map(|(id, _, _)| id);
    VoiceSettingsView {
        settings,
        engines,
        selected,
    }
}

fn pick_engine(
    settings: &VoiceSettings,
    engines: &[VoiceEngineInfo],
) -> Option<(String, PathBuf, &'static EngineSpec)> {
    let wanted = settings.engine.trim();
    let candidates: Vec<&VoiceEngineInfo> = if wanted.is_empty() {
        engines.iter().filter(|engine| engine.available).collect()
    } else {
        engines
            .iter()
            .filter(|engine| engine.id == wanted && engine.available)
            .collect()
    };
    for engine in candidates {
        let Some(spec) = engine_spec(&engine.id) else {
            continue;
        };
        let Some(path) = engine.path.as_ref().map(PathBuf::from) else {
            continue;
        };
        return Some((engine.id.clone(), path, spec));
    }
    None
}

fn model_for(settings: &VoiceSettings, spec: &EngineSpec) -> String {
    let configured = settings.model.trim();
    if !configured.is_empty() {
        return configured.to_string();
    }
    if spec.id == "whisper-cli" {
        if let Ok(env_model) = std::env::var("WHISPER_MODEL") {
            if !env_model.trim().is_empty() {
                return env_model;
            }
        }
        // whisper.cpp ships no model; look in the usual caches before asking the user.
        if let Some(home) = home() {
            for dir in [
                home.join(".cache/whisper"),
                home.join("Library/Application Support/whisper"),
                home.join("whisper.cpp/models"),
            ] {
                if let Ok(entries) = fs::read_dir(&dir) {
                    let mut found: Vec<PathBuf> = entries
                        .flatten()
                        .map(|entry| entry.path())
                        .filter(|path| {
                            path.extension().map(|ext| ext == "bin").unwrap_or(false)
                                && path
                                    .file_name()
                                    .map(|name| name.to_string_lossy().starts_with("ggml"))
                                    .unwrap_or(false)
                        })
                        .collect();
                    found.sort();
                    if let Some(path) = found.pop() {
                        return path.to_string_lossy().to_string();
                    }
                }
            }
        }
    }
    spec.default_model.to_string()
}

fn run_command(mut command: Command, timeout: Duration) -> Result<(String, String), String> {
    let dir = std::env::temp_dir().join(format!(
        "aimychats-voice-{}-{}",
        std::process::id(),
        now_millis()
    ));
    fs::create_dir_all(&dir).map_err(|e| format!("无法创建临时目录：{e}"))?;
    let out_path = dir.join("stdout");
    let err_path = dir.join("stderr");
    let out = fs::File::create(&out_path).map_err(|e| format!("无法创建临时文件：{e}"))?;
    let err = fs::File::create(&err_path).map_err(|e| format!("无法创建临时文件：{e}"))?;

    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .map_err(|e| format!("无法启动转写命令：{e}"))?;

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = fs::remove_dir_all(&dir);
                    return Err(format!("转写超时（{} 秒）。", timeout.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&dir);
                return Err(format!("等待转写进程失败：{error}"));
            }
        }
    }

    let mut stdout = String::new();
    let mut stderr = String::new();
    let _ = fs::File::open(&out_path).and_then(|mut f| f.read_to_string(&mut stdout));
    let _ = fs::File::open(&err_path).and_then(|mut f| f.read_to_string(&mut stderr));
    let _ = fs::remove_dir_all(&dir);
    Ok((stdout, stderr))
}

fn strip_timestamps(text: &str) -> String {
    text.lines()
        .filter(|line| {
            let trimmed = line.trim();
            !(trimmed.starts_with('[') && trimmed.contains("-->") && trimmed.ends_with(']'))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn transcribe_wav(
    engine_path: &Path,
    spec: &EngineSpec,
    model: &str,
    language: &str,
    wav: &Path,
    timeout: Duration,
) -> Result<String, String> {
    let label = language.trim();
    if spec.id == "whisper-cli" {
        if model.trim().is_empty() {
            return Err(
                "whisper-cli 需要一个 ggml 模型文件。请在「设置 → 语音转写」里填写路径，或先运行 whisper.cpp 的模型下载脚本。"
                    .into(),
            );
        }
        let mut command = Command::new(engine_path);
        command
            .arg("-m")
            .arg(model)
            .arg("-f")
            .arg(wav)
            .arg("-nt");
        if !label.is_empty() {
            command.arg("-l").arg(label);
        }
        let (stdout, stderr) = run_command(command, timeout)?;
        let text = strip_timestamps(&stdout);
        if text.is_empty() {
            let tail: String = stderr.chars().rev().take(400).collect::<String>().chars().rev().collect();
            return Err(format!("whisper-cli 没有输出文字。{}", tail.trim()));
        }
        return Ok(text);
    }

    // mlx_whisper and openai-whisper share the same CLI shape.
    let dir = std::env::temp_dir().join(format!(
        "aimychats-asr-{}-{}",
        std::process::id(),
        now_millis()
    ));
    fs::create_dir_all(&dir).map_err(|e| format!("无法创建临时目录：{e}"))?;
    let mut command = Command::new(engine_path);
    command
        .arg(wav)
        .arg("--model")
        .arg(model)
        .arg("--output_format")
        .arg("txt")
        .arg("--output_dir")
        .arg(&dir);
    if !label.is_empty() {
        command.arg("--language").arg(label);
    }
    let (stdout, stderr) = run_command(command, timeout)?;

    let mut text = String::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if entry.path().extension().map(|ext| ext == "txt").unwrap_or(false) {
                text = fs::read_to_string(entry.path()).unwrap_or_default();
                break;
            }
        }
    }
    let _ = fs::remove_dir_all(&dir);
    if text.trim().is_empty() {
        text = strip_timestamps(&stdout);
    }
    if text.trim().is_empty() {
        let tail: String = stderr.chars().rev().take(400).collect::<String>().chars().rev().collect();
        return Err(format!("转写没有输出文字。{}", tail.trim()));
    }
    Ok(text.trim().to_string())
}

/// Transcribe one already-decoded WAV file with the configured engine.
pub fn transcribe_file(app_data: &Path, wav: &Path, timeout_secs: u64) -> Result<(String, String, String), String> {
    let settings = load_settings(app_data);
    let engines = engines();
    let Some((_id, path, spec)) = pick_engine(&settings, &engines) else {
        return Err(
            "没有检测到本地转写引擎。可安装 whisper.cpp（brew install whisper-cpp）、mlx-whisper 或 openai-whisper，然后在「设置 → 语音转写」里选择。"
                .into(),
        );
    };
    let model = model_for(&settings, spec);
    let text = transcribe_wav(
        &path,
        spec,
        &model,
        &settings.language,
        wav,
        Duration::from_secs(timeout_secs.clamp(20, 1800)),
    )?;
    Ok((text, spec.id.to_string(), model))
}

/// Read SILK blobs from the decrypted media shards.
///
/// `VoiceInfo.svr_id` is an INTEGER on current builds but TEXT on some older ones, so the
/// match is done in Rust after a full scan; the table is small (a few hundred rows).
pub fn read_voice_blobs(
    cache: &Path,
    server_ids: &[i64],
) -> Result<BTreeMap<i64, Vec<u8>>, String> {
    let directory = cache.join("message");
    let mut paths: Vec<_> = fs::read_dir(&directory).ok().into_iter().flatten().flatten()
        .map(|entry| entry.path())
        .filter(|path| path.file_name().and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("media_") && name.ends_with(".db")))
        .collect();
    paths.sort();
    if paths.is_empty() {
        return Err(
            "没有找到解密后的 media_0.db。请先在「本地数据 → 密钥、解密与缓存」里重新解密。"
                .into(),
        );
    }
    let wanted: std::collections::HashSet<String> = server_ids
        .iter()
        .map(|id| id.to_string())
        .collect();
    let mut out = BTreeMap::new();
    for path in paths {
    let conn = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| format!("无法读取语音数据库：{e}"))?;
    let mut stmt = conn.prepare("SELECT svr_id, voice_data FROM VoiceInfo")
        .map_err(|e| format!("无法读取语音数据：{e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, rusqlite::types::Value>(0).ok(),
                row.get::<_, Option<Vec<u8>>>(1).ok().flatten(),
            ))
        })
        .map_err(|e| format!("读取 VoiceInfo 失败：{e}"))?;
    for row in rows.flatten() {
        let (key, blob) = row;
        let Some(blob) = blob else { continue };
        let Some(key) = key else { continue };
        let text = match key {
            rusqlite::types::Value::Integer(value) => value.to_string(),
            rusqlite::types::Value::Text(value) => value,
            _ => continue,
        };
        if wanted.contains(&text) {
            if let Ok(id) = text.parse::<i64>() {
                out.insert(id, blob);
            }
        }
    }
    }
    Ok(out)
}

/// Encode a cached WAV as a data URI for inline playback in the webview.
pub fn audio_data_uri(path: &Path) -> Result<String, String> {
    let meta = fs::metadata(path).map_err(|e| format!("无法读取语音文件：{e}"))?;
    if meta.len() > 12 * 1024 * 1024 {
        return Err("语音文件过大，无法在页面内直接播放。".into());
    }
    let bytes = fs::read(path).map_err(|e| format!("无法读取语音文件：{e}"))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:audio/wav;base64,{encoded}"))
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_secs() as i64)
        .unwrap_or(0)
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_voice_from_all_media_shards() {
        let temp = tempfile::tempdir().unwrap();
        let message = temp.path().join("message");
        fs::create_dir_all(&message).unwrap();
        for (shard, id) in [(0, 10), (1, 20)] {
            let db = rusqlite::Connection::open(message.join(format!("media_{shard}.db"))).unwrap();
            db.execute("CREATE TABLE VoiceInfo (svr_id INTEGER, voice_data BLOB)", []).unwrap();
            db.execute("INSERT INTO VoiceInfo VALUES (?1, ?2)", rusqlite::params![id, vec![id as u8]]).unwrap();
        }
        let found = read_voice_blobs(temp.path(), &[10, 20]).unwrap();
        assert_eq!(found.get(&10), Some(&vec![10]));
        assert_eq!(found.get(&20), Some(&vec![20]));
    }

    #[test]
    fn wav_header_is_valid_and_sized() {
        let pcm = vec![0i16; 2400];
        let wav = pcm_to_wav(&pcm, SAMPLE_RATE);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + pcm.len() * 2);
        let declared = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]);
        assert_eq!(declared as usize, pcm.len() * 2);
    }

    #[test]
    fn rejects_blobs_that_are_not_silk() {
        let error = decode_pcm(b"definitely not silk").unwrap_err();
        assert!(error.contains("SILK"), "{error}");
    }

    #[test]
    fn transcript_key_is_account_scoped() {
        assert_eq!(transcript_key("acc", 42), "acc|42");
    }

    #[cfg(unix)]
    #[test]
    fn transcribe_file_drives_an_external_cli_end_to_end() {
        use std::os::unix::fs::PermissionsExt;

        // A stand-in for openai-whisper that honours --output_dir like the real CLI.
        let bin = std::env::temp_dir().join(format!("aimychats-asr-bin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&bin);
        fs::create_dir_all(&bin).unwrap();
        let script = bin.join("whisper");
        fs::write(
            &script,
            "#!/bin/sh\nout=\"\"\nwhile [ $# -gt 0 ]; do if [ \"$1\" = \"--output_dir\" ]; then out=\"$2\"; fi; shift; done\nprintf '你好世界' > \"$out/out.txt\"\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let app_data = std::env::temp_dir().join(format!("aimychats-asr-data-{}", std::process::id()));
        let _ = fs::remove_dir_all(&app_data);
        fs::create_dir_all(&app_data).unwrap();
        save_settings(
            &app_data,
            &VoiceSettings {
                engine: "whisper".into(),
                model: "tiny".into(),
                language: "zh".into(),
            },
        )
        .unwrap();

        let wav = app_data.join("clip.wav");
        fs::write(&wav, pcm_to_wav(&vec![0i16; 240], SAMPLE_RATE)).unwrap();

        let old_path = std::env::var_os("PATH");
        let mut dirs = vec![bin.clone()];
        if let Some(existing) = old_path.clone() {
            dirs.extend(std::env::split_paths(&existing));
        }
        std::env::set_var("PATH", std::env::join_paths(dirs).unwrap());

        let result = transcribe_file(&app_data, &wav, 30);

        match old_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        let _ = fs::remove_dir_all(&bin);
        let _ = fs::remove_dir_all(&app_data);

        let (text, engine, model) = result.expect("fake engine must be driven correctly");
        assert_eq!(text, "你好世界");
        assert_eq!(engine, "whisper");
        assert_eq!(model, "tiny");
    }
}
