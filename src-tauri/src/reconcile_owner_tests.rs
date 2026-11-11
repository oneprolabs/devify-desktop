//! Isolated owner-reconciliation tests: all credentials and settings are temporary,
//! and HTTP goes only to loopback mock servers.
use super::*;
use std::io::{Read, Write};
use std::net::TcpListener as StdListener;
use std::path::PathBuf;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

fn token() -> String {
    format!("header.{}.signature", URL_SAFE_NO_PAD.encode(br#"{"device_id":"device-a","exp":4102444800}"#))
}

fn cursor(time: i64) -> settings::SyncCursor {
    settings::SyncCursor { time, local_id: 1, boundary_count: Some(1) }
}

fn job(id: &str, owner: Option<settings::UploadOwner>, base: &str) -> settings::UploadJob {
    let mut item = settings::UploadJob {
        id: id.into(), account_id: "wechat-a".into(), space_id: 7, batch_id: Some(42),
        owner, status: "processing".into(), create_started: Some(false),
        scope: Some(settings::UploadScope { usernames: vec!["chat".into()], since: None,
            until: None, base_url: base.into(), message_selection: BTreeMap::new() }),
        sync_advance: true, ..Default::default()
    };
    item.covered.insert("chat".into(), cursor(100));
    item
}

fn batch() -> serde_json::Value {
    serde_json::json!({"id": 42, "space_id": 7, "uploaded_by_id": 9,
        "source_type": "wechat", "source_account_id": "wechat-a", "device_id": "device-a",
        "status": "completed", "message_count": 1})
}

fn fixture(base: &str, jobs: Vec<settings::UploadJob>) -> (tempfile::TempDir, settings::UploadOwner, String) {
    let data = tempfile::tempdir().unwrap();
    let owner = settings::UploadOwner { base_url: base.into(), space_id: 7,
        account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
    let mut value = settings::AppSettings::default();
    value.active_account_id = owner.account_id.clone();
    value.devify.base_url = base.into();
    value.devify.active_space_id = Some(7);
    value.devify.device_id = owner.device_id.clone();
    value.upload_queue = jobs;
    settings::save(data.path(), &value).unwrap();
    let token = token();
    desktop_credentials::save(data.path(), &desktop_credentials::Credentials {
        base_url: base.into(), access: token.clone(), refresh: "test-refresh".into(),
    }).unwrap();
    (data, owner, token)
}

// Serve exactly one user, device and batch request with the same bearer token. The
// callback runs while the batch request is in flight, so tests can race a transaction.
fn serve(listener: StdListener, expected_token: String, remote: serde_json::Value,
         during_batch: impl FnOnce() + Send + 'static) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut callback = Some(during_batch);
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 1024];
            while !bytes.ends_with(b"\r\n\r\n") {
                let n = stream.read(&mut buffer).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
            }
            let request = String::from_utf8(bytes).unwrap();
            assert!(request.to_ascii_lowercase().contains(&format!("authorization: bearer {}\r\n", expected_token.to_ascii_lowercase())));
            let response = if request.starts_with("GET /api/v1/auth/user ") {
                serde_json::json!({"data": {"id": 9}})
            } else if request.starts_with("GET /api/v1/auth/desktop/devices ") {
                serde_json::json!({"data": [{"device_id": "device-a", "status": "active"}]})
            } else {
                assert!(request.starts_with("GET /api/v1/chat/uploads/42 "));
                callback.take().unwrap()();
                serde_json::json!({"data": remote})
            };
            let body = response.to_string();
            // A timeout test intentionally drops the connection before the response.
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
        }
    })
}

