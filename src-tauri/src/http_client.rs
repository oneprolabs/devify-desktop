//! Process-wide HTTP clients for Devify.
//!
//! A `reqwest::Client` owns its connection pool, so building one per command pays
//! a fresh DNS lookup, TCP handshake and TLS handshake for every request — the
//! tasks page alone polls every 2–30 s. One client per traffic profile keeps a
//! keep-alive socket (and its TLS session) alive between commands.
//!
//! The three profiles exist because they differ in semantics, not convenience:
//!
//! - [`api`] — ordinary JSON calls; redirects keep reqwest's default policy.
//! - [`strict`] — calls that carry a bearer token or a one-time code; a 3xx is
//!   reported to the caller instead of silently forwarding credentials elsewhere.
//! - [`upload`] — whole-package uploads and multipart feedback, which need the
//!   same redirect rule but may legitimately run for minutes.

use std::sync::OnceLock;
use std::time::Duration;

/// A wrong LAN address must fail the login screen quickly instead of leaving it
/// spinning until the kernel gives up.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Deadline for ordinary API calls. Also the upper bound of one retry attempt.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Chat uploads carry a whole batch and can take much longer than a JSON call.
pub const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// Keep below the reverse proxy's idle limit (nginx defaults to 75 s) so a pooled
/// socket is never reused after the server has already closed it.
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// A page can issue a handful of concurrent reads; each needs its own socket
/// because HTTP/1.1 cannot multiplex.
const POOL_MAX_IDLE_PER_HOST: usize = 8;
/// Detect a dead peer while a socket sits idle instead of at the next request.
const TCP_KEEPALIVE: Duration = Duration::from_secs(30);

fn build(request_timeout: Duration, redirect: reqwest::redirect::Policy) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(request_timeout)
        .redirect(redirect)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .pool_max_idle_per_host(POOL_MAX_IDLE_PER_HOST)
        .tcp_keepalive(TCP_KEEPALIVE)
        .build()
        // Keep the app usable if the TLS backend cannot be initialised: requests then
        // fail per call instead of the client constructor failing forever.
        .unwrap_or_else(|_| reqwest::Client::new())
}

static API: OnceLock<reqwest::Client> = OnceLock::new();
static STRICT: OnceLock<reqwest::Client> = OnceLock::new();
static UPLOAD: OnceLock<reqwest::Client> = OnceLock::new();

/// Ordinary JSON API calls. Redirects follow reqwest's default policy.
pub fn api() -> &'static reqwest::Client {
    API.get_or_init(|| build(REQUEST_TIMEOUT, reqwest::redirect::Policy::default()))
}

/// Requests carrying a bearer token or a one-time code: a redirect must be
/// reported to the caller rather than replayed against another host.
pub fn strict() -> &'static reqwest::Client {
    STRICT.get_or_init(|| build(REQUEST_TIMEOUT, reqwest::redirect::Policy::none()))
}

/// Whole-package uploads and multipart feedback: same redirect rule, long deadline.
pub fn upload() -> &'static reqwest::Client {
    UPLOAD.get_or_init(|| build(UPLOAD_TIMEOUT, reqwest::redirect::Policy::none()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_profile_hands_out_one_shared_client() {
        // Cloning a `Client` bumps one `Arc`; a second pool would defeat the point.
        assert!(std::ptr::eq(api(), api()));
        assert!(std::ptr::eq(strict(), strict()));
        assert!(std::ptr::eq(upload(), upload()));
        assert!(!std::ptr::eq(api(), strict()));
        assert!(!std::ptr::eq(strict(), upload()));
    }

    #[test]
    fn uploads_get_a_longer_deadline_than_json_calls() {
        assert!(UPLOAD_TIMEOUT > REQUEST_TIMEOUT);
        assert!(POOL_IDLE_TIMEOUT < Duration::from_secs(75));
    }
}
