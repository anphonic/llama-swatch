mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use llama_swatch_lib::events::EventStream;
use llama_swatch_lib::monitor::{Connection, Snapshot};
use llama_swatch_lib::runtime::{start, MonitorConfig, SnapshotSink};
use llama_swatch_lib::state::{ModelState, Thresholds};
use serde_json::json;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

struct ChanSink(UnboundedSender<Snapshot>);

impl SnapshotSink for ChanSink {
    fn emit(&self, s: &Snapshot) {
        let _ = self.0.send(s.clone());
    }
}

fn config(url: String) -> MonitorConfig {
    MonitorConfig { base_url: url, api_key: None, poll_interval: Duration::from_millis(200), version_retry: Duration::from_millis(200), thresholds: Thresholds::default() }
}

async fn wait_for(rx: &mut UnboundedReceiver<Snapshot>, what: &str, pred: impl Fn(&Snapshot) -> bool) -> Snapshot {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let s = rx.recv().await.expect("sink closed");
            if pred(&s) {
                return s;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for: {what}"))
}

fn state_of<'a>(s: &'a Snapshot, id: &str) -> Option<&'a ModelState> {
    s.models.iter().find(|m| m.id == id).map(|m| &m.state)
}

#[tokio::test]
async fn healthy_server_produces_connected_snapshot() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    let (tx, mut rx) = unbounded_channel();
    let _handle = start(config(server.uri()), Arc::new(ChanSink(tx))).unwrap();
    let first = rx.recv().await.unwrap();
    assert_eq!(first.connection, Connection::Connecting, "an initial snapshot is published immediately");
    let s = wait_for(&mut rx, "connected", |s| matches!(s.connection, Connection::Connected { .. })).await;
    assert_eq!(s.version.as_deref(), Some("v188"));
    assert_eq!(s.models.len(), 3);
    assert!(s.stats.is_some());
    assert_eq!(s.event_stream, EventStream::Offline, "no /api/events route mounted");
    assert_eq!(s.version_note.as_deref(), Some("Tested with llama-swap v249–v262; this server reports v188"));
}

#[tokio::test]
async fn unauthorized_is_reported() {
    let server = MockServer::start().await;
    Mock::given(path("/running")).respond_with(ResponseTemplate::new(401)).with_priority(1).mount(&server).await;
    mount_healthy(&server).await;
    let (tx, mut rx) = unbounded_channel();
    let _handle = start(config(server.uri()), Arc::new(ChanSink(tx))).unwrap();
    wait_for(&mut rx, "unauthorized", |s| s.connection == Connection::Unauthorized).await;
}

#[tokio::test]
async fn unreachable_is_reported() {
    let (tx, mut rx) = unbounded_channel();
    let _handle = start(config(closed_port_url()), Arc::new(ChanSink(tx))).unwrap();
    wait_for(&mut rx, "unreachable", |s| matches!(s.connection, Connection::Unreachable { .. })).await;
}

