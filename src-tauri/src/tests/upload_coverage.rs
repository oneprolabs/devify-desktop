use super::upload_coverage;
use crate::settings::{self, SyncCursor, UploadJob, UploadScope};
use std::collections::BTreeMap;

#[test]
fn batch_plan_preserves_same_second_and_shard_identity() {
    use crate::settings::MessageRef;
    let make = |time, local_id, source_shard: &str| MessageRef {
        time,
        local_id,
        source_shard: source_shard.into(),
    };
    let messages = vec![
        make(20, 2, "b"),
        make(10, 1, "b"),
        make(10, 1, "a"),
        make(20, 2, "a"),
        make(30, 3, "a"),
    ];
    let first = super::plan_sync_batch(&messages, None, 3).unwrap();
    assert_eq!(first.messages, vec![make(10, 1, "a"), make(10, 1, "b")]);
    assert!(first.has_more);
    let second = super::plan_sync_batch(
        &messages,
        Some(SyncCursor {
            time: 10,
            local_id: 1,
            boundary_count: Some(2),
        }),
        3,
    )
    .unwrap();
    assert_eq!(
        second.messages,
        vec![make(20, 2, "a"), make(20, 2, "b"), make(30, 3, "a")]
    );
    assert!(!second.has_more);
}

#[test]
fn batch_plan_refuses_to_split_oversized_second() {
    use crate::settings::MessageRef;
    let messages: Vec<_> = (0..4)
        .map(|id| MessageRef {
            time: 10,
            local_id: id,
            source_shard: "a".into(),
        })
        .collect();
    assert!(super::plan_sync_batch(&messages, None, 3).is_err());
}

#[test]
fn oversized_manual_upload_is_rejected_before_submission() {
    assert!(super::validate_upload_batch_size(1000).is_ok());
    assert!(super::validate_upload_batch_size(1001).is_err());
}

fn owner() -> settings::UploadOwner {
    settings::UploadOwner {
        base_url: "https://example.test".into(),
        space_id: "1".into(),
        account_id: "acct".into(),
        user_id: 7,
        device_id: "device".into(),
    }
}

#[test]
fn upload_recovery_reuses_an_archive_from_before_a_client_upgrade() {
    use sha2::{Digest, Sha256};

    let records = [serde_json::json!({"message": "synthetic test"})];
    let build = |version: &str| {
        super::build_upload_archive(
            &serde_json::json!({"client": {"name": "AIMyChats", "version": version}}),
            [
                ("conversations.jsonl", &[]),
                ("messages.jsonl", &records),
                ("message_parts.jsonl", &[]),
                ("avatars.jsonl", &[]),
            ],
            std::iter::empty(),
        )
    };
    let before = build("0.1.7").unwrap();
    let digest = hex::encode(Sha256::digest(&before));
    let (after, _, version) = super::select_upload_archive(Some(&digest), None, build).unwrap();
    assert_eq!(version, "0.1.7");
    assert_eq!(after, before);
    assert!(super::select_upload_archive(Some(&digest), Some("0.1.6"), build).is_err());
    assert!(super::select_upload_archive(Some("different-digest"), None, build).is_err());
}

#[test]
fn manual_sync_recovers_only_its_own_unlinked_failed_batch() {
    let owner = owner();
    let mut job = UploadJob {
        id: "sync-old".into(),
        status: "failed".into(),
        owner: Some(owner.clone()),
        create_started: Some(true),
        package_digest: Some("archive".into()),
        scope: Some(UploadScope {
            usernames: vec!["one".into()],
            message_selection: BTreeMap::from([(
                "one".into(),
                vec![settings::MessageRef {
                    time: 10,
                    local_id: 1,
                    source_shard: "shard".into(),
                }],
            )]),
            base_url: owner.base_url.clone(),
            since: None,
            until: None,
        }),
        ..Default::default()
    };
    assert!(super::recoverable_sync_job(&job, &owner, "one"));
    assert!(!super::recoverable_sync_job(&job, &owner, "another"));
    let other = settings::UploadOwner {
        user_id: owner.user_id + 1,
        ..owner.clone()
    };
    assert!(!super::recoverable_sync_job(&job, &other, "one"));
    job.legacy_failure_confirmed = true;
    assert!(!super::recoverable_sync_job(&job, &owner, "one"));
    assert!(super::job_never_blocks_sync(&job));
}

