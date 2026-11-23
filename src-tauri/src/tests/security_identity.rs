//! Identity race regressions. Register from lib.rs with `#[cfg(test)] mod security_identity_tests;`.
//! Run with `--test-threads=1`: credential generation and the identity lock are process-global.

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener as StdTcpListener;
use std::thread;

fn credentials(base: &str, access: &str) -> desktop_credentials::Credentials {
    desktop_credentials::Credentials { base_url: base.into(), access: access.into(), refresh: "synthetic-refresh".into(), device_session: None, session_device_id: None }
}

fn settings_for(data: &Path, base: &str) -> settings::AppSettings {
    settings::update(data, |value| {
        value.devify.base_url = base.into();
        value.devify.active_space_id = Some("7".into());
        value.devify.device_id = "device-1".into();
        value.active_account_id = "account-1".into();
        value.automation.enabled = true;
        value.automation.daily_summary_enabled = true;
        value.automation.daily_time = "08:30".into();
        Ok(())
    }).unwrap()
}

fn daily_identity(data: &Path, base: &str, value: &settings::AppSettings) -> DailyIdentity {
    let (credentials, generation) = desktop_credentials::load_with_generation(data, base).unwrap();
    DailyIdentity {
        data: data.to_path_buf(), base: base.into(), credentials, generation,
        space_id: value.devify.active_space_id.clone().unwrap(), account_id: value.active_account_id.clone(),
        device_id: value.devify.device_id.clone(), enabled: value.automation.enabled && value.automation.daily_summary_enabled,
        time_local: value.automation.daily_time.trim().into(), topic_id: value.automation.daily_topic_id,
        topic_space_id: value.automation.daily_topic_space_id.clone(),
    }
}

#[test]
fn old_auth_refresh_and_401_responses_cannot_replace_or_clear_new_login() {
    let dir = tempfile::tempdir().unwrap();
    let base = "https://example.invalid";
    settings_for(dir.path(), base);
    let old_attempt = desktop_credentials::begin_auth_attempt().unwrap();
    let old = credentials(base, "same-synthetic-token");
    let new_attempt = desktop_credentials::begin_auth_attempt().unwrap();
    settings::with_identity(dir.path(), |_| {
        assert!(desktop_credentials::save_if_generation_locked(dir.path(), new_attempt, &old)?);
        assert!(!desktop_credentials::save_if_generation_locked(dir.path(), old_attempt, &credentials(base, "old-response"))?);
        Ok(())
    }).unwrap();
    let (_, old_request_generation) = desktop_credentials::load_with_generation(dir.path(), base).unwrap();
    // The same token can be issued again, so byte comparison alone is insufficient.
    desktop_credentials::save(dir.path(), &old).unwrap();
    assert!(!desktop_credentials::clear_if_access(dir.path(), base, &old.access, old_request_generation).unwrap());
    assert!(!desktop_credentials::clear_if_current(dir.path(), old_request_generation, &old).unwrap());
    assert!(!desktop_credentials::save_if_current(dir.path(), old_request_generation, &old, &credentials(base, "stale-refresh")).unwrap());
    assert!(desktop_credentials::load(dir.path(), base).unwrap() == old);
    let pending = desktop_credentials::begin_auth_attempt().unwrap();
    desktop_credentials::clear(dir.path()).unwrap();
    settings::with_identity(dir.path(), |_| {
        assert!(!desktop_credentials::save_if_generation_locked(dir.path(), pending, &old)?);
        Ok(())
    }).unwrap();
    assert!(desktop_credentials::load(dir.path(), base).is_err());
}

