//! First-phase runtime checks use temporary source databases and loopback identities.
use super::*;

#[test]
fn frozen_config_request_keeps_original_service_token_and_payload() {
    use std::io::{Read, Write};
    let temp = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut raw = Vec::new();
        loop {
            let mut buf = [0; 2048];
            let n = socket.read(&mut buf).unwrap();
            assert!(n > 0);
            raw.extend_from_slice(&buf[..n]);
            if let Some(end) = raw.windows(4).position(|v| v == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&raw[..end]).to_lowercase();
                let length: usize = headers.lines().find_map(|line| line.strip_prefix("content-length:").map(|s| s.trim().parse().unwrap())).unwrap();
                if raw.len() >= end + 4 + length { break; }
            }
        }
        let body = r#"{"saved":true,"mutation_id":"original"}"#;
        write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        String::from_utf8(raw).unwrap()
    });
    settings::update(temp.path(), |s| { s.devify.base_url = "http://127.0.0.1:1".into(); Ok(()) }).unwrap();
    let body = serde_json::json!({"mutation_id":"original","payload":{"usernames":["private-chat"]}});
    let result = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(
        fixed_identity_json(temp.path(), &base, "original-token", reqwest::Method::PUT, "/api/v1/spaces/original/desktop/sync-config", Some(&body))
    ).unwrap();
    assert_eq!(result["saved"],true);
    let raw = server.join().unwrap();
    assert!(raw.starts_with("PUT /api/v1/spaces/original/desktop/sync-config HTTP/1.1"));
    assert!(raw.to_lowercase().contains("authorization: bearer original-token"));
    let sent: serde_json::Value = serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(sent,body);
}

#[test]
fn retry_follows_remote_phase_instead_of_restarting_a_completed_batch() {
    use task_runtime::{retry_remote_action, RetryRemoteAction::*};
    for phase in ["created", "uploading"] {
        assert_eq!(retry_remote_action(phase).unwrap(), ResumeArchive);
    }
    assert_eq!(retry_remote_action("failed").unwrap(), RetryParsing);
    for phase in ["processing", "completed"] {
        assert_eq!(retry_remote_action(phase).unwrap(), Reconcile);
    }
    assert!(retry_remote_action("cancelled").is_err());
    assert!(retry_remote_action("").is_err());
}