#[test]
fn saved_upload_states_do_not_block_a_later_sync() {
    let owner = owner();
    let scope_for = |name: &str| UploadScope {
        usernames: vec![name.into()],
        since: None,
        until: None,
        base_url: owner.base_url.clone(),
        message_selection: BTreeMap::new(),
    };
    let mut failed = UploadJob {
        id: "sync-old".into(),
        status: "failed".into(),
        owner: Some(owner.clone()),
        account_id: owner.account_id.clone(),
        space_id: owner.space_id.clone(),
        create_started: Some(true),
        package_digest: Some("archive".into()),
        scope: Some(scope_for("one")),
        ..Default::default()
    };
    assert!(!super::upload_job_blocks_username(&failed, &owner, "one"));
    failed.batch_id = Some(4);
    failed.status = "processing".into();
    assert!(!super::upload_job_blocks_username(&failed, &owner, "one"));
    failed.status = "running".into();
    failed.batch_id = None;
    failed.create_started = Some(true);
    assert!(super::abandon_interrupted_sync_job(&mut failed));
    assert_eq!(failed.status, "failed");
    assert!(!failed.legacy_failure_confirmed);
    failed.create_started = Some(false);
    failed.status = "running".into();
    assert!(super::abandon_interrupted_sync_job(&mut failed));
    assert!(failed.legacy_failure_confirmed);
    assert!(!super::upload_job_blocks_username(&failed, &owner, "one"));
    let mut manual = UploadJob {
        id: "manual-0".into(),
        status: "queued".into(),
        owner: Some(owner.clone()),
        account_id: owner.account_id.clone(),
        space_id: owner.space_id.clone(),
        create_started: Some(false),
        manual_group: Some("manual".into()),
        scope: Some(scope_for("one")),
        ..Default::default()
    };
    assert!(super::upload_job_blocks_username(&manual, &owner, "one"));
    assert!(!super::upload_job_blocks_username(
        &manual, &owner, "another"
    ));
    manual.scope = Some(UploadScope {
        usernames: Vec::new(),
        ..scope_for("one")
    });
    assert!(!super::upload_job_blocks_username(&manual, &owner, "one"));
    manual.scope = None;
    assert!(!super::upload_job_blocks_username(&manual, &owner, "one"));
}

#[test]
fn a_progress_conflict_is_retried_and_other_errors_are_not() {
    assert!(super::sync_progress_conflict(
        "同步进度已改变，请重新同步。"
    ));
    assert!(!super::sync_progress_conflict("服务端登录状态已改变。"));
    assert!(!super::sync_progress_conflict(
        "zhangminhaian：同步进度已改变，请重新同步。"
    ));
}

#[test]
fn switching_login_does_not_pause_automatic_upload() {
    assert!(!super::should_pause_sync_for_owner_change(false, false));
    assert!(!super::should_pause_sync_for_owner_change(true, true));
    assert!(super::should_pause_sync_for_owner_change(true, false));
}

fn login_owner(user_id: i64, space_id: &str) -> settings::UploadOwner {
    settings::UploadOwner {
        base_url: "https://app.aimychats.com".into(),
        space_id: space_id.into(),
        account_id: "wechat".into(),
        user_id,
        device_id: "this-mac".into(),
    }
}

fn remember(sync: &mut settings::SyncSettings, chat: &str, time: i64) {
    sync.cursors.entry("wechat".into()).or_default().insert(
        chat.into(),
        SyncCursor {
            time,
            local_id: 1,
            boundary_count: Some(1),
        },
    );
}

/// Applies the same owner check the backup uses. Manual sync without saved progress
/// stops for confirmation and leaves the previous login's cursors in place.
fn switch_login(
    current: &mut settings::AppSettings,
    owner: &settings::UploadOwner,
    manual: bool,
) -> &'static str {
    let mismatch = super::validate_sync_owner(current, owner).is_err();
    if mismatch
        && !current.sync.has_saved_progress(owner)
        && super::should_pause_sync_for_owner_change(manual, false)
    {
        return "手动同步先确认首次范围，进度保持不动";
    }
    current.sync.activate_owner(owner);
    if current
        .sync
        .cursors
        .get(&owner.account_id)
        .is_some_and(|items| !items.is_empty())
    {
        "自动备份改用这个登录，并恢复它自己的进度"
    } else {
        "自动备份改用这个登录，没有旧进度，按起始时间重新采集"
    }
}

