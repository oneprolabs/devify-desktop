//! CLI adapter for fixed cloud read endpoints; no server credentials or URL forwarding.
use super::*;

pub(super) fn is_command(command: &str) -> bool {
    matches!(
        command,
        "cloud-context"
            | "todos"
            | "todo"
            | "reports"
            | "report"
            | "topics"
            | "topic"
            | "topic-reports"
    )
}
fn path(cli: &Cli) -> Result<String, String> {
    let command = if cli.command == "cloud-context" {
        "context"
    } else {
        cli.command.as_str()
    };
    let list = matches!(command, "todos" | "reports" | "topics");
    let detail = matches!(command, "todo" | "report" | "topic" | "topic-reports");
    if !is_command(&cli.command) {
        return Err("未知云端命令。".into());
    }
    let allowed: &[&str] = match command {
        "context" => &[],
        "todos" => &[
            "space",
            "page",
            "page-size",
            "category",
            "status",
            "keyword",
        ],
        "reports" => &["space", "page", "page-size", "status"],
        "topics" => &["space", "page", "page-size"],
        _ => &["space", "id"],
    };
    if cli
        .args
        .values
        .keys()
        .any(|key| !allowed.contains(&key.as_str()))
        || cli.args.flags.iter().any(|key| key != "json")
    {
        return Err(
            "该云端命令不支持此参数；云端空间不是聊天来源 --account。请运行 aimychats-cli help。".into(),
        );
    }
    let mut pairs = Vec::new();
    if list || detail {
        pairs.push(format!("space_id={}", encode(cli.args.required("space")?)));
    }
    if detail {
        let id = cli
            .args
            .signed("id")?
            .filter(|id| *id > 0)
            .ok_or("--id 必须是正整数。")?;
        pairs.push(format!("id={id}"));
    }
    for (flag, key, max) in [
        ("page", "page", u32::MAX as usize),
        ("page-size", "page_size", 100),
    ] {
        if let Some(n) = cli.args.number(flag)? {
            if n == 0 || n > max {
                return Err(format!("--{flag} 范围为 1–{max}。"));
            }
            pairs.push(format!("{key}={n}"));
        }
    }
    for (flag, key) in [
        ("category", "viewer_relevance"),
        ("status", "status"),
        ("keyword", "q"),
    ] {
        if let Some(value) = cli.args.last(flag) {
            pairs.push(format!("{key}={}", encode(value)));
        }
    }
    Ok(format!(
        "/api/cloud/{command}{}{}",
        if pairs.is_empty() { "" } else { "?" },
        pairs.join("&")
    ))
}
pub(super) fn dispatch(cli: &Cli) -> Result<String, String> {
    let value = request_json(cli, "GET", &path(cli)?, None)?;
    let mut text = pretty(&value)?;
    if !cli.json {
        if let Some(page) = value["next_page"].as_u64() {
            text.push_str(&format!(
                "\n下一页：保持当前空间和筛选参数，传入 --page {page}。"
            ));
        }
        if value["history_complete"] == false {
            text.push_str("\n注意：仅最近最多 50 份主题报告，不是完整历史。请逐页浏览 reports，核对 source_info.subscription_id。");
        }
    }
    Ok(text)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn cli(command: &str, flags: &[&str]) -> Cli {
        Cli {
            endpoint: "http://127.0.0.1:18766".into(),
            token: None,
            json: true,
            command: command.into(),
            args: parse_flags(&flags.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap(),
        }
    }
    #[test]
    fn a_missing_cloud_record_is_not_misreported_as_an_old_client() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 4096];
            stream.read(&mut request).unwrap();
            let body = r#"{"error":"Resource not found or forbidden","status":404}"#;
            write!(
                stream,
                "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let mut query = cli("report", &["--space", "space-91", "--id", "9"]);
        query.endpoint = endpoint;
        let error = super::super::dispatch(query).unwrap_err();
        server.join().unwrap();
        assert!(error.contains("Resource not found or forbidden"));
        assert!(error.contains("HTTP 404"));
        assert!(!error.contains("改造前"));
    }

    #[test]
    fn cloud_commands_dispatch_the_documented_bounded_get() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            let (mut stream, _) = loop {
                if let Ok(socket) = listener.accept() {
                    break socket;
                }
                if std::time::Instant::now() > deadline {
                    return String::new();
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 4096];
            let n = stream.read(&mut request).unwrap();
            let body = r#"{"source":"remote","space_id":"space-91","list":[],"next_page":3}"#;
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            String::from_utf8_lossy(&request[..n]).into_owned()
        });
        let raw: Vec<_> = [
            "--space",
            "space-91",
            "--page",
            "2",
            "--page-size=2",
            "--category",
            "my_todo",
            "--status",
            "unfinished",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let parsed = parse_flags(&raw);
        assert!(parsed.is_ok(), "cloud query flags must parse");
        let result = super::super::dispatch(Cli {
            endpoint,
            token: Some("local-test-token".into()),
            json: true,
            command: "todos".into(),
            args: parsed.unwrap(),
        })
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&result).unwrap()["next_page"],
            3
        );
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /api/cloud/todos?space_id=space-91&page=2&page_size=2&viewer_relevance=my_todo&status=unfinished "),"{request}");
        assert!(request
            .to_lowercase()
            .contains("authorization: bearer local-test-token"));
    }

    #[test]
    fn detail_context_and_list_paths_are_fixed() {
        assert_eq!(
            path(&cli("cloud-context", &[])).unwrap(),
            "/api/cloud/context"
        );
        for (command, resource) in [
            ("todo", "todo"),
            ("report", "report"),
            ("topic", "topic"),
            ("topic-reports", "topic-reports"),
        ] {
            assert_eq!(
                path(&cli(command, &["--space", "space-91", "--id=7"])).unwrap(),
                format!("/api/cloud/{resource}?space_id=space-91&id=7")
            );
        }
        assert_eq!(
            path(&cli(
                "reports",
                &["--space", "space-91", "--page", "3", "--status", "failed"]
            ))
            .unwrap(),
            "/api/cloud/reports?space_id=space-91&page=3&status=failed"
        );
    }
    #[test]
    fn invalid_or_ignored_flags_never_reach_the_reader() {
        for flags in [
            vec![],
            vec!["--space", "s", "--page", "0"],
            vec!["--space", "s", "--page-size", "101"],
            vec!["--space", "s", "--account", "wxid"],
        ] {
            assert!(path(&cli("todos", &flags)).is_err());
        }
        assert!(path(&cli("todo", &["--space", "s", "--id", "-1"])).is_err());
        assert!(path(&cli(
            "topic-reports",
            &["--space", "s", "--id", "7", "--page", "2"]
        ))
        .is_err());
        assert!(path(&cli("reports", &["--space", "s", "--keyword", "ignored"])).is_err());
    }
}
