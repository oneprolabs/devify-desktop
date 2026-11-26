use super::*;
use task_runtime::WechatBackupRecovery::{Blocked, Resumed, Unchanged};

#[test]
fn verified_wechat_keys_resume_only_the_affected_backup_from_its_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    let owner = identity(data, 1);
    let now = Local::now();
    let last_run = now.to_rfc3339();
    let message_db = "message/message_0.db";
    let session_db = "session/session.db";
    let wa_account = "123@s.whatsapp.net";
    let cursor = settings::SyncCursor {
        time: 100,
        local_id: 2,
        boundary_count: Some(1),
    };
    let original = settings::update(data, |value| {
        value.backup.mode = "interval".into();
        value.chat.sync_interval_seconds = 1800;
        value.sync.cursors.insert(
            owner.account_id.clone(),
            HashMap::from([("chat".to_owned(), cursor)]),
        );
        value.runtime.sources.insert(
            owner.account_id.clone(),
            config_runtime::Source {
                owner: Some(owner.clone()),
                completed: true,
                acknowledged: Some(serde_json::json!({})),
                last_run_at: Some(last_run.clone()),
                ..Default::default()
            },
        );
        value.runtime.sources.insert(
            wa_account.into(),
            config_runtime::Source {
                last_error: Some("independent source error".into()),
                ..Default::default()
            },
        );
        Ok(())
    })
    .unwrap();
    assert!(!task_runtime::source_needs_execution(
        &original, &owner, now, false
    ));

    let outcome = |rel: &str, status: &str| DecryptOutcome {
        account: owner.account_id.clone(),
        rel: rel.into(),
        status: status.into(),
        pages: 1,
        failed_page: None,
        error: None,
        wal: None,
        output: None,
    };
    let verified = [
        outcome(session_db, "ok"),
        outcome(message_db, "cached"),
        outcome("head_image/head_image.db", "no-key"),
    ];
    let key_failure = Err(task_runtime::SourceTickError::WechatKey(message_db.into()));
    let record = |result: &Result<(), task_runtime::SourceTickError>| {
        task_runtime::record_source_result(
            data,
            &owner,
            original.user_settings_revision,
            false,
            result,
        )
        .unwrap();
        settings::load(data).unwrap()
    };
    let first = record(&key_failure);
    let source = &first.runtime.sources[&owner.account_id];
    assert!(!task_runtime::wechat_backup_needs_keys(
        &owner.account_id,
        source
    ));
    assert!(source.retry_after.is_some());
    assert!(
        task_runtime::source_needs_execution(&first, &owner, now, false),
        "retry without waiting for the regular interval"
    );
    assert!(
        matches!(
            task_runtime::resume_wechat_backup(data, &owner, &verified).unwrap(),
            Resumed
        ),
        "the next successful read should recover without user action"
    );
    settings::update(data, |value| {
        value
            .runtime
            .sources
            .get_mut(&owner.account_id)
            .unwrap()
            .key_recovery_pending = false;
        Ok(())
    })
    .unwrap();

    record(&key_failure);
    let read_failure = record(&Err(task_runtime::SourceTickError::Backup(
        "temporary read error".into(),
    )));
    assert!(!task_runtime::wechat_backup_needs_keys(
        &owner.account_id,
        &read_failure.runtime.sources[&owner.account_id]
    ));
    let first_again = record(&key_failure);
    assert!(!task_runtime::wechat_backup_needs_keys(
        &owner.account_id,
        &first_again.runtime.sources[&owner.account_id]
    ));
    let blocked = record(&key_failure);
    let source = &blocked.runtime.sources[&owner.account_id];
    assert!(task_runtime::wechat_backup_needs_keys(
        &owner.account_id,
        source
    ));
    let blocked_after_read_error = record(&Err(task_runtime::SourceTickError::Backup(
        "temporary read error after confirmation".into(),
    )));
    let source = &blocked_after_read_error.runtime.sources[&owner.account_id];
    assert!(
        task_runtime::wechat_backup_needs_keys(&owner.account_id, source),
        "a temporary read failure must retain an already-confirmed recovery action"
    );
    let retry_after = source.retry_after.clone();
    let later_shard = "message/message_1.db";
    for invalid in [
        vec![],
        vec![outcome(message_db, "ok")],
        vec![
            outcome(session_db, "ok"),
            outcome(message_db, "ok"),
            outcome(later_shard, "no-key"),
        ],
        vec![outcome(session_db, "ok"), outcome(message_db, "failed")],
        vec![
            outcome(session_db, "ok"),
            outcome(message_db, "ok"),
            outcome("message/media_0.db", "failed"),
        ],
    ] {
        assert!(matches!(
            task_runtime::resume_wechat_backup(data, &owner, &invalid).unwrap(),
            Blocked(_)
        ));
    }
    let mixed = [
        outcome(session_db, "ok"),
        outcome(message_db, "no-key"),
        outcome(later_shard, "failed"),
    ];
    assert!(matches!(
        task_runtime::verify_wechat_backup_read(&owner.account_id, Some(message_db), &mixed),
        Err(task_runtime::WechatBackupReadError::Read(_))
    ));
    assert_eq!(
        settings::load(data).unwrap().runtime.sources[&owner.account_id].retry_after,
        retry_after
    );
    assert_eq!(
        serde_json::to_value(&settings::load(data).unwrap().sync.cursors).unwrap(),
        serde_json::to_value(&original.sync.cursors).unwrap()
    );
    let other = settings::UploadOwner {
        user_id: 2,
        ..owner.clone()
    };
    assert!(matches!(
        task_runtime::resume_wechat_backup(data, &other, &verified).unwrap(),
        Unchanged
    ));
    assert!(matches!(
        task_runtime::resume_wechat_backup(data, &owner, &verified).unwrap(),
        Resumed
    ));
    assert!(matches!(
        task_runtime::resume_wechat_backup(data, &owner, &verified).unwrap(),
        Unchanged
    ));

    // An error from a scan that started before key recovery must not reapply backoff.
    record(&key_failure);
    let mut resumed = settings::load(data).unwrap();
    let source = &resumed.runtime.sources[&owner.account_id];
    assert!(source.key_recovery_pending);
    assert_eq!(source.failures, 0);
    assert!(source.retry_after.is_none());
    assert!(source.last_error.is_none());
    assert_eq!(source.last_run_at.as_deref(), Some(last_run.as_str()));
    assert_eq!(
        resumed.runtime.sources[wa_account].last_error.as_deref(),
        Some("independent source error")
    );
    assert_eq!(
        serde_json::to_value(&resumed.sync.cursors).unwrap(),
        serde_json::to_value(&original.sync.cursors).unwrap()
    );
    assert!(task_runtime::source_needs_execution(
        &resumed, &owner, now, false
    ));
    let cache = decrypt::cache_dir(data, &owner.account_id);
    fs::create_dir_all(cache.join("message")).unwrap();
    let conn = rusqlite::Connection::open(cache.join("message/message_0.db")).unwrap();
    let table = format!("Msg_{}", query::md5_hex("chat"));
    conn.execute_batch(&format!(
        "CREATE TABLE {table}(local_id INTEGER,server_id INTEGER,create_time INTEGER,message_content TEXT,local_type INTEGER);
         INSERT INTO {table} VALUES(2,2,100,'already uploaded',1),(3,3,101,'still pending',1);"
    ))
    .unwrap();
    let job = plan_automatic_batch(data, &resumed, &owner, &["chat".into()])
        .unwrap()
        .unwrap();
    let messages = &job.scope.as_ref().unwrap().message_selection["chat"];
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].local_id, 3);

    resumed.backup.window_enabled = true;
    resumed.backup.days.clear();
    assert!(!task_runtime::source_needs_execution(
        &resumed, &owner, now, false
    ));
    resumed.backup.window_enabled = false;
    resumed.backup.mode = "manual".into();
    assert!(!task_runtime::source_needs_execution(
        &resumed, &owner, now, false
    ));
}

