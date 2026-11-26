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
        "period_start":"2023-11-14T22:13:20Z","period_end":"2023-11-14T22:13:20Z",
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
        last_error: None,
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
    let (frozen, plan) = prompt::freeze(&config, &mut body, &settings, "local", true).unwrap();
    assert_eq!(body["language"], "按原文语言");
    assert_eq!(body["instructions"], "Focus on deadlines");
    config.scenarios[4].template = "Changed later".into();
    config.version = "v8".into();
    let generated =
        prompt::local_prompt(&frozen, &body, &prompt::local_input(&body).unwrap()).unwrap();
    assert!(plan.is_none());
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
fn summary_period_uses_local_calendar_dates_and_checks_sparse_requests() {
    use chrono::{Local, TimeDelta, TimeZone};
    let config = fixture();
    let settings = settings::LanguageSettings::default();
    let at = |(y, m, d)| Local.with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap();
    for (from, to, first_invalid) in [
        ((2024, 1, 1), (2024, 1, 31), (2024, 2, 1)),
        ((2024, 1, 31), (2024, 2, 29), (2024, 3, 1)),
        ((2025, 1, 31), (2025, 2, 28), (2025, 3, 1)),
        ((2024, 3, 1), (2024, 3, 31), (2024, 4, 1)),
        ((2025, 12, 15), (2026, 1, 14), (2026, 1, 15)),
    ] {
        let start = at(from);
        let end = at(to);
        let mut body = input(&config, "summary");
        body["period_start"] = start.to_rfc3339().into();
        body["period_end"] = end.to_rfc3339().into();
        body["messages"][0]["time"] = end.timestamp().into();
        assert!(prompt::freeze(&config, &mut body, &settings, "local", true).is_ok());
        assert!(prompt::freeze(&config, &mut body, &settings, "cloud", true)
            .unwrap_err()
            .contains("一周"));
        body["period_end"] = at(first_invalid).to_rfc3339().into();
        assert!(prompt::freeze(&config, &mut body, &settings, "local", true)
            .unwrap_err()
            .contains("一个自然月"));
    }
    let mut body = input(&config, "summary");
    let start = at((2024, 3, 9));
    let end = start + TimeDelta::days(7);
    body["period_start"] = start.to_rfc3339().into();
    body["period_end"] = end.to_rfc3339().into();
    body["messages"][0]["time"] = start.timestamp().into();
    assert!(prompt::freeze(&config, &mut body, &settings, "cloud", true).is_ok());
    body["messages"][0]["time"] = (end.timestamp() + 1).into();
    assert!(prompt::freeze(&config, &mut body, &settings, "local", true)
        .unwrap_err()
        .contains("不在本次时间范围内"));
    for invalid_end in [
        "not a date".to_string(),
        (start - TimeDelta::seconds(1)).to_rfc3339(),
    ] {
        body["period_end"] = invalid_end.into();
        assert!(prompt::freeze(&config, &mut body, &settings, "local", true)
            .unwrap_err()
            .contains("时间范围无效"));
    }
}

#[test]
fn all_messages_admits_long_local_history_only_and_still_checks_input() {
    let config = fixture();
    let settings = settings::LanguageSettings::default();
    let mut body = input(&config, "summary");
    body["mode"] = "time".into();
    body["all_messages"] = true.into();
    body["period_end"] = "2024-01-14T22:13:20Z".into();
    body["messages"].as_array_mut().unwrap().push(json!({
        "username":"group", "time":1705270400, "text":"Follow-up"
    }));
    assert!(prompt::freeze(&config, &mut body, &settings, "local", true).is_ok());
    assert!(prompt::freeze(&config, &mut body, &settings, "cloud", true)
        .unwrap_err()
        .contains("仅支持本地"));
    body["mode"] = "pick".into();
    assert!(prompt::freeze(&config, &mut body, &settings, "local", true).is_err());
    body["mode"] = "time".into();
    body["messages"][1]["time"] = 1705270401_i64.into();
    assert!(prompt::freeze(&config, &mut body, &settings, "local", true)
        .unwrap_err()
        .contains("不在本次时间范围内"));
    body["messages"][1]["time"] = 1705270400_i64.into();
    body["messages"][1]["text"] = "中".repeat(5000).into();
    assert!(prompt::freeze(&config, &mut body, &settings, "local", true)
        .unwrap_err()
        .contains("超过本地单次总结范围"));
}

