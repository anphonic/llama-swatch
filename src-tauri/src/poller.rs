//! One poll tick: health first (for latency and reachability), then
//! everything else concurrently.

use std::time::Duration;

use crate::api::{ActivityEntry, ModelEntry, RunningModel, StatsResponse, VersionInfo};
use crate::client::{ClientError, LlamaSwapClient};

pub const ACTIVITY_LIMIT: u32 = 100;

#[derive(Debug, Clone, PartialEq)]
pub enum Feature<T> {
    Available(T),
    /// 404 (or, for `/api/version`, an undecodable body): not usable, do not retry.
    Unavailable,
    /// Transient failure: keep whatever we had.
    Failed,
}

impl<T> Feature<T> {
    /// Classifies an optional endpoint. Unauthorized/Unreachable here are
    /// intentionally `Failed` (keep the previous value): `/running` carries the
    /// auth and reachability signal for the whole tick.
    fn from_result(endpoint: &str, r: Result<T, ClientError>) -> Self {
        match r {
            Ok(v) => Feature::Available(v),
            Err(ClientError::NotFound) => Feature::Unavailable,
            Err(e) => {
                // serde messages describe the JSON shape only; they never contain the key.
                log_decode(endpoint, &e);
                Feature::Failed
            }
        }
    }
}

/// `/api/version` classification. A malformed body (a proxy answering 200 with HTML, a
/// changed shape) will not fix itself, so it is final like a 404; only transport errors
/// and non-404 HTTP errors stay `Failed` (retried).
fn version_feature(r: Result<VersionInfo, ClientError>) -> Feature<VersionInfo> {
    match r {
        Err(ClientError::Decode(m)) => {
            log_decode("/api/version", &ClientError::Decode(m));
            Feature::Unavailable
        }
        other => Feature::from_result("/api/version", other),
    }
}

fn log_decode(endpoint: &str, e: &ClientError) {
    // serde messages describe the JSON shape only; they never contain the key.
    if let ClientError::Decode(m) = e {
        eprintln!("llama-swap {endpoint} returned malformed JSON: {m}");
    }
}

/// Best-effort: errors become `None` (logged if malformed).
fn best_effort<T>(endpoint: &str, r: Option<Result<T, ClientError>>) -> Option<T> {
    match r? {
        Ok(v) => Some(v),
        Err(e) => {
            log_decode(endpoint, &e);
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PollData {
    pub latency: Duration,
    pub running: Vec<RunningModel>,
    pub models: Option<Vec<ModelEntry>>,
    /// `None` when not requested this tick; otherwise the outcome of the fetch.
    pub version: Option<Feature<VersionInfo>>,
    pub stats: Feature<StatsResponse>,
    pub activity: Feature<Vec<ActivityEntry>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PollOutcome {
    Ok(PollData),
    Unauthorized,
    Unreachable(String),
    Error(String),
}

pub async fn poll_once(client: &LlamaSwapClient, want_models: bool, want_version: bool) -> PollOutcome {
    let latency = match client.health().await {
        Ok((_, latency)) => latency,
        Err(ClientError::Unreachable(m)) => return PollOutcome::Unreachable(m),
        Err(ClientError::Unauthorized) => return PollOutcome::Unauthorized,
        Err(e) => return PollOutcome::Error(format!("/health: {e}")),
    };
    let (running, stats, activity, models, version) = tokio::join!(
        client.running(),
        client.stats(),
        client.activity(ACTIVITY_LIMIT),
        async {
            if want_models {
                Some(client.models().await)
            } else {
                None
            }
        },
        async {
            if want_version {
                Some(client.version().await)
            } else {
                None
            }
        },
    );
    let running = match running {
        Ok(r) => r,
        Err(ClientError::Unauthorized) => return PollOutcome::Unauthorized,
        Err(ClientError::Unreachable(m)) => return PollOutcome::Unreachable(m),
        Err(e) => return PollOutcome::Error(format!("/running: {e}")),
    };
    PollOutcome::Ok(PollData {
        latency,
        running,
        models: best_effort("/v1/models", models),
        version: version.map(version_feature),
        stats: Feature::from_result("/api/metrics/stats", stats),
        activity: Feature::from_result("/api/metrics/activity", activity),
    })
}