#[test]
fn simulated_account_switches_keep_each_login_separate() {
    let user_a = login_owner(96, "space-a");
    let user_b = login_owner(89, "space-b");
    let user_c = login_owner(120, "space-c");
    let user_a_other_space = login_owner(96, "space-a-other");
    let mut current = settings::AppSettings::default();
    current.active_account_id = "wechat".into();
    current
        .sync
        .owners
        .insert(user_a.account_id.clone(), user_a.clone());
    current.sync.last_error = Some(super::SYNC_OWNER_PAUSE.into());
    current.sync.last_status = Some("failed".into());
    remember(&mut current.sync, "会话甲", 100);
    remember(&mut current.sync, "会话乙", 200);
    remember(&mut current.sync, "会话丙", 300);

    println!("开始：用户96 空间A 有会话甲/乙/丙，备份页是红色暂停");
    let switched_to_b = switch_login(&mut current, &user_b, false);
    println!("用户89 自动备份：{switched_to_b}");
    assert_eq!(
        switched_to_b,
        "自动备份改用这个登录，没有旧进度，按起始时间重新采集"
    );
    assert_eq!(current.sync.owners.get("wechat"), Some(&user_b));
    assert!(current.sync.last_error.is_none());
    assert!(current.sync.cursors.get("wechat").is_none());
    assert!(current.sync.has_saved_progress(&user_a));
    remember(&mut current.sync, "会话甲", 900);

    println!("用户89 上传后，会话甲停在 900");
    let manual_c = switch_login(&mut current, &user_c, true);
    println!("用户120 手动同步：{manual_c}");
    assert_eq!(manual_c, "手动同步先确认首次范围，进度保持不动");
    assert_eq!(current.sync.owners.get("wechat"), Some(&user_b));
    assert_eq!(
        current
            .sync
            .cursors
            .get("wechat")
            .unwrap()
            .get("会话甲")
            .unwrap()
            .time,
        900
    );

    let switched_to_c = switch_login(&mut current, &user_c, false);
    println!("用户120 自动备份：{switched_to_c}");
    assert_eq!(
        switched_to_c,
        "自动备份改用这个登录，没有旧进度，按起始时间重新采集"
    );
    assert!(current.sync.has_saved_progress(&user_b));
    assert!(current.sync.cursors.get("wechat").is_none());

    let back_to_a = switch_login(&mut current, &user_a, false);
    println!("切回用户96：{back_to_a}");
    assert_eq!(back_to_a, "自动备份改用这个登录，并恢复它自己的进度");
    let restored = current.sync.cursors.get("wechat").unwrap();
    assert_eq!(restored.get("会话甲").unwrap().time, 100);
    assert_eq!(restored.get("会话乙").unwrap().time, 200);
    assert_eq!(restored.get("会话丙").unwrap().time, 300);

    let back_to_b = switch_login(&mut current, &user_b, false);
    println!("再切回用户89：{back_to_b}");
    assert_eq!(back_to_b, "自动备份改用这个登录，并恢复它自己的进度");
    assert_eq!(
        current
            .sync
            .cursors
            .get("wechat")
            .unwrap()
            .get("会话甲")
            .unwrap()
            .time,
        900
    );
    assert!(!current
        .sync
        .cursors
        .get("wechat")
        .unwrap()
        .contains_key("会话乙"));

    let same_user_new_space = switch_login(&mut current, &user_a_other_space, false);
    println!("用户96 换到另一个空间：{same_user_new_space}");
    assert_eq!(
        same_user_new_space,
        "自动备份改用这个登录，没有旧进度，按起始时间重新采集"
    );
    assert!(current.sync.cursors.get("wechat").is_none());
    assert!(current.sync.has_saved_progress(&user_a));
    assert!(current.sync.has_saved_progress(&user_b));
}

