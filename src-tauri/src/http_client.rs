//! Process-wide HTTP clients for Devify.
//!
//! A `reqwest::Client` owns its connection pool, so building one per command pays
//! a fresh DNS lookup, TCP handshake and TLS handshake for every request — the
//! tasks page alone polls every 2–30 s. Each traffic profile shares its
//! connection pools across commands, one per connection route.
//!
//! The three profiles exist because they differ in semantics, not convenience:
//!
//! - [`api`] — ordinary JSON calls; redirects keep reqwest's default policy.
//! - [`strict`] — calls that carry a bearer token or a one-time code; a 3xx is
//!   reported to the caller instead of silently forwarding credentials elsewhere.
//! - [`upload`] — whole-package uploads and multipart feedback, which need the
//!   same redirect rule but may legitimately run for minutes.

use crate::network_connection::{self, Mode};
use crate::settings::OFFICIAL_API_BASE;
use reqwest::{Client, IntoUrl, Method, RequestBuilder, Url};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

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

fn build(request_timeout: Duration, follows_redirects: bool, direct: bool) -> Client {
    let redirect = if follows_redirects {
        reqwest::redirect::Policy::default()
    } else {
        reqwest::redirect::Policy::none()
    };
    let builder = Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(request_timeout)
        .redirect(redirect)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .pool_max_idle_per_host(POOL_MAX_IDLE_PER_HOST)
        .tcp_keepalive(TCP_KEEPALIVE);
    let builder = if direct { builder.no_proxy() } else { builder };
    builder.build().expect("initialize Devify HTTP client")
}

pub(crate) struct DevifyClient {
    system: Mutex<Option<Client>>,
    direct: Client,
    timeout: Duration,
    follows_redirects: bool,
}

impl DevifyClient {
    fn new(timeout: Duration, follows_redirects: bool) -> Self {
        Self {
            system: Mutex::new(None),
            direct: build(timeout, follows_redirects, true),
            timeout,
            follows_redirects,
        }
    }

    fn system_client(&self) -> Client {
        let mut system = self
            .system
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        system
            .get_or_insert_with(|| build(self.timeout, self.follows_redirects, false))
            .clone()
    }

    pub fn request(&'static self, method: Method, url: impl IntoUrl) -> DevifyRequest {
        DevifyRequest {
            client: self,
            builder: self.direct.request(method, url),
        }
    }

    pub fn get(&'static self, url: impl IntoUrl) -> DevifyRequest {
        self.request(Method::GET, url)
    }

    pub fn post(&'static self, url: impl IntoUrl) -> DevifyRequest {
        self.request(Method::POST, url)
    }

    pub fn put(&'static self, url: impl IntoUrl) -> DevifyRequest {
        self.request(Method::PUT, url)
    }

    pub fn delete(&'static self, url: impl IntoUrl) -> DevifyRequest {
        self.request(Method::DELETE, url)
    }
}

pub(crate) struct DevifyRequest {
    client: &'static DevifyClient,
    builder: RequestBuilder,
}

impl DevifyRequest {
    pub fn bearer_auth(mut self, token: impl std::fmt::Display) -> Self {
        self.builder = self.builder.bearer_auth(token);
        self
    }

    pub fn json(mut self, body: &(impl serde::Serialize + ?Sized)) -> Self {
        self.builder = self.builder.json(body);
        self
    }

    pub fn query(mut self, query: &(impl serde::Serialize + ?Sized)) -> Self {
        self.builder = self.builder.query(query);
        self
    }

    pub fn header(mut self, name: reqwest::header::HeaderName, value: &str) -> Self {
        self.builder = self.builder.header(name, value);
        self
    }

    pub fn multipart(mut self, form: reqwest::multipart::Form) -> Self {
        self.builder = self.builder.multipart(form);
        self
    }

    pub fn try_clone(&self) -> Option<Self> {
        Some(Self {
            client: self.client,
            builder: self.builder.try_clone()?,
        })
    }