#[test]
fn all_messages_uses_the_prompt_budget_for_more_than_400_messages() {
    let config = fixture();
    let settings = settings::LanguageSettings::default();
    let mut body = input(&config, "summary");
    body["mode"] = "time".into();
    let time = body["messages"][0]["time"].as_i64().unwrap();
    body["messages"] = json!((0..401)
        .map(|i| json!({
            "username": "group", "display": "Team", "sender": "Member",
            "is_group": true, "time": time, "text": if i == 400 { "last" } else { "ok" }
        }))
        .collect::<Vec<_>>());
    assert!(prompt::freeze(&config, &mut body, &settings, "local", true)
        .unwrap_err()
        .contains("已超限"));

    body["all_messages"] = true.into();
    let (frozen, plan) = prompt::freeze(&config, &mut body, &settings, "local", true).unwrap();
    let prompt =
        prompt::local_prompt(&frozen, &body, &prompt::local_input(&body).unwrap()).unwrap();
    assert_eq!(plan.unwrap().bytes, prompt.len());
    let material: serde_json::Value = serde_json::from_str(prompt.lines().last().unwrap()).unwrap();
    let rows = material["messages_by_date"][0]["messages"]
        .as_array()
        .unwrap();
    assert_eq!(rows.len(), 401);
    assert_eq!(rows[400][3], "last");
    assert_eq!(material["speakers"], json!(["Member"]));

    body["messages"][0]["text"] = "a".repeat(12000).into();
    assert!(prompt::freeze(&config, &mut body, &settings, "local", true)
        .unwrap_err()
        .contains("超过本地单次总结范围"));
}

#[test]
fn all_messages_accepts_multiple_budgeted_batches() {
    let config = fixture();
    let mut body = input(&config, "summary");
    body["mode"] = "time".into();
    body["all_messages"] = true.into();
    let time = body["messages"][0]["time"].as_i64().unwrap();
    body["messages"] = json!((0..60)
        .map(|i| json!({
            "username": "group", "display": "Team", "sender": "Member",
            "time": time, "text": format!("{i}:{}", "中".repeat(300))
        }))
        .collect::<Vec<_>>());
    let (snapshot, plan) = prompt::freeze(
        &config,
        &mut body,
        &settings::LanguageSettings::default(),
        "local",
        true,
    )
    .unwrap();
    let plan = plan.unwrap();
    assert!(plan.ends.len() > 4);
    let mut progress = super::batches::Progress::new(plan);
    let budget = snapshot.scenario.budgets.local_cli.max_request_bytes;
    let mut expected_index = 0;
    let mut merged_intermediate = false;
    let mut restored = false;
    loop {
        let step = progress.next(&snapshot, &body).unwrap();
        assert!(step.prompt().len() <= budget);
        let output = match &step {
            super::batches::Step::Batch(prompt) => {
                let material: serde_json::Value =
                    serde_json::from_str(prompt.lines().last().unwrap()).unwrap();
                for row in material["messages_by_date"][0]["messages"]
                    .as_array()
                    .unwrap()
                {
                    assert_eq!(
                        row[3].as_str().unwrap().split(':').next().unwrap(),
                        expected_index.to_string()
                    );
                    expected_index += 1;
                }
                "中".repeat(750)
            }
            super::batches::Step::Merge { .. } => {
                merged_intermediate = true;
                "已保留事件、决定和待办。".to_string()
            }
            super::batches::Step::Final(_) => "最终总结".to_string(),
        };
        if let Some(final_summary) = progress.accept(&snapshot, &body, step, output).unwrap() {
            assert_eq!(final_summary, "最终总结");
            break;
        }
        if !restored {
            let persisted = serde_json::to_string(&progress).unwrap();
            progress = serde_json::from_str(&persisted).unwrap();
            assert_eq!(progress.counts().0, 1);
            let next = progress.next(&snapshot, &body).unwrap();
            assert!(progress
                .accept(&snapshot, &body, next, "x".repeat(budget))
                .is_err());
            assert_eq!(
                progress.counts().0,
                1,
                "A failed batch must stay available for retry"
            );
            restored = true;
        }
    }
    assert!(merged_intermediate);
    assert_eq!(expected_index, 60);
}