#[test]
fn continuation_visits_unread_chats_first_then_only_same_scope_partial_chats() {
    let temp = tempfile::tempdir().unwrap();
    let owner = identity(temp.path(), 1);
    let mut value = settings::load(temp.path()).unwrap();
    value.runtime.sources.insert(owner.account_id.clone(), config_runtime::Source { scan_remaining: 8, ..Default::default() });
    let chunk = settings::InitialChunk { since: Some(1), until: Some(100),
        baseline: settings::SyncCursor { time: 99, local_id: 300, boundary_count: None },
        position: settings::SyncCursor { time: 20, local_id: 20, boundary_count: Some(1) },
        final_chunk: false, space_id: owner.space_id.clone(), base_url: owner.base_url.clone() };
    let mut other = chunk.clone(); other.space_id = "other-space".into();
    value.sync.initial_progress.insert(owner.account_id.clone(), [("partial".into(),chunk), ("other".into(),other)].into_iter().collect());
    let names = vec!["complete".into(),"partial".into(),"other".into()];
    assert!(task_runtime::unfinished_scan_names(&value,&owner,&names).is_empty());
    value.runtime.sources.get_mut(&owner.account_id).unwrap().scan_remaining = 0;
    assert_eq!(task_runtime::unfinished_scan_names(&value,&owner,&names),vec!["partial"]);
    assert!(task_runtime::unfinished_scan_names(&value,&owner,&["complete".into()]).is_empty());
    value.sync.initial_progress.clear();
    assert!(task_runtime::unfinished_scan_names(&value,&owner,&names).is_empty());
}
fn identity(data: &Path, user: i64) -> settings::UploadOwner {
    let owner = settings::UploadOwner {
        base_url: "http://127.0.0.1:18999".into(),
        space_id: "space".into(),
        account_id: "local".into(),
        user_id: user,
        device_id: "device".into(),
    };
    let token = format!(
        "x.{}.x",
        URL_SAFE_NO_PAD.encode(
            serde_json::json!({"user_id":user,"device_id":"device","exp":4102444800_i64})
                .to_string()
        )
    );
    let mut s = settings::load(data).unwrap();
    s.devify.base_url = owner.base_url.clone();
    s.devify.active_space_id = Some(owner.space_id.clone());
    s.devify.device_id = owner.device_id.clone();
    settings::save(data, &s).unwrap();
    desktop_credentials::save(
        data,
        &desktop_credentials::Credentials {
            base_url: owner.base_url.clone(),
            access: token,
            refresh: "local-test".into(),
            device_session: None,
            session_device_id: None,
        },
    )
    .unwrap();
    owner
}
#[test]
fn keys_and_plaintext_are_separate_after_account_switch_and_signout() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    let first = identity(data, 1);
    let root = local_scope::root(data);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("wechat-keys.json"), "old private key").unwrap();
    let cache = decrypt::cache_dir(data, "same-wechat");
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("private"), "old plaintext").unwrap();
    let second = identity(data, 2);
    assert!(!config_runtime::same_principal(&first, &second));
    assert_ne!(local_scope::root(data), root);
    assert!(!keys::store_path(data).exists());
    assert!(!decrypt::cache_dir(data, "same-wechat")
        .join("private")
        .exists());
    desktop_credentials::clear(data).unwrap();
    assert!(!keys::store_path(data).exists());
    identity(data, 1);
    assert_eq!(local_scope::root(data), root);
    assert!(decrypt::cache_dir(data, "same-wechat")
        .join("private")
        .exists());
}
#[test]
fn immutable_archive_is_verified_and_configuration_pause_survives_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let raw = b"original archive";
    let digest = hex::encode(sha2::Sha256::digest(raw));
    upload_archive::save(temp.path(), &digest, raw).unwrap();
    assert_eq!(upload_archive::load(temp.path(), &digest).unwrap(), raw);
    fs::write(
        temp.path()
            .join("upload-archives")
            .join(format!("{digest}.zip")),
        "changed",
    )
    .unwrap();
    assert!(upload_archive::load(temp.path(), &digest).is_err());
    settings::update(temp.path(), |s| {
        s.runtime.paused = true;
        Ok(())
    })
    .unwrap();
    assert!(settings::load(temp.path()).unwrap().runtime.paused);
}
#[test]
fn wechat_planner_builds_one_bounded_batch_for_multiple_chats_and_continues_after_completion() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    let owner = identity(data, 1);
    let cache = decrypt::cache_dir(data, &owner.account_id);
    fs::create_dir_all(cache.join("session")).unwrap();
    fs::create_dir_all(cache.join("message")).unwrap();
    let sessions = rusqlite::Connection::open(cache.join("session/session.db")).unwrap();
    sessions.execute_batch("CREATE TABLE Session(username TEXT,display_name TEXT,last_timestamp INTEGER);INSERT INTO Session VALUES('a','A',1000),('b','B',1000);").unwrap();
    let messages = rusqlite::Connection::open(cache.join("message/message_0.db")).unwrap();
    for name in ["a", "b"] {
        let table = format!("Msg_{}", query::md5_hex(name));
        messages.execute_batch(&format!("CREATE TABLE {table}(local_id INTEGER,server_id INTEGER,create_time INTEGER,message_content TEXT,local_type INTEGER);")).unwrap();
        for id in 1..=250 {
            messages
                .execute(
                    &format!("INSERT INTO {table} VALUES(?1,?1,?1,'text',1)"),
                    [id],
                )
                .unwrap();
        }
    }
    let mut value = settings::load(data).unwrap();
    value
        .chat
        .allowed_usernames
        .insert(owner.account_id.clone(), vec!["a".into(), "b".into()]);
    value.chat.initial_ranges.insert(
        owner.account_id.clone(),
        ["a", "b"]
            .into_iter()
            .map(|n| {
                (
                    n.into(),
                    settings::InitialRange {
                        since: Some(1),
                        until: None,
                    },
                )
            })
            .collect(),
    );
    value.sync.activate_owner(&owner);
    let names = vec!["a".into(), "b".into()];
    let mut first = plan_automatic_batch(data, &value, &owner, &names)
        .unwrap()
        .unwrap();
    let scope = first.scope.as_ref().unwrap();
    let prepared = prepare_upload(
        &cache,
        &owner.account_id,
        &owner.space_id,
        &owner.base_url,
        &scope.usernames,
        None,
        None,
        &scope.message_selection,
    )
    .unwrap();
    export_archive("wechat", &owner.account_id, &prepared);
    assert_eq!(scope.usernames.len(), 2);
    assert!(
        scope
            .message_selection
            .values()
            .map(Vec::len)
            .sum::<usize>()
            <= 200
    );
    assert!(value.sync.cursors.is_empty());
    first.status = "completed".into();
    value.upload_queue.push(first);
    apply_completed_uploads(&mut value);
    let second = plan_automatic_batch(data, &value, &owner, &names)
        .unwrap()
        .unwrap();
    assert!(second.scope.unwrap().message_selection["a"][0].local_id > 1);
    let page =
        query::sessions_page(&cache, Some(&names.iter().cloned().collect()), Some(1), 1).unwrap();
    assert_eq!(page.len(), 1);
}
#[test]
fn logout_invalidates_old_work_without_waiting_for_the_network() {
    let temp = tempfile::tempdir().unwrap();
    let owner = identity(temp.path(), 1);
    let token = desktop_credentials::load(temp.path(), &owner.base_url)
        .unwrap()
        .access;
    settings::update(temp.path(), |s| {
        s.active_account_id = owner.account_id.clone();
        Ok(())
    })
    .unwrap();
    let started = std::time::Instant::now();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            sign_out_local_session(temp.path()).await.unwrap();
        });
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(desktop_credentials::load(temp.path(), &owner.base_url).is_err());
    identity(temp.path(), 2);
    let current = settings::load(temp.path()).unwrap();
    assert!(validate_owner_state(
        &current,
        &owner,
        &desktop_credentials::load(temp.path(), &owner.base_url)
            .unwrap()
            .access
    )
    .is_err());
    assert!(request_generation(temp.path(), &owner.base_url, &token).is_err());
}