#[test]
fn refresh_401_returns_logged_out_only_when_its_cas_still_owns_absence() {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let dir = tempfile::tempdir().unwrap();
    settings_for(dir.path(), &base);
    desktop_credentials::save(dir.path(), &credentials(&base, "synthetic-expired")).unwrap();
    let before = desktop_credentials::generation().unwrap();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut request = [0u8; 4096];
        let size = socket.read(&mut request).unwrap();
        assert!(String::from_utf8_lossy(&request[..size]).starts_with("POST /api/v1/auth/token/refresh "));
        socket.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut cleared_generation = None;
    assert!(runtime.block_on(devify_access_token_with_cleanup(dir.path(), &base, &mut cleared_generation)).is_err());
    server.join().unwrap();
    assert_eq!(cleared_generation, Some(before.wrapping_add(1)));
    assert!(!auth_status_after_token_failure(dir.path(), &base, before, cleared_generation).unwrap().authenticated);

    // A new attempt invalidates the old status result even before it saves a credential.
    desktop_credentials::begin_auth_attempt().unwrap();
    assert!(auth_status_after_token_failure(dir.path(), &base, before, cleared_generation).is_err());
    desktop_credentials::save(dir.path(), &credentials(&base, "new-login")).unwrap();
    assert!(auth_status_after_token_failure(dir.path(), &base, before, cleared_generation).is_err());
    assert_eq!(desktop_credentials::load(dir.path(), &base).unwrap().access, "new-login");
    settings::update(dir.path(), |s| { s.devify.base_url = "https://other.invalid".into(); Ok(()) }).unwrap();
    assert!(auth_status_after_token_failure(dir.path(), &base, before, cleared_generation).is_err());
}

#[test]
fn stale_refresh_401_cannot_report_new_credentials_as_logged_out() {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let dir = tempfile::tempdir().unwrap();
    settings_for(dir.path(), &base);
    desktop_credentials::save(dir.path(), &credentials(&base, "synthetic-expired")).unwrap();
    let before = desktop_credentials::generation().unwrap();
    let data = dir.path().to_path_buf();
    let server_base = base.clone();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut request = [0u8; 4096];
        socket.read(&mut request).unwrap();
        desktop_credentials::save(&data, &credentials(&server_base, "new-login")).unwrap();
        socket.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut cleared_generation = None;
    assert!(runtime.block_on(devify_access_token_with_cleanup(dir.path(), &base, &mut cleared_generation)).is_err());
    server.join().unwrap();
    assert_eq!(cleared_generation, None);
    assert!(auth_status_after_token_failure(dir.path(), &base, before, cleared_generation).is_err());
    assert_eq!(desktop_credentials::load(dir.path(), &base).unwrap().access, "new-login");
}

#[test]
fn identity_changes_invalidate_pending_auth_even_after_aba() {
    let dir = tempfile::tempdir().unwrap();
    let base = "https://example.invalid";
    settings_for(dir.path(), base);
    for field in ["base", "space", "device", "account"] {
        let generation = desktop_credentials::begin_auth_attempt().unwrap();
        settings::update(dir.path(), |s| {
            match field {
                "base" => s.devify.base_url = "https://other.invalid".into(),
                "space" => s.devify.active_space_id = Some("8".into()),
                "device" => s.devify.device_id = "device-2".into(),
                _ => s.active_account_id = "account-2".into(),
            }
            Ok(())
        }).unwrap();
        settings_for(dir.path(), base);
        settings::with_identity(dir.path(), |_| {
            assert!(!desktop_credentials::save_if_generation_locked(dir.path(), generation, &credentials(base, "old-response"))?);
            Ok(())
        }).unwrap();
    }
}

#[test]
fn daily_stale_topic_warns_without_overwriting_old_topic() {
    let dir = tempfile::tempdir().unwrap();
    let base = "https://example.invalid";
    settings_for(dir.path(), base);
    desktop_credentials::save(dir.path(), &credentials(base, "synthetic-token")).unwrap();
    let value = settings::update(dir.path(), |s| {
        s.automation.daily_topic_id = Some(123);
        s.automation.daily_topic_space_id = Some("8".into());
        Ok(())
    }).unwrap();
    let context = daily_identity(dir.path(), base, &value);
    let warning = daily_stale_topic_warning(&value, "7").unwrap();
    settings::update(dir.path(), |s| {
        context.validate_settings(s)?;
        s.automation.last_schedule_error = Some(warning.into());
        Ok(())
    }).unwrap();
    let stored = settings::load(dir.path()).unwrap();
    assert_eq!(stored.automation.daily_topic_id, Some(123));
    assert_eq!(stored.automation.daily_topic_space_id, Some("8".into()));
    assert_eq!(stored.automation.last_schedule_error.as_deref(), Some(warning));
    assert!(daily_stale_topic_warning(&stored, "8").is_none());
    let mut no_topic = stored.clone();
    no_topic.automation.daily_topic_id = None;
    assert!(daily_stale_topic_warning(&no_topic, "7").is_none());
}