#[test]
fn a_legacy_manual_create_is_reconciled_without_guessing_its_batch() {
    use super::CreateRecovery;
    let owner = owner();
    let job = UploadJob {
        id: "upload-old".into(),
        status: "failed".into(),
        account_id: owner.account_id.clone(),
        space_id: owner.space_id.clone(),
        owner: Some(owner.clone()),
        create_started: Some(true),
        package_digest: Some("original".into()),
        scope: Some(UploadScope {
            usernames: vec!["one".into()],
            base_url: owner.base_url.clone(),
            since: None,
            until: None,
            message_selection: BTreeMap::new(),
        }),
        ..Default::default()
    };
    let lookup = serde_json::json!({"batch": null});
    assert_eq!(
        super::resolve_unlinked_create(&owner, &job, &lookup),
        Ok(CreateRecovery::ContinueSync)
    );
    assert!(super::resolve_unlinked_create(&owner, &job, &serde_json::json!({})).is_err());
    let mut lookup = serde_json::json!({"archive_received": true, "batch": {
        "id": 80, "space_id": owner.space_id, "uploaded_by_id": owner.user_id,
        "source_type": "wechat", "source_account_id": owner.account_id,
        "device_id": owner.device_id, "status": "completed", "archive_sha256": "another"
    }});
    assert!(super::resolve_unlinked_create(&owner, &job, &lookup).is_err());
    lookup["batch"]["archive_sha256"] = serde_json::json!("original");
    assert_eq!(
        super::resolve_unlinked_create(&owner, &job, &lookup),
        Ok(CreateRecovery::Link(80))
    );
    lookup["batch"]["status"] = serde_json::json!("uploading");
    assert_eq!(
        super::resolve_unlinked_create(&owner, &job, &lookup),
        Ok(CreateRecovery::Submit(80))
    );
    lookup["batch"]["device_id"] = serde_json::json!("different");
    assert!(super::resolve_unlinked_create(&owner, &job, &lookup).is_err());
}

#[test]
fn legacy_auto_key_collision_replans_without_adopting_another_archive() {
    let owner = owner();
    let job = UploadJob {
        id: "sync-old".into(),
        status: "failed".into(),
        message: "创建上传批次失败：409 Conflict".into(),
        account_id: owner.account_id.clone(),
        space_id: owner.space_id.clone(),
        owner: Some(owner.clone()),
        create_started: Some(true),
        create_key: Some("wechat-old-content-key".into()),
        package_digest: Some("original".into()),
        scope: Some(UploadScope {
            usernames: vec!["one".into()],
            base_url: owner.base_url.clone(),
            since: None,
            until: None,
            message_selection: BTreeMap::new(),
        }),
        ..Default::default()
    };
    let mut lookup = serde_json::json!({"archive_received":true,"batch":{
            "id":80,"space_id":owner.space_id,"uploaded_by_id":owner.user_id,"source_type":"wechat",
            "source_account_id":owner.account_id,"device_id":owner.device_id,"idempotency_key":"wechat-old-content-key",
            "status":"completed","archive_sha256":"different"}});
    assert_eq!(
        super::resolve_unlinked_create(&owner, &job, &lookup),
        Ok(super::CreateRecovery::ReplanLegacyCollision)
    );
    lookup["batch"]["status"] = serde_json::json!("processing");
    assert!(super::resolve_unlinked_create(&owner, &job, &lookup).is_err());
    lookup["batch"]["status"] = serde_json::json!("completed");
    lookup["batch"]["device_id"] = serde_json::json!("other-device");
    assert!(super::resolve_unlinked_create(&owner, &job, &lookup).is_err());
    let mut current = with_jobs(vec![job]);
    current.upload_queue[0].status = "cancelled".into();
    super::apply_completed_uploads(&mut current);
    assert!(current.sync.cursors.is_empty());
}