#[test]
fn whatsapp_automatic_batches_cover_every_message_without_advancing_before_completion() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("ChatStorage.sqlite");
    let conn = rusqlite::Connection::open(&source).unwrap();
    conn.execute_batch("CREATE TABLE ZWACHATSESSION(Z_PK INTEGER PRIMARY KEY,ZCONTACTJID TEXT,ZPARTNERNAME TEXT,ZLASTMESSAGEDATE REAL,ZLASTMESSAGETEXT TEXT,ZUNREADCOUNT INTEGER);CREATE TABLE ZWAMESSAGE(Z_PK INTEGER PRIMARY KEY,ZCHATSESSION INTEGER,ZISFROMME INTEGER,ZMESSAGETYPE INTEGER,ZMESSAGEDATE REAL,ZFROMJID TEXT,ZTEXT TEXT,ZMEDIAITEM INTEGER,ZSTANZAID TEXT);INSERT INTO ZWACHATSESSION VALUES(1,'chat@g.us','Group',1000,NULL,0);").unwrap();
    for id in 1..=450 {
        conn.execute(
            "INSERT INTO ZWAMESSAGE VALUES(?1,1,0,0,?1,'sender','text',NULL,?2)",
            rusqlite::params![id, format!("M{id}")],
        )
        .unwrap();
    }
    std::env::set_var("WECHATIFY_WHATSAPP_DB", &source);
    let data = temp.path().join("app");
    fs::create_dir_all(&data).unwrap();
    let mut owner = identity(&data, 1);
    owner.account_id = "10000000000@s.whatsapp.net".into();
    let mut value = settings::load(&data).unwrap();
    value.sync.activate_owner(&owner);
    value
        .chat
        .allowed_usernames
        .insert(owner.account_id.clone(), vec!["chat@g.us".into()]);
    value.chat.initial_ranges.insert(
        owner.account_id.clone(),
        HashMap::from([(
            "chat@g.us".into(),
            settings::InitialRange {
                since: Some(978307201),
                until: None,
            },
        )]),
    );
    let mut seen = HashSet::new();
    for round in 0..10 {
        let Some(mut job) =
            plan_automatic_batch(&data, &value, &owner, &["chat@g.us".into()]).unwrap()
        else {
            break;
        };
        if round == 0 {
            let scope = job.scope.as_ref().unwrap();
            let prepared = prepare_whatsapp_upload(
                &data,
                &owner.account_id,
                &owner.space_id,
                &owner.base_url,
                &scope.usernames,
                None,
                None,
                &scope.message_selection,
            )
            .unwrap();
            export_archive("whatsapp", &owner.account_id, &prepared);
        }
        for message in &job.scope.as_ref().unwrap().message_selection["chat@g.us"] {
            assert!(seen.insert(message.local_id), "an unchanged boundary must not be uploaded again");
        }
        assert!(
            job.scope
                .as_ref()
                .unwrap()
                .message_selection
                .values()
                .map(Vec::len)
                .sum::<usize>()
                <= 200
        );
        // A restart before completion retains the old confirmed position.
        let before = serde_json::to_value(&value.sync.cursors).unwrap();
        value.upload_queue.push(job.clone());
        assert_eq!(serde_json::to_value(&value.sync.cursors).unwrap(), before);
        value.upload_queue.last_mut().unwrap().status = "completed".into();
        apply_completed_uploads(&mut value);
        job.status = "completed".into();
    }
    assert_eq!(
        seen.len(),
        450,
        "a full bounded page must never be mistaken for the final page"
    );
    assert_eq!(
        value.sync.cursors[&owner.account_id]["chat@g.us"].local_id,
        450
    );
    assert!(plan_automatic_batch(&data,&value,&owner,&["chat@g.us".into()]).unwrap().is_none());
    // Old installed clients saved no exact boundary count. One confirmed replay
    // repairs that checkpoint; later idle scans must stop producing new jobs.
    value.sync.cursors.get_mut(&owner.account_id).unwrap().get_mut("chat@g.us").unwrap().boundary_count = None;
    let mut repair = plan_automatic_batch(&data,&value,&owner,&["chat@g.us".into()]).unwrap().unwrap();
    assert_eq!(repair.scope.as_ref().unwrap().message_selection["chat@g.us"].len(),1);
    repair.status = "completed".into(); value.upload_queue.push(repair);
    apply_completed_uploads(&mut value);
    assert_eq!(value.sync.cursors[&owner.account_id]["chat@g.us"].boundary_count,Some(1));
    assert!(plan_automatic_batch(&data,&value,&owner,&["chat@g.us".into()]).unwrap().is_none());
    std::env::remove_var("WECHATIFY_WHATSAPP_DB");
}

