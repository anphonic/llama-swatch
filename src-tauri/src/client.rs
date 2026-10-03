//! Typed HTTP client for llama-swap. Every call returns `Result`; nothing panics.

use std::time::{Duration, Instant};

use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::api::{
    ActivityEntry, ActivityResponse, ModelEntry, ModelsResponse, RunningModel, RunningResponse, StatsResponse,
    VersionInfo,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

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

    fn get(&self, path: &str) -> RequestBuilder {
        let rb = self.http.get(format!("{}{}", self.base, path));
        match &self.key {
            Some(k) => rb.bearer_auth(k),
            None => rb,
        }
    }

    async fn send(rb: RequestBuilder) -> Result<Response, ClientError> {
        let resp = rb.send().await.map_err(|e| ClientError::Unreachable(describe(e)))?;
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

    /// Opens the SSE stream. Only the connect phase is time-limited; the body stays open.
    pub async fn events(&self) -> Result<Response, ClientError> {
        Self::send(self.get("/api/events").header("Accept", "text/event-stream")).await
    }
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
