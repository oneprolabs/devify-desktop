use super::*;

#[test]
fn signing_out_interrupts_unfinished_uploads() {
    for status in ["queued", "running", "uploading", "processing"] {
        let mut job = settings::UploadJob {
            status: status.into(),
            ..Default::default()
        };
        assert!(abandon_upload_for_logout(&mut job));
        assert_eq!(job.status, "failed");
        assert!(job.legacy_failure_confirmed);
    }
    for status in ["completed", "failed", "cancelled"] {
        let mut job = settings::UploadJob {
            status: status.into(),
            ..Default::default()
        };
        assert!(!abandon_upload_for_logout(&mut job));
        assert_eq!(job.status, status);
    }
}

#[test]
fn interrupted_create_keeps_the_remote_lookup_path() {
    let mut job = settings::UploadJob {
        status: "running".into(),
        create_started: Some(true),
        ..Default::default()
    };
    assert!(abandon_upload_for_logout(&mut job));
    assert_eq!(job.status, "failed");
    assert!(!job.legacy_failure_confirmed);
    assert!(job.message.contains("核对云端批次"));
}

#[test]
fn an_upload_only_stops_after_its_epoch_moves() {
    assert!(!upload_aborted_since(3, 3));
    assert!(upload_aborted_since(3, 4));
}

#[test]
fn signing_out_waits_for_an_upload_that_releases_the_lease() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let base = "https://example.test";
    let mut value = settings::AppSettings::default();
    value.active_account_id = "wechat-a".into();
    value.devify.base_url = base.into();
    value.devify.active_space_id = Some("7".into());
    value.upload_queue = vec![settings::UploadJob {
        id: "sync-1".into(),
        account_id: "wechat-a".into(),
        space_id: "7".into(),
        status: "running".into(),
        ..Default::default()
    }];
    settings::save(data, &value).unwrap();
    desktop_credentials::save(
        data,
        &desktop_credentials::Credentials {
            base_url: base.into(),
            access: "access".into(),
            refresh: "refresh".into(),
            device_session: None,
            session_device_id: None,
        },
    )
    .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        // A running sync holds the read lease for a moment; sign-out waits instead of failing.
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
        });
        let started = std::time::Instant::now();
        let cleared = sign_out_local_session(data).await.unwrap();
        assert!(cleared.is_some());
        assert!(started.elapsed() < LOGOUT_UPLOAD_DRAIN_BUDGET);
        release.await.unwrap();
    });
    assert!(desktop_credentials::load(data, base).is_err());
    let saved = settings::load(data).unwrap();
    assert_eq!(saved.upload_queue[0].status, "failed");
    assert!(saved.upload_queue[0].legacy_failure_confirmed);
}