#[test]
fn same_batch_on_another_server_and_ownerless_jobs_are_never_completed() {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let other = StdListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let other_base = format!("http://{}", other.local_addr().unwrap());
    let owner = settings::UploadOwner { base_url: base.clone(), space_id: 7, account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
    let foreign_owner = settings::UploadOwner { base_url: other_base.clone(), ..owner.clone() };
    let current = job("current", Some(owner.clone()), &base);
    let foreign = job("foreign", Some(foreign_owner), &other_base);
    let legacy = job("legacy", None, &base);
    let mut previously_completed = job("old-completed", None, &base);
    previously_completed.status = "completed".into();
    previously_completed.covered.insert("chat".into(), cursor(999));
    let (data, _, fixed_token) = fixture(&base, vec![current.clone(), foreign.clone(), legacy.clone(), previously_completed]);
    let data_path: PathBuf = data.path().to_path_buf();
    let server = serve(listener, fixed_token.clone(), batch(), move || {
        // The read lease must reject service and credential changes mid-request, while an
        // unrelated transaction can safely persist durable create metadata.
        assert!(settings::update(&data_path, |s| { s.devify.active_space_id = Some(8); Ok(()) }).is_err());
        assert!(desktop_credentials::clear(&data_path).is_err());
        settings::update(&data_path, |s| {
            s.upload_queue[0].create_started = Some(true);
            s.upload_queue[0].package_digest = Some("digest-written-concurrently".into());
            Ok(())
        }).unwrap();
    });
    runtime().block_on(async {
        let (verified, token) = verified_upload_identity(data.path(), "wechat-a", 7).await.unwrap();
        assert_eq!(verified, owner);
        let _lease = UPLOAD_IDENTITY_LOCK.read().await;
        assert!(reconcile_owned_job(data.path(), &verified, &token, &foreign).await.is_err());
        assert!(reconcile_owned_job(data.path(), &verified, &token, &legacy).await.is_err());
        reconcile_owned_job(data.path(), &verified, &token, &current).await.unwrap();
    });
    server.join().unwrap();
    other.set_nonblocking(true).unwrap();
    assert_eq!(other.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    let saved = settings::load(data.path()).unwrap();
    assert_eq!(saved.upload_queue[0].status, "completed");
    assert_eq!(saved.upload_queue[0].create_started, Some(true));
    assert_eq!(saved.upload_queue[0].package_digest.as_deref(), Some("digest-written-concurrently"));
    assert_eq!(saved.upload_queue[1].status, "processing");
    assert_eq!(saved.upload_queue[2].status, "processing");
    assert_eq!(saved.sync.cursors["wechat-a"]["chat"].time, 100);
    assert_eq!(saved.sync.owners.get("wechat-a"), Some(&owner));
    assert!(saved.upload_queue[3].sync_advance);
}

#[test]
fn remote_identity_and_concurrent_cursor_changes_fail_closed() {
    // Validate every required remote attribute, including same-space different user/device.
    let owner = settings::UploadOwner { base_url: "https://example.test".into(), space_id: 7,
        account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
    for (field, wrong) in [("id", serde_json::json!(43)), ("space_id", serde_json::json!(8)),
        ("uploaded_by_id", serde_json::json!(10)), ("source_type", serde_json::json!("other")),
        ("source_account_id", serde_json::json!("wechat-b")), ("device_id", serde_json::json!("device-b"))] {
        let mut invalid = batch();
        invalid[field] = wrong;
        assert!(validate_remote_owner(&owner, 42, &invalid, None).is_err(), "{field}");
    }
    assert!(validate_remote_owner(&owner, 42, &batch(), None).is_ok());

    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let owner = settings::UploadOwner { base_url: base.clone(), ..owner };
    let snapshot = job("current", Some(owner.clone()), &base);
    let (data, _, fixed_token) = fixture(&base, vec![snapshot.clone()]);
    let data_path = data.path().to_path_buf();
    let server = serve(listener, fixed_token, batch(), move || {
        settings::update(&data_path, |s| { s.upload_queue[0].covered.insert("chat".into(), cursor(200)); Ok(()) }).unwrap();
    });
    runtime().block_on(async {
        let (verified, token) = verified_upload_identity(data.path(), "wechat-a", 7).await.unwrap();
        let _lease = UPLOAD_IDENTITY_LOCK.read().await;
        assert!(reconcile_owned_job(data.path(), &verified, &token, &snapshot).await.is_err());
    });
    server.join().unwrap();
    let saved = settings::load(data.path()).unwrap();
    assert_eq!(saved.upload_queue[0].status, "processing");
    assert_eq!(saved.upload_queue[0].covered["chat"].time, 200);
    assert!(saved.sync.cursors.is_empty());
}

#[test]
fn remote_wrong_user_or_device_never_advances_the_job() {
    for field in ["uploaded_by_id", "device_id"] {
        let listener = StdListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let owner = settings::UploadOwner { base_url: base.clone(), space_id: 7,
            account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
        let snapshot = job("current", Some(owner), &base);
        let (data, _, fixed_token) = fixture(&base, vec![snapshot.clone()]);
        let mut remote = batch();
        remote[field] = if field == "uploaded_by_id" { serde_json::json!(10) } else { serde_json::json!("device-b") };
        let server = serve(listener, fixed_token, remote, || {});
        runtime().block_on(async {
            let (verified, token) = verified_upload_identity(data.path(), "wechat-a", 7).await.unwrap();
            let _lease = UPLOAD_IDENTITY_LOCK.read().await;
            assert!(reconcile_owned_job(data.path(), &verified, &token, &snapshot).await.is_err());
        });
        server.join().unwrap();
        let saved = settings::load(data.path()).unwrap();
        assert_eq!(saved.upload_queue[0].status, "processing");
        assert!(saved.sync.cursors.is_empty());
    }
}

#[test]
fn old_ownerless_cursor_blocks_completed_job_advancement() {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let owner = settings::UploadOwner { base_url: base.clone(), space_id: 7,
        account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
    let snapshot = job("current", Some(owner), &base);
    let (data, _, fixed_token) = fixture(&base, vec![snapshot.clone()]);
    settings::update(data.path(), |s| { s.sync.cursors.entry("wechat-a".into()).or_default().insert("chat".into(), cursor(900)); Ok(()) }).unwrap();
    let server = serve(listener, fixed_token, batch(), || {});
    runtime().block_on(async {
        let (verified, token) = verified_upload_identity(data.path(), "wechat-a", 7).await.unwrap();
        let _lease = UPLOAD_IDENTITY_LOCK.read().await;
        reconcile_owned_job(data.path(), &verified, &token, &snapshot).await.unwrap();
    });
    server.join().unwrap();
    let saved = settings::load(data.path()).unwrap();
    assert_eq!(saved.upload_queue[0].status, "completed");
    assert!(saved.upload_queue[0].message.contains("已保留待确认"));
    assert!(saved.upload_queue[0].sync_advance);
    assert_eq!(saved.sync.cursors["wechat-a"]["chat"].time, 900);
    assert!(saved.sync.owners.is_empty());
}

#[test]
fn foreign_cursor_owner_does_not_block_plain_manual_completion() {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let owner_b = settings::UploadOwner { base_url: base.clone(), space_id: 7,
        account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
    let mut owner_a = owner_b.clone(); owner_a.user_id = 10;
    let mut snapshot = job("manual", Some(owner_b.clone()), &base);
    snapshot.sync_advance = false;
    let (data, _, fixed_token) = fixture(&base, vec![snapshot.clone()]);
    settings::update(data.path(), |s| {
        s.sync.owners.insert("wechat-a".into(), owner_a.clone());
        s.sync.cursors.entry("wechat-a".into()).or_default().insert("chat".into(), cursor(900));
        Ok(())
    }).unwrap();
    let server = serve(listener, fixed_token, batch(), || {});
    runtime().block_on(async {
        let (verified, token) = verified_upload_identity(data.path(), "wechat-a", 7).await.unwrap();
        let _lease = UPLOAD_IDENTITY_LOCK.read().await;
        reconcile_owned_job(data.path(), &verified, &token, &snapshot).await.unwrap();
    });
    server.join().unwrap();
    let saved = settings::load(data.path()).unwrap();
    assert_eq!(saved.upload_queue[0].status, "completed");
    assert!(!saved.upload_queue[0].message.contains("待确认"));
    assert_eq!(saved.sync.owners["wechat-a"], owner_a);
    assert_eq!(saved.sync.cursors["wechat-a"]["chat"].time, 900);
    assert_eq!(saved.upload_queue[0].covered["chat"].time, 100);
}

#[test]
fn conflicting_sync_and_voice_effects_are_preserved_and_reverified_before_retry() {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let owner_b = settings::UploadOwner { base_url: base.clone(), space_id: 7,
        account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
    let mut owner_a = owner_b.clone(); owner_a.user_id = 10;
    let mut snapshot = job("deferred", Some(owner_b.clone()), &base);
    snapshot.initial_baselines.insert("chat".into(), cursor(100));
    snapshot.initial_chunks.insert("chat".into(), settings::InitialChunk {
        since: None, until: None, baseline: cursor(100), position: cursor(100),
        final_chunk: true, space_id: 7, base_url: base.clone(),
    });
    snapshot.continue_sync = true;
    let processed = settings::VoiceRef { local_id: 1, time: 10, server_id: 11, source_shard: "a".into() };
    let pending = settings::VoiceRef { local_id: 2, time: 20, server_id: 22, source_shard: "a".into() };
    snapshot.voice_processed.insert("chat".into(), vec![processed]);
    snapshot.voice_pending.insert("chat".into(), vec![pending.clone()]);
    let (data, _, fixed_token) = fixture(&base, vec![snapshot.clone()]);
    settings::update(data.path(), |s| {
        s.sync.owners.insert("wechat-a".into(), owner_a.clone());
        s.sync.cursors.entry("wechat-a".into()).or_default().insert("chat".into(), cursor(900));
        Ok(())
    }).unwrap();
    let first = serve(listener.try_clone().unwrap(), fixed_token.clone(), batch(), || {});
    runtime().block_on(async {
        let (verified, token) = verified_upload_identity(data.path(), "wechat-a", 7).await.unwrap();
        let _lease = UPLOAD_IDENTITY_LOCK.read().await;
        reconcile_owned_job(data.path(), &verified, &token, &snapshot).await.unwrap();
    });
    first.join().unwrap();
    let saved = settings::load(data.path()).unwrap();
    let deferred = saved.upload_queue[0].clone();
    assert_eq!(deferred.status, "completed");
    assert!(deferred.message.contains("已保留待确认"));
    assert!(upload_job_needs_reconciliation(&deferred));
    assert_eq!(serde_json::to_value(&deferred.initial_baselines).unwrap(), serde_json::to_value(&snapshot.initial_baselines).unwrap());
    assert_eq!(serde_json::to_value(&deferred.initial_chunks).unwrap(), serde_json::to_value(&snapshot.initial_chunks).unwrap());
    assert_eq!(serde_json::to_value(&deferred.voice_processed).unwrap(), serde_json::to_value(&snapshot.voice_processed).unwrap());
    assert_eq!(serde_json::to_value(&deferred.voice_pending).unwrap(), serde_json::to_value(&snapshot.voice_pending).unwrap());
    assert!(deferred.sync_advance && deferred.continue_sync);
    assert_eq!(saved.sync.owners["wechat-a"], owner_a);
    assert_eq!(saved.sync.cursors["wechat-a"]["chat"].time, 900);
    assert!(saved.sync.pending_voice.is_empty());

    // Simulate explicit removal of the conflicting old state, not ownership inference.
    settings::update(data.path(), |s| {
        s.sync.owners.remove("wechat-a");
        s.sync.cursors.remove("wechat-a");
        Ok(())
    }).unwrap();
    let mut changed_remote = batch(); changed_remote["status"] = serde_json::json!("failed");
    let second = serve(listener.try_clone().unwrap(), fixed_token.clone(), changed_remote, || {});
    runtime().block_on(async {
        let (verified, token) = verified_upload_identity(data.path(), "wechat-a", 7).await.unwrap();
        let _lease = UPLOAD_IDENTITY_LOCK.read().await;
        assert!(reconcile_owned_job(data.path(), &verified, &token, &deferred).await.is_err());
    });
    second.join().unwrap();
    let blocked = settings::load(data.path()).unwrap();
    assert_eq!(blocked.upload_queue[0].status, "completed");
    assert!(blocked.upload_queue[0].sync_advance);
    assert!(blocked.sync.cursors.is_empty());
    assert!(blocked.sync.owners.is_empty());

    let third = serve(listener, fixed_token, batch(), || {});
    runtime().block_on(async {
        let (verified, token) = verified_upload_identity(data.path(), "wechat-a", 7).await.unwrap();
        let _lease = UPLOAD_IDENTITY_LOCK.read().await;
        reconcile_owned_job(data.path(), &verified, &token, &deferred).await.unwrap();
    });
    third.join().unwrap();
    let applied = settings::load(data.path()).unwrap();
    assert_eq!(applied.sync.owners["wechat-a"], owner_b);
    assert_eq!(applied.sync.cursors["wechat-a"]["chat"].time, 100);
    assert_eq!(applied.sync.pending_voice["wechat-a"]["chat"], vec![pending]);
    assert!(!upload_job_needs_reconciliation(&applied.upload_queue[0]));
    assert!(!applied.upload_queue[0].message.contains("待确认"));
}

#[test]
fn coverage_does_not_infer_owner_from_old_jobs_or_cursors() {
    let base = "https://example.test";
    let owner = settings::UploadOwner { base_url: base.into(), space_id: 7,
        account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
    let mut value = settings::AppSettings::default();
    value.sync.cursors.entry("wechat-a".into()).or_default().insert("chat".into(), cursor(900));
    let mut old = job("old", None, base); old.status = "completed".into();
    let mut user = owner.clone(); user.user_id = 10;
    let mut user_job = job("different-user", Some(user), base); user_job.status = "completed".into();
    let mut device = owner.clone(); device.device_id = "device-b".into();
    let mut device_job = job("different-device", Some(device), base); device_job.status = "completed".into();
    value.upload_queue = vec![old, user_job, device_job];
    assert!(upload_coverage(&value, &owner).is_empty());
    assert_eq!(conversation_job_state(&value, &owner, "chat"), None);
    let mut current = job("current", Some(owner.clone()), base); current.status = "completed".into();
    current.verified_completion_digest = upload_completion_digest(&current);
    value.upload_queue.push(current);
    assert_eq!(upload_coverage(&value, &owner)["chat"].time, 100);
    value.sync.owners.insert("wechat-a".into(), owner.clone());
    assert_eq!(upload_coverage(&value, &owner)["chat"].time, 900);
}

#[test]
fn legacy_sync_and_job_history_are_uncertain_without_becoming_coverage() {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let mut legacy = job("legacy", None, &format!("{base}/"));
    legacy.status = "completed".into();
    legacy.scope.as_mut().unwrap().usernames = vec!["job-chat".into()];
    let (data, owner, fixed_token) = fixture(&base, vec![legacy]);
    settings::update(data.path(), |s| {
        s.sync.cursors.entry(owner.account_id.clone()).or_default().insert("cursor-chat".into(), cursor(100));
        s.sync.initial_progress.entry(owner.account_id.clone()).or_default().insert("initial-chat".into(), settings::InitialChunk {
            since: None, until: None, baseline: cursor(100), position: cursor(100),
            final_chunk: false, space_id: 7, base_url: base.clone(),
        });
        s.sync.continuation.entry(owner.account_id.clone()).or_default().insert("continuation-chat".into(), settings::ContinuationScope {
            space_id: 7, base_url: base.clone(), manual: None,
        });
        s.sync.pending_voice.entry(owner.account_id.clone()).or_default().insert("voice-chat".into(), vec![]);
        s.sync.cursors.entry("other-account".into()).or_default().insert("other-chat".into(), cursor(100));
        Ok(())
    }).unwrap();
    let before = settings::load(data.path()).unwrap();
    let (names, all) = legacy_uncertain_coverage(&before, &owner);
    assert!(!all && names.len() == 5);
    for name in ["cursor-chat", "initial-chat", "continuation-chat", "voice-chat", "job-chat"] {
        assert!(names.contains(name));
    }
    assert!(!names.contains("other-chat"));
    runtime().block_on(async {
        let _lease = UPLOAD_IDENTITY_LOCK.read().await;
        let (filtered, integrated, all) = verified_coverage_settings(data.path(), &owner, &fixed_token).await.unwrap();
        assert!(!all && integrated == names);
        assert!(filtered.upload_queue.is_empty());
        assert!(upload_coverage(&filtered, &owner).is_empty());
        assert!(conversation_job_state(&filtered, &owner, "job-chat").is_none());
    });
    // This read-only path must not claim, retry, or advance the persisted legacy state.
    let after = settings::load(data.path()).unwrap();
    assert!(serde_json::to_value(before).unwrap() == serde_json::to_value(after).unwrap());
    listener.set_nonblocking(true).unwrap();
    assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
}

#[test]
fn legacy_unknown_scope_fails_closed_without_inheriting_other_services() {
    let base = "https://example.test";
    let owner = settings::UploadOwner { base_url: base.into(), space_id: 7,
        account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
    let mut no_scope = job("none", None, base); no_scope.scope = None;
    let mut empty_scope = job("empty", None, base); empty_scope.scope.as_mut().unwrap().usernames.clear();
    let bad_base = job("bad-base", None, " ");
    let malformed_base = job("malformed-base", None, "://");
    for legacy in [no_scope, empty_scope, bad_base, malformed_base] {
        let mut value = settings::AppSettings::default();
        value.upload_queue.push(legacy);
        let (_, all) = legacy_uncertain_coverage(&value, &owner);
        assert!(all);
    }
    let mut foreign_scope = job("foreign-service", None, "https://other.test");
    foreign_scope.scope.as_mut().unwrap().usernames.clear();
    let mut value = settings::AppSettings::default(); value.upload_queue.push(foreign_scope);
    let (names, all) = legacy_uncertain_coverage(&value, &owner);
    assert!(names.is_empty() && !all);
}

#[test]
fn legacy_history_does_not_cross_account_space_service_or_explicit_owner() {
    let base = "https://example.test";
    let owner = settings::UploadOwner { base_url: base.into(), space_id: 7,
        account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
    let mut foreign_owner = owner.clone(); foreign_owner.user_id += 1;
    let mut value = settings::AppSettings::default();
    value.sync.cursors.entry(owner.account_id.clone()).or_default().insert("cursor-chat".into(), cursor(100));
    value.sync.initial_progress.entry(owner.account_id.clone()).or_default().insert("initial-chat".into(), settings::InitialChunk {
        since: None, until: None, baseline: cursor(100), position: cursor(100),
        final_chunk: false, space_id: 7, base_url: base.into(),
    });
    value.sync.continuation.entry(owner.account_id.clone()).or_default().insert("continuation-chat".into(), settings::ContinuationScope {
        space_id: 7, base_url: base.into(), manual: None,
    });
    value.sync.pending_voice.entry(owner.account_id.clone()).or_default().insert("voice-chat".into(), vec![]);
    let mut other_account = job("account", None, base); other_account.account_id = "wechat-b".into(); other_account.scope = None;
    let mut other_space = job("space", None, base); other_space.space_id = 8; other_space.scope = None;
    let other_service = job("service", None, "https://other.test");
    let mut other_user = job("owner", Some(foreign_owner.clone()), base); other_user.scope = None;
    let mut other_device = owner.clone(); other_device.device_id = "device-b".into();
    let mut device_job = job("device", Some(other_device), base); device_job.scope = None;
    value.upload_queue = vec![other_account, other_space, other_service, other_user, device_job];
    for assigned in [foreign_owner, owner.clone()] {
        value.sync.owners.insert(owner.account_id.clone(), assigned);
        let (names, all) = legacy_uncertain_coverage(&value, &owner);
        assert!(names.is_empty() && !all);
    }
}

#[test]
fn no_history_and_verified_history_are_not_legacy_uncertainty() {
    let base = "https://example.test";
    let owner = settings::UploadOwner { base_url: base.into(), space_id: 7,
        account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
    let mut value = settings::AppSettings::default();
    let (names, all) = legacy_uncertain_coverage(&value, &owner);
    assert!(names.is_empty() && !all);
    value.sync.owners.insert(owner.account_id.clone(), owner.clone());
    value.sync.cursors.entry(owner.account_id.clone()).or_default().insert("chat".into(), cursor(900));
    let mut current = job("verified", Some(owner.clone()), base);
    current.status = "completed".into();
    current.verified_completion_digest = upload_completion_digest(&current);
    assert!(upload_completion_is_verified(&current));
    value.upload_queue.push(current);
    let (names, all) = legacy_uncertain_coverage(&value, &owner);
    assert!(names.is_empty() && !all);
    assert_eq!(upload_coverage(&value, &owner)["chat"].time, 900);
}

#[test]
fn integrated_coverage_preserves_same_owner_unverified_uncertainty() {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let owner = settings::UploadOwner { base_url: base.clone(), space_id: 7,
        account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
    let mut unverified = job("unverified", Some(owner.clone()), &base);
    unverified.status = "completed".into();
    let mut verified = job("verified", Some(owner.clone()), &base);
    verified.status = "completed".into();
    verified.scope.as_mut().unwrap().usernames = vec!["verified-chat".into()];
    verified.covered.insert("verified-chat".into(), cursor(100));
    verified.verified_completion_digest = upload_completion_digest(&verified);
    let (data, _, fixed_token) = fixture(&base, vec![unverified, verified]);
    runtime().block_on(async {
        let _lease = UPLOAD_IDENTITY_LOCK.read().await;
        let (filtered, names, all) = verified_coverage_settings(data.path(), &owner, &fixed_token).await.unwrap();
        assert!(!all && names.len() == 1 && names.contains("chat"));
        assert!(filtered.upload_queue.len() == 1);
        assert_eq!(upload_coverage(&filtered, &owner)["verified-chat"].time, 100);
    });
    listener.set_nonblocking(true).unwrap();
    assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
}

#[test]
fn completion_marker_binds_owner_batch_scope_and_coverage() {
    let owner = settings::UploadOwner { base_url: "https://example.test".into(), space_id: 7,
        account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
    let mut original = job("job", Some(owner.clone()), &owner.base_url);
    original.status = "completed".into();
    original.sync_advance = false;
    original.verified_completion_digest = upload_completion_digest(&original);
    assert!(upload_completion_is_verified(&original));
    let mut value = settings::AppSettings::default(); value.upload_queue.push(original.clone());
    assert_eq!(upload_coverage(&value, &owner)["chat"].time, 100);
    let mut other_user = owner.clone(); other_user.user_id += 1;
    let mut other_device = owner.clone(); other_device.device_id = "device-b".into();
    assert!(upload_coverage(&value, &other_user).is_empty());
    assert!(upload_coverage(&value, &other_device).is_empty());
    let mut altered = Vec::new();
    let mut item = original.clone(); item.id = "other".into(); altered.push(item);
    let mut item = original.clone(); item.batch_id = Some(43); altered.push(item);
    let mut item = original.clone(); item.owner.as_mut().unwrap().user_id += 1; altered.push(item);
    let mut item = original.clone(); item.owner.as_mut().unwrap().device_id = "device-b".into(); altered.push(item);
    let mut item = original.clone(); item.scope.as_mut().unwrap().usernames.push("other".into()); altered.push(item);
    let mut item = original.clone(); item.scope.as_mut().unwrap().base_url = "https://other.test".into(); altered.push(item);
    let mut item = original.clone(); item.covered.insert("chat".into(), cursor(200)); altered.push(item);
    let mut item = original.clone(); item.updated_at = "different".into(); altered.push(item);
    let mut item = original.clone(); item.status = "processing".into(); altered.push(item);
    for item in altered { assert!(!upload_completion_is_verified(&item)); }
    let legacy: settings::UploadJob = serde_json::from_value(serde_json::json!({"id": "legacy", "status": "completed"})).unwrap();
    assert!(legacy.verified_completion_digest.is_none());
    assert!(!upload_completion_is_verified(&legacy));
    assert!(!serde_json::to_string(&original).unwrap().contains(&token()));
}

#[test]
fn forty_completed_jobs_in_one_chat_converge_over_bounded_rounds() {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let owner = settings::UploadOwner { base_url: base.clone(), space_id: 7,
        account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
    let jobs: Vec<_> = (0..40).map(|i| {
        let mut item = job(&format!("job-{i:02}"), Some(owner.clone()), &base);
        item.batch_id = Some(100 + i); item.status = "completed".into(); item.sync_advance = false;
        item.covered.insert("chat".into(), cursor(100 + i));
        item
    }).collect();
    let (data, _, fixed_token) = fixture(&base, jobs);
    listener.set_nonblocking(true).unwrap();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut checks = 0;
        loop {
            if stop_rx.try_recv().is_ok() { break; }
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1)); continue;
                }
                Err(error) => panic!("{error}"),
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut bytes = Vec::new(); let mut buffer = [0; 1024];
            while !bytes.ends_with(b"\r\n\r\n") {
                let n = stream.read(&mut buffer).unwrap(); assert!(n > 0); bytes.extend_from_slice(&buffer[..n]);
            }
            let request = String::from_utf8(bytes).unwrap();
            assert!(request.to_ascii_lowercase().contains(&format!("authorization: bearer {}\r\n", fixed_token.to_ascii_lowercase())));
            let response = if request.starts_with("GET /api/v1/auth/user ") {
                serde_json::json!({"data": {"id": 9}})
            } else if request.starts_with("GET /api/v1/auth/desktop/devices ") {
                serde_json::json!({"data": [{"device_id": "device-a", "status": "active"}]})
            } else {
                let path = request.lines().next().unwrap().split_whitespace().nth(1).unwrap();
                let id: i64 = path.strip_prefix("/api/v1/chat/uploads/").unwrap().parse().unwrap();
                checks += 1;
                let mut remote = batch(); remote["id"] = serde_json::json!(id);
                serde_json::json!({"data": remote})
            };
            let body = response.to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
        checks
    });
    let cursor = std::sync::Mutex::new(None);
    runtime().block_on(async {
        let (verified, token) = verified_upload_identity(data.path(), "wechat-a", 7).await.unwrap();
        let _lease = UPLOAD_IDENTITY_LOCK.read().await;
        let (_, initial_uncertain, _) = verified_coverage_settings(data.path(), &verified, &token).await.unwrap();
        assert!(initial_uncertain.contains("chat"));
        for round in 0..5 {
            let current = settings::load(data.path()).unwrap();
            let pending: Vec<_> = current.upload_queue.into_iter().filter(upload_job_needs_reconciliation).collect();
            assert_eq!(pending.len(), 40 - round * RECONCILE_MAX_BATCH_CHECKS);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
            assert_eq!(reconcile_owned_round(data.path(), &verified, &token, &pending, &cursor, deadline).await, RECONCILE_MAX_BATCH_CHECKS);
        }
        let (filtered, uncertain, all) = verified_coverage_settings(data.path(), &verified, &token).await.unwrap();
        assert_eq!(filtered.upload_queue.len(), 40);
        assert!(uncertain.is_empty() && !all);
        assert_eq!(upload_coverage(&filtered, &owner)["chat"].time, 139);
    });
    stop_tx.send(()).unwrap();
    assert_eq!(server.join().unwrap(), 40);
}

#[test]
fn reconcile_round_caps_requests_and_rotates_without_starvation() {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let owner = settings::UploadOwner { base_url: base.clone(), space_id: 7,
        account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
    let jobs: Vec<_> = (0..21).rev().map(|i| {
        let mut item = job(&format!("job-{i:02}"), Some(owner.clone()), &base);
        item.batch_id = Some(42 + i);
        item
    }).collect();
    let (data, _, fixed_token) = fixture(&base, jobs.clone());
    listener.set_nonblocking(true).unwrap();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut batches = Vec::new();
        loop {
            if stop_rx.try_recv().is_ok() { break; }
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1)); continue;
                }
                Err(error) => panic!("{error}"),
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut bytes = Vec::new(); let mut buffer = [0; 1024];
            while !bytes.ends_with(b"\r\n\r\n") {
                let n = stream.read(&mut buffer).unwrap(); assert!(n > 0); bytes.extend_from_slice(&buffer[..n]);
            }
            let request = String::from_utf8(bytes).unwrap();
            assert!(request.to_ascii_lowercase().contains(&format!("authorization: bearer {}\r\n", fixed_token.to_ascii_lowercase())));
            let response = if request.starts_with("GET /api/v1/auth/user ") {
                serde_json::json!({"data": {"id": 9}})
            } else if request.starts_with("GET /api/v1/auth/desktop/devices ") {
                serde_json::json!({"data": [{"device_id": "device-a", "status": "active"}]})
            } else {
                let path = request.lines().next().unwrap().split_whitespace().nth(1).unwrap();
                let id: i64 = path.strip_prefix("/api/v1/chat/uploads/").unwrap().parse().unwrap();
                batches.push(id);
                let mut remote = batch(); remote["id"] = serde_json::json!(id); remote["status"] = serde_json::json!("processing");
                serde_json::json!({"data": remote})
            };
            let body = response.to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
        batches
    });
    let cursor = std::sync::Mutex::new(None);
    runtime().block_on(async {
        let (verified, token) = verified_upload_identity(data.path(), "wechat-a", 7).await.unwrap();
        let _lease = UPLOAD_IDENTITY_LOCK.read().await;
        for _ in 0..3 {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
            assert_eq!(reconcile_owned_round(data.path(), &verified, &token, &jobs, &cursor, deadline).await, RECONCILE_MAX_BATCH_CHECKS);
        }
    });
    stop_tx.send(()).unwrap();
    let checked = server.join().unwrap();
    assert_eq!(checked.len(), 3 * RECONCILE_MAX_BATCH_CHECKS);
    assert_eq!(checked.iter().copied().collect::<HashSet<_>>().len(), jobs.len());
    assert!(settings::load(data.path()).unwrap().upload_queue.iter().all(|item| item.status == "processing"));
}

#[test]
fn cancelled_reconcile_get_preserves_state_and_releases_identity_lease() {
    for (deadline_ms, server_delay_ms, limit_ms) in [(50, 200, 500), (3000, 2200, 1800)] {
        let listener = StdListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let owner = settings::UploadOwner { base_url: base.clone(), space_id: 7,
            account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
        let snapshot = job("current", Some(owner), &base);
        let (data, _, fixed_token) = fixture(&base, vec![snapshot.clone()]);
        let server = serve(listener, fixed_token, batch(), move || {
            std::thread::sleep(Duration::from_millis(server_delay_ms));
        });
        let cursor = std::sync::Mutex::new(None);
        let elapsed = runtime().block_on(async {
            let (verified, token) = verified_upload_identity(data.path(), "wechat-a", 7).await.unwrap();
            let lease = UPLOAD_IDENTITY_LOCK.read().await;
            let started = std::time::Instant::now();
            let deadline = tokio::time::Instant::now() + Duration::from_millis(deadline_ms);
            assert_eq!(reconcile_owned_round(data.path(), &verified, &token, &[snapshot], &cursor, deadline).await, 1);
            let elapsed = started.elapsed();
            drop(lease);
            assert!(UPLOAD_IDENTITY_LOCK.try_write().is_ok());
            elapsed
        });
        assert!(elapsed < Duration::from_millis(limit_ms), "timeout took {elapsed:?}");
        assert_eq!(cursor.lock().unwrap().as_deref(), Some("current"));
        server.join().unwrap();
        let saved = settings::load(data.path()).unwrap();
        assert_eq!(saved.upload_queue[0].status, "processing");
        assert!(saved.sync.cursors.is_empty());
    }
}

#[test]
fn reconcile_rotation_advances_by_actual_attempts_after_short_rounds() {
    let jobs: Vec<_> = (0..19).rev().map(|i| job(&format!("job-{i:02}"), None, "https://example.test")).collect();
    let mut last = None::<String>;
    let mut seen = HashSet::new();
    // A deadline may allow only one request every round, even when the cap is larger.
    for _ in 0..jobs.len() {
        let next = next_reconcile_job(&jobs, last.as_deref()).unwrap();
        assert!(seen.insert(next.id.clone()));
        last = Some(next.id.clone());
    }
    assert_eq!(seen.len(), jobs.len());
    assert_eq!(next_reconcile_job(&jobs, last.as_deref()).unwrap().id, "job-00");
}

fn legacy_completed(id: &str, base: &str) -> settings::UploadJob {
    let mut item = job(id, None, base);
    item.status = "completed".into();
    item.sync_advance = false;
    item
}

fn inert_failed(id: &str, base: &str) -> settings::UploadJob {
    let mut item = job(id, None, base);
    item.status = "failed".into();
    item.batch_id = None;
    item.sync_advance = false;
    item.covered.clear();
    item
}

fn adopt_with_remote(remote: serde_json::Value, cursor_time: i64) -> (settings::AppSettings, settings::UploadOwner) {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let jobs = vec![legacy_completed("sync-legacy", &base), inert_failed("sync-never-sent", &base)];
    let (data, owner, fixed_token) = fixture(&base, jobs);
    settings::update(data.path(), |s| {
        s.sync.cursors.entry("wechat-a".into()).or_default().insert("chat".into(), cursor(cursor_time));
        Ok(())
    }).unwrap();
    let server = serve(listener, fixed_token, remote, || {});
    runtime().block_on(async {
        let (verified, token) = verified_upload_identity(data.path(), "wechat-a", 7).await.unwrap();
        let _lease = UPLOAD_IDENTITY_LOCK.read().await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        adopt_legacy_upload_history(data.path(), &verified, &token, deadline).await.unwrap();
    });
    server.join().unwrap();
    (settings::load(data.path()).unwrap(), owner)
}

#[test]
fn server_confirmed_legacy_history_is_adopted_and_no_longer_uncertain() {
    let (saved, owner) = adopt_with_remote(batch(), 100);
    let adopted = &saved.upload_queue[0];
    assert_eq!(adopted.owner.as_ref(), Some(&owner));
    assert!(upload_completion_is_verified(adopted));
    assert!(saved.upload_queue[1].owner.is_none());
    assert_eq!(saved.sync.owners.get("wechat-a"), Some(&owner));
    let (uncertain, uncertain_all) = legacy_uncertain_coverage(&saved, &owner);
    assert!(uncertain.is_empty() && !uncertain_all);
    assert!(validate_sync_owner(&saved, &owner).is_ok());
    assert_eq!(upload_coverage(&saved, &owner)["chat"].time, 100);
}

#[test]
fn remote_mismatch_keeps_legacy_history_unowned_and_uncertain() {
    let mut remote = batch();
    remote["device_id"] = serde_json::json!("device-b");
    let (saved, owner) = adopt_with_remote(remote, 100);
    assert!(saved.upload_queue[0].owner.is_none());
    assert!(saved.sync.owners.is_empty());
    assert!(legacy_uncertain_coverage(&saved, &owner).0.contains("chat"));
}

#[test]
fn legacy_cursor_beyond_verified_coverage_is_not_adopted() {
    let (saved, owner) = adopt_with_remote(batch(), 500);
    assert_eq!(saved.upload_queue[0].owner.as_ref(), Some(&owner));
    assert!(saved.sync.owners.is_empty());
    assert!(legacy_uncertain_coverage(&saved, &owner).0.contains("chat"));
}

#[test]
fn inert_legacy_failure_is_not_uncertain_and_fresh_accounts_are_not_pinned() {
    let base = "https://example.test";
    let owner = settings::UploadOwner { base_url: base.into(), space_id: 7,
        account_id: "wechat-a".into(), user_id: 9, device_id: "device-a".into() };
    let mut value = settings::AppSettings::default();
    value.upload_queue = vec![inert_failed("sync-never-sent", base)];
    assert!(legacy_job_is_inert(&value.upload_queue[0]));
    let (uncertain, uncertain_all) = legacy_uncertain_coverage(&value, &owner);
    assert!(uncertain.is_empty() && !uncertain_all);
    assert!(!legacy_cursors_explained(&value, &owner));
}
