use super::*;
use serde_json::json;

pub(super) fn fixture() -> Catalog {
    let scope = Scope {
        server: "http://localhost:7000".into(),
        account_id: "7".into(),
        space_id: "space".into(),
        capabilities: Capabilities {
            enforced: true,
            features: vec!["cloud.chat_summary".into()],
            internal: false,
        },
    };
    let template = "仅据资料生成所选内容，不编造。".to_string();
    let budget = Budget {
        unit: "utf8_bytes".into(),
        max_request_bytes: 12000,
        prompt_bytes: template.len(),
        reserved_overhead_bytes: 2048,
        max_content_bytes: 12000 - template.len() - 2048,
    };
    let scenarios = [
        "summary",
        "action_items",
        "timeline",
        "three_part",
        "custom",
    ]
    .into_iter()
    .map(|id| Scenario {
        id: id.into(),
        name: id.into(),
        description: "Description".into(),
        requires_instructions: id == "custom",
        template: template.clone(),
        sha256: hex::encode(Sha256::digest(template.as_bytes())),
        budgets: Budgets {
            cloud: budget.clone(),
            local_cli: budget.clone(),
        },
    })
    .collect();
    Catalog {
        schema_version: 1,
        version: "test-v7".into(),
        scope,
        default_scenario: "three_part".into(),
        scenarios,
        max_instructions_chars: 4000,
        output_format: "markdown".into(),
        min_check_interval_seconds: 21600,
        retry_after_seconds: 900,
        no_cache_behavior: "require_online_initialization".into(),
    }
}
fn input(config: &Catalog, scenario: &str) -> serde_json::Value {
    json!({"scenario":scenario,"prompt_version":config.version,"prompt_digest":config.scenarios.iter().find(|s|s.id==scenario).unwrap().sha256,
        "instructions":"","language":"English","title":"Group","range_label":"Today",
        "messages":[{"username":"group","display":"Group","sender":"Member","time":1700000000,"text":"Release on Friday","is_group":true}]})
}