#[test]
fn each_batch_also_respects_the_character_limit() {
    let mut config = fixture();
    for scenario in &mut config.scenarios {
        scenario.budgets.local_cli.max_request_bytes = 256 * 1024;
    }
    let mut body = input(&config, "summary");
    body["mode"] = "time".into();
    body["all_messages"] = true.into();
    let time = body["messages"][0]["time"].as_i64().unwrap();
    body["messages"] = json!([
        {"username": "group", "time": time, "text": "a".repeat(20_000)},
        {"username": "group", "time": time, "text": "b".repeat(20_000)},
        {"username": "group", "time": time, "text": "c"},
    ]);
    let (_, plan) = prompt::freeze(
        &config,
        &mut body,
        &settings::LanguageSettings::default(),
        "local",
        true,
    )
    .unwrap();
    assert_eq!(plan.unwrap().ends, [2, 3]);
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
fn catalog_http_failures_are_classified_before_decoding_the_body() {
    use reqwest::StatusCode;
    for (status, body, expected) in [
        (
            StatusCode::FORBIDDEN,
            &b"<html>"[..],
            "没有权限读取当前空间的总结配置。",
        ),
        (
            StatusCode::NOT_FOUND,
            &br#"{"detail":"Space not found"}"#[..],
            "当前空间不存在或无权访问，请重新选择空间。",
        ),
        (
            StatusCode::NOT_FOUND,
            &br#"{"code":404,"message":"Not found","data":{"detail":"No Space matches the given query."}}"#[..],
            "当前空间不存在或无权访问，请重新选择空间。",
        ),
        (
            StatusCode::NOT_FOUND,
            &b"<html>Not found</html>"[..],
            "当前服务不支持聊天总结配置，请联系管理员升级服务端。",
        ),
        (
            StatusCode::BAD_REQUEST,
            &br#"{"detail":"Unsupported summary configuration schema."}"#[..],
            "总结配置版本暂不支持，请检查客户端和服务端版本。",
        ),
        (
            StatusCode::BAD_GATEWAY,
            &b"<html>"[..],
            "总结服务暂不可用，请稍后重试。",
        ),
        (StatusCode::OK, &b"<html>"[..], "总结配置响应格式无效。"),
    ] {
        assert_eq!(
            transport::decode_catalog(status, None, body, &fixture().scope).unwrap_err(),
            expected
        );
    }
}

#[test]
fn authentication_failure_only_backs_off_the_rejected_login() {
    let message = "服务端需要重新验证登录，请稍后重试。";
    let failure = transport::AuthRetry::new(3, 1000, message.into());
    let error = failure.for_generation(3, 1001).unwrap();
    assert_eq!(error.message, message);
    assert_eq!(error.retry_at, Some(1015));
    assert!(
        failure.for_generation(4, 1001).is_none(),
        "A fresh login can retry immediately"
    );
    assert!(
        failure.for_generation(3, 1015).is_none(),
        "A stable login can retry when its short cooldown ends"
    );
}

#[test]
fn refreshes_are_throttled_coalesced_and_backed_off_even_when_forced() {
    use transport::Refresh::{Background, Forced, Missing};
    let config = fixture();
    let mut cache = Cache {
        scope: config.scope.clone(),
        config: Some(config),
        etag: Some("\"v7\"".into()),
        checked_at: 1000,
        retry_at: 0,
        last_error: None,
        refresh_id: "new".into(),
    };
    assert!(!transport::refresh_needed(&cache, Some("new"), 1001, Background).unwrap());
    assert!(transport::refresh_needed(&cache, Some("new"), 22600, Background).unwrap());
    assert!(transport::refresh_needed(&cache, Some("new"), 1001, Forced).unwrap());
    assert!(
        !transport::refresh_needed(&cache, Some("old"), 1001, Forced).unwrap(),
        "an update completed while waiting satisfies forced checks too"
    );
    cache.retry_at = 1900;
    assert!(transport::refresh_needed(&cache, Some("new"), 1001, Forced).is_err());
    assert!(transport::refresh_needed(&cache, Some("new"), 1900, Forced).unwrap());
    assert!(!transport::refresh_needed(&cache, None, 1001, Missing).unwrap());
    assert!(!transport::refresh_needed(&cache, Some("new"), 22600, Missing).unwrap());
    cache.config = None;
    assert!(transport::refresh_needed(&cache, None, 1001, Missing).is_err());
    assert!(transport::refresh_needed(&cache, None, 1900, Missing).unwrap());
}

#[test]
fn missing_cache_cannot_use_304_and_recovers_after_a_successful_fetch() {
    let config = fixture();
    let mut cache = Cache {
        scope: config.scope.clone(),
        config: None,
        etag: None,
        checked_at: 0,
        retry_at: 0,
        refresh_id: String::new(),
        last_error: None,
    };
    let error = transport::apply_fetch(&mut cache, Ok(None), 1000).unwrap_err();
    assert!(cache.config.is_none());
    assert_eq!(error.retry_at, Some(1900));
    let waiting =
        transport::refresh_needed(&cache, None, 1200, transport::Refresh::Missing).unwrap_err();
    assert_eq!(waiting.message, error.message);
    assert_eq!(waiting.retry_at, error.retry_at);

    transport::apply_fetch(&mut cache, Ok(Some((config, Some("\"v7\"".into())))), 1900).unwrap();
    assert_eq!(cache.retry_at, 0);
    assert!(cache.last_error.is_none());
    transport::apply_fetch(&mut cache, Err("connection failed".into()), 2800).unwrap_err();
    assert_eq!(cache.config.as_ref().unwrap().version, fixture().version);
    assert_eq!(cache.etag.as_deref(), Some("\"v7\""));
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
        last_error: None,
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
