//! Correlate Devify requests using content-free local diagnostics, and retry the
//! failures that are safe to repeat.
use crate::diagnostics::{self, Code, EventInput, Module, Operation};
use crate::http_client::REQUEST_TIMEOUT;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, Instant};

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

/// How hard one request tries before its failure is reported.
///
/// Only idempotent reads may be repeated after a timeout or a 5xx: the server may
/// already have applied a write, and repeating it would duplicate a device, topic
/// or upload. For writes, a connect failure is the only proof that nothing arrived.
#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    /// Total attempts, including the first one.
    attempts: u32,
    /// Reads are safe to repeat; writes are only repeated when nothing was sent.
    idempotent: bool,
    /// Upper bound for one attempt, so `attempts` cannot stretch the deadline.
    attempt_timeout: Duration,
}

impl RetryPolicy {
    /// Reads: retry connect failures, timeouts and transient gateway statuses.
    pub const READ: Self = Self {
        attempts: 3,
        idempotent: true,
        attempt_timeout: REQUEST_TIMEOUT,
    };

    /// Writes: retry only when the connection could not be established at all.
    pub const WRITE: Self = Self {
        attempts: 2,
        idempotent: false,
        attempt_timeout: REQUEST_TIMEOUT,
    };

    pub fn for_method(method: &reqwest::Method) -> Self {
        if method == reqwest::Method::GET || method == reqwest::Method::HEAD {
            Self::READ
        } else {
            Self::WRITE
        }
    }
}

/// All attempts of one request share this deadline, so retrying an unresponsive
/// server never makes the UI wait much longer than a single request would.
const RETRY_BUDGET: Duration = Duration::from_secs(45);
const RETRY_BASE_DELAY: Duration = Duration::from_millis(300);
/// Do not start an attempt with less time than this left in the budget.
const RETRY_MIN_WINDOW: Duration = Duration::from_secs(2);

/// Gateway and rate-limit answers that mean "try again", not "the request is wrong".
fn transient_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 429 | 502 | 503 | 504)
}

fn retry_delay(attempt: u32) -> Duration {
    RETRY_BASE_DELAY * 2u32.pow(attempt.saturating_sub(1).min(3))
}

pub trait RequestTrace {
    fn send_traced(
        self,
        root: &Path,
        module: Module,
    ) -> impl std::future::Future<Output = Result<reqwest::Response, reqwest::Error>> + Send;

    /// Send with bounded retries. Every attempt keeps its own `X-Request-ID`, so
    /// the diagnostics file shows the retry chain instead of hiding it.
    fn send_traced_retrying(
        self,
        root: &Path,
        module: Module,
        policy: RetryPolicy,
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

    async fn send_traced_retrying(
        self,
        root: &Path,
        module: Module,
        policy: RetryPolicy,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let started = Instant::now();
        let attempts = policy.attempts.max(1);
        let mut pending = self;
        let mut attempt = 1;

        loop {
            // A multipart or streamed body cannot be replayed; `try_clone` then returns
            // `None` and this attempt is the last one whatever happens.
            let next = if attempt < attempts { pending.try_clone() } else { None };
            let remaining = RETRY_BUDGET.saturating_sub(started.elapsed());
            let result = pending
                .timeout(remaining.min(policy.attempt_timeout))
                .send_traced(root, module.clone())
                .await;
            let retryable = match &result {
                Ok(response) => policy.idempotent && transient_status(response.status()),
                Err(error) => {
                    // A connect error proves nothing was sent. A timeout or a 5xx on a
                    // write may still have applied on the server, so it is reported.
                    error.is_connect() || (policy.idempotent && error.is_timeout())
                }
            };
            let Some(next) = next.filter(|_| retryable) else {
                return result;
            };
            let delay = retry_delay(attempt);
            if RETRY_BUDGET.saturating_sub(started.elapsed()) <= delay + RETRY_MIN_WINDOW {
                return result;
            }
            // The failed response is dropped without draining its body: buffering an
            // arbitrarily large error page is worse than needing a new socket.
            drop(result);
            tokio::time::sleep(delay).await;
            pending = next;
            attempt += 1;
        }
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

    /// Serve `statuses` in order. Stops after the list or after five seconds, so a
    /// missing retry fails the count assertion instead of hanging the test.
    fn serve(
        listener: std::net::TcpListener,
        statuses: Vec<u16>,
    ) -> std::thread::JoinHandle<Vec<String>> {
        std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut ids = Vec::new();
            for status in statuses {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                Instant::now() < deadline,
                                "attempt {} never arrived",
                                ids.len() + 1
                            );
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("accept failed: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
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
                ids.push(id);
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nX-Request-ID: attempt\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                )
                .unwrap();
            }
            ids
        })
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn transient_reads_are_retried_with_a_new_request_id() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = serve(listener, vec![503, 200]);
        let root = tempfile::tempdir().unwrap();
        let response = runtime().block_on(async {
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap()
                .get(format!("http://{address}/api/v1/spaces"))
                .send_traced_retrying(root.path(), Module::App, RetryPolicy::READ)
                .await
                .unwrap()
        });
        assert_eq!(response.status().as_u16(), 200);
        let ids = server.join().unwrap();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        // Both attempts stay visible in the diagnostics file.
        assert_eq!(
            diagnostics::snapshot(root.path(), 0)["events"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn writes_are_not_replayed_after_a_gateway_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = serve(listener, vec![503]);
        let root = tempfile::tempdir().unwrap();
        let response = runtime().block_on(async {
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap()
                .post(format!("http://{address}/api/v1/auth/desktop/devices"))
                .json(&serde_json::json!({"label": "desk"}))
                .send_traced_retrying(root.path(), Module::Auth, RetryPolicy::WRITE)
                .await
                .unwrap()
        });
        assert_eq!(response.status().as_u16(), 503);
        assert_eq!(server.join().unwrap().len(), 1);
    }

    #[test]
    fn writes_still_retry_a_connection_that_never_opened() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let root = tempfile::tempdir().unwrap();
        let result = runtime().block_on(
            reqwest::Client::builder().no_proxy().build().unwrap()
                .post(format!("http://{address}/api/v1/auth/login"))
                .json(&serde_json::json!({"username": "a", "password": "b"}))
                .send_traced_retrying(root.path(), Module::Auth, RetryPolicy::WRITE),
        );
        assert!(result.is_err());
        let logs = diagnostics::snapshot(root.path(), 0);
        let events = logs["events"].as_array().unwrap();
        assert_eq!(events.len(), 4);
        assert_eq!(events[1]["code"], "NETWORK_CONNECT");
        assert_eq!(events[3]["code"], "NETWORK_CONNECT");
    }
}