#[test]
fn old_server_list_requires_an_exact_archive_match_in_the_same_device() {
    let owner = owner();
    let job = UploadJob {
        package_digest: Some("original".into()),
        ..Default::default()
    };
    let mut batch = serde_json::json!({
        "id": 80, "space_id": owner.space_id, "uploaded_by_id": owner.user_id,
        "source_type": "wechat", "source_account_id": owner.account_id,
        "device_id": owner.device_id, "archive_sha256": "different",
        "archive_received_at": "2026-11-17T10:13:08Z"
    });
    assert!(!super::legacy_recovery_matches(&owner, &job, &batch).unwrap());
    batch["archive_sha256"] = serde_json::json!("original");
    assert!(super::legacy_recovery_matches(&owner, &job, &batch).unwrap());
    batch["device_id"] = serde_json::json!("different");
    assert!(!super::legacy_recovery_matches(&owner, &job, &batch).unwrap());
}

fn job(status: &str, covered: SyncCursor, fallback_at: &str) -> UploadJob {
    let mut covered_map = BTreeMap::new();
    covered_map.insert("one".to_string(), covered);
    UploadJob {
        id: "job".into(),
        account_id: "acct".into(),
        space_id: "1".into(),
        batch_id: Some(1),
        owner: Some(owner()),
        status: status.into(),
        created_at: fallback_at.into(),
        updated_at: fallback_at.into(),
        scope: Some(UploadScope {
            usernames: vec!["one".into()],
            since: None,
            until: None,
            base_url: "https://example.test".into(),
            message_selection: BTreeMap::new(),
        }),
        covered: covered_map,
        ..UploadJob::default()
    }
}

fn with_jobs(jobs: Vec<UploadJob>) -> settings::AppSettings {
    let mut value = settings::AppSettings::default();
    value.upload_queue = jobs
        .into_iter()
        .map(|mut item| {
            item.verified_completion_digest = super::upload_completion_digest(&item);
            item
        })
        .collect();
    value.sync.owners.insert("acct".into(), owner());
    value
}

#[test]
fn equal_position_preserves_precise_boundary_count() {
    let mut value = with_jobs(vec![job(
        "completed",
        SyncCursor {
            time: 100,
            local_id: 5,
            boundary_count: None,
        },
        "",
    )]);
    value.sync.cursors.entry("acct".into()).or_default().insert(
        "one".into(),
        SyncCursor {
            time: 100,
            local_id: 5,
            boundary_count: Some(1),
        },
    );
    assert_eq!(
        upload_coverage(&value, &owner())["one"].boundary_count,
        Some(1)
    );
}

#[test]
fn voice_retry_selects_only_pending_and_incremental_messages() {
    use crate::settings::{MessageRef, VoiceRef};
    let r = |time, local_id| MessageRef {
        time,
        local_id,
        source_shard: "message_0.db".into(),
    };
    let messages = vec![r(10, 1), r(20, 2), r(30, 3), r(40, 4)];
    let voice = vec![VoiceRef {
        time: 10,
        local_id: 1,
        server_id: 11,
        source_shard: "message_0.db".into(),
    }];
    assert_eq!(
        super::sync_message_selection(
            &messages,
            Some(SyncCursor {
                time: 30,
                local_id: 3,
                boundary_count: Some(1)
            }),
            &voice
        ),
        vec![r(10, 1), r(30, 3), r(40, 4)]
    );
}

#[test]
fn submitted_job_is_not_reported_as_completed() {
    let position = SyncCursor {
        time: 100,
        local_id: 5,
        boundary_count: Some(1),
    };
    let mut value = with_jobs(vec![job("processing", position, "2026-11-06T00:00:00Z")]);
    value.devify.active_space_id = Some("1".into());
    assert_eq!(
        super::conversation_job_state(&value, &owner(), "one"),
        Some("processing")
    );
}

#[test]
fn older_processing_job_is_not_hidden_by_newer_completed_job() {
    let position = SyncCursor {
        time: 100,
        local_id: 5,
        boundary_count: Some(1),
    };
    let mut value = with_jobs(vec![
        job("completed", position, "2026-11-06T01:00:00Z"),
        job("processing", position, "2026-11-06T00:00:00Z"),
    ]);
    value.devify.active_space_id = Some("1".into());
    assert_eq!(
        super::conversation_job_state(&value, &owner(), "one"),
        Some("processing")
    );
}

