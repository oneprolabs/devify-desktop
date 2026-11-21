use super::*;
use std::net::TcpListener as StdListener;

fn valid_page() -> serde_json::Value {
    serde_json::json!({
        "list": [{"id": 9, "status": "processing", "trigger": "scheduled"}],
        "pagination": {"total": 1, "page": 1, "page_size": 100, "next": null, "previous": null}
    })
}

#[test]
fn pagination_rejects_zero_and_overflow() {
    assert_eq!(upload_list_pagination(None, None).unwrap(), (1, 100));
    assert_eq!(upload_list_pagination(Some(2), Some(1)).unwrap(), (2, 1));
    for (page, size) in [(0, 1), (1, 0), (1, 101), (UPLOAD_LIST_MAX_PAGE + 1, 1)] {
        assert!(upload_list_pagination(Some(page), Some(size)).is_err());
    }
}

#[test]
fn upload_list_requires_complete_pagination_and_keeps_server_rows() {
    let expected = valid_page();
    assert_eq!(
        parse_upload_list(serde_json::json!({"code": 0, "data": expected})).unwrap(),
        expected
    );
    assert_eq!(parse_upload_list(expected.clone()).unwrap(), expected);
    for payload in [
        serde_json::json!({"data": null}),
        serde_json::json!({"list": null, "pagination": {}}),
        serde_json::json!({"list": [null], "pagination": {"total": 1, "page": 1, "page_size": 10, "next": null, "previous": null}}),
        serde_json::json!({"list": [], "pagination": {"total": 0, "page": 1, "page_size": 10, "next": null}}),
        serde_json::json!({"list": [], "pagination": {"total": 0, "page": 0, "page_size": 10, "next": null, "previous": null}}),
        serde_json::json!({"list": [], "pagination": {"total": 0, "page": 1, "page_size": 101, "next": null, "previous": null}}),
    ] {
        assert!(parse_upload_list(payload).is_err());
    }
}

#[test]
fn list_rejects_wrong_space_and_unsafe_service_address() {
    let mut settings = settings::AppSettings::default();
    settings.devify.active_space_id = Some("4".into());
    settings.devify.base_url = "https://example.test/".into();
    assert_eq!(
        upload_list_base_url(&settings, "4").unwrap(),
        "https://example.test"
    );
    assert!(upload_list_base_url(&settings, "3").is_err());
    assert!(upload_list_base_url(&settings, "").is_err());
    for base in [
        "https://user:password@example.test",
        "https://example.test/?token=secret",
        "file:///tmp/data",
        "https://example.test/#fragment",
    ] {
        settings.devify.base_url = base.into();
        assert!(upload_list_base_url(&settings, "4").is_err());
    }
}

#[test]
fn list_http_request_is_paged_authenticated_and_does_not_follow_redirect() {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buf = [0; 1024];
        while !request.ends_with(b"\r\n\r\n") {
            let count = stream.read(&mut buf).unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buf[..count]);
        }
        stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/other\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        String::from_utf8(request).unwrap()
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (status, body) = runtime
        .block_on(fetch_upload_list(
            tempfile::tempdir().unwrap().path(),
            &format!("http://{address}"),
            "42",
            2,
            30,
            "test-token",
        ))
        .unwrap();
    assert_eq!(status, reqwest::StatusCode::FOUND);
    assert!(body.is_empty());
    let request = server.join().unwrap().to_ascii_lowercase();
    assert!(
        request.starts_with("get /api/v1/spaces/42/chat/uploads?page=2&page_size=30 http/1.1\r\n")
    );
    assert!(request.contains("authorization: bearer test-token\r\n"));
}

#[test]
fn trigger_respects_original_sync_source_and_unknown_history() {
    assert_eq!(
        sync_upload_trigger(true, false, None),
        UploadTrigger::Manual
    );
    assert_eq!(
        sync_upload_trigger(false, false, None),
        UploadTrigger::Scheduled
    );
    assert_eq!(
        sync_upload_trigger(true, true, Some(true)),
        UploadTrigger::Manual
    );
    assert_eq!(
        sync_upload_trigger(true, true, Some(false)),
        UploadTrigger::Scheduled
    );
    assert_eq!(
        sync_upload_trigger(true, true, None),
        UploadTrigger::Unknown
    );
    for (trigger, expected) in [
        (UploadTrigger::Manual, "manual"),
        (UploadTrigger::Scheduled, "scheduled"),
        (UploadTrigger::Unknown, "unknown"),
    ] {
        let body = upload_create_body(
            "account", "key", "device", "label", "mac", "wechat", trigger,
        );
        assert_eq!(body["trigger"], expected);
        assert_eq!(body["source_type"], "wechat");
        assert_eq!(body["platform"], "mac");
        assert_eq!(body["idempotency_key"], "key");
    }
    let whatsapp = upload_create_body(
        "8615652425800@s.whatsapp.net",
        "key",
        "device",
        "label",
        "macos",
        "whatsapp",
        UploadTrigger::Manual,
    );
    assert_eq!(whatsapp["source_type"], "whatsapp");
    assert_eq!(whatsapp["platform"], "macos");
    let legacy: settings::ContinuationScope = serde_json::from_value(
        serde_json::json!({"space_id": "1", "base_url": "https://example.test"}),
    )
    .unwrap();
    assert_eq!(legacy.manual, None);
    let legacy_job: settings::UploadJob =
        serde_json::from_value(serde_json::json!({"id": "old"})).unwrap();
    assert_eq!(legacy_job.continuation_manual, None);
    for source in [Some(true), Some(false), None] {
        let mut value = settings::AppSettings::default();
        value.upload_queue.push(settings::UploadJob {
            account_id: "account".into(),
            space_id: "1".into(),
            status: "completed".into(),
            continue_sync: true,
            continuation_manual: source,
            scope: Some(settings::UploadScope {
                usernames: vec!["chat".into()],
                since: None,
                until: None,
                base_url: "https://example.test".into(),
                message_selection: BTreeMap::new(),
            }),
            ..Default::default()
        });
        apply_completed_uploads(&mut value);
        assert_eq!(value.sync.continuation["account"]["chat"].manual, source);
    }
}
