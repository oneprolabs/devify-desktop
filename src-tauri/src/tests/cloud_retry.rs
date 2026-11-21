use super::*;

fn fixture() -> (settings::AppSettings, settings::UploadJob, CloudRetryAuth) {
    let scope = settings::UploadScope {
        usernames: vec!["chat".into()],
        since: Some(1),
        until: Some(5),
        base_url: "https://example.test".into(),
        message_selection: BTreeMap::new(),
    };
    let job = settings::UploadJob {
        id: "job".into(),
        space_id: "4".into(),
        account_id: "account".into(),
        batch_id: Some(8),
        status: "failed".into(),
        scope: Some(scope.clone()),
        ..Default::default()
    };
    let mut settings = settings::AppSettings::default();
    settings.active_account_id = "account".into();
    settings.devify.active_space_id = Some("4".into());
    settings.devify.base_url = "https://example.test".into();
    settings.devify.device_id = "device".into();
    settings
        .chat
        .allowed_usernames
        .insert("account".into(), vec!["chat".into()]);
    let auth = CloudRetryAuth {
        token: "synthetic-token".into(),
        base_url: "https://example.test".into(),
        job_id: "job".into(),
        space_id: "4".into(),
        account_id: "account".into(),
        batch_id: 8,
        device_id: "device".into(),
        user_id: 3,
        idempotency_key: "package-key".into(),
        scope,
    };
    (settings, job, auth)
}

fn remote_batch() -> serde_json::Value {
    serde_json::json!({"id": 8, "space_id": "4", "uploaded_by_id": 3,
            "source_type": "wechat", "source_account_id": "account",
            "device_id": "device", "status": "failed", "idempotency_key": "package-key"})
}

#[test]
fn remote_identity_requires_every_field_to_match() {
    let (_, _, auth) = fixture();
    let user = serde_json::json!({"id": 3});
    assert_eq!(
        validate_cloud_retry_remote(&auth, &user, &remote_batch()).unwrap(),
        (3, "package-key".into())
    );
    for (field, wrong) in [
        ("id", serde_json::json!(9)),
        ("space_id", serde_json::json!("5")),
        ("uploaded_by_id", serde_json::json!(2)),
        ("source_type", serde_json::json!("other")),
        ("source_account_id", serde_json::json!("other")),
        ("device_id", serde_json::json!("other")),
        ("status", serde_json::json!("processing")),
        ("idempotency_key", serde_json::json!("")),
    ] {
        let mut batch = remote_batch();
        batch[field] = wrong;
        assert!(
            validate_cloud_retry_remote(&auth, &user, &batch).is_err(),
            "{field}"
        );
        batch.as_object_mut().unwrap().remove(field);
        assert!(
            validate_cloud_retry_remote(&auth, &user, &batch).is_err(),
            "missing {field}"
        );
    }
    assert!(validate_cloud_retry_remote(&auth, &serde_json::json!({}), &remote_batch()).is_err());
    assert!(
        validate_cloud_retry_remote(&auth, &serde_json::json!({"id": 0}), &remote_batch()).is_err()
    );
    assert!(
        validate_cloud_retry_remote(&auth, &serde_json::json!({"id": 2}), &remote_batch()).is_err()
    );
}

#[test]
fn retry_context_requires_original_account_space_service_device_scope_and_token() {
    let (settings, job, auth) = fixture();
    assert!(validate_cloud_retry_context(&settings, &job, &auth, "synthetic-token").is_ok());
    assert!(validate_cloud_retry_context(&settings, &job, &auth, "changed-token").is_err());
    let mut changed = settings.clone();
    changed.active_account_id = "other".into();
    assert!(validate_cloud_retry_context(&changed, &job, &auth, "synthetic-token").is_err());
    let mut changed = settings.clone();
    changed.devify.active_space_id = Some("5".into());
    assert!(validate_cloud_retry_context(&changed, &job, &auth, "synthetic-token").is_err());
    let mut changed = settings.clone();
    changed.devify.base_url = "https://other.test".into();
    assert!(validate_cloud_retry_context(&changed, &job, &auth, "synthetic-token").is_err());
    let mut changed = settings.clone();
    changed.devify.device_id = "other".into();
    assert!(validate_cloud_retry_context(&changed, &job, &auth, "synthetic-token").is_err());
    let mut changed = settings.clone();
    changed.devify.device_id.clear();
    assert!(validate_cloud_retry_context(&changed, &job, &auth, "synthetic-token").is_err());
    let mut changed = settings.clone();
    changed.chat.allowed_usernames.clear();
    assert!(validate_cloud_retry_context(&changed, &job, &auth, "synthetic-token").is_err());
    let mut changed = job.clone();
    changed.batch_id = Some(9);
    assert!(validate_cloud_retry_context(&settings, &changed, &auth, "synthetic-token").is_err());
    let mut changed = job.clone();
    changed.status = "processing".into();
    assert!(validate_cloud_retry_context(&settings, &changed, &auth, "synthetic-token").is_err());
    let mut changed = job.clone();
    changed.scope.as_mut().unwrap().until = Some(6);
    assert!(validate_cloud_retry_context(&settings, &changed, &auth, "synthetic-token").is_err());
}

#[test]
fn retry_identity_response_allows_five_mib_but_rejects_more() {
    let mut body = Vec::new();
    let first = vec![0u8; 1024 * 1024 + 1];
    append_retry_identity_body(&mut body, &first).unwrap();
    let rest = vec![0u8; UPLOAD_LIST_MAX_RESPONSE_BYTES - first.len()];
    append_retry_identity_body(&mut body, &rest).unwrap();
    assert_eq!(body.len(), UPLOAD_LIST_MAX_RESPONSE_BYTES);
    assert!(append_retry_identity_body(&mut body, &[0]).is_err());
    assert_eq!(body.len(), UPLOAD_LIST_MAX_RESPONSE_BYTES);
}

#[test]
fn package_key_and_device_claim_are_not_guessed() {
    let (_, _, auth) = fixture();
    assert!(validate_cloud_retry_package(&auth, "4", "account", "package-key").is_ok());
    assert!(validate_cloud_retry_package(&auth, "4", "account", "changed-key").is_err());
    assert!(validate_cloud_retry_package(&auth, "5", "account", "package-key").is_err());
    assert!(validate_cloud_retry_package(&auth, "4", "other", "package-key").is_err());
    let token = format!(
        "header.{}.signature",
        URL_SAFE_NO_PAD.encode(br#"{"device_id":"device"}"#)
    );
    assert_eq!(cloud_retry_device_id("", &token).unwrap(), "device");
    assert_eq!(cloud_retry_device_id("device", &token).unwrap(), "device");
    assert!(cloud_retry_device_id("other", &token).is_err());
    assert!(cloud_retry_device_id("", "synthetic-token").is_err());
    assert!(cloud_retry_device_id(" device", "synthetic-token").is_err());
}