#[test]
fn catalogs_validate_schema_digest_scope_default_and_budget() {
    let config = fixture();
    validate(&config, &config.scope).unwrap();
    assert_ne!(config.default_scenario, config.scenarios[0].id);
    for mutate in [
        |c: &mut Catalog| c.schema_version = 2,
        |c: &mut Catalog| c.scenarios[0].template.push('!'),
        |c: &mut Catalog| c.default_scenario = "missing".into(),
        |c: &mut Catalog| c.scenarios[0].budgets.cloud.max_content_bytes += 1,
        |c: &mut Catalog| c.scope.account_id = "another-user".into(),
        |c: &mut Catalog| c.scope.space_id = "another-space".into(),
        |c: &mut Catalog| c.scope.server = "https://another-server".into(),
        |c: &mut Catalog| c.scope.capabilities.internal = true,
    ] {
        let mut invalid = config.clone();
        mutate(&mut invalid);
        assert!(validate(&invalid, &config.scope).is_err());
    }
}
#[test]
fn atomic_cache_keeps_valid_rules_when_replacement_is_invalid_or_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let config = fixture();
    let cache = Cache {
        scope: config.scope.clone(),
        config: Some(config.clone()),
        etag: Some("\"v7\"".into()),
        checked_at: 10,
        retry_at: 0,
        refresh_id: "first".into(),
    };
    write(dir.path(), &cache).unwrap();
    let mut broken = cache.clone();
    broken.config.as_mut().unwrap().scenarios[0].sha256.clear();
    assert!(write(dir.path(), &broken).is_err());
    let parent = path(dir.path(), &cache.scope)
        .parent()
        .unwrap()
        .to_path_buf();
    let mut partial = tempfile::NamedTempFile::new_in(parent).unwrap();
    partial.write_all(b"{partial").unwrap();
    assert_eq!(
        load(dir.path(), &cache.scope).unwrap().etag.as_deref(),
        Some("\"v7\"")
    );
    let mut other = cache.scope.clone();
    other.account_id = "8".into();
    assert!(load(dir.path(), &other).is_none());
    other = cache.scope.clone();
    other.capabilities.features.push("ai.local_agent".into());
    assert!(load(dir.path(), &other).is_none());
    // Restart validation rejects tampered persisted templates too.
    fs::write(
        path(dir.path(), &cache.scope),
        serde_json::to_vec(&broken).unwrap(),
    )
    .unwrap();
    assert!(load(dir.path(), &cache.scope).is_none());
}
#[test]
fn requests_freeze_scenario_language_instructions_and_original_template() {
    let mut config = fixture();
    let settings = settings::LanguageSettings {
        ui: "中文".into(),
        summary: "English".into(),
        ask: "跟随提问".into(),
    };
    let mut body = input(&config, "custom");
    assert!(prompt::freeze(&config, &mut body, &settings, "local", true).is_err());
    body["instructions"] = "  Focus on deadlines  ".into();
    body["language"] = "按原文语言".into();
    let frozen = prompt::freeze(&config, &mut body, &settings, "local", true).unwrap();
    assert_eq!(body["language"], "按原文语言");
    assert_eq!(body["instructions"], "Focus on deadlines");
    config.scenarios[4].template = "Changed later".into();
    config.version = "v8".into();
    let generated =
        prompt::local_prompt(&frozen, &body, &prompt::local_input(&body).unwrap()).unwrap();
    assert!(generated.starts_with("仅据资料"));
    assert!(!generated.contains("Changed later"));
    assert!(
        generated.contains("Focus on deadlines")
            && generated.contains("按原文语言")
            && generated.contains("不要调用工具")
    );
    let persisted: prompt::Frozen =
        serde_json::from_str(&serde_json::to_string(&frozen).unwrap()).unwrap();
    assert_eq!(persisted.version, "test-v7");
    assert_eq!(prompt::language(None, &settings).unwrap(), "English");
    assert_eq!(
        prompt::language(Some("跟随界面"), &settings).unwrap(),
        "中文"
    );
    assert_eq!(prompt::language(Some("es"), &settings).unwrap(), "es");
}
#[test]
fn user_requirements_and_json_escaping_consume_the_budget_without_truncation() {
    let config = fixture();
    let settings = settings::LanguageSettings::default();
    let mut body = input(&config, "summary");
    body["instructions"] = "😀".repeat(4001).into();
    assert!(prompt::freeze(&config, &mut body, &settings, "cloud", true).is_err());
    body["instructions"] = "\"".repeat(4000).into();
    body["messages"][0]["text"] = format!("{}x", "\n".repeat(1500)).into();
    assert!(prompt::freeze(&config, &mut body, &settings, "cloud", true).is_err());
    assert_eq!(body["messages"][0]["text"].as_str().unwrap().len(), 1501);
    body["instructions"] = "".into();
    assert!(prompt::freeze(&config, &mut body, &settings, "cloud", true).is_ok());
    body["prompt_digest"] = "obsolete".into();
    assert!(prompt::freeze(&config, &mut body, &settings, "cloud", true).is_err());
}
#[test]
fn only_the_documented_409_means_the_server_created_no_task() {
    let known = transport::cloud_error(
        reqwest::StatusCode::CONFLICT,
        &json!({"data":{"error_code":"summary_config_changed"}}),
    );
    assert!(known.config_changed);
    for (status, payload) in [
        (
            reqwest::StatusCode::CONFLICT,
            json!({"data":{"error_code":"request_conflict"}}),
        ),
        (
            reqwest::StatusCode::CONFLICT,
            json!({"error_code":"summary_config_changed"}),
        ),
        (
            reqwest::StatusCode::BAD_REQUEST,
            json!({"data":{"error_code":"summary_config_changed"}}),
        ),
    ] {
        assert!(!transport::cloud_error(status, &payload).config_changed);
    }
}

#[test]
fn refreshes_are_throttled_coalesced_and_backed_off_even_when_forced() {
    let config = fixture();
    let mut cache = Cache {
        scope: config.scope.clone(),
        config: Some(config),
        etag: Some("\"v7\"".into()),
        checked_at: 1000,
        retry_at: 0,
        refresh_id: "new".into(),
    };
    assert!(!transport::refresh_needed(&cache, Some("new"), 1001, false).unwrap());
    assert!(transport::refresh_needed(&cache, Some("new"), 22600, false).unwrap());
    assert!(transport::refresh_needed(&cache, Some("new"), 1001, true).unwrap());
    assert!(
        !transport::refresh_needed(&cache, Some("old"), 1001, true).unwrap(),
        "an update completed while waiting satisfies forced checks too"
    );
    cache.retry_at = 1900;
    assert!(transport::refresh_needed(&cache, Some("new"), 1001, true).is_err());
    assert!(transport::refresh_needed(&cache, Some("new"), 1900, true).unwrap());
}