#[test]
fn daily_request_uses_fixed_identity_and_merge_preserves_unrelated_settings() {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let value = settings_for(dir.path(), &base);
    desktop_credentials::save(dir.path(), &credentials(&base, "synthetic-token")).unwrap();
    let context = daily_identity(dir.path(), &base, &value);
    let data = dir.path().to_path_buf();
    let expected_base = base.clone();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut request = vec![0u8; 4096];
        let size = socket.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..size]).to_lowercase();
        assert!(request.starts_with("get /api/v1/spaces/7/topics "));
        assert!(request.contains("authorization: bearer synthetic-token"));
        // Identity writes are refused while the request's read lease is held.
        assert!(desktop_credentials::save(&data, &credentials(&expected_base, "new-login")).is_err());
        settings::update(&data, |s| { s.devify.subject = "unrelated edit".into(); Ok(()) }).unwrap();
        let body = r#"{"data":[]}"#;
        write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let _lease = UPLOAD_IDENTITY_LOCK.read().await;
        let result = daily_request(&context, reqwest::Method::GET, "/api/v1/spaces/7/topics".into(), None).await.unwrap();
        assert_eq!(result, serde_json::json!([]));
        settings::update(dir.path(), |s| {
            context.validate_settings(s)?;
            s.automation.last_schedule_error = None;
            Ok(())
        }).unwrap();
    });
    server.join().unwrap();
    assert_eq!(settings::load(dir.path()).unwrap().devify.subject, "unrelated edit");
    settings::update(dir.path(), |s| { s.devify.active_space_id = Some("8".into()); Ok(()) }).unwrap();
    assert!(context.validate().is_err());
}

#[test]
fn refresh_server_error_keeps_credentials_and_reports_unavailable() {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let dir = tempfile::tempdir().unwrap();
    settings_for(dir.path(), &base);
    let original = credentials(&base, "synthetic-expired");
    desktop_credentials::save(dir.path(), &original).unwrap();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        socket.read(&mut request).unwrap();
        socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut cleared = None;
    let error = runtime.block_on(devify_access_token_with_cleanup(dir.path(), &base, &mut cleared)).unwrap_err();
    assert!(matches!(error, DevifyTokenError::Unavailable(_)));
    assert!(cleared.is_none());
    assert!(desktop_credentials::load(dir.path(), &base).unwrap() == original);
    server.join().unwrap();
}

#[test]
fn refresh_blocked_by_upload_preserves_login_and_recovers_after_the_lease_ends() {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let dir = tempfile::tempdir().unwrap();
    settings_for(dir.path(), &base);
    let original = credentials(&base, "synthetic-expired");
    desktop_credentials::save(dir.path(), &original).unwrap();
    let before = desktop_credentials::generation().unwrap();
    let server = thread::spawn(move || {
        for _ in 0..1 {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut request = [0u8; 4096];
            socket.read(&mut request).unwrap();
            let body = r#"{"access":"synthetic-refreshed","refresh":"synthetic-rotated"}"#;
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
    });
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let lease = UPLOAD_IDENTITY_LOCK.read().await;
        let mut cleared = None;
        let error = devify_access_token_with_cleanup(dir.path(), &base, &mut cleared).await.unwrap_err();
        assert!(matches!(error, DevifyTokenError::Unavailable(_)), "an upload blocks remote token rotation without logging out");
        assert!(cleared.is_none());
        assert_eq!(desktop_credentials::generation().unwrap(), before);
        assert!(desktop_credentials::load(dir.path(), &base).unwrap() == original);
        assert!(desktop_credentials::begin_auth_attempt().is_err(), "keep the upload identity guard intact");
        drop(lease);
        assert_eq!(devify_access_token_with_cleanup(dir.path(), &base, &mut cleared).await.unwrap(), "synthetic-refreshed");
        assert_eq!(desktop_credentials::generation().unwrap(), before.wrapping_add(1));
        assert_eq!(desktop_credentials::load(dir.path(), &base).unwrap().refresh, "synthetic-rotated");
        assert!(cleared.is_none());
    });
    server.join().unwrap();
}

