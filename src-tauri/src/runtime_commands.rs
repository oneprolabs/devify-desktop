//! Configuration submission and task-state IPC.
use super::*;

static CONFIG_SUBMIT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(super) async fn flush_config_outbox(app: &AppHandle) -> Result<(), String> {
    let _serial = CONFIG_SUBMIT.lock().await;
    let data = app_data(app);
    config_runtime::activate_profile(&data)?;
    let initial = settings::load(&data)?;
    let legacy: Vec<_> = initial
        .sync
        .owners
        .values()
        .filter(|o| !initial.runtime.sources.contains_key(&o.account_id) && initial.sync.enabled)
        .cloned()
        .collect();
    if !legacy.is_empty() {
        settings::update(&data, |s| {
            for old in legacy {
                if config_runtime::local_owner(&data, s, &old.account_id)
                    .is_ok_and(|o| config_runtime::same_principal(&o, &old))
                {
                    config_runtime::enqueue(s, old)?;
                }
            }
            Ok(())
        })?;
    }
    for _ in 0..128 {
        let value = settings::load(&data)?;
        let pending = value
            .runtime
            .pending
            .iter()
            .find(|m| {
                config_runtime::local_owner(&data, &value, &m.owner.account_id)
                    .is_ok_and(|o| config_runtime::same_principal(&o, &m.owner))
            })
            .cloned();
        let Some(mutation) = pending else {
            return Ok(());
        };
        let owner = &mutation.owner;
        // The outbox belongs to its original login, including while another
        // login or service replaces the current settings during an await.
        let result = async {
            let token = devify_access_token_async(&data, &owner.base_url).await?;
            validate_owner_state(&settings::load(&data)?, owner, &token)?;
            fixed_identity_json(&data, &owner.base_url, &token, reqwest::Method::PUT,
                &format!("/api/v1/spaces/{}/desktop/sync-config", owner.space_id),
                Some(&mutation.body)).await
        }.await;
        let response = match result {
            Ok(v) => v,
            Err(error) => {
                let _ = settings::update(&data, |s| {
                    if config_runtime::local_owner(&data, s, &owner.account_id)
                        .is_ok_and(|o| config_runtime::same_principal(&o, owner))
                    {
                        s.runtime.last_error = Some(error.clone());
                    }
                    Ok(())
                });
                emit_task_runtime(app);
                return Err(error);
            }
        };
        if response.get("saved").and_then(serde_json::Value::as_bool) != Some(true)
            || response.get("mutation_id") != mutation.body.get("mutation_id")
            || response.get("local_revision") != mutation.body.get("local_revision")
            || response.get("target") != mutation.body.get("target")
        {
            return Err("服务端未确认本次设置，请重试。".into());
        }
        settings::update(&data, |s| {
            let live = config_runtime::local_owner(&data, s, &owner.account_id)?;
            if !config_runtime::same_principal(&live, owner) {
                return Err("登录状态已改变。".into());
            }
            let index = s
                .runtime
                .pending
                .iter()
                .position(|m| {
                    m.body == mutation.body && config_runtime::same_principal(&m.owner, owner)
                })
                .ok_or("找不到待确认的设置。")?;
            s.runtime.pending.remove(index);
            if let Some(source) = s.runtime.sources.get_mut(&owner.account_id) {
                source.acknowledged = Some(mutation.body["payload"].clone());
                if mutation.body["payload"]["setup_state"] == "completed" {
                    source.active = Some(mutation.body["payload"].clone());
                }
            }
            s.runtime.last_error = None;
            Ok(())
        })?;
    }
    Ok(())
}

#[tauri::command]
pub(super) async fn connector_setup_state(
    app: AppHandle,
    account_id: String,
    completed: bool,
    skip_backup: Option<bool>,
    wizard_step: Option<String>,
) -> Result<settings::AppSettings, String> {
    if account_id.trim().is_empty() {
        return Err("请先选择本机账号。".into());
    }
    if wizard_step.as_deref().is_some_and(|step| !matches!(step, "sessions" | "backup")) {
        return Err("无效的连接向导步骤。".into());
    }
    let data = app_data(&app);
    settings::update(&data, |s| {
        let owner = config_runtime::local_owner(&data, s, &account_id)?;
        let source = s.runtime.sources.entry(account_id.clone()).or_default();
        if source
            .owner
            .as_ref()
            .is_some_and(|old| !config_runtime::same_principal(old, &owner))
        {
            // An explicit new wizard is the only action that can rebind a source.
            if completed {
                return Err("请为当前登录账号重新完成连接向导。".into());
            }
            *source = Default::default();
        }
        source.owner = Some(owner.clone());
        source.draft = !completed;
        source.completed = completed;
        source.wizard_step = if completed { None } else { Some(wizard_step.clone().unwrap_or_else(|| "sessions".into())) };
        source.backup_enabled = Some(!skip_backup.unwrap_or(false));
        if completed && !skip_backup.unwrap_or(false) {
            let names = s
                .chat
                .allowed_usernames
                .get(&account_id)
                .cloned()
                .unwrap_or_default();
            if names.is_empty() {
                return Err("请先选择要备份的会话。".into());
            }
            let since = Utc::now().timestamp() - 7 * 86400;
            let ranges = s.chat.initial_ranges.entry(account_id.clone()).or_default();
            for name in names {
                ranges.entry(name).or_insert(settings::InitialRange {
                    since: Some(since),
                    until: None,
                });
            }
        }
        config_runtime::enqueue(s, owner)
    })?;
    emit_task_runtime(&app);
    flush_config_outbox(&app).await?;
    TASK_WAKE.notify_one();
    emit_task_runtime(&app);
    settings::load(&data)
}