fn export_archive(platform: &str, account: &str, prepared: &PreparedUpload) {
    let Some(dir) = std::env::var_os("AIMYCHATS_E2E_ARTIFACT_DIR") else {
        return;
    };
    let manifest = serde_json::json!({"schema_version":format!("{platform}.v1"),"platform":platform,"source_namespace":account,"counts":{"conversations":prepared.conversations.len(),"messages":prepared.messages.len()},"scope":{"conversation_ids":prepared.preview.sessions.iter().map(|s|s.username.clone()).collect::<Vec<_>>()},"client":{"name":"AIMyChats","version":env!("CARGO_PKG_VERSION")}});
    let raw = build_upload_archive(
        &manifest,
        [
            ("conversations.jsonl", &prepared.conversations),
            ("messages.jsonl", &prepared.messages),
            ("message_parts.jsonl", &prepared.parts),
            ("avatars.jsonl", &prepared.avatars),
        ],
        prepared
            .avatar_files
            .iter()
            .chain(prepared.media_files.iter()),
    )
    .unwrap();
    let dir = PathBuf::from(dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{platform}.zip")), raw).unwrap();
}

#[test]
fn login_profiles_keep_selection_schedule_and_pause_separate() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    identity(data, 1);
    config_runtime::activate_profile(data).unwrap();
    settings::update(data, |s| {
        s.chat
            .allowed_usernames
            .insert("local".into(), vec!["private-one".into()]);
        s.chat.sync_interval_seconds = 300;
        s.runtime.paused = true;
        Ok(())
    })
    .unwrap();
    identity(data, 2);
    config_runtime::activate_profile(data).unwrap();
    let two = settings::load(data).unwrap();
    assert!(two.chat.allowed_usernames.is_empty());
    assert!(!two.sync.enabled);
    assert!(!two.runtime.paused);
    settings::update(data, |s| {
        s.chat
            .allowed_usernames
            .insert("local".into(), vec!["private-two".into()]);
        Ok(())
    })
    .unwrap();
    identity(data, 1);
    config_runtime::activate_profile(data).unwrap();
    let one = settings::load(data).unwrap();
    assert_eq!(one.chat.allowed_usernames["local"], vec!["private-one"]);
    assert_eq!(one.chat.sync_interval_seconds, 300);
    assert!(one.runtime.paused);
}

