use super::*;

fn original_job() -> settings::UploadJob {
    settings::UploadJob {
        id: "manual-1".into(),
        account_id: "wechat-account".into(),
        space_id: "7".into(),
        owner: Some(settings::UploadOwner {
            base_url: "https://example.invalid".into(),
            space_id: "7".into(),
            account_id: "wechat-account".into(),
            user_id: 19,
            device_id: "device-a".into(),
        }),
        create_started: Some(false),
        status: "running".into(),
        scope: Some(settings::UploadScope {
            usernames: vec!["conversation".into()],
            since: None,
            until: None,
            base_url: "https://example.invalid".into(),
            message_selection: BTreeMap::new(),
        }),
        ..Default::default()
    }
}

fn upload(batch_id: i64) -> DevifyUploadResult {
    DevifyUploadResult {
        job_id: String::new(),
        batch_id,
        status: "processing".into(),
        message_count: 1,
        conversations: 1,
        latest_position: None,
        covered: BTreeMap::new(),
        voice_pending: BTreeMap::new(),
        voice_processed: BTreeMap::new(),
    }
}

#[test]
fn failed_response_keeps_persisted_batch_and_archive_identity() {
    let expected = original_job();
    let mut stored = expected.clone();
    stored.batch_id = Some(42);
    stored.create_started = Some(true);
    stored.package_digest = Some("digest-before-post".into());
    let outcome = Err("network interrupted".to_string());
    apply_manual_upload_outcome(&mut stored, &expected, &outcome, "submitted").unwrap();
    assert_eq!(stored.batch_id, Some(42));
    assert_eq!(stored.create_started, Some(true));
    assert_eq!(stored.package_digest.as_deref(), Some("digest-before-post"));
    assert_eq!(stored.status, "processing");
}

#[test]
fn outcome_cannot_change_owner_or_other_batch() {
    let expected = original_job();
    let mut stored = expected.clone();
    stored.batch_id = Some(42);
    stored.create_started = Some(true);
    stored.package_digest = Some("persisted".into());
    assert!(apply_manual_upload_outcome(&mut stored, &expected, &Ok(upload(43)), "submitted").is_err());
    assert_eq!(stored.batch_id, Some(42));
    assert_eq!(stored.package_digest.as_deref(), Some("persisted"));
    let mut foreign = stored.clone();
    foreign.owner.as_mut().unwrap().user_id += 1;
    assert!(apply_manual_upload_outcome(&mut foreign, &expected, &Err("failed".into()), "submitted").is_err());
}

#[test]
fn completed_task_is_not_downgraded_by_late_response() {
    let expected = original_job();
    let mut stored = expected.clone();
    stored.batch_id = Some(42);
    stored.create_started = Some(true);
    stored.status = "completed".into();
    stored.message = "verified complete".into();
    apply_manual_upload_outcome(&mut stored, &expected, &Err("late network error".into()), "submitted").unwrap();
    assert_eq!(stored.status, "completed");
    assert_eq!(stored.message, "verified complete");
}

#[test]
fn later_failed_and_cancelled_states_survive_success_and_error_responses() {
    for status in ["failed", "cancelled"] {
        for success in [false, true] {
            let expected = original_job();
            let mut stored = expected.clone();
            stored.batch_id = Some(42);
            stored.create_started = Some(true);
            stored.status = status.into();
            stored.updated_at = "later-server-observation".into();
            stored.message = "verified terminal status".into();
            let outcome = if success { Ok(upload(42)) } else { Err("late error".into()) };
            apply_manual_upload_outcome(&mut stored, &expected, &outcome, "submitted").unwrap();
            assert_eq!(stored.status, status);
            assert_eq!(stored.message, "verified terminal status");
        }
    }
}

#[test]
fn explicit_retry_can_leave_its_original_failed_state() {
    let mut expected = original_job();
    expected.status = "failed".into();
    expected.updated_at = "original-failure".into();
    let mut stored = expected.clone();
    stored.batch_id = Some(42);
    stored.create_started = Some(true);
    apply_manual_upload_outcome(&mut stored, &expected, &Ok(upload(42)), "retried").unwrap();
    assert_eq!(stored.status, "processing");
}

#[test]
fn failure_before_post_keeps_safe_unstarted_state() {
    let expected = original_job();
    let mut stored = expected.clone();
    apply_manual_upload_outcome(&mut stored, &expected, &Err("preview failed".into()), "submitted").unwrap();
    assert_eq!(stored.status, "failed");
    assert_eq!(stored.batch_id, None);
    assert_eq!(stored.create_started, Some(false));
}
