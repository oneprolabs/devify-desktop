//! Local voice playback, transcription and voice-engine preferences.
use crate::chat::access::require_visible_session;
use crate::wechat::{decrypt, voice};
use crate::{app_data, capabilities, entitlements, local_scope, settings, wechat_sync, SyncState};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct VoiceRequest {
    local_id: i64,
    server_id: String,
}

#[derive(Serialize)]
pub(crate) struct VoiceAudio {
    data_uri: String,
    duration_secs: f64,
}

#[tauri::command]
pub(crate) async fn wechat_voice_status(app: AppHandle) -> voice::VoiceSettingsView {
    voice::view(&app_data(&app))
}

#[tauri::command]
pub(crate) async fn wechat_voice_settings_save(
    app: AppHandle,
    engine: String,
    model: String,
    language: String,
) -> Result<voice::VoiceSettingsView, String> {
    let settings = voice::VoiceSettings {
        engine: engine.trim().to_string(),
        model: model.trim().to_string(),
        language: if language.trim().is_empty() {
            "zh".to_string()
        } else {
            language.trim().to_string()
        },
    };
    voice::save_settings(&app_data(&app), &settings)?;
    Ok(voice::view(&app_data(&app)))
}

/// Decode and transcribe selected voice messages. Results are cached by `account|server_id`.
async fn voice_blobs_with_refresh(
    app: &AppHandle,
    state: State<'_, SyncState>,
    account_id: &str,
    ids: &[i64],
) -> Result<std::collections::BTreeMap<i64, Vec<u8>>, String> {
    let cache = decrypt::cache_dir(&app_data(app), account_id);
    let cached = voice::read_voice_blobs(&cache, ids).unwrap_or_default();
    if ids.iter().all(|id| cached.contains_key(id)) {
        return Ok(cached);
    }
    wechat_sync(app.clone(), state, account_id.to_string())
        .await
        .map_err(|error| format!("无法刷新语音数据：{error}"))?;
    voice::read_voice_blobs(&cache, ids)
}

#[tauri::command]
pub(crate) async fn wechat_voice_transcribe(
    app: AppHandle,
    state: State<'_, SyncState>,
    account_id: String,
    username: String,
    items: Vec<VoiceRequest>,
) -> Result<Vec<voice::VoiceResult>, String> {
    require_visible_session(&app, &account_id, &username)?;
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = items
        .iter()
        .map(|item| {
            item.server_id
                .parse()
                .map_err(|_| "无效的语音标识。".to_string())
        })
        .collect::<Result<_, _>>()?;
    let data_dir = app_data(&app);
    let private_data = local_scope::root(&data_dir);
    let settings = settings::load(&data_dir)?;
    entitlements::require(
        &data_dir,
        &settings.devify.base_url,
        capabilities::LOCAL_VOICE_TRANSCRIBE,
    )?;
    let blobs = voice_blobs_with_refresh(&app, state, &account_id, &ids).await?;
    let view = voice::view(&data_dir);
    if view.selected.is_none() {
        return Err(
            "没有检测到本地转写引擎。可安装 whisper.cpp（brew install whisper-cpp）、mlx-whisper 或 openai-whisper，然后在「设置 → 语音转写」里选择。"
                .into(),
        );
    }

    let mut transcripts = voice::load_transcripts(&private_data);
    let mut results = Vec::new();
    for (item, server_id) in items.into_iter().zip(ids) {
        let key = voice::transcript_key(&account_id, server_id);
        let mut result = voice::VoiceResult {
            server_id,
            local_id: item.local_id,
            wav: None,
            duration_secs: 0.0,
            text: None,
            cached: false,
            error: None,
        };
        let Some(blob) = blobs.get(&server_id) else {
            result.error = Some("本地暂未找到这条语音，请刷新微信数据后重试。".into());
            results.push(result);
            continue;
        };
        match voice::ensure_wav(&private_data, &account_id, server_id, blob) {
            Ok((wav, duration)) => {
                result.wav = Some(wav.to_string_lossy().to_string());
                result.duration_secs = duration;
                if let Some(entry) = transcripts.get(&key) {
                    result.text = Some(entry.text.clone());
                    result.cached = true;
                } else {
                    match voice::transcribe_file(&data_dir, &wav, 300) {
                        Ok((text, engine, model)) => {
                            transcripts.insert(
                                key,
                                voice::TranscriptEntry {
                                    text: text.clone(),
                                    engine,
                                    model,
                                    created_at: voice::now_secs(),
                                },
                            );
                            let _ = voice::save_transcripts(&private_data, &transcripts);
                            result.text = Some(text);
                        }
                        Err(error) => result.error = Some(error),
                    }
                }
            }
            Err(error) => result.error = Some(error),
        }
        results.push(result);
    }
    Ok(results)
}

/// Decode one voice message to a data URI so the webview can play it.
#[tauri::command]
pub(crate) async fn wechat_voice_audio(
    app: AppHandle,
    state: State<'_, SyncState>,
    account_id: String,
    username: String,
    server_id: String,
) -> Result<VoiceAudio, String> {
    require_visible_session(&app, &account_id, &username)?;
    let server_id: i64 = server_id
        .parse()
        .map_err(|_| "无效的语音标识。".to_string())?;
    let data_dir = app_data(&app);
    let private_data = local_scope::root(&data_dir);
    let blobs = voice_blobs_with_refresh(&app, state, &account_id, &[server_id]).await?;
    let blob = blobs
        .get(&server_id)
        .ok_or_else(|| "本地暂未找到这条语音，请刷新微信数据后重试。".to_string())?;
    let (wav, duration) = voice::ensure_wav(&private_data, &account_id, server_id, blob)?;
    Ok(VoiceAudio {
        data_uri: voice::audio_data_uri(&wav)?,
        duration_secs: duration,
    })
}
