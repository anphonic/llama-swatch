//! Typed HTTP client for llama-swap. Every call returns `Result`; nothing panics.

use std::time::{Duration, Instant};

use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::api::{
    ActivityEntry, ActivityResponse, ModelEntry, ModelsResponse, RunningModel, RunningResponse, StatsResponse,
    VersionInfo,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
/// `GET /upstream/{model}/` blocks until the model is up, which can take minutes.
const LOAD_TIMEOUT: Duration = Duration::from_secs(600);
/// At most 4096 bytes of a refusal body are read when a load is answered with a 4xx.
const REFUSAL_BODY_CAP: usize = 4096;
/// At most 256 bytes are read past the cap so a key straddling it can be redacted whole.
const REFUSAL_KEY_READ_AHEAD: usize = 256;
/// At most 200 characters of llama-swap's refusal text are shown to the user.
const REFUSAL_REASON_CHARS: usize = 200;
/// Unloading waits for the upstream process to stop.
const UNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClientError {
    #[error("cannot reach llama-swap: {0}")]
    Unreachable(String),
    #[error("llama-swap rejected the API key")]
    Unauthorized,
    #[error("endpoint not found")]
    NotFound,
    #[error("HTTP {0}")]
    Http(u16),
    #[error("{0}")]
    Redirect(String),
    /// llama-swap answered a load with a 4xx and `/running` shows the model is not starting.
    /// Holds llama-swap's reason, already sanitized for display (no key, URL credentials or query).
    #[error("llama-swap refused it: {0}")]
    LoadRefused(String),
    /// llama-swap answered a load with a 4xx and `/running` could not be read to tell whether the
    /// model started. Holds the whole message, already sanitized for display.
    #[error("{0}")]
    LoadUnconfirmed(String),
    #[error("invalid model id")]
    InvalidModelId,
    #[error("unexpected response: {0}")]
    Decode(String),
}

#[derive(Clone)]
pub struct LlamaSwapClient {
    base: String,
    key: Option<String>,
    http: Client,
}

impl std::fmt::Debug for LlamaSwapClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlamaSwapClient")
            .field("base", &self.base)
            .field("key", &self.key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Short reason if `text` (one link of an error chain) looks like a TLS/certificate failure.
fn tls_reason(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let hit = ["certificate", "unknownissuer", "unknown issuer", "badcertificate", "tls", "handshake", "invalid peer"]
        .iter()
        .any(|k| lower.contains(k));
    hit.then(|| {
        let t = text.trim();
        let t = t.strip_prefix("invalid peer certificate: ").unwrap_or(t);
        t.chars().take(120).collect()
    })
}

fn describe(e: reqwest::Error) -> String {
    if e.is_timeout() {
        return "timed out".into();
    }
    if e.is_connect() {
        let mut src: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(&e);
        while let Some(err) = src {
            if let Some(reason) = tls_reason(&err.to_string()) {
                return format!("TLS certificate not trusted: {reason}");
            }
            src = err.source();
        }
        return "connection refused or host not found".into();
    }
    e.without_url().to_string()
}

impl LlamaSwapClient {
    pub fn new(base_url: &str, key: Option<String>) -> Result<Self, ClientError> {
        let http = Client::builder()
            .connect_timeout(REQUEST_TIMEOUT)
            .tcp_keepalive(Duration::from_secs(30))
            // The API key must only ever go to the saved URL, never to a redirect target.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| ClientError::Unreachable(describe(e)))?;
        Ok(Self {
            base: base_url.trim_end_matches('/').to_string(),
            key: key.filter(|k| !k.trim().is_empty()),
            http,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let rb = self.http.request(method, format!("{}{}", self.base, path));
        match &self.key {
            Some(k) => rb.bearer_auth(k),
            None => rb,
        }
    }

    fn get(&self, path: &str) -> RequestBuilder {
        self.request(Method::GET, path)
    }

    async fn send(rb: RequestBuilder) -> Result<Response, ClientError> {
        Self::check(rb.send().await.map_err(|e| ClientError::Unreachable(describe(e)))?)
    }

    /// Maps a response's status to `Ok(resp)` or the matching error.
    fn check(resp: Response) -> Result<Response, ClientError> {
        match resp.status() {
            s if s.is_success() => Ok(resp),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(ClientError::Unauthorized),
            StatusCode::NOT_FOUND => Err(ClientError::NotFound),
            s if s.is_redirection() => {
                let loc = resp.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok());
                Err(ClientError::Redirect(redirect_advice(resp.url(), loc)))
            }
            s => Err(ClientError::Http(s.as_u16())),
        }
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let resp = Self::send(self.get(path).timeout(REQUEST_TIMEOUT)).await?;
        let bytes = resp.bytes().await.map_err(|e| ClientError::Unreachable(describe(e)))?;
        serde_json::from_slice(&bytes).map_err(|e| ClientError::Decode(e.to_string()))
    }

    /// Body text and round-trip latency of `GET /health` (unauthenticated in llama-swap).
    pub async fn health(&self) -> Result<(String, Duration), ClientError> {
        let start = Instant::now();
        let resp = Self::send(self.get("/health").timeout(REQUEST_TIMEOUT)).await?;
        let text = resp.text().await.map_err(|e| ClientError::Unreachable(describe(e)))?;
        Ok((text, start.elapsed()))
    }

    pub async fn version(&self) -> Result<VersionInfo, ClientError> {
        self.get_json("/api/version").await
    }

    pub async fn running(&self) -> Result<Vec<RunningModel>, ClientError> {
        Ok(self.get_json::<RunningResponse>("/running").await?.running)
    }

    pub async fn models(&self) -> Result<Vec<ModelEntry>, ClientError> {
        Ok(self.get_json::<ModelsResponse>("/v1/models").await?.data)
    }

    pub async fn stats(&self) -> Result<StatsResponse, ClientError> {
        self.get_json("/api/metrics/stats").await
    }

    pub async fn activity(&self, limit: u32) -> Result<Vec<ActivityEntry>, ClientError> {
        let path = format!("/api/metrics/activity?limit={limit}");
        Ok(self.get_json::<ActivityResponse>(&path).await?.data)
    }

    /// Asks llama-swap to start `id` by requesting its upstream root. The call blocks until the
    /// model is up (minutes, possibly). A 2xx means it came up. A 4xx is ambiguous: llama-swap
    /// refuses with one (unknown model, path in `ignorePaths`, no router) without starting
    /// anything, but a started upstream with no root page answers 404 too. So on a 4xx `/running`
    /// is asked once: a model listed as `ready` or `starting` is a success, anything else is
    /// [`ClientError::LoadRefused`] with llama-swap's capped, sanitized reason, and a `/running`
    /// that fails is [`ClientError::LoadUnconfirmed`]. 5xx, redirects, a rejected key (401/403)
    /// and network errors fail as for any other call.
    pub async fn load_model(&self, id: &str) -> Result<(), ClientError> {
        let path = format!("/upstream/{}/", encode_segment(id)?);
        let resp = self
            .get(&path)
            .timeout(LOAD_TIMEOUT)
            .send()
            .await
            .map_err(|e| ClientError::Unreachable(describe(e)))?;
        let status = resp.status();
        if !status.is_client_error() || matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Self::check(resp).map(drop);
        }
        // Read a little past the cap so a key straddling it is redacted whole; a longer key cut
        // at the end is still masked by `redact`'s trailing-prefix check.
        let key = self.key.as_deref();
        let ahead = key.map_or(0, |k| k.len().min(REFUSAL_KEY_READ_AHEAD));
        let body = read_capped(resp, REFUSAL_BODY_CAP + ahead).await;
        let reason = sanitize_reason(&redact(&body, key));
        let code = status.as_u16();
        match self.running().await {
            Ok(running)
                if running.iter().any(|m| m.model == id && matches!(m.state.as_str(), "ready" | "starting")) =>
            {
                Ok(())
            }
            Ok(_) if reason.is_empty() => Err(ClientError::LoadRefused(format!("HTTP {code}"))),
            Ok(_) => Err(ClientError::LoadRefused(reason)),
            Err(_) => {
                let mut msg = format!("llama-swap answered HTTP {code} and the model list could not be checked");
                if !reason.is_empty() {
                    msg = format!("{msg}: {reason}");
                }
                Err(ClientError::LoadUnconfirmed(msg))
            }
        }
    }

    pub async fn unload_model(&self, id: &str) -> Result<(), ClientError> {
        let path = format!("/api/models/unload/{}", encode_segment(id)?);
        Self::send(self.request(Method::POST, &path).timeout(UNLOAD_TIMEOUT)).await.map(drop)
    }

    /// Opens the SSE stream. Only the connect phase is time-limited; the body stays open.
    pub async fn events(&self) -> Result<Response, ClientError> {
        Self::send(self.get("/api/events").header("Accept", "text/event-stream")).await
    }
}

/// At most `cap` bytes of the body; a read error ends the body early.
async fn read_capped(mut resp: Response, cap: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    while buf.len() < cap {
        match resp.chunk().await {
            Ok(Some(chunk)) => buf.extend_from_slice(&chunk[..chunk.len().min(cap - buf.len())]),
            _ => break,
        }
    }
    buf
}

/// Server bytes as text with the API key, any `user:password@` and any `?query`/`#fragment` in a
/// URL removed. Every byte covered by an occurrence of the key (overlapping ones included) and a
/// trailing part that could be the start of a key cut off by a read cap is masked on the raw
/// bytes, before decoding, so a multibyte key cut mid-character is caught too. Each masked run
/// becomes `<redacted>`.
fn redact(body: &[u8], key: Option<&str>) -> String {
    let mut masked = vec![false; body.len()];
    if let Some(k) = key.map(str::as_bytes).filter(|k| !k.is_empty()) {
        for i in 0..body.len() {
            if body[i..].starts_with(k) {
                masked[i..i + k.len()].fill(true);
            }
        }
        // Longest proper prefix of the key that the body ends with.
        let mut tail = (1..k.len().min(body.len() + 1)).rev().map(|n| body.len() - n);
        if let Some(at) = tail.find(|&at| k.starts_with(&body[at..])) {
            masked[at..].fill(true);
        }
    }
    let mut bytes = Vec::with_capacity(body.len());
    for (i, &b) in body.iter().enumerate() {
        if !masked[i] {
            bytes.push(b);
        } else if i == 0 || !masked[i - 1] {
            bytes.extend_from_slice(b"<redacted>");
        }
    }
    let mut out = String::from_utf8_lossy(&bytes).into_owned();
    // Only URLs with a scheme (`x://`) are recognized; a bare `host/path?q` is left as is.
    let mut from = 0;
    while let Some(i) = out[from..].find("://") {
        let host = from + i + 3;
        let rest = &out[host..];
        let url_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let end = rest[..url_end].find(['/', '?', '#']).unwrap_or(url_end);
        if let Some(at) = rest[..end].rfind('@') {
            out.replace_range(host..host + at + 1, "");
            continue;
        }
        if let Some(q) = rest[..url_end].find(['?', '#']) {
            out.replace_range(host + q..host + url_end, "");
        }
        from = host;
    }
    out
}

/// Server text made safe to show: control, bidi and zero-width characters become spaces,
/// whitespace runs collapse, and the result is cut to [`REFUSAL_REASON_CHARS`] characters.
fn sanitize_reason(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| match c {
            c if c.is_control() => ' ',
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' => ' ',
            c => c,
        })
        .collect();
    let joined = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= REFUSAL_REASON_CHARS {
        return joined;
    }
    let mut cut: String = joined.chars().take(REFUSAL_REASON_CHARS - 1).collect();
    cut.push('\u{2026}');
    cut
}

/// Percent-encodes `id` as exactly one URL path segment: everything but unreserved characters is
/// escaped, so `/`, `?`, `#`, `%` and spaces cannot leave the segment. `.` and `..` are refused
/// because URL parsing would treat them (even as `%2E`) as dot segments and climb out.
fn encode_segment(id: &str) -> Result<String, ClientError> {
    if id.trim().is_empty() || id == "." || id == ".." {
        return Err(ClientError::InvalidModelId);
    }
    let mut out = String::with_capacity(id.len());
    for b in id.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    Ok(out)
}

/// Endpoint paths this client requests; used to recover the base URL from a redirect target.
const ENDPOINTS: &[&str] = &[
    "/health",
    "/api/version",
    "/running",
    "/v1/models",
    "/api/metrics/stats",
    "/api/metrics/activity",
    "/api/events",
];

/// Builds the "use this URL instead" message for a 3xx. The Location is resolved against the request
/// URL, the endpoint we asked for is stripped to get a base, and that base is normalized. When no
/// clean base can be derived the message names only scheme and host, never the raw Location.
fn redirect_advice(request: &reqwest::Url, location: Option<&str>) -> String {
    const GENERIC: &str = "check the URL (http vs https)";
    let target = location.and_then(|l| request.join(l).ok()).filter(|u| matches!(u.scheme(), "http" | "https"));
    let Some(target) = target else {
        return format!("llama-swap redirected the request \u{2014} {GENERIC}");
    };
    let endpoint = ENDPOINTS.iter().find(|e| request.path().ends_with(**e));
    let path = target.path().trim_end_matches('/');
    let base = endpoint.and_then(|e| path.strip_suffix(*e)).and_then(|prefix| {
        let mut u = target.clone();
        u.set_path(prefix);
        crate::config::normalize_base_url(u.as_str()).ok()
    });
    match base {
        Some(base) => format!("llama-swap redirected to {base} \u{2014} use that URL instead"),
        None => {
            let host = target.host_str().unwrap_or("another host");
            let port = target.port().map(|p| format!(":{p}")).unwrap_or_default();
            format!("llama-swap redirected to {}://{host}{port} \u{2014} {GENERIC}", target.scheme())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TestResult {
    Ok { version: String },
    Unreachable { message: String },
    /// The server answered with a 3xx; `message` says which URL to use instead.
    Redirect { message: String },
    Unauthorized,
    NotLlamaSwap { message: String },
}

/// `/health` tells "reachable" apart from "not there"; `/api/version` tells
/// "good key" apart from "bad key". Older llama-swap without `/api/version`
/// falls back to `/running`.
pub async fn test_connection(client: &LlamaSwapClient) -> TestResult {
    match client.health().await {
        Err(ClientError::Unreachable(message)) => return TestResult::Unreachable { message },
        Err(ClientError::Redirect(message)) => return TestResult::Redirect { message },
        Err(ClientError::Unauthorized) => return TestResult::Unauthorized,
        Err(e) => return TestResult::NotLlamaSwap { message: format!("/health: {e}") },
        Ok((body, _)) if body.trim() != "OK" => {
            return TestResult::NotLlamaSwap { message: "/health did not answer OK".into() }
        }
        Ok(_) => {}
    }
    match client.version().await {
        Ok(v) if !v.version.is_empty() => TestResult::Ok { version: v.version },
        Ok(_) => TestResult::NotLlamaSwap { message: "/api/version has no version".into() },
        Err(ClientError::Unauthorized) => TestResult::Unauthorized,
        Err(ClientError::Unreachable(message)) => TestResult::Unreachable { message },
        Err(ClientError::Redirect(message)) => TestResult::Redirect { message },
        Err(ClientError::NotFound) => match client.running().await {
            Ok(_) => TestResult::Ok { version: "unknown".into() },
            Err(ClientError::Unauthorized) => TestResult::Unauthorized,
            Err(ClientError::Unreachable(message)) => TestResult::Unreachable { message },
            Err(ClientError::Redirect(message)) => TestResult::Redirect { message },
            Err(e) => TestResult::NotLlamaSwap { message: format!("/running: {e}") },
        },
        Err(e) => TestResult::NotLlamaSwap { message: format!("/api/version: {e}") },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use wiremock::matchers::{any, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn redirect_is_not_followed_and_reports_base_url() {
        let server = MockServer::start().await;
        let target = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(308).insert_header("Location", format!("{}/health", target.uri())))
            .expect(2) // one per call below (health, then test_connection); never a retry or follow
            .mount(&server)
            .await;
        // Matches ANY request: if a redirect were followed, this would be hit (and trip expect(0)).
        Mock::given(any()).respond_with(ResponseTemplate::new(200).set_body_string("OK")).expect(0).mount(&target).await;

        let c = LlamaSwapClient::new(&server.uri(), Some("k".into())).unwrap();
        let err = c.health().await.unwrap_err();
        let ClientError::Redirect(msg) = &err else { panic!("expected Redirect, got {err:?}") };
        assert!(msg.contains(&format!("redirected to {} ", target.uri())), "{msg}");
        assert!(!msg.contains("/health"), "{msg}");
        assert!(msg.contains("use that URL instead"), "{msg}");
        match test_connection(&c).await {
            TestResult::Redirect { message } => assert!(message.contains("redirected to"), "{message}"),
            other => panic!("{other:?}"),
        }
        // wiremock verifies expect(2)/expect(0) on drop: no extra request, nothing reached the target.
    }

    #[tokio::test]
    async fn running_fallback_redirect_reports_redirect() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/health")).respond_with(ResponseTemplate::new(200).set_body_string("OK")).mount(&server).await;
        Mock::given(method("GET")).and(path("/api/version")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
        Mock::given(method("GET"))
            .and(path("/running"))
            .respond_with(ResponseTemplate::new(308).insert_header("Location", "https://elsewhere.invalid/running"))
            .mount(&server)
            .await;
        let c = LlamaSwapClient::new(&server.uri(), Some("k".into())).unwrap();
        match test_connection(&c).await {
            TestResult::Redirect { message } => assert!(message.contains("redirected to"), "{message}"),
            other => panic!("expected Redirect, got {other:?}"),
        }
    }

    fn advice(request: &str, location: Option<&str>) -> String {
        redirect_advice(&reqwest::Url::parse(request).unwrap(), location)
    }

    #[test]
    fn redirect_advice_suggests_base_not_endpoint() {
        // absolute cross-scheme
        let m = advice("http://h/health", Some("https://h/health"));
        assert!(m.contains("redirected to https://h \u{2014} use that URL instead"), "{m}");
        // relative Location resolved against the request URL
        let m = advice("http://h:8080/api/version", Some("/api/version"));
        assert!(m.contains("redirected to http://h:8080 "), "{m}");
        let m = advice("http://h/health", Some("//other/health"));
        assert!(m.contains("redirected to http://other "), "{m}");
        // proxy prefix preserved
        let m = advice("http://h/llama/health", Some("https://h/llama/health"));
        assert!(m.contains("redirected to https://h/llama "), "{m}");
        // query on the request path does not defeat endpoint detection
        let m = advice("http://h/api/metrics/activity?limit=5", Some("https://h/api/metrics/activity?limit=5"));
        assert!(m.contains("redirected to https://h "), "{m}");
    }

    #[test]
    fn redirect_advice_falls_back_without_echoing_location() {
        // endpoint not recoverable from the target
        let m = advice("http://h/health", Some("https://h/login?next=%2Fhealth"));
        assert!(m.contains("https://h"), "{m}");
        assert!(!m.contains("login") && !m.contains("use that URL instead"), "{m}");
        assert!(m.contains("http vs https"), "{m}");
        // not http(s)
        let m = advice("http://h/health", Some("ftp://x/health"));
        assert!(!m.contains("ftp"), "{m}");
        // missing / unparseable Location
        assert!(advice("http://h/health", None).contains("http vs https"));
        assert!(advice("http://h/health", Some("http://[bad")).contains("http vs https"));
    }

    #[test]
    fn describe_flags_certificate_errors() {
        assert!(tls_reason("invalid peer certificate: UnknownIssuer").is_some());
        assert!(tls_reason("received fatal alert: BadCertificate").is_some());
        assert!(tls_reason("tcp connect error: Connection refused (os error 10061)").is_none());
        assert!(tls_reason("dns error: failed to lookup address information").is_none());
    }

    #[tokio::test]
    async fn closed_port_still_reports_connection_refused() {
        // Regression guard: walking the source chain must not misclassify a plain refusal as TLS.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let c = LlamaSwapClient::new(&format!("https://127.0.0.1:{port}"), None).unwrap();
        match c.health().await {
            Err(ClientError::Unreachable(m)) => assert!(m.contains("connection refused"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn read_capped_stops_at_the_cap() {
        let server = MockServer::start().await;
        Mock::given(any()).respond_with(ResponseTemplate::new(409).set_body_string("y".repeat(100_000))).mount(&server).await;
        let resp = reqwest::get(server.uri()).await.unwrap();
        assert_eq!(read_capped(resp, REFUSAL_BODY_CAP).await.len(), REFUSAL_BODY_CAP); // at most 4096
        let resp = reqwest::get(server.uri()).await.unwrap();
        assert_eq!(read_capped(resp, 1_000_000).await.len(), 100_000);
    }

    fn red(text: &str, key: &str) -> String {
        redact(text.as_bytes(), Some(key))
    }

    #[test]
    fn redact_masks_whole_and_trailing_partial_keys() {
        assert_eq!(red("a k1 b k1", "k1"), "a <redacted> b <redacted>");
        assert_eq!(red("end k", "key"), "end <redacted>");
        assert_eq!(red("end ke", "key"), "end <redacted>");
        assert_eq!(red("end kx", "key"), "end kx");
        assert_eq!(redact(b"no key", None), "no key");
        assert_eq!(redact(b"abc", Some("")), "abc");
    }

    #[test]
    fn redact_masks_bordered_keys_at_the_end() {
        // Regression: stripping a trailing prefix before replacing broke the full-key match.
        assert_eq!(red("x abca", "abca"), "x <redacted>");
        assert_eq!(red("bad key: abcdabcd", "abcdabcd"), "bad key: <redacted>");
        assert_eq!(red("bad key: xA1b2C3d4E5x", "xA1b2C3d4E5x"), "bad key: <redacted>");
    }

    #[test]
    fn redact_masks_a_multibyte_key_cut_mid_character() {
        let key = "abcdefgh\u{e9}-zz";
        let cut = "x abcdefgh\u{e9}".len() - 1; // inside the two bytes of the e-acute
        assert_eq!(redact(&"x abcdefgh\u{e9}-zz".as_bytes()[..cut], Some(key)), "x <redacted>");
    }

    #[test]
    fn redact_never_leaves_four_bytes_of_the_key_at_any_cut() {
        // Longer than REFUSAL_KEY_READ_AHEAD, so it can be cut anywhere by the read cap.
        let long: String = (0..300).map(|i| b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"[i * 7 % 36] as char).collect();
        let keys = ["xA1b2C3d4E5x", "abcdabcd", "aaaa1aaaa", "abca", "s3cret-key-0123456789", "abcdefgh\u{e9}-zz", &long];
        for key in keys {
            // Any leaked run of 4+ bytes contains a shortest char-aligned window of 4+ bytes.
            let bounds: Vec<usize> = key.char_indices().map(|(i, _)| i).chain([key.len()]).collect();
            let windows: Vec<&str> = bounds
                .iter()
                .filter_map(|&a| bounds.iter().find(|&&b| b >= a + 4).map(|&b| &key[a..b]))
                .collect();
            for prefix in ["", "bad: ", "<<"] {
                for suffix in ["", " zz.", " tail"] {
                    let full = format!("{prefix}{key}{suffix}");
                    for cut in 0..=full.len() {
                        let out = redact(&full.as_bytes()[..cut], Some(key));
                        for w in &windows {
                            assert!(!out.contains(w), "key {key:?} cut {cut}: {out:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn debug_output_redacts_key() {
        let c = LlamaSwapClient::new("http://box:8080", Some("super-secret".into())).unwrap();
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("super-secret"), "{dbg}");
        assert!(dbg.contains("<redacted>"));
    }

    #[test]
    fn blank_key_is_treated_as_none() {
        let c = LlamaSwapClient::new("http://box:8080/", Some("  ".into())).unwrap();
        assert!(c.key.is_none());
        assert_eq!(c.base_url(), "http://box:8080");
    }

    #[test]
    fn test_result_serializes_for_the_frontend() {
        let v = serde_json::to_value(TestResult::NotLlamaSwap { message: "x".into() }).unwrap();
        assert_eq!(v, serde_json::json!({"kind":"notLlamaSwap","message":"x"}));
    }
}
