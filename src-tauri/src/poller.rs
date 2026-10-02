//! One poll tick: health first (for latency and reachability), then
//! everything else concurrently.

use std::time::Duration;

use crate::api::{ActivityEntry, ModelEntry, RunningModel, StatsResponse, VersionInfo};
use crate::client::{ClientError, LlamaSwapClient};

pub const ACTIVITY_LIMIT: u32 = 100;

#[derive(Debug, Clone, PartialEq)]
pub enum Feature<T> {
    Available(T),
    /// 404: this llama-swap version does not have the endpoint.
    Unavailable,
    /// Transient failure: keep whatever we had.
    Failed,
}

impl<T> Feature<T> {
    fn from_result(r: Result<T, ClientError>) -> Self {
        match r {
            Ok(v) => Feature::Available(v),
            Err(ClientError::NotFound) => Feature::Unavailable,
            Err(e) => {
                // serde messages describe the JSON shape only; they never contain the key.
                if let ClientError::Decode(m) = &e {
                    eprintln!("llama-swap returned malformed JSON: {m}");
                }
                Feature::Failed
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PollData {
    pub latency: Duration,
    pub running: Vec<RunningModel>,
    pub models: Option<Vec<ModelEntry>>,
    pub version: Option<VersionInfo>,
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
        models: models.and_then(|r| r.ok()),
        version: version.and_then(|r| r.ok()),
        stats: Feature::from_result(stats),
        activity: Feature::from_result(activity),
    })
}