#[test]
fn older_failed_job_is_not_hidden_by_newer_completed_job() {
    let position = SyncCursor {
        time: 100,
        local_id: 5,
        boundary_count: Some(1),
    };
    let mut value = with_jobs(vec![
        job("completed", position, "2026-11-06T01:00:00Z"),
        job("failed", position, "2026-11-06T00:00:00Z"),
    ]);
    value.devify.active_space_id = Some("1".into());
    assert_eq!(
        super::conversation_job_state(&value, &owner(), "one"),
        Some("history_failed")
    );
}

#[test]
fn newer_failed_job_remains_failure() {
    let position = SyncCursor {
        time: 100,
        local_id: 5,
        boundary_count: Some(1),
    };
    let mut value = with_jobs(vec![
        job("completed", position, "2026-11-06T00:00:00Z"),
        job("failed", position, "2026-11-06T01:00:00Z"),
    ]);
    value.devify.active_space_id = Some("1".into());
    assert_eq!(
        super::conversation_job_state(&value, &owner(), "one"),
        Some("failed")
    );
}

#[test]
fn automatic_cursor_advances_only_after_completion() {
    let position = SyncCursor {
        time: 100,
        local_id: 5,
        boundary_count: Some(1),
    };
    let mut item = job("processing", position, "2026-11-06T00:00:00Z");
    item.sync_advance = true;
    let mut value = with_jobs(vec![item]);
    super::apply_completed_uploads(&mut value);
    assert!(!value.sync.cursors.contains_key("acct"));
    value.upload_queue[0].status = "completed".into();
    super::apply_completed_uploads(&mut value);
    assert_eq!(value.sync.cursors["acct"]["one"].time, 100);
}

#[test]
fn continuation_is_requested_only_after_completed_batch() {
    let position = SyncCursor {
        time: 100,
        local_id: 1,
        boundary_count: Some(1),
    };
    let mut item = job("processing", position, "");
    item.continue_sync = true;
    let mut value = with_jobs(vec![item]);
    super::apply_completed_uploads(&mut value);
    assert!(value.sync.continuation.is_empty());
    value.upload_queue[0].status = "failed".into();
    super::apply_completed_uploads(&mut value);
    assert!(value.sync.continuation.is_empty());
    value.upload_queue[0].status = "completed".into();
    super::apply_completed_uploads(&mut value);
    assert_eq!(value.sync.continuation["acct"]["one"].space_id, "1");
    assert!(!value.upload_queue[0].continue_sync);
}

#[test]
fn continuation_stays_with_original_space_and_allowed_chat() {
    let mut value = with_jobs(vec![]);
    value.devify.active_space_id = Some("1".into());
    value.devify.base_url = "https://example.test".into();
    value
        .chat
        .allowed_usernames
        .insert("acct".into(), vec!["one".into()]);
    value
        .sync
        .continuation
        .entry("acct".into())
        .or_default()
        .insert(
            "one".into(),
            settings::ContinuationScope {
                space_id: "1".into(),
                base_url: "https://example.test".into(),
                manual: Some(true),
            },
        );
    assert_eq!(
        super::continuation_sessions(&value, "acct"),
        vec!["one".to_string()]
    );
    value.devify.active_space_id = Some("2".into());
    assert!(super::continuation_sessions(&value, "acct").is_empty());
    value.devify.active_space_id = Some("1".into());
    value.chat.allowed_usernames.clear();
    assert!(super::continuation_sessions(&value, "acct").is_empty());
    value
        .chat
        .allowed_usernames
        .insert("acct".into(), vec!["one".into()]);
    value
        .sync
        .continuation
        .get_mut("acct")
        .unwrap()
        .get_mut("one")
        .unwrap()
        .manual = Some(false);
    assert!(super::continuation_sessions(&value, "acct").is_empty());
    value.sync.enabled = true;
    assert_eq!(
        super::continuation_sessions(&value, "acct"),
        vec!["one".to_string()]
    );
}