#[test]
fn reconfiguration_keeps_completed_browsing_scope_until_new_ack() {
    let mut value = settings::AppSettings::default();
    value
        .chat
        .allowed_usernames
        .insert("local".into(), vec!["new".into()]);
    value.runtime.sources.insert(
        "local".into(),
        config_runtime::Source {
            draft: true,
            active: Some(serde_json::json!({"setup_state":"completed","usernames":["old"]})),
            ..Default::default()
        },
    );
    assert_eq!(config_runtime::browse_names(&value, "local"), vec!["old"]);
    value.runtime.sources.get_mut("local").unwrap().active = None;
    assert!(config_runtime::browse_names(&value, "local").is_empty());
}

#[test]
fn space_switch_archives_old_profile_before_any_new_configuration_is_enqueued() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    identity(data, 1);
    config_runtime::activate_profile(data).unwrap();
    settings::update(data, |s| {
        s.chat
            .allowed_usernames
            .insert("local".into(), vec!["private-old-space".into()]);
        s.devify.active_space_id = Some("new-space".into());
        config_runtime::activate_settings_profile(data, s)?;
        assert!(s.chat.allowed_usernames.is_empty());
        assert!(s.runtime.sources.is_empty());
        assert!(s.runtime.pending.is_empty());
        Ok(())
    })
    .unwrap();
    settings::update(data, |s| {
        s.devify.active_space_id = Some("space".into());
        config_runtime::activate_settings_profile(data, s)?;
        assert_eq!(s.chat.allowed_usernames["local"], vec!["private-old-space"]);
        Ok(())
    })
    .unwrap();
}

#[test]
fn interrupted_logout_journal_is_completed_before_credentials_can_be_reused() {
    let temp = tempfile::tempdir().unwrap();
    let owner = identity(temp.path(), 1);
    settings::update(temp.path(), |s| {
        s.runtime.logout_pending = true;
        Ok(())
    })
    .unwrap();
    recover_local_logout(temp.path()).unwrap();
    assert!(desktop_credentials::load(temp.path(), &owner.base_url).is_err());
    assert!(!settings::load(temp.path()).unwrap().runtime.logout_pending);
    recover_local_logout(temp.path()).unwrap();
}

#[test]
fn a_late_source_error_does_not_poison_another_login_profile() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    let owner = identity(data, 1);
    config_runtime::activate_profile(data).unwrap();
    let revision = settings::load(data).unwrap().user_settings_revision;
    let current = identity(data, 2);
    config_runtime::activate_profile(data).unwrap();
    settings::update(data, |s| {
        s.runtime.sources.insert(
            "local".into(),
            config_runtime::Source {
                owner: Some(current.clone()),
                completed: true,
                acknowledged: Some(serde_json::json!({})),
                ..Default::default()
            },
        );
        Ok(())
    })
    .unwrap();
    task_runtime::record_source_result(
        data,
        &owner,
        revision,
        &Err("old user's failed upload".into()),
    )
    .unwrap();
    let s = settings::load(data).unwrap();
    assert!(s.runtime.sources["local"].last_error.is_none());
    assert_eq!(s.runtime.sources["local"].failures, 0);
}