#[test]
fn cache_commit_allows_backup_readers_but_rejects_late_login_or_space_responses() {
    use base64::Engine;
    let dir = tempfile::tempdir().unwrap();
    let config = fixture();
    let token = format!(
        "test.{}.sig",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"user_id":7}"#)
    );
    let mut value = settings::AppSettings::default();
    value.devify.base_url = config.scope.server.clone();
    value.devify.active_space_id = Some(config.scope.space_id.clone());
    settings::save(dir.path(), &value).unwrap();
    let credentials = desktop_credentials::Credentials {
        base_url: config.scope.server.clone(),
        access: token.clone(),
        refresh: "test".into(),
        device_session: None,
        session_device_id: None,
    };
    desktop_credentials::save(dir.path(), &credentials).unwrap();
    entitlements::write_snapshot(
        dir.path(),
        &entitlements::Snapshot {
            user_id: Some(7),
            base_url: config.scope.server.clone(),
            features: config.scope.capabilities.features.clone(),
            enforced: true,
            ..Default::default()
        },
    );
    let (_, generation) =
        desktop_credentials::load_with_generation(dir.path(), &config.scope.server).unwrap();
    let cache = Cache {
        scope: config.scope.clone(),
        config: Some(config.clone()),
        etag: Some("\"v7\"".into()),
        checked_at: 1,
        retry_at: 0,
        refresh_id: "first".into(),
    };
    {
        let _backup = crate::UPLOAD_IDENTITY_LOCK.try_read().unwrap();
        transport::commit(dir.path(), &cache, &token, generation).unwrap();
    }
    desktop_credentials::save(dir.path(), &credentials).unwrap();
    let mut late = cache.clone();
    late.etag = Some("\"late\"".into());
    assert!(transport::commit(dir.path(), &late, &token, generation).is_err());
    let (_, generation) =
        desktop_credentials::load_with_generation(dir.path(), &config.scope.server).unwrap();
    settings::update(dir.path(), |s| {
        s.devify.active_space_id = Some("other-space".into());
        Ok(())
    })
    .unwrap();
    assert!(transport::commit(dir.path(), &late, &token, generation).is_err());
    assert_eq!(
        load(dir.path(), &config.scope).unwrap().etag.as_deref(),
        Some("\"v7\"")
    );
}

#[test]
fn conditional_http_preserves_etag_and_empty_304_is_a_success_not_a_json_error() {
    use crate::devify_http::RequestTrace;
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut data = Vec::new();
        let mut bytes = [0; 1024];
        while !data.windows(4).any(|b| b == b"\r\n\r\n") {
            let n = stream.read(&mut bytes).unwrap();
            assert!(n > 0);
            data.extend_from_slice(&bytes[..n]);
        }
        let request = String::from_utf8(data).unwrap().to_lowercase();
        assert!(request.contains("schema_version=1") && request.contains("if-none-match: \"v7\""));
        write!(
            stream,
            "HTTP/1.1 304 Not Modified\r\nETag: \"v7\"\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!(
                "http://{address}/chat-summary-config?schema_version=1"
            ))
            .header(reqwest::header::IF_NONE_MATCH, "\"v7\"")
            .send_traced(dir.path(), crate::diagnostics::Module::App)
            .await
            .unwrap();
        let status = response.status();
        let etag = response.headers()[reqwest::header::ETAG]
            .to_str()
            .unwrap()
            .to_string();
        let body = response.bytes().await.unwrap();
        assert!(body.is_empty());
        assert_eq!(etag, "\"v7\"");
        assert!(
            transport::decode_catalog(status, Some(etag), &body, &fixture().scope)
                .unwrap()
                .is_none()
        );
    });
    server.join().unwrap();
    let lines = crate::diagnostics::snapshot(dir.path(), 0)["lines"]
        .as_array()
        .unwrap()
        .clone();
    assert!(lines
        .iter()
        .any(|line| line.as_str().unwrap().contains("status=304")));
    assert!(!lines
        .iter()
        .any(|line| line.as_str().unwrap().contains("HTTP_ERROR")));
    assert!(transport::decode_catalog(
        reqwest::StatusCode::NOT_FOUND,
        None,
        b"<html>",
        &fixture().scope
    )
    .unwrap_err()
    .contains("升级服务端"));
}