#[test]
fn previously_failed_shard_must_reappear_before_backup_can_resume() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    let owner = identity(data, 1);
    let missing = "message/message_1.db";
    let original = settings::update(data, |value| {
        value.runtime.sources.insert(
            owner.account_id.clone(),
            config_runtime::Source {
                owner: Some(owner.clone()),
                completed: true,
                acknowledged: Some(serde_json::json!({})),
                wechat_key_failure: Some(config_runtime::WechatKeyFailure {
                    database: missing.into(),
                    confirmed: true,
                    interrupted: false,
                }),
                ..Default::default()
            },
        );
        Ok(())
    })
    .unwrap();
    let outcome = |rel: &str| DecryptOutcome {
        account: owner.account_id.clone(),
        rel: rel.into(),
        status: "ok".into(),
        pages: 1,
        failed_page: None,
        error: None,
        wal: None,
        output: None,
    };
    let partial = [
        outcome("session/session.db"),
        outcome("message/message_0.db"),
    ];
    // This is the read gate used before the scheduler plans a new batch.
    let result =
        task_runtime::verify_wechat_backup_read(&owner.account_id, Some(missing), &partial);
    let Err(task_runtime::WechatBackupReadError::Read(error)) = result else {
        panic!("partial discovery must not be accepted as recovered");
    };
    task_runtime::record_source_result(
        data,
        &owner,
        original.user_settings_revision,
        false,
        &Err(task_runtime::SourceTickError::Backup(error)),
    )
    .unwrap();
    let stored = settings::load(data).unwrap();
    let source = &stored.runtime.sources[&owner.account_id];
    assert_eq!(
        source.wechat_key_failure.as_ref().unwrap().database,
        missing
    );
    assert!(
        task_runtime::wechat_backup_needs_keys(&owner.account_id, source),
        "the recovery action stays available while a required shard is missing"
    );
    let blocked = || task_runtime::resume_wechat_backup(data, &owner, &partial).unwrap();
    assert!(matches!(
        blocked(),
        Blocked(task_runtime::WechatBackupReadError::Read(_))
    ));
    assert!(
        matches!(blocked(), Blocked(_)),
        "the next retry must not forget the missing shard"
    );
    assert!(!settings::load(data).unwrap().runtime.sources[&owner.account_id].key_recovery_pending);

    assert_eq!(
        settings::load(data).unwrap().settings_revision,
        stored.settings_revision
    );
    let complete = [partial[0].clone(), partial[1].clone(), outcome(missing)];
    assert!(matches!(
        task_runtime::resume_wechat_backup(data, &owner, &complete).unwrap(),
        Resumed
    ));
}