#[test]
fn an_idle_or_server_processing_source_does_not_start_local_execution() {
    let temp = tempfile::tempdir().unwrap();
    let owner = identity(temp.path(), 1);
    let mut s = settings::load(temp.path()).unwrap();
    s.backup.mode = "interval".into();
    s.chat.sync_interval_seconds = 1800;
    s.runtime.sources.insert(
        owner.account_id.clone(),
        config_runtime::Source {
            last_run_at: Some(Utc::now().to_rfc3339()),
            ..Default::default()
        },
    );
    assert!(!task_runtime::source_needs_execution(
        &s,
        &owner,
        Local::now()
    ));
    s.runtime
        .sources
        .get_mut(&owner.account_id)
        .unwrap()
        .last_run_at = None;
    assert!(task_runtime::source_needs_execution(
        &s,
        &owner,
        Local::now()
    ));
    s.upload_queue.push(settings::UploadJob {
        id: "server-job".into(),
        owner: Some(owner.clone()),
        status: "processing".into(),
        batch_id: Some(1),
        ..Default::default()
    });
    assert!(!task_runtime::source_needs_execution(
        &s,
        &owner,
        Local::now()
    ));
    s.upload_queue[0].message = "上传中断，等待恢复".into();
    assert!(task_runtime::source_needs_execution(
        &s,
        &owner,
        Local::now()
    ));
}

#[test]
fn missing_wechat_transcript_does_not_block_other_selected_chats_or_advance_its_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    let owner = identity(data, 1);
    let cache = decrypt::cache_dir(data, &owner.account_id);
    fs::create_dir_all(cache.join("message")).unwrap();
    let db = cache.join("message/message_0.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    let table = format!("Msg_{}", query::md5_hex("present"));
    conn.execute_batch(&format!("CREATE TABLE {table}(local_id INTEGER,server_id INTEGER,create_time INTEGER,message_content TEXT,local_type INTEGER);INSERT INTO {table} VALUES(1,1,100,'text',1);")).unwrap();
    let mut value = settings::load(data).unwrap();
    value.chat.initial_ranges.insert(owner.account_id.clone(), [("present".into(), settings::InitialRange { since: Some(1), until: None })].into_iter().collect());
    let job = plan_automatic_batch(data, &value, &owner, &["missing".into(), "present".into()]).unwrap().unwrap();
    assert_eq!(job.scope.as_ref().unwrap().usernames, vec!["present"]);
    assert!(!job.initial_chunks.contains_key("missing"));
    assert!(value.sync.cursors.is_empty());
    drop(conn);
    fs::write(&db, b"not a sqlite database").unwrap();
    assert!(plan_automatic_batch(data, &value, &owner, &["missing".into(), "present".into()]).is_err());
}

#[test]
fn backup_changes_only_invalidate_affected_accounts() {
    let wechat = "wxid_desktop";
    let whatsapp = "123@s.whatsapp.net";
    let before = settings::AppSettings::default();
    let mut after = before.clone();
    after
        .chat
        .allowed_usernames
        .insert(whatsapp.into(), vec!["chat".into()]);
    assert!(!shared_backup_changed(&before, &after));
    assert!(account_backup_changed(&before, &after, whatsapp));
    assert!(!account_backup_changed(&before, &after, wechat));
    after.upload_queue = vec![
        settings::UploadJob {
            account_id: wechat.into(),
            status: "queued".into(),
            create_started: Some(false),
            ..Default::default()
        },
        settings::UploadJob {
            account_id: whatsapp.into(),
            status: "queued".into(),
            create_started: Some(false),
            ..Default::default()
        },
        settings::UploadJob {
            account_id: whatsapp.into(),
            status: "uploading".into(),
            create_started: Some(true),
            batch_id: Some(2),
            ..Default::default()
        },
    ];
    invalidate_unsent_backups(&before, &mut after);
    assert_eq!(after.upload_queue[0].status, "queued");
    assert_eq!(after.upload_queue[1].status, "cancelled");
    assert_eq!(after.upload_queue[2].status, "uploading");

    after = before.clone();
    after.connectors.waitlist.push("Slack".into());
    assert!(!account_backup_changed(&before, &after, whatsapp));
    assert!(!account_backup_changed(&before, &after, wechat));
    after.backup.attachments = if before.backup.attachments == "text" {
        "all"
    } else {
        "text"
    }
    .into();
    assert!(shared_backup_changed(&before, &after));
    assert!(account_backup_changed(&before, &after, whatsapp));
    assert!(account_backup_changed(&before, &after, wechat));
}


