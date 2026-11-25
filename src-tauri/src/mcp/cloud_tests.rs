use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::io::{Read, Write};
use std::net::TcpListener;

fn args(value: Value) -> Args {
    serde_json::from_value(value).unwrap()
}
fn token() -> String {
    format!(
        "e30.{}.sig",
        URL_SAFE_NO_PAD.encode(json!({"user_id": 91, "exp": 4102444800_i64}).to_string())
    )
}
fn fixture(base: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    crate::settings::update(dir.path(), |s| {
        s.devify.base_url = base.into();
        s.devify.active_space_id = Some("space-91".into());
        Ok(())
    })
    .unwrap();
    crate::desktop_credentials::save(
        dir.path(),
        &crate::desktop_credentials::Credentials {
            base_url: base.into(),
            access: token(),
            refresh: String::new(),
            device_session: None,
            session_device_id: None,
        },
    )
    .unwrap();
    crate::entitlements::write_snapshot(
        dir.path(),
        &crate::entitlements::Snapshot {
            user_id: Some(91),
            base_url: base.into(),
            enforced: true,
            features: vec![
                "bridge.mcp".into(),
                "cloud.events".into(),
                "cloud.reports".into(),
                "cloud.topics".into(),
            ],
            source: "server".into(),
            fetched_at: chrono::Utc::now().timestamp(),
            ..Default::default()
        },
    );
    dir
}
fn run<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}
fn mock_server(
    status: u16,
    body: Value,
    change: impl FnOnce() + Send + 'static,
) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        while !bytes.windows(4).any(|s| s == b"\r\n\r\n") {
            let n = socket.read(&mut buffer).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buffer[..n]);
        }
        change();
        let text = json!({"data":body}).to_string();
        write!(socket,"HTTP/1.1 {status} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",text.len()).unwrap();
        String::from_utf8(bytes).unwrap()
    });
    (base, handle)
}
#[test]
fn queries_are_bounded_encoded_and_resource_specific() {
    let q = args(
        json!({"space_id":"space-91","page":2,"page_size":2,"viewer_relevance":"my_todo","status":"unfinished","q":"报价 &合同"}),
    );
    assert_eq!(q.path("todos","space-91").unwrap(),"/api/v1/spaces/space-91/events?page=2&page_size=2&viewer_relevance=my_todo&status=unfinished&q=%E6%8A%A5%E4%BB%B7+%26%E5%90%88%E5%90%8C&include_relevance_counts=true");
    for input in [
        json!({"page":0}),
        json!({"page_size":101}),
        json!({"id":0}),
        json!({"viewer_relevance":"mine"}),
        json!({"q":"x".repeat(201)}),
    ] {
        assert!(args(input).path("todos", "space-91").is_err());
    }
    assert!(args(json!({"q":"ignored"}))
        .path("reports", "space-91")
        .is_err());
    assert!(args(json!({"id":1,"page":2}))
        .path("topic-reports", "space-91")
        .is_err());
    assert!(serde_json::from_value::<Args>(json!({"url":"https://outside.invalid"})).is_err());
    assert!(args(json!({})).path("generate", "space-91").is_err());
}
#[test]
fn cloud_context_and_invalid_requests_do_not_contact_the_server() {
    let dir = fixture("https://example.invalid");
    let context = run(read(dir.path(), "context", Args::default())).unwrap();
    assert_eq!(context["space_id"], "space-91");
    assert!(!context.to_string().contains(&token()));
    assert!(run(read(dir.path(), "todos", args(json!({"space_id":"other"})))).is_err());
    let mut snapshot = crate::entitlements::load(dir.path()).unwrap();
    snapshot.features.clear();
    crate::entitlements::write_snapshot(dir.path(), &snapshot);
    assert_eq!(
        run(read(dir.path(), "context", Args::default()))
            .unwrap_err()
            .status,
        403
    );
    crate::desktop_credentials::clear(dir.path()).unwrap();
    assert!(run(read(dir.path(), "context", Args::default())).is_err());
}
#[test]
fn list_preserves_server_pages_filters_and_evidence() {
    let body = json!({"list":[{"id":7,"space_id":"space-91","viewer_relevance":"my_todo","status":"in_progress","evidence":[{"message_id":91}]}],"pagination":{"page":2,"next":"https://server.invalid/page3","total":7},"relevance_counts":{"my_todo":7,"related":0,"other":0},"completion_filter_supported":true,"search_supported":true});
    let (base, server) = mock_server(200, body, || {});
    let dir = fixture(&base);
    let value=run(read(dir.path(),"todos",args(json!({"space_id":"space-91","page":2,"page_size":2,"status":"unfinished","viewer_relevance":"my_todo"})))).unwrap();
    assert_eq!(value["list"][0]["status"], "in_progress");
    assert_eq!(value["list"][0]["evidence"][0]["message_id"], 91);
    assert_eq!(value["next_page"], 3);
    assert_eq!(value["pagination"]["total"], 7);
    let request = server.join().unwrap();
    assert!(request.starts_with("GET /api/v1/spaces/space-91/events?page=2&page_size=2"));
    assert!(request.contains("viewer_relevance=my_todo&status=unfinished"));
}
#[test]
fn details_preserve_report_body_period_status_and_topic_membership() {
    let body = json!({"id":9,"space_id":"space-91","body":"Existing report body","period_start":"2026-11-01","period_end":"2026-11-07","status":"ready","source_info":{"kind":"topic_subscription","subscription_id":3},"sections":[{"title":"Progress"}]});
    let (base, server) = mock_server(200, body.clone(), || {});
    let dir = fixture(&base);
    let value = run(read(
        dir.path(),
        "report",
        args(json!({"space_id":"space-91","id":9})),
    ))
    .unwrap();
    assert_eq!(value["item"], body);
    assert_eq!(value["source"], "remote");
    assert!(server.join().unwrap().starts_with("GET /api/v1/reports/9 "));
}
#[test]
fn errors_and_cross_space_details_are_not_empty_successes() {
    for status in [401, 403, 404] {
        let (base, server) = mock_server(status, json!({"detail":"denied"}), || {});
        let dir = fixture(&base);
        assert_eq!(
            run(read(
                dir.path(),
                "todo",
                args(json!({"space_id":"space-91","id":7}))
            ))
            .unwrap_err()
            .status,
            status
        );
        server.join().unwrap();
    }
    for body in [json!({"id":7,"space_id":"other"}), json!({"id":7})] {
        let (base, server) = mock_server(200, body, || {});
        let dir = fixture(&base);
        assert!(run(read(
            dir.path(),
            "todo",
            args(json!({"space_id":"space-91","id":7}))
        ))
        .is_err());
        server.join().unwrap();
    }
}
#[test]
fn old_responses_are_discarded_after_identity_changes() {
    for change in [
        "logout",
        "space",
        "service",
        "account",
        "login",
        "space-aba",
    ] {
        let (tx, rx) = std::sync::mpsc::channel::<std::path::PathBuf>();
        let (base, server) = mock_server(200, json!({"id":7,"space_id":"space-91"}), move || {
            let data = rx.recv().unwrap();
            if change == "logout" {
                crate::desktop_credentials::clear(&data).unwrap();
            } else if change == "login" {
                let settings = crate::settings::load(&data).unwrap();
                let credentials =
                    crate::desktop_credentials::load(&data, &settings.devify.base_url).unwrap();
                // Even a re-login issuing the same token must invalidate the old response.
                crate::desktop_credentials::save(&data, &credentials).unwrap();
            } else if change == "space-aba" {
                crate::settings::update(&data, |s| {
                    s.devify.active_space_id = Some("other".into());
                    Ok(())
                })
                .unwrap();
                crate::settings::update(&data, |s| {
                    s.devify.active_space_id = Some("space-91".into());
                    Ok(())
                })
                .unwrap();
            } else {
                crate::settings::update(&data, |s| {
                    match change {
                        "space" => s.devify.active_space_id = Some("other".into()),
                        "service" => s.devify.base_url = "https://other.invalid".into(),
                        _ => s.active_account_id = "another-source".into(),
                    };
                    Ok(())
                })
                .unwrap();
            }
        });
        let dir = fixture(&base);
        tx.send(dir.path().into()).unwrap();
        assert!(
            run(read(
                dir.path(),
                "todo",
                args(json!({"space_id":"space-91","id":7}))
            ))
            .is_err(),
            "{change}"
        );
        server.join().unwrap();
    }
}
#[test]
fn topic_reports_are_explicitly_bounded_and_association_is_checked() {
    let rows:Vec<_>=(1..=50).map(|id|json!({"id":id,"space_id":"space-91","source_info":{"subscription_id":3},"status":"queued"})).collect();
    let (base, server) = mock_server(200, json!(rows), || {});
    let dir = fixture(&base);
    let value = run(read(
        dir.path(),
        "topic-reports",
        args(json!({"space_id":"space-91","id":3})),
    ))
    .unwrap();
    assert_eq!(value["history_complete"], false);
    assert_eq!(value["server_limit"], 50);
    assert_eq!(value["topic_id"], 3);
    assert!(server
        .join()
        .unwrap()
        .starts_with("GET /api/v1/topics/3/reports "));
    let (base, server) = mock_server(
        200,
        json!([{"id":1,"space_id":"space-91","source_info":{"subscription_id":4}}]),
        || {},
    );
    let dir = fixture(&base);
    assert!(run(read(
        dir.path(),
        "topic-reports",
        args(json!({"space_id":"space-91","id":3}))
    ))
    .is_err());
    server.join().unwrap();
}