#[test]
fn voice_retry_is_kept_until_server_completes() {
    let voice = settings::VoiceRef {
        local_id: 1,
        time: 10,
        server_id: 2,
        source_shard: "message_0.db".into(),
    };
    let mut item = job(
        "processing",
        SyncCursor {
            time: 10,
            local_id: 1,
            boundary_count: Some(1),
        },
        "",
    );
    item.voice_processed
        .insert("one".into(), vec![voice.clone()]);
    let mut value = with_jobs(vec![item]);
    value
        .sync
        .pending_voice
        .entry("acct".into())
        .or_default()
        .insert("one".into(), vec![voice]);
    super::apply_completed_uploads(&mut value);
    assert_eq!(value.sync.pending_voice["acct"]["one"].len(), 1);
    value.upload_queue[0].status = "failed".into();
    super::apply_completed_uploads(&mut value);
    assert_eq!(value.sync.pending_voice["acct"]["one"].len(), 1);
    value.upload_queue[0].status = "completed".into();
    super::apply_completed_uploads(&mut value);
    assert!(!value.sync.pending_voice.contains_key("acct"));
}

#[test]
fn initial_history_baseline_waits_for_completion() {
    let cursor = SyncCursor {
        time: 300,
        local_id: 3,
        boundary_count: Some(1),
    };
    let mut item = job("processing", cursor, "2026-11-06T00:00:00Z");
    item.initial_baselines.insert("one".into(), cursor);
    let mut value = with_jobs(vec![item]);
    super::apply_completed_history_baselines(&mut value);
    assert!(!value.sync.cursors.contains_key("acct"));
    value.upload_queue[0].status = "completed".into();
    super::apply_completed_history_baselines(&mut value);
    assert_eq!(value.sync.cursors["acct"]["one"].time, 300);
}

#[test]
fn initial_chunk_checkpoint_survives_failure_and_finishes_only_on_last_batch() {
    let baseline = SyncCursor {
        time: 300,
        local_id: 3,
        boundary_count: Some(1),
    };
    let first = SyncCursor {
        time: 100,
        local_id: 1,
        boundary_count: Some(1),
    };
    let mut item = job("processing", first, "2026-11-06T00:00:00Z");
    item.initial_chunks.insert(
        "one".into(),
        settings::InitialChunk {
            since: Some(10),
            until: Some(200),
            baseline,
            position: first,
            final_chunk: false,
            space_id: "1".into(),
            base_url: "https://example.test".into(),
        },
    );
    let mut value = with_jobs(vec![item]);
    super::apply_completed_uploads(&mut value);
    assert!(value.sync.initial_progress.is_empty());
    value.upload_queue[0].status = "failed".into();
    super::apply_completed_uploads(&mut value);
    assert!(value.sync.initial_progress.is_empty());
    assert!(!value.upload_queue[0].initial_chunks.is_empty());
    value.upload_queue[0].status = "completed".into();
    super::apply_completed_uploads(&mut value);
    assert_eq!(
        value.sync.initial_progress["acct"]["one"].position.time,
        100
    );
    assert!(value.sync.cursors.get("acct").is_none());
    let mut last = job("completed", first, "2026-11-06T00:01:00Z");
    last.initial_chunks.insert(
        "one".into(),
        settings::InitialChunk {
            since: Some(10),
            until: Some(200),
            baseline,
            position: first,
            final_chunk: true,
            space_id: "1".into(),
            base_url: "https://example.test".into(),
        },
    );
    value.upload_queue.push(last);
    super::apply_completed_uploads(&mut value);
    assert!(!value.sync.initial_progress.contains_key("acct"));
    assert_eq!(value.sync.cursors["acct"]["one"].time, 300);
}

