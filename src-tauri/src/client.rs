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

fn describe(e: reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".into()
    } else if e.is_connect() {
        "connection refused or host not found".into()
    } else {
        e.without_url().to_string()
    }
}

impl LlamaSwapClient {
    pub fn new(base_url: &str, key: Option<String>) -> Result<Self, ClientError> {
        let http = Client::builder()
            .connect_timeout(REQUEST_TIMEOUT)
            .tcp_keepalive(Duration::from_secs(30))
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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TestResult {
    Ok { version: String },
    Unreachable { message: String },
    Unauthorized,
    NotLlamaSwap { message: String },
}

/// `/health` tells "reachable" apart from "not there"; `/api/version` tells
/// "good key" apart from "bad key". Older llama-swap without `/api/version`
/// falls back to `/running`.
pub async fn test_connection(client: &LlamaSwapClient) -> TestResult {
    match client.health().await {
        Err(ClientError::Unreachable(message)) => return TestResult::Unreachable { message },
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
        Err(ClientError::NotFound) => match client.running().await {
            Ok(_) => TestResult::Ok { version: "unknown".into() },
            Err(ClientError::Unauthorized) => TestResult::Unauthorized,
            Err(e) => TestResult::NotLlamaSwap { message: format!("/running: {e}") },
        },
        Err(e) => TestResult::NotLlamaSwap { message: format!("/api/version: {e}") },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