#[tokio::test]
async fn dropped_event_stream_clears_busy() {
    let server = MockServer::start().await;
    let body = sse_frame(
        "inflight",
        json!({"operation":"snapshot","requests":[{"id":"7","model":"qwen3-30b","resp_bytes":128,"elapsed_ms":2000}]}),
    );
    // Delay so the first poll has marked qwen3-30b ready before the event lands.
    // The response body then ends, which is exactly what a llama-swap restart looks like.
    Mock::given(path("/api/events"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(body.into_bytes(), "text/event-stream")
                .set_delay(Duration::from_millis(500)),
        )
        .mount(&server)
        .await;
    mount_healthy(&server).await;
    let (tx, mut rx) = unbounded_channel();
    let _handle = start(config(server.uri()), Arc::new(ChanSink(tx))).unwrap();

    wait_for(&mut rx, "busy from live event", |s| {
        s.event_stream == EventStream::Live && matches!(state_of(s, "qwen3-30b"), Some(ModelState::Busy { requests: 1, .. }))
    })
    .await;
    wait_for(&mut rx, "loaded, not idle, after stream drop", |s| {
        s.event_stream == EventStream::Offline && state_of(s, "qwen3-30b") == Some(&ModelState::Loaded)
    })
    .await;

    // The event stream must reconnect (after the 2 s backoff): a second GET /api/events arrives.
    let reconnected = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let n = server
                .received_requests()
                .await
                .unwrap_or_default()
                .iter()
                .filter(|r| r.url.path() == "/api/events")
                .count();
            if n >= 2 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(reconnected.is_ok(), "event stream did not reconnect");
}

#[tokio::test]
async fn undecodable_events_are_reported_as_unreadable() {
    let server = MockServer::start().await;
    // A newer llama-swap whose inflight payload no longer parses, three frames in a row.
    let bad = "event:message\ndata:{\"type\":\"inflight\",\"data\":\"{not json\"}\n\n".repeat(3);
    Mock::given(path("/api/events"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(bad.into_bytes(), "text/event-stream")
                .set_delay(Duration::from_millis(500)),
        )
        .mount(&server)
        .await;
    mount_healthy(&server).await;
    let (tx, mut rx) = unbounded_channel();
    let _handle = start(config(server.uri()), Arc::new(ChanSink(tx))).unwrap();
    let s = wait_for(&mut rx, "unreadable", |s| s.event_stream == EventStream::Unreadable).await;
    assert_eq!(state_of(&s, "qwen3-30b"), Some(&ModelState::Loaded));
}

#[tokio::test]
async fn upserts_that_fail_are_not_masked_by_removes_and_model_status() {
    let server = MockServer::start().await;
    // Only the upsert entry shape changed (elapsed_ms became a string); removes and
    // modelStatus still decode. They must not keep the stream Live.
    let frame = |kind: &str, data: serde_json::Value| {
        let env = serde_json::json!({ "type": kind, "data": data.to_string() });
        format!("event:message\ndata:{env}\n\n")
    };
    let bad_upsert = frame(
        "inflight",
        serde_json::json!({"operation":"upsert","request":{"id":"1","model":"qwen3-30b","elapsed_ms":"900","resp_bytes":0}}),
    );
    let remove = frame("inflight", serde_json::json!({"operation":"remove","id":"1"}));
    let model_status = frame("modelStatus", serde_json::json!({}));
    let body = format!("{bad_upsert}{remove}{model_status}").repeat(3);
    Mock::given(path("/api/events"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(body.into_bytes(), "text/event-stream")
                .set_delay(Duration::from_millis(500)),
        )
        .mount(&server)
        .await;
    mount_healthy(&server).await;
    let (tx, mut rx) = unbounded_channel();
    let _handle = start(config(server.uri()), Arc::new(ChanSink(tx))).unwrap();
    let s = wait_for(&mut rx, "unreadable, never live", |s| {
        assert_ne!(s.event_stream, EventStream::Live, "removes and modelStatus must not make the stream live");
        s.event_stream == EventStream::Unreadable
    })
    .await;
    assert_eq!(state_of(&s, "qwen3-30b"), Some(&ModelState::Loaded));
}

#[tokio::test]
async fn stopping_the_handle_stops_snapshots() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    let (tx, mut rx) = unbounded_channel();
    let handle = start(config(server.uri()), Arc::new(ChanSink(tx))).unwrap();
    wait_for(&mut rx, "connected", |s| matches!(s.connection, Connection::Connected { .. })).await;
    handle.stop();
    tokio::time::sleep(Duration::from_millis(300)).await;
    while rx.try_recv().is_ok() {} // drain anything emitted before the abort landed
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(rx.try_recv().is_err(), "no snapshots after stop");
}

#[tokio::test]
async fn transient_version_failure_is_retried_until_it_succeeds() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    // The first /api/version answers 500; later ones fall through to the healthy mock.
    Mock::given(path("/api/version"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    let (tx, mut rx) = unbounded_channel();
    let _handle = start(config(server.uri()), Arc::new(ChanSink(tx))).unwrap();
    let first = wait_for(&mut rx, "connected", |s| matches!(s.connection, Connection::Connected { .. })).await;
    assert_eq!(first.version, None, "the 500 must not yield a version");
    wait_for(&mut rx, "version after retry", |s| s.version.as_deref() == Some("v188")).await;
}

#[tokio::test]
async fn malformed_version_json_is_final_like_a_404() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    Mock::given(path("/api/version")).respond_with(ResponseTemplate::new(200).set_body_string("<html>proxy</html>")).with_priority(1).mount(&server).await;
    let (tx, mut rx) = unbounded_channel();
    let _handle = start(config(server.uri()), Arc::new(ChanSink(tx))).unwrap();
    wait_for(&mut rx, "connected", |s| matches!(s.connection, Connection::Connected { .. })).await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    let asked = server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == "/api/version").count();
    assert_eq!(asked, 1, "a decode error will not fix itself; do not retry");
}

#[tokio::test]
async fn persistent_version_5xx_is_retried_at_the_retry_cadence_not_every_tick() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    Mock::given(path("/api/version")).respond_with(ResponseTemplate::new(500)).with_priority(1).mount(&server).await;
    let (tx, mut rx) = unbounded_channel();
    // 100 ms ticks, 500 ms version retry.
    let mut cfg = config(server.uri());
    cfg.poll_interval = Duration::from_millis(100);
    cfg.version_retry = Duration::from_millis(500);
    let _handle = start(cfg, Arc::new(ChanSink(tx))).unwrap();
    wait_for(&mut rx, "connected", |s| matches!(s.connection, Connection::Connected { .. })).await;
    tokio::time::sleep(Duration::from_millis(1700)).await;
    let asked = server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == "/api/version").count();
    assert!(asked >= 2, "5xx must still be retried, asked {asked}");
    assert!(asked <= 5, "retries must follow the retry cadence, not every tick; asked {asked}");
}

#[tokio::test]
async fn missing_version_endpoint_is_not_re_asked_every_tick() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    Mock::given(path("/api/version")).respond_with(ResponseTemplate::new(404)).with_priority(1).mount(&server).await;
    let (tx, mut rx) = unbounded_channel();
    let _handle = start(config(server.uri()), Arc::new(ChanSink(tx))).unwrap();
    wait_for(&mut rx, "connected", |s| matches!(s.connection, Connection::Connected { .. })).await;
    // Let several 200 ms ticks pass.
    tokio::time::sleep(Duration::from_millis(900)).await;
    let asked = server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == "/api/version").count();
    assert_eq!(asked, 1, "404 means unsupported; do not retry");
}