#[test]
fn http_and_mcp_share_the_cloud_handler_and_keep_local_authentication() {
    let dir = fixture("https://example.invalid");
    let state = super::super::AppState {
        app_data: dir.path().into(),
        token: Some("local-test-token".into()),
        base_url: "http://127.0.0.1:0".into(),
        next_session: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)),
        sessions: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };
    let response = run(super::super::api_cloud(
        axum::extract::State(state.clone()),
        axum::http::HeaderMap::new(),
        axum::extract::Path("context".into()),
        axum::extract::Query(Args::default()),
    ));
    assert_eq!(response.status(), 401);
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", "Bearer local-test-token".parse().unwrap());
    let response = run(super::super::api_cloud(
        axum::extract::State(state),
        headers,
        axum::extract::Path("context".into()),
        axum::extract::Query(Args::default()),
    ));
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let bytes = run(axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)).unwrap();
    let http: Value = serde_json::from_slice(&bytes).unwrap();
    let rpc=run(super::super::handle_rpc(dir.path(),json!({"jsonrpc":"2.0","id":91,"method":"tools/call","params":{"name":"aimychats_get_cloud_context","arguments":{}}}))).unwrap();
    let mcp: Value =
        serde_json::from_str(rpc["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(http, mcp);
    let denied = run(super::super::call_tool(
        dir.path(),
        "aimychats_list_todos",
        &json!({"arguments":{"space_id":"other"}}),
    ));
    assert_eq!(denied["isError"], true);
}

#[test]
fn empty_and_queued_failed_reports_remain_distinct_and_topics_page_locally() {
    let (base, server) = mock_server(
        200,
        json!({"list":[],"pagination":{"page":1,"next":null,"total":0}}),
        || {},
    );
    let dir = fixture(&base);
    let empty = run(read(
        dir.path(),
        "reports",
        args(json!({"space_id":"space-91"})),
    ))
    .unwrap();
    assert_eq!(empty["list"], json!([]));
    assert_eq!(empty["next_page"], Value::Null);
    server.join().unwrap();
    for status in ["queued", "failed"] {
        let (base, server) = mock_server(
            200,
            json!({"id":7,"space_id":"space-91","status":status,"body":""}),
            || {},
        );
        let dir = fixture(&base);
        let result = run(read(
            dir.path(),
            "report",
            args(json!({"space_id":"space-91","id":7})),
        ))
        .unwrap();
        assert_eq!(result["item"]["status"], status);
        assert_eq!(result["item"]["body"], "");
        server.join().unwrap();
    }
    let items:Vec<_>=(1..=3).map(|id|json!({"id":id,"space_id":"space-91","instruction":"Configuration, not report text"})).collect();
    let (base, server) = mock_server(200, json!(items), || {});
    let dir = fixture(&base);
    let page = run(read(
        dir.path(),
        "topics",
        args(json!({"space_id":"space-91","page":2,"page_size":1})),
    ))
    .unwrap();
    assert_eq!(page["list"][0]["id"], 2);
    assert_eq!(page["next_page"], 3);
    assert_eq!(page["pagination"]["total"], 3);
    assert!(server
        .join()
        .unwrap()
        .starts_with("GET /api/v1/spaces/space-91/topics "));
}

#[test]
fn unsupported_filters_and_overlarge_responses_fail_explicitly() {
    let (base, server) = mock_server(
        200,
        json!({"list":[],"pagination":{"page":1,"next":null}}),
        || {},
    );
    let dir = fixture(&base);
    assert!(run(read(
        dir.path(),
        "todos",
        args(json!({"space_id":"space-91","status":"done"}))
    ))
    .is_err());
    server.join().unwrap();
    let (base, server) = mock_server(
        200,
        json!({"id":7,"space_id":"space-91","body":"x".repeat(2*1024*1024)}),
        || {},
    );
    let dir = fixture(&base);
    assert!(run(read(
        dir.path(),
        "report",
        args(json!({"space_id":"space-91","id":7}))
    ))
    .unwrap_err()
    .message
    .contains("2 MiB"));
    server.join().unwrap();
}

#[test]
fn live_http_and_sse_discovery_use_the_new_tools_without_writes() {
    run(async {
        let dir = fixture("https://example.invalid");
        let reserved = TcpListener::bind("127.0.0.1:0").unwrap();
        let bind = reserved.local_addr().unwrap().to_string();
        drop(reserved);
        let base = format!("http://{bind}");
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let data = dir.path().to_path_buf();
        let service = tokio::spawn(super::super::start(
            data,
            bind,
            Some("local-test-token".into()),
            Some(ready_tx),
            Some(stop_rx),
        ));
        ready_rx.await.unwrap().unwrap();
        let client = reqwest::Client::new();
        assert_eq!(
            client
                .get(format!("{base}/api/cloud/context"))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(
            client
                .post(format!("{base}/api/cloud/reports"))
                .bearer_auth("local-test-token")
                .send()
                .await
                .unwrap()
                .status(),
            405
        );
        let context: Value = client
            .get(format!("{base}/api/cloud/context"))
            .bearer_auth("local-test-token")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(context["space_id"], "space-91");
        let mut sse = client
            .get(format!("{base}/sse"))
            .bearer_auth("local-test-token")
            .send()
            .await
            .unwrap();
        async fn event(response: &mut reqwest::Response) -> String {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                let mut text = String::new();
                while !text.contains("\n\n") {
                    text.push_str(
                        std::str::from_utf8(&response.chunk().await.unwrap().unwrap()).unwrap(),
                    );
                }
                text
            })
            .await
            .unwrap()
        }
        let initial = event(&mut sse).await;
        let endpoint = initial
            .lines()
            .find_map(|s| s.strip_prefix("data: "))
            .unwrap();
        assert!(endpoint.starts_with("/messages?sessionId="));
        let posted = client
            .post(format!("{base}{endpoint}"))
            .bearer_auth("local-test-token")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .send()
            .await
            .unwrap();
        assert_eq!(posted.status(), 202);
        let message = event(&mut sse).await;
        let rpc: Value = serde_json::from_str(
            message
                .lines()
                .find_map(|s| s.strip_prefix("data: "))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(rpc["result"]["tools"].as_array().unwrap().len(), 14);
        assert!(rpc["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == "aimychats_get_report"));
        drop(sse);
        stop_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), service)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    });
}

#[test]
fn a_legacy_empty_page_cannot_claim_support_for_personal_categories() {
    let query = args(json!({"space_id":"space-91","viewer_relevance":"my_todo"}));
    assert!(output(
        "todos",
        &query,
        "space-91",
        json!({"list":[],"pagination":{"page":1,"next":null}})
    )
    .is_err());
}