#[test]
fn invalid_refresh_response_does_not_log_out_or_discard_credentials() {
    for body in ["not json", "{}", r#"{"access":""}"#] {
        let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let dir = tempfile::tempdir().unwrap();
        settings_for(dir.path(), &base);
        let original = credentials(&base, "synthetic-expired");
        desktop_credentials::save(dir.path(), &original).unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            socket.read(&mut request).unwrap();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        });
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let mut cleared = None;
        let error = runtime.block_on(devify_access_token_with_cleanup(dir.path(), &base, &mut cleared)).unwrap_err();
        assert!(matches!(error, DevifyTokenError::Unavailable(_)));
        assert!(cleared.is_none());
        assert!(desktop_credentials::load(dir.path(), &base).unwrap() == original);
        server.join().unwrap();
    }
}

#[test]
fn legacy_credentials_remain_readable_after_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let base = "https://example.com";
    std::fs::write(dir.path().join("devify-credentials.json"), r#"{"base_url":"https://example.com","access":"old","refresh":"old-refresh"}"#).unwrap();
    let stored = desktop_credentials::load(dir.path(), base).unwrap();
    assert_eq!(stored.refresh, "old-refresh");
    assert!(stored.device_session.is_none());
}

#[test]
fn expired_access_uses_persistent_device_session() {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let dir = tempfile::tempdir().unwrap();
    settings_for(dir.path(), &base);
    let mut original = credentials(&base, "expired");
    original.device_session = Some("device-secret".into());
    original.session_device_id = Some("mac".into());
    desktop_credentials::save(dir.path(), &original).unwrap();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        let size = socket.read(&mut request).unwrap();
        assert!(String::from_utf8_lossy(&request[..size]).starts_with("POST /api/v1/auth/desktop/session/refresh "));
        let body = r#"{"access":"new-access"}"#;
        write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut cleared = None;
    assert_eq!(runtime.block_on(devify_access_token_resolve(dir.path(), &base, &mut cleared, false)).unwrap(), "new-access");
    assert_eq!(desktop_credentials::load(dir.path(), &base).unwrap().device_session, original.device_session);
    server.join().unwrap();
}