#[test]
fn backup_switch_legacy_states_never_silently_enable_uploads() {
    for enabled in [false, true] {
        for paused in [false, true] {
            let mut value = settings::AppSettings::default();
            value.sync.enabled = enabled;
            value.runtime.paused = paused;
            let effective = config_runtime::backup_enabled(&value);
            assert_eq!(effective, enabled && !paused);
            assert_eq!(config_runtime::payload(&value, "local", false)["enabled"], effective);
            config_runtime::set_backup_enabled(&mut value, effective);
            assert_eq!(value.sync.enabled, effective);
            assert_eq!(value.runtime.paused, !effective);
        }
    }
}

#[test]
fn backup_switch_repairs_real_state_and_durably_submits_each_owned_source() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    let owner = identity(data, 1);
    let mut foreign = owner.clone();
    foreign.account_id = "foreign".into();
    foreign.user_id = 2;
    settings::update(data, |s| {
        // This is the production failure: UI unpaused, real switch disabled.
        s.sync.enabled = false;
        s.runtime.paused = false;
        s.chat.allowed_usernames.insert(owner.account_id.clone(), vec!["chosen".into()]);
        s.runtime.sources.insert(owner.account_id.clone(), config_runtime::Source {
            owner: Some(owner.clone()), completed: true,
            acknowledged: Some(config_runtime::payload(s, &owner.account_id, false)),
            ..Default::default()
        });
        s.runtime.sources.insert("wa@s.whatsapp.net".into(), config_runtime::Source {
            owner: Some(settings::UploadOwner { account_id: "wa@s.whatsapp.net".into(), ..owner.clone() }),
            draft: true, wizard_step: Some("sessions".into()), ..Default::default()
        });
        s.runtime.sources.insert("foreign".into(), config_runtime::Source {
            owner: Some(foreign.clone()), completed: true, ..Default::default()
        });
        Ok(())
    }).unwrap();
    let revision = settings::load(data).unwrap().user_settings_revision;
    runtime_commands::set_backup_paused(data, false).unwrap();
    let opened = settings::load(data).unwrap();
    assert!(config_runtime::backup_enabled(&opened));
    assert!(opened.user_settings_revision > revision);
    assert_eq!(opened.runtime.pending.len(), 2);
    assert!(opened.runtime.pending.iter().all(|m| m.owner.user_id == 1));
    let request = opened.runtime.pending.iter().find(|m| m.owner.account_id == owner.account_id).unwrap();
    assert_eq!(request.body["payload"]["enabled"], true);
    assert_eq!(request.body["payload"]["usernames"], serde_json::json!(["chosen"]));
    assert!(!config_runtime::ready(&opened, &owner), "no execution before configuration acknowledgement");
    assert!(opened.runtime.sources["wa@s.whatsapp.net"].draft);
    assert!(!opened.runtime.sources["wa@s.whatsapp.net"].completed);
    runtime_commands::set_backup_paused(data, false).unwrap();
    assert_eq!(settings::load(data).unwrap().runtime.pending.len(), 2, "repeated enabling does not duplicate configuration tasks");
    // Acknowledge exactly the immutable requests the service would confirm.
    settings::update(data, |s| {
        for m in s.runtime.pending.drain(..) {
            let source = s.runtime.sources.get_mut(&m.owner.account_id).unwrap();
            source.acknowledged = Some(m.body["payload"].clone());
        }
        Ok(())
    }).unwrap();
    let ready = settings::load(data).unwrap();
    assert!(config_runtime::ready(&ready, &owner));
    assert!(task_runtime::source_needs_execution(&ready, &owner, Local::now()));
    runtime_commands::set_backup_paused(data, true).unwrap();
    let paused = settings::load(data).unwrap();
    assert!(!config_runtime::backup_enabled(&paused));
    assert!(!paused.sync.enabled);
    assert!(paused.runtime.paused);
    assert!(!config_runtime::ready(&paused, &owner));
    assert!(paused.runtime.pending.iter().all(|m| m.body["payload"]["enabled"] == false));
}