    pub(crate) fn prepare(self, root: &Path) -> Result<PreparedRequest, reqwest::Error> {
        let request = self.builder.build()?;
        // An unreadable preference must not silently bypass a configured proxy.
        let mode = network_connection::load(root).unwrap_or(Mode::SystemProxy);
        let route = choose_route(mode, request.url(), proxy_route_recent(root));
        let use_direct = route != Route::System;
        // A write on a redirect-following client may already have reached the
        // original URL when the redirected connection fails.
        let can_fallback = route == Route::DirectThenSystem
            && (!self.client.follows_redirects
                || matches!(*request.method(), Method::GET | Method::HEAD));
        let fallback = can_fallback
            .then(|| request.try_clone())
            .flatten()
            .map(|request| {
                let url = request.url().clone();
                (
                    RequestBuilder::from_parts(self.client.system_client(), request),
                    url,
                )
            });
        let client = if use_direct {
            self.client.direct.clone()
        } else {
            self.client.system_client()
        };
        Ok(PreparedRequest {
            first: RequestBuilder::from_parts(client, request),
            fallback,
        })
    }
}

pub(crate) struct PreparedRequest {
    pub first: RequestBuilder,
    pub fallback: Option<(RequestBuilder, Url)>,
}

#[derive(Debug, Eq, PartialEq)]
enum Route {
    System,
    Direct,
    DirectThenSystem,
}

fn choose_route(mode: Mode, url: &Url, recent_proxy: bool) -> Route {
    match mode {
        Mode::Direct => Route::Direct,
        Mode::Auto if !recent_proxy && url[..url::Position::BeforePath] == *OFFICIAL_API_BASE => {
            Route::DirectThenSystem
        }
        Mode::Auto | Mode::SystemProxy => Route::System,
    }
}

// Give multipart requests (which cannot be cloned) the known working route
// during a sequence of calls, and periodically test direct connectivity again.
const PROXY_ROUTE_TTL: Duration = Duration::from_secs(60);
static RECENT_PROXY: Mutex<Option<(PathBuf, Instant)>> = Mutex::new(None);

fn proxy_route_recent(root: &Path) -> bool {
    RECENT_PROXY.lock().is_ok_and(|route| {
        route
            .as_ref()
            .is_some_and(|(path, seen)| path == root && seen.elapsed() < PROXY_ROUTE_TTL)
    })
}

pub(crate) fn remember_proxy_route(root: &Path) {
    if let Ok(mut route) = RECENT_PROXY.lock() {
        *route = Some((root.to_owned(), Instant::now()));
    }
}

pub(crate) fn forget_proxy_route(root: &Path) {
    if let Ok(mut route) = RECENT_PROXY.lock() {
        if route.as_ref().is_some_and(|(path, _)| path == root) {
            *route = None;
        }
    }
}

static API: OnceLock<DevifyClient> = OnceLock::new();
static STRICT: OnceLock<DevifyClient> = OnceLock::new();
static UPLOAD: OnceLock<DevifyClient> = OnceLock::new();

pub(crate) fn connection_mode_changed(root: &Path) {
    forget_proxy_route(root);
    // Reqwest snapshots the system proxy when creating a client. Refresh that
    // snapshot after a change in the desktop app's connection preference.
    for client in [API.get(), STRICT.get(), UPLOAD.get()]
        .into_iter()
        .flatten()
    {
        *client
            .system
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
    }
}

/// Ordinary JSON API calls. Redirects follow reqwest's default policy.
pub fn api() -> &'static DevifyClient {
    API.get_or_init(|| DevifyClient::new(REQUEST_TIMEOUT, true))
}

/// Requests carrying a bearer token or a one-time code: a redirect must be
/// reported to the caller rather than replayed against another host.
pub fn strict() -> &'static DevifyClient {
    STRICT.get_or_init(|| DevifyClient::new(REQUEST_TIMEOUT, false))
}

/// Whole-package uploads and multipart feedback: same redirect rule, long deadline.
pub fn upload() -> &'static DevifyClient {
    UPLOAD.get_or_init(|| DevifyClient::new(UPLOAD_TIMEOUT, false))
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

    #[test]
    fn auto_routes_only_the_official_https_origin_directly() {
        let official = Url::parse("https://app.aimychats.com/api/v1/auth/desktop/config").unwrap();
        assert_eq!(
            choose_route(Mode::Auto, &official, false),
            Route::DirectThenSystem
        );
        assert_eq!(choose_route(Mode::Auto, &official, true), Route::System);
        assert_eq!(
            choose_route(Mode::SystemProxy, &official, false),
            Route::System
        );
        for address in [
            "https://app.aimychats.com:444/",
            "http://app.aimychats.com/",
            "https://app.aimychats.com.example.org/",
            "http://127.0.0.1:7000/",
        ] {
            let url = Url::parse(address).unwrap();
            assert_eq!(choose_route(Mode::Auto, &url, false), Route::System);
            assert_eq!(choose_route(Mode::Direct, &url, false), Route::Direct);
        }
    }
}