#[test]
fn recovered_login_clears_only_refresh_errors_for_the_current_owner() {
    let dir = tempfile::tempdir().unwrap();
    let base = "https://example.invalid";
    let token = format!("x.{}.x", URL_SAFE_NO_PAD.encode(r#"{"user_id":7}"#));
    settings_for(dir.path(), base);
    desktop_credentials::save(dir.path(), &credentials(base, &token)).unwrap();
    let refresh_error = "刷新登录状态失败（HTTP 404），请稍后重试。";
    let owner = settings::UploadOwner {
        base_url: base.into(),
        user_id: 7,
        device_id: "device-1".into(),
        space_id: "7".into(),
        account_id: "current".into(),
    };
    settings::update(dir.path(), |value| {
        value.runtime.last_error = Some(refresh_error.into());
        value.runtime.sources.insert(
            "current".into(),
            config_runtime::Source {
                owner: Some(owner.clone()),
                last_error: Some(refresh_error.into()),
                retry_after: Some("2099-01-01T00:00:00Z".into()),
                failures: 2,
                ..Default::default()
            },
        );
        value.runtime.sources.insert(
            "other".into(),
            config_runtime::Source {
                owner: Some(settings::UploadOwner {
                    user_id: 8,
                    account_id: "other".into(),
                    ..owner.clone()
                }),
                last_error: Some(refresh_error.into()),
                ..Default::default()
            },
        );
        value.runtime.sources.insert(
            "network".into(),
            config_runtime::Source {
                owner: Some(settings::UploadOwner {
                    account_id: "network".into(),
                    ..owner
                }),
                last_error: Some("网络暂时不可用".into()),
                ..Default::default()
            },
        );
        Ok(())
    })
    .unwrap();

    clear_recovered_refresh_errors(dir.path(), base, "old-access");
    assert_eq!(
        settings::load(dir.path())
            .unwrap()
            .runtime
            .last_error
            .as_deref(),
        Some(refresh_error)
    );

    clear_recovered_refresh_errors(dir.path(), base, &token);
    let runtime = settings::load(dir.path()).unwrap().runtime;
    assert!(runtime.last_error.is_none());
    let current = &runtime.sources["current"];
    assert!(current.last_error.is_none());
    assert!(current.retry_after.is_none());
    assert_eq!(current.failures, 0);
    assert_eq!(
        runtime.sources["other"].last_error.as_deref(),
        Some(refresh_error)
    );
    assert_eq!(
        runtime.sources["network"].last_error.as_deref(),
        Some("网络暂时不可用")
    );
}

#[test]
fn missing_device_session_endpoint_is_not_retried_on_every_registration() {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let dir = tempfile::tempdir().unwrap();
    settings_for(dir.path(), &base);
    let original = credentials(&base, "synthetic-access");
    desktop_credentials::save(dir.path(), &original).unwrap();
    let registration = DevifyDeviceResult {
        device_id: "device-1".into(),
        label: "Test device".into(),
        platform: "macos".into(),
        status: "active".into(),
        generation: desktop_credentials::generation().unwrap(),
        summary_config_version: None,
    };
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut request = [0u8; 4096];
        let size = socket.read(&mut request).unwrap();
        assert!(request[..size].starts_with(b"POST /api/v1/auth/desktop/session "));
        socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (saved, generation) = runtime
        .block_on(establish_device_session(dir.path(), &base, &registration))
        .unwrap();
    server.join().unwrap();
    assert!(saved == original);
    assert_eq!(generation, registration.generation);

    // The server has stopped, so a second network request would fail.
    let (saved, generation) = runtime
        .block_on(establish_device_session(dir.path(), &base, &registration))
        .unwrap();
    assert!(saved == original);
    assert_eq!(generation, registration.generation);
}

#[test]
fn rejected_access_can_force_refresh_before_its_local_expiry() {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let dir = tempfile::tempdir().unwrap();
    settings_for(dir.path(), &base);
    let payload = URL_SAFE_NO_PAD.encode(format!("{{\"exp\":{}}}", chrono::Utc::now().timestamp() + 3600));
    desktop_credentials::save(dir.path(), &credentials(&base, &format!("x.{payload}.x"))).unwrap();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        let size = socket.read(&mut request).unwrap();
        assert!(String::from_utf8_lossy(&request[..size]).starts_with("POST /api/v1/auth/token/refresh "));
        let body = r#"{"access":"refreshed-token"}"#;
        write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut cleared = None;
    assert_eq!(runtime.block_on(devify_access_token_resolve(dir.path(), &base, &mut cleared, true)).unwrap(), "refreshed-token");
    assert_eq!(desktop_credentials::load(dir.path(), &base).unwrap().access, "refreshed-token");
    server.join().unwrap();
}

#[test]
fn desktop_login_config_reports_html_as_service_configuration_error() {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let settings = settings_for(dir.path(), &base);
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        let size = socket.read(&mut request).unwrap();
        let path = if crate::build_flavor::DEVELOPMENT { "/api/v1/auth/desktop/config?channel=development" } else { "/api/v1/auth/desktop/config" };
        assert!(String::from_utf8_lossy(&request[..size]).starts_with(&format!("GET {path} ")));
        let body = "<html>not the login API</html>";
        write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let error = match runtime.block_on(devify_auth_config(dir.path(), &settings.devify)) {
        Ok(_) => panic!("HTML cannot be a login configuration"),
        Err(error) => error,
    };
    assert!(error.contains("服务端登录配置响应无效"));
    server.join().unwrap();
}