#[test]
fn backup_switch_submission_failure_rolls_back_the_switch_with_its_outbox() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    let owner = identity(data, 1);
    settings::update(data, |s| {
        s.chat.allowed_usernames.insert(owner.account_id.clone(), vec!["chosen".into()]);
        s.runtime.sources.insert(owner.account_id.clone(), config_runtime::Source {
            owner: Some(owner.clone()), completed: true, ..Default::default()
        });
        for n in 0..128 {
            s.runtime.pending.push(config_runtime::Mutation {
                owner: owner.clone(), body: serde_json::json!({"mutation_id":n,"payload":{"enabled":false}})
            });
        }
        Ok(())
    }).unwrap();
    let before = settings::load(data).unwrap();
    assert!(runtime_commands::set_backup_paused(data, false).is_err());
    let after = settings::load(data).unwrap();
    assert_eq!(after.settings_revision, before.settings_revision);
    assert!(!after.sync.enabled);
    assert_eq!(after.runtime.pending.len(), 128);
}

#[test]
fn backup_switch_is_restored_only_for_its_own_login_profile() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    identity(data, 1);
    config_runtime::activate_profile(data).unwrap();
    runtime_commands::set_backup_paused(data, false).unwrap();
    assert!(config_runtime::backup_enabled(&settings::load(data).unwrap()));
    identity(data, 2);
    config_runtime::activate_profile(data).unwrap();
    assert!(!config_runtime::backup_enabled(&settings::load(data).unwrap()));
    runtime_commands::set_backup_paused(data, true).unwrap();
    identity(data, 1);
    config_runtime::activate_profile(data).unwrap();
    assert!(config_runtime::backup_enabled(&settings::load(data).unwrap()));
}


#[test]
fn backup_pause_reserves_a_stop_intent_without_rewriting_a_full_outbox() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    let owner = identity(data, 1);
    settings::update(data, |s| {
        config_runtime::set_backup_enabled(s, true);
        s.chat.allowed_usernames.insert(owner.account_id.clone(), vec!["chosen".into()]);
        s.runtime.sources.insert(owner.account_id.clone(), config_runtime::Source {
            owner: Some(owner.clone()), completed: true,
            acknowledged: Some(config_runtime::payload(s, &owner.account_id, false)),
            ..Default::default()
        });
        let mut old_owner = owner.clone();
        old_owner.user_id = 2;
        for n in 0..128 {
            s.runtime.pending.push(config_runtime::Mutation {
                owner: old_owner.clone(), body: serde_json::json!({"mutation_id":n,"payload":{"enabled":true}})
            });
        }
        Ok(())
    }).unwrap();
    let before = settings::load(data).unwrap();
    assert!(config_runtime::ready(&before, &owner));
    runtime_commands::set_backup_paused(data, true).unwrap();
    let after = settings::load(data).unwrap();
    assert!(!config_runtime::backup_enabled(&after));
    assert!(!config_runtime::ready(&after, &owner));
    assert_eq!(after.runtime.pending.len(), 129);
    assert_eq!(serde_json::to_value(&after.runtime.pending[..128]).unwrap(),
        serde_json::to_value(&before.runtime.pending).unwrap(), "never discard or rewrite old requests");
    assert!(config_runtime::same_principal(&after.runtime.pending[128].owner, &owner));
    assert_eq!(after.runtime.pending[128].body["payload"]["enabled"], false);
    runtime_commands::set_backup_paused(data, true).unwrap();
    assert_eq!(settings::load(data).unwrap().runtime.pending.len(), 129, "repeated pauses deduplicate");
    assert!(runtime_commands::set_backup_paused(data, false).is_err(), "normal enabling is still capacity-limited");
    assert!(!config_runtime::backup_enabled(&settings::load(data).unwrap()));
    // Other edits cannot use the stop reserve to grow the disabled queue forever.
    assert!(settings::update(data, |s| {
        s.backup.wifi_only = !s.backup.wifi_only;
        config_runtime::enqueue_current_sources(data, s, false)
    }).is_err());
    let restored = settings::load(data).unwrap();
    assert_eq!(restored.backup.wifi_only, after.backup.wifi_only);
    assert_eq!(restored.runtime.pending.len(), 129);
}
