#![allow(dead_code)]

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub const VERSION: &str = include_str!("../fixtures/version.json");
pub const RUNNING: &str = include_str!("../fixtures/running.json");
pub const MODELS: &str = include_str!("../fixtures/models.json");
pub const STATS: &str = include_str!("../fixtures/stats.json");
pub const ACTIVITY: &str = include_str!("../fixtures/activity.json");

pub fn json_body(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "application/json")
}

/// Mounts every polled endpoint with fixture data (default priority 5).
/// Tests override individual routes with `.with_priority(1)`.
pub async fn mount_healthy(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200).set_body_string("OK"))
        .mount(server)
        .await;
    for (p, body) in [
        ("/api/version", VERSION),
        ("/running", RUNNING),
        ("/v1/models", MODELS),
        ("/api/metrics/stats", STATS),
        ("/api/metrics/activity", ACTIVITY),
    ] {
        Mock::given(method("GET")).and(path(p)).respond_with(json_body(body)).mount(server).await;
    }
}

/// One llama-swap SSE frame: the inner `data` is JSON encoded as a string.
pub fn sse_frame(kind: &str, data: serde_json::Value) -> String {
    format!("event:message\ndata:{}\n\n", json!({ "type": kind, "data": data.to_string() }))
}

/// A localhost URL with nothing listening on it.
pub fn closed_port_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}
