#![cfg(feature = "aux-binaries")]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn run_cli(endpoint: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aimychats-cli"))
        .args(["--endpoint", endpoint, "--token", "synthetic-test-token"])
        .args(args)
        .output()
        .expect("run aimychats-cli")
}

fn request(args: &[&str], body: &str) -> (String, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let body = body.to_string();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "aimychats-cli did not send a request"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).unwrap();
            assert!(!header.is_empty(), "incomplete request headers");
            if header == "\r\n" {
                break;
            }
        }
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        line
    });
    let output = run_cli(&endpoint, args);
    let line = server.join();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (line.unwrap(), String::from_utf8(output.stdout).unwrap())
}

#[test]
fn documented_session_commands_forward_offset_and_json() {
    let body = r#"{"account_id":"test-account","sessions":[],"next_offset":4}"#;
    for (args, offset) in [
        (
            vec![
                "sessions",
                "--account",
                "test-account",
                "--limit",
                "2",
                "--offset",
                "2",
                "--json",
            ],
            2,
        ),
        (
            vec![
                "--json",
                "sessions",
                "--account",
                "test-account",
                "--limit=2",
                "--offset=2",
            ],
            2,
        ),
        (
            vec![
                "sessions",
                "--account",
                "test-account",
                "--limit",
                "2",
                "--offset",
                "0",
                "--json",
            ],
            0,
        ),
        (
            vec![
                "sessions",
                "--account",
                "test-account",
                "--limit",
                "2",
                "--json",
            ],
            0,
        ),
    ] {
        let (line, output) = request(&args, body);
        assert_eq!(
            line,
            format!(
                "GET /api/sessions?limit=2&offset={offset}&account_id=test-account HTTP/1.1\r\n"
            )
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output).unwrap(),
            serde_json::from_str::<serde_json::Value>(body).unwrap()
        );
    }
}

#[test]
fn human_readable_continuation_is_executable() {
    let (line, output) = request(
        &[
            "sessions",
            "--account",
            "test-account",
            "--limit",
            "2",
            "--offset",
            "2",
        ],
        r#"{"account_id":"test-account","sessions":[],"next_offset":4}"#,
    );
    assert!(line.contains("limit=2&offset=2&"));
    assert!(output.contains("aimychats-cli sessions --account 'test-account' --limit 2 --offset 4"));
    let (line, output) = request(
        &[
            "sessions",
            "--account",
            "test-account",
            "--limit",
            "2",
            "--offset",
            "4",
        ],
        r#"{"account_id":"test-account","sessions":[],"next_offset":null}"#,
    );
    assert!(line.contains("limit=2&offset=4&"));
    assert!(!output.contains("继续："));
}

#[test]
fn rejects_missing_or_invalid_offsets_before_http() {
    for (flags, error) in [
        (vec!["--offset"], "--offset 需要一个值"),
        (vec!["--offset", "--limit", "2"], "--offset 需要一个值"),
        (vec!["--offset="], "--offset 需要正整数"),
        (vec!["--offset", "-1"], "--offset 需要正整数"),
        (vec!["--offset=-1"], "--offset 需要正整数"),
        (vec!["--offset", "abc"], "--offset 需要正整数"),
        (vec!["--offset=1.5"], "--offset 需要正整数"),
        (vec!["--offset=18446744073709551616"], "--offset 需要正整数"),
    ] {
        let mut args = vec!["sessions"];
        args.extend(flags);
        let output = run_cli("http://127.0.0.1:0", &args);
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains(error));
    }
}

#[test]
fn message_cursor_flags_still_reach_http() {
    let (line, _) = request(
        &[
            "--json",
            "messages",
            "--chat",
            "test-chat",
            "--before",
            "1730000000",
            "--before-id",
            "-2",
            "--before-shard",
            "message_0.db",
        ],
        r#"{"messages":[],"has_more":false}"#,
    );
    assert_eq!(line, "GET /api/messages?username=test-chat&limit=100&before=1730000000&before_id=-2&before_shard=message_0.db HTTP/1.1\r\n");
}