#[tauri::command]
pub(super) fn task_runtime_pause(app: AppHandle, paused: bool) -> Result<(), String> {
    set_backup_paused(&app_data(&app), paused)?;
    if paused {
        abort_in_flight_uploads();
    }
    TASK_WAKE.notify_one();
    emit_task_runtime(&app);
    // The durable local switch takes effect immediately, including while offline.
    // Scheduling waits for this same transaction's configuration acknowledgement.
    tauri::async_runtime::spawn(async move {
        let _ = flush_config_outbox(&app).await;
        TASK_WAKE.notify_one();
        emit_task_runtime(&app);
    });
    Ok(())
}

pub(super) fn set_backup_paused(data: &Path, paused: bool) -> Result<(), String> {
    settings::update(data, |s| {
        config_runtime::set_backup_enabled(s, !paused);
        config_runtime::enqueue_current_sources(data, s, paused)
    })?;
    Ok(())
}

#[tauri::command]
pub(super) fn task_runtime_status(app: AppHandle) -> Result<serde_json::Value, String> {
    let data = app_data(&app);
    let s = settings::load(&data)?;
    let accounts: HashSet<_> = s
        .upload_queue
        .iter()
        .map(|j| j.account_id.clone())
        .chain(s.runtime.sources.keys().cloned())
        .collect();
    let owners: HashMap<_, _> = accounts
        .into_iter()
        .filter_map(|a| {
            config_runtime::local_owner(&data, &s, &a)
                .ok()
                .map(|o| (a, o))
        })
        .collect();
    let mut jobs: Vec<_> = s
        .upload_queue
        .iter()
        .filter(|j| {
            owners.get(&j.account_id).is_some_and(|o| {
                j.owner
                    .as_ref()
                    .is_some_and(|old| config_runtime::same_principal(old, o))
            })
        })
        .cloned()
        .collect();
    jobs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    jobs.truncate(200);
    for job in &mut jobs {
        if let Some(scope) = job.scope.as_mut() {
            scope.message_selection.clear();
        }
        job.covered.clear();
        job.voice_pending.clear();
        job.voice_processed.clear();
        job.initial_chunks.clear();
        job.initial_baselines.clear();
    }
    let errors: Vec<_> = s
        .runtime
        .sources
        .iter()
        .filter(|(a, source)| {
            owners.get(*a).is_some_and(|o| {
                source
                    .owner
                    .as_ref()
                    .is_some_and(|old| config_runtime::same_principal(old, o))
            })
        })
        .filter(|(account, source)| !task_runtime::wechat_backup_needs_keys(account, source))
        .filter_map(|(account, source)| {
            let error = source.last_error.as_deref()?;
            // Upgrade old user-facing key errors to a neutral status until a fresh read confirms them.
            Some(
                if !whatsapp::is_account(account)
                    && matches!(
                        error,
                        "数据库已重新加密，需重新提取密钥。"
                            | "没有该数据库的密钥。请先用密钥工具提取并导入。"
                            | "保存的密钥格式无效。"
                            | "已保存的密钥未通过数据库首页校验。"
                    )
                {
                    task_runtime::WECHAT_BACKUP_RECHECK
                } else {
                    error
                },
            )
        })
        .take(3)
        .collect();
    let error = s
        .runtime
        .last_error
        .clone()
        .or_else(|| (!errors.is_empty()).then(|| errors.join("；")));
    let context = desktop_credentials::load(&data, &s.devify.base_url)
        .ok()
        .and_then(|c| {
            access_token_claim(&c.access, "user_id").map(|user| {
                hex::encode(sha2::Sha256::digest(
                    format!(
                        "{}\0{user}\0{}\0{}",
                        s.devify.base_url,
                        s.devify.active_space_id.as_deref().unwrap_or(""),
                        s.devify.device_id
                    )
                    .as_bytes(),
                ))
            })
        });
    let running = app
        .state::<UploadState>()
        .syncing
        .uploading_accounts()
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    let summaries=summary_jobs::snapshots(&app);
    let running=running || summaries.iter().any(|j|matches!(j["status"].as_str(),Some("queued"|"running"|"processing")));
    let mut jobs=serde_json::to_value(&jobs).map_err(|e|e.to_string())?.as_array().cloned().unwrap_or_default();
    jobs.extend(summaries);
    let sources = source_snapshots(&s, &owners);
    Ok(
        serde_json::json!({"context":context,"revision":s.settings_revision,"sources":sources,"paused":!config_runtime::backup_enabled(&s),"running":running,"pending_config":s.runtime.pending.iter().filter(|m|config_runtime::local_owner(&data,&s,&m.owner.account_id).is_ok_and(|o|config_runtime::same_principal(&o,&m.owner))).count(),"error":error,"jobs":jobs}),
    )
}