#[test]
fn initial_batches_resume_after_restart_and_retry_without_skipping() {
    use crate::settings::MessageRef;
    let records: Vec<_> = (0..2300)
        .map(|n| MessageRef {
            time: 100 + n / 2,
            local_id: n / 2,
            source_shard: format!("shard-{}", n % 2),
        })
        .collect();
    let baseline = SyncCursor {
        time: 2000,
        local_id: 9999,
        boundary_count: Some(1),
    };
    let mut value = with_jobs(vec![]);
    let mut sent = std::collections::HashSet::new();
    for expected in [1000, 1000, 300] {
        let position = value
            .sync
            .initial_progress
            .get("acct")
            .and_then(|v| v.get("one"))
            .map(|p| p.position);
        let batch = super::plan_sync_batch(&records, position, 1000).unwrap();
        assert_eq!(batch.messages.len(), expected);
        assert!(batch.messages.iter().all(|item| sent.insert(item.clone())));
        let last = batch.messages.last().unwrap();
        let boundary_count = batch
            .messages
            .iter()
            .filter(|item| item.time == last.time)
            .count();
        let position = SyncCursor {
            time: last.time,
            local_id: last.local_id,
            boundary_count: Some(boundary_count),
        };
        let mut item = job("failed", position, "");
        item.initial_chunks.insert(
            "one".into(),
            settings::InitialChunk {
                since: Some(100),
                until: Some(1250),
                baseline,
                position,
                final_chunk: !batch.has_more,
                space_id: "1".into(),
                base_url: "https://example.test".into(),
            },
        );
        value.upload_queue.push(item);
        super::apply_completed_uploads(&mut value);
        assert!(value.sync.cursors.get("acct").is_none());
        value.upload_queue.last_mut().unwrap().status = "completed".into();
        super::apply_completed_uploads(&mut value);
        value = serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
    }
    assert_eq!(sent.len(), records.len());
    assert!(value.sync.initial_progress.is_empty());
    assert_eq!(value.sync.cursors["acct"]["one"].time, baseline.time);
}

#[test]
fn initial_progress_cannot_be_reused_for_another_space() {
    let chunk = settings::InitialChunk {
        since: None,
        until: Some(100),
        baseline: SyncCursor::default(),
        position: SyncCursor::default(),
        final_chunk: false,
        space_id: "1".into(),
        base_url: "https://example.test".into(),
    };
    assert!(super::initial_progress_matches(
        &chunk,
        "1",
        "https://example.test/"
    ));
    assert!(!super::initial_progress_matches(
        &chunk,
        "2",
        "https://example.test"
    ));
    assert!(!super::initial_progress_matches(
        &chunk,
        "1",
        "https://another.test"
    ));
}

#[test]
fn only_completed_jobs_advance_coverage() {
    let position = SyncCursor {
        boundary_count: None,
        time: 180,
        local_id: 9,
    };
    assert!(
        upload_coverage(&with_jobs(vec![job("processing", position, "")]), &owner()).is_empty()
    );
    assert!(upload_coverage(&with_jobs(vec![job("failed", position, "")]), &owner()).is_empty());
    let completed = upload_coverage(&with_jobs(vec![job("completed", position, "")]), &owner());
    assert_eq!(
        completed.get("one").map(|c| (c.time, c.local_id)),
        Some((180, 9))
    );
}

#[test]
fn keeps_the_newest_position_and_ignores_other_accounts() {
    let older = SyncCursor {
        boundary_count: None,
        time: 100,
        local_id: 1,
    };
    let newer = SyncCursor {
        boundary_count: None,
        time: 200,
        local_id: 2,
    };
    let mut foreign = job("completed", newer, "");
    foreign.account_id = "other".into();
    let coverage = upload_coverage(
        &with_jobs(vec![
            job("completed", newer, ""),
            job("completed", older, ""),
            foreign,
        ]),
        &owner(),
    );
    assert_eq!(
        coverage.get("one").map(|c| (c.time, c.local_id)),
        Some((200, 2))
    );
}

#[test]
fn jobs_without_coverage_do_not_infer_completion_time() {
    let mut legacy = job(
        "completed",
        SyncCursor::default(),
        "2026-10-01T10:00:00+00:00",
    );
    legacy.covered.clear();
    let coverage = upload_coverage(&with_jobs(vec![legacy]), &owner());
    assert!(coverage.is_empty());
}

#[test]
fn modern_jobs_do_not_claim_conversations_they_skipped() {
    let mut modern = job(
        "completed",
        SyncCursor {
            boundary_count: None,
            time: 200,
            local_id: 2,
        },
        "2026-10-01T10:00:00+00:00",
    );
    // Another conversation was uploaded, this one was not part of the package.
    modern.scope.as_mut().unwrap().usernames = vec!["one".into(), "two".into()];
    let coverage = upload_coverage(&with_jobs(vec![modern]), &owner());
    assert_eq!(coverage.get("one").map(|c| c.time), Some(200));
    assert!(coverage.get("two").is_none());
}
