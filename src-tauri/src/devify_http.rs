//! Correlate Devify requests using content-free local diagnostics.
use crate::diagnostics::{self, Code, EventInput, Module, Operation};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub struct RequestId(String);

impl TryFrom<String> for RequestId {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if (1..=80).contains(&value.len())
            && value.as_bytes()[0].is_ascii_alphanumeric()
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        {
            Ok(Self(value))
        } else {
            Err("请求编号格式无效。")
        }
    }
}

impl From<RequestId> for String {
    fn from(value: RequestId) -> Self {
        value.0
    }
}

fn new_request_id() -> RequestId {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let mut bytes = [0; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        // Keep requests diagnosable even if OS entropy is temporarily unavailable.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        return RequestId(format!(
            "{nanos:x}-{:x}-{:x}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
    }
    RequestId(hex::encode(bytes))
}

pub trait RequestTrace {
    fn send_traced(
        self,
        root: &Path,
        module: Module,
    ) -> impl std::future::Future<Output = Result<reqwest::Response, reqwest::Error>> + Send;
}

impl RequestTrace for reqwest::RequestBuilder {
    async fn send_traced(
        self,
        root: &Path,
        module: Module,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let id = new_request_id();
        let started = std::time::Instant::now();
        let event = EventInput {
            module,
            operation: Operation::Start,
            code: None,
            batch_id: None,
            duration_ms: None,
            request_id: Some(id.clone()),
            server_request_id: None,
            http_status: None,
        };
        let _ = diagnostics::record(root, event.clone());
        let result = self.header("X-Request-ID", id.0.as_str()).send().await;
        let mut completed = event;
        completed.duration_ms = Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64);
        completed.operation = Operation::Success;
        match &result {
            Ok(response) => {
                completed.http_status = Some(response.status().as_u16());
                completed.server_request_id = response
                    .headers()
                    .get("X-Request-ID")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| RequestId::try_from(v.to_owned()).ok())
                    .filter(|v| *v != id);
                if !response.status().is_success() {
                    completed.operation = Operation::Failure;
                    completed.code = Some(Code::HttpError);
                }
            }
            Err(error) => {
                completed.operation = Operation::Failure;
                completed.code = Some(if error.is_timeout() {
                    Code::NetworkTimeout
                } else if error.is_connect() {
                    Code::NetworkConnect
                } else {
                    Code::NetworkRequest
                });
            }
        }
        let _ = diagnostics::record(root, completed);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn request_ids_correlate_retries_and_http_failures_without_content() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut ids = Vec::new();
            for status in [200, 503, 400] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut data = Vec::new();
                let mut chunk = [0u8; 1024];
                while !data.windows(4).any(|v| v == b"\r\n\r\n") {
                    let len = stream.read(&mut chunk).unwrap();
                    assert!(len > 0);
                    data.extend_from_slice(&chunk[..len]);
                }
                let header = String::from_utf8(data).unwrap();
                let id = header
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case("x-request-id"))
                            .map(|(_, v)| v.trim().to_string())
                    })
                    .unwrap();
                ids.push(id.clone());
                let returned = match status {
                    503 => "gateway-456",
                    400 => "bad id",
                    _ => &id,
                };
                write!(stream, "HTTP/1.1 {status} Test\r\nX-Request-ID: {returned}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            }
            ids
        });
        let root = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            for expected in [200, 503, 400] {
                let response = client
                    .get(format!("http://{address}/private-account?token=secret"))
                    .bearer_auth("private-credential")
                    .send_traced(root.path(), Module::Upload)
                    .await
                    .unwrap();
                assert_eq!(response.status().as_u16(), expected);
            }
        });
        let ids = server.join().unwrap();
        assert_ne!(ids[0], ids[1]);
        let logs = diagnostics::snapshot(root.path(), 0);
        let events = logs["events"].as_array().unwrap();
        assert_eq!(events.len(), 6);
        assert_eq!(events[0]["request_id"], ids[0]);
        assert_eq!(events[1]["request_id"], ids[0]);
        assert_eq!(events[3]["request_id"], ids[1]);
        assert_eq!(events[3]["http_status"], 503);
        assert_eq!(events[3]["code"], "HTTP_ERROR");
        assert_eq!(events[3]["server_request_id"], "gateway-456");
        assert!(events[5].get("server_request_id").is_none());
        assert!(!logs.to_string().contains("private-"));
        assert!(!logs.to_string().contains("secret"));
    }

    #[test]
    fn connection_failure_has_an_id_and_safe_error_code() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let root = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap()
                .get(format!("http://{address}"))
                .send_traced(root.path(), Module::Auth),
        );
        assert!(result.is_err());
        let logs = diagnostics::snapshot(root.path(), 0);
        assert_eq!(logs["events"][1]["code"], "NETWORK_CONNECT");
        assert_eq!(
            logs["events"][0]["request_id"],
            logs["events"][1]["request_id"]
        );
    }

    #[test]
    fn untrusted_ids_cannot_inject_log_content() {
        for value in ["", "bad id", "id\nsecret", "user@example.com"] {
            assert!(RequestId::try_from(value.to_string()).is_err());
        }
        assert!(RequestId::try_from("x".repeat(81)).is_err());
    }
}