fn source_snapshots(s: &settings::AppSettings, owners: &HashMap<String, settings::UploadOwner>) -> Vec<serde_json::Value> {
    s.runtime.sources.iter().filter(|(account, source)| {
        owners.get(*account).is_some_and(|owner| source.owner.as_ref()
            .is_some_and(|old| config_runtime::same_principal(old, owner)))
    }).map(|(account, source)| serde_json::json!({
        "account_id": account,
        "platform": crate::expected_source_type(account),
        "setup_complete": source.completed && !source.draft,
        "backup_enabled": source.backup_enabled != Some(false),
        "backup_needs_keys": task_runtime::wechat_backup_needs_keys(account, source),
        "wizard_step": if source.completed && !source.draft { None } else { Some(resume_step(source)) },
    })).collect()
}

fn resume_step(source: &config_runtime::Source) -> &str {
    source.wizard_step.as_deref().filter(|step| matches!(*step, "sessions" | "backup")).unwrap_or_else(|| {
        // Older clients did not persist a step. Only a confirmed selection can
        // advance that legacy draft to the shared-schedule confirmation.
        if source.acknowledged.as_ref().is_some_and(|body| {
            body["usernames"].as_array().is_some_and(|names| !names.is_empty())
                && body["initial_ranges"].as_object().is_some_and(|ranges| !ranges.is_empty())
        }) { "backup" } else { "sessions" }
    })
}

#[cfg(test)]
mod source_snapshot_tests {
    use super::*;

    #[test]
    fn resume_uses_saved_step_and_only_migrates_confirmed_legacy_selections() {
        let mut source = config_runtime::Source { draft: true, ..Default::default() };
        assert_eq!(resume_step(&source), "sessions");
        source.acknowledged = Some(serde_json::json!({"usernames":["chat"],"initial_ranges":{"chat":{"since":1}}}));
        assert_eq!(resume_step(&source), "backup");
        source.wizard_step = Some("sessions".into());
        assert_eq!(resume_step(&source), "sessions");
        source.wizard_step = Some("backup".into());
        let restored: config_runtime::Source = serde_json::from_value(serde_json::to_value(source).unwrap()).unwrap();
        assert_eq!(resume_step(&restored), "backup");
    }

    #[test]
    fn unfinished_setup_survives_restart_and_is_hidden_from_another_login() {
        let account = "123@s.whatsapp.net".to_string();
        let owner = settings::UploadOwner { account_id: account.clone(), user_id: 1, device_id: "device".into(), space_id: "space".into(), base_url: "https://example.invalid".into() };
        let mut s = settings::AppSettings::default();
        s.runtime.sources.insert(account.clone(), config_runtime::Source { owner: Some(owner.clone()), draft: true, completed: false, ..Default::default() });
        let owners = HashMap::from([(account.clone(), owner.clone())]);
        let restored = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(source_snapshots(&restored, &owners)[0]["setup_complete"], false);
        let source = s.runtime.sources.get_mut(&account).unwrap();
        source.draft = false;
        source.completed = true;
        assert_eq!(source_snapshots(&s, &owners)[0]["setup_complete"], true);
        let other = settings::UploadOwner { user_id: 2, ..owner };
        assert!(source_snapshots(&s, &HashMap::from([(account, other)])).is_empty());
    }
}
pub(super) fn emit_task_runtime(app: &AppHandle) {
    if let Ok(value) = task_runtime_status(app.clone()) {
        let _ = app.emit("task-runtime", value);
    }
}
