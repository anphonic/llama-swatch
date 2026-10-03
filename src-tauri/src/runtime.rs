//! The two background loops: REST polling and the `/api/events` stream.
//! Both write into one `MonitorState` and publish a fresh `Snapshot` after every change.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use tauri::async_runtime::{spawn, JoinHandle};
use tokio::sync::Notify;

use crate::backoff::Backoff;
use crate::client::{ClientError, LlamaSwapClient};
use crate::events::{decode_event, StreamEvent};
use crate::monitor::{MonitorState, Snapshot};
use crate::poller::{poll_once, Feature, PollOutcome};
use crate::sse::SseParser;
use crate::state::Thresholds;

pub const MODELS_REFRESH: Duration = Duration::from_secs(30);
const UNAUTHORIZED_RETRY: Duration = Duration::from_secs(30);
const EVENTS_UNAVAILABLE_RETRY: Duration = Duration::from_secs(60);
/// Bound on waiting for the event stream's response headers.
const EVENTS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_BASE: Duration = Duration::from_secs(2);
const BACKOFF_CAP: Duration = Duration::from_secs(30);

pub trait SnapshotSink: Send + Sync + 'static {
    fn emit(&self, snapshot: &Snapshot);
}

pub struct MonitorConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub poll_interval: Duration,
    /// How often to retry `/api/version` after a transient failure (not every tick).
    pub version_retry: Duration,
    pub thresholds: Thresholds,
}

pub struct MonitorHandle {
    tasks: Vec<JoinHandle<()>>,
}

impl MonitorHandle {
    pub fn stop(&self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl Drop for MonitorHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

struct Shared {
    state: Mutex<MonitorState>,
    sink: Arc<dyn SnapshotSink>,
    thresholds: Thresholds,
    /// Poked by `modelStatus` events so the poll loop refreshes immediately.
    wake: Notify,
}

impl Shared {
    fn with_state(&self, f: impl FnOnce(&mut MonitorState)) {
        f(&mut self.state.lock().expect("monitor state poisoned"));
    }

    fn publish(&self) {
        let snapshot = self
            .state
            .lock()
            .expect("monitor state poisoned")
            .snapshot(Instant::now(), now_ms(), &self.thresholds);
        self.sink.emit(&snapshot);
    }
}

pub fn start(cfg: MonitorConfig, sink: Arc<dyn SnapshotSink>) -> Result<MonitorHandle, ClientError> {
    let client = LlamaSwapClient::new(&cfg.base_url, cfg.api_key)?;
    let shared = Arc::new(Shared {
        state: Mutex::new(MonitorState::new(client.base_url())),
        sink,
        thresholds: cfg.thresholds,
        wake: Notify::new(),
    });
    shared.publish();
    let poll = spawn(poll_loop(client.clone(), shared.clone(), cfg.poll_interval, cfg.version_retry));
    let events = spawn(event_loop(client, shared));
    Ok(MonitorHandle { tasks: vec![poll, events] })
}

async fn poll_loop(client: LlamaSwapClient, shared: Arc<Shared>, interval: Duration, version_retry: Duration) {
    let mut backoff = Backoff::new(BACKOFF_BASE, BACKOFF_CAP);
    let mut need_version = true;
    // After a transient /api/version failure, wait this long before asking again.
    let mut version_retry_at: Option<Instant> = None;
    let mut models_fetched_at: Option<Instant> = None;
    loop {
        let want_models = models_fetched_at.map_or(true, |t| t.elapsed() >= MODELS_REFRESH);
        let ask_version = need_version && version_retry_at.map_or(true, |t| Instant::now() >= t);
        let outcome = poll_once(&client, want_models, ask_version).await;
        let delay = match &outcome {
            PollOutcome::Ok(d) => {
                backoff.reset();
                // Version is fetched once per (re)connection. Success, 404 and malformed JSON
                // settle it; a transient failure is retried, but only every `version_retry`.
                match d.version {
                    Some(Feature::Failed) => version_retry_at = Some(Instant::now() + version_retry),
                    Some(_) => need_version = false,
                    None => {}
                }
                if d.models.is_some() {
                    models_fetched_at = Some(Instant::now());
                }
                interval
            }
            PollOutcome::Unauthorized => {
                need_version = true;
                version_retry_at = None;
                UNAUTHORIZED_RETRY
            }
            PollOutcome::Unreachable(_) | PollOutcome::Error(_) => {
                if let PollOutcome::Error(m) = &outcome {
                    eprintln!("llama-swap poll error: {m}");
                }
                need_version = true;
                version_retry_at = None;
                backoff.next_delay()
            }
        };
        shared.with_state(|s| s.apply_poll(outcome, Instant::now(), now_ms()));
        shared.publish();
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shared.wake.notified() => {}
        }
    }
}

async fn event_loop(client: LlamaSwapClient, shared: Arc<Shared>) {
    let mut backoff = Backoff::new(BACKOFF_BASE, BACKOFF_CAP);
    loop {
        // A server that accepts TCP but never sends headers must not hang this loop.
        let connected = match tokio::time::timeout(EVENTS_CONNECT_TIMEOUT, client.events()).await {
            Ok(r) => r,
            Err(_) => Err(ClientError::Unreachable("timed out waiting for event stream".into())),
        };
        let delay = match connected {
            Ok(resp) => {
                backoff.reset();
                shared.with_state(|s| s.set_live_events(true));
                shared.publish();
                // Fresh parser per connection: a dropped stream's partial frame must not leak.
                let mut parser = SseParser::new();
                let mut stream = resp.bytes_stream();
                while let Some(Ok(chunk)) = stream.next().await {
                    let mut changed = false;
                    for payload in parser.push(&chunk) {
                        match decode_event(&payload) {
                            Ok(StreamEvent::Inflight(msg)) => {
                                shared.with_state(|s| s.apply_inflight(msg, Instant::now()));
                                changed = true;
                            }
                            Ok(StreamEvent::ModelStatus) => shared.wake.notify_one(),
                            Ok(StreamEvent::Other) | Err(_) => {}
                        }
                    }
                    if changed {
                        shared.publish();
                    }
                }
                shared.with_state(|s| s.set_live_events(false));
                shared.publish();
                backoff.next_delay()
            }
            Err(ClientError::NotFound) => EVENTS_UNAVAILABLE_RETRY,
            Err(_) => backoff.next_delay(),
        };
        tokio::time::sleep(delay).await;
    }
}
