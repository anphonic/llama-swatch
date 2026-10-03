//! Pure model-state derivation. No I/O and no clocks: callers pass durations in.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::events::InflightView;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Thresholds {
    pub load_timeout_s: u64,
    pub stop_timeout_s: u64,
    pub first_byte_timeout_s: u64,
    pub stream_stall_timeout_s: u64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self { load_timeout_s: 120, stop_timeout_s: 30, first_byte_timeout_s: 90, stream_stall_timeout_s: 30 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ModelState {
    NotLoaded,
    Loading { elapsed_s: u64 },
    Idle { uptime_s: u64 },
    Busy { requests: usize, oldest_elapsed_s: u64 },
    Stalled { reason: String },
    Unloading,
}

/// Rules are evaluated top to bottom: Stalled, Loading, Unloading, Busy, Idle, NotLoaded.
pub fn derive_state(
    process_state: Option<&str>,
    inflight: &[InflightView],
    since_state_change: Duration,
    t: &Thresholds,
) -> ModelState {
    let in_state_s = since_state_change.as_secs();
    match process_state {
        Some("starting") if in_state_s > t.load_timeout_s => {
            return ModelState::Stalled {
                reason: format!("Loading for {in_state_s} s (timeout {} s)", t.load_timeout_s),
            };
        }
        Some("stopping") if in_state_s > t.stop_timeout_s => {
            return ModelState::Stalled {
                reason: format!("Unloading for {in_state_s} s (timeout {} s)", t.stop_timeout_s),
            };
        }
        Some("ready") => {
            if let Some(reason) = stalled_request(inflight, since_state_change, t) {
                return ModelState::Stalled { reason };
            }
        }
        _ => {}
    }
    match process_state {
        Some("starting") => ModelState::Loading { elapsed_s: in_state_s },
        Some("stopping") => ModelState::Unloading,
        Some("ready") if !inflight.is_empty() => ModelState::Busy {
            requests: inflight.len(),
            oldest_elapsed_s: inflight.iter().map(|v| v.elapsed.as_secs()).max().unwrap_or(0),
        },
        Some("ready") => ModelState::Idle { uptime_s: in_state_s },
        _ => ModelState::NotLoaded,
    }
}

/// Request-level stall rules, applied only while the model is `ready`.
/// A request that queued during loading is timed from when the model became
/// ready (`ready_for`), not from when it arrived.
fn stalled_request(inflight: &[InflightView], ready_for: Duration, t: &Thresholds) -> Option<String> {
    for v in inflight {
        if v.resp_bytes <= 0 {
            let waiting = v.elapsed.min(ready_for).as_secs();
            if waiting > t.first_byte_timeout_s {
                return Some(format!("No response for {waiting} s"));
            }
        } else {
            let quiet = v.since_bytes_change.as_secs();
            if quiet > t.stream_stall_timeout_s {
                return Some(format!("Output stopped {quiet} s ago"));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ModelState::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    fn view(elapsed: u64, bytes: i64, since_change: u64) -> InflightView {
        InflightView { elapsed: secs(elapsed), resp_bytes: bytes, since_bytes_change: secs(since_change) }
    }

    fn stalled(reason: &str) -> ModelState {
        Stalled { reason: reason.into() }
    }

    #[test]
    fn table() {
        let t = Thresholds::default();
        let cases: Vec<(&str, Option<&str>, Vec<InflightView>, u64, ModelState)> = vec![
            ("absent", None, vec![], 0, NotLoaded),
            ("stopped", Some("stopped"), vec![], 5, NotLoaded),
            ("shutdown", Some("shutdown"), vec![], 5, NotLoaded),
            ("unknown state", Some("weird"), vec![], 5, NotLoaded),
            ("loading", Some("starting"), vec![], 10, Loading { elapsed_s: 10 }),
            ("loading at threshold", Some("starting"), vec![], 120, Loading { elapsed_s: 120 }),
            ("loading over", Some("starting"), vec![], 121, stalled("Loading for 121 s (timeout 120 s)")),
            ("loading ignores queued request", Some("starting"), vec![view(200, 0, 200)], 100, Loading { elapsed_s: 100 }),
            ("unloading", Some("stopping"), vec![], 5, Unloading),
            ("unloading over", Some("stopping"), vec![], 31, stalled("Unloading for 31 s (timeout 30 s)")),
            ("idle", Some("ready"), vec![], 42, Idle { uptime_s: 42 }),
            ("busy one", Some("ready"), vec![view(6, 100, 0)], 300, Busy { requests: 1, oldest_elapsed_s: 6 }),
            ("busy two oldest", Some("ready"), vec![view(3, 10, 0), view(9, 50, 1)], 300, Busy { requests: 2, oldest_elapsed_s: 9 }),
            ("first byte at threshold", Some("ready"), vec![view(90, 0, 90)], 300, Busy { requests: 1, oldest_elapsed_s: 90 }),
            ("no first byte", Some("ready"), vec![view(91, 0, 91)], 300, stalled("No response for 91 s")),
            ("first byte timed from ready", Some("ready"), vec![view(150, 0, 150)], 40, Busy { requests: 1, oldest_elapsed_s: 150 }),
            ("stream at threshold", Some("ready"), vec![view(60, 500, 30)], 300, Busy { requests: 1, oldest_elapsed_s: 60 }),
            ("stream stalled", Some("ready"), vec![view(60, 500, 31)], 300, stalled("Output stopped 31 s ago")),
            ("one bad among healthy", Some("ready"), vec![view(5, 100, 0), view(100, 0, 100)], 300, stalled("No response for 100 s")),
        ];
        for (name, ps, inflight, since, expected) in cases {
            assert_eq!(derive_state(ps, &inflight, secs(since), &t), expected, "case: {name}");
        }
    }

    #[test]
    fn custom_thresholds_are_respected() {
        let t = Thresholds { load_timeout_s: 10, stop_timeout_s: 5, first_byte_timeout_s: 3, stream_stall_timeout_s: 2 };
        assert_eq!(derive_state(Some("starting"), &[], secs(11), &t), stalled("Loading for 11 s (timeout 10 s)"));
        assert_eq!(derive_state(Some("ready"), &[view(4, 0, 4)], secs(60), &t), stalled("No response for 4 s"));
        assert_eq!(derive_state(Some("ready"), &[view(9, 5, 3)], secs(60), &t), stalled("Output stopped 3 s ago"));
    }

    #[test]
    fn serializes_for_the_frontend() {
        let v = serde_json::to_value(Busy { requests: 2, oldest_elapsed_s: 9 }).unwrap();
        assert_eq!(v, serde_json::json!({"kind":"busy","requests":2,"oldestElapsedS":9}));
        let v = serde_json::to_value(NotLoaded).unwrap();
        assert_eq!(v, serde_json::json!({"kind":"notLoaded"}));
        let v = serde_json::to_value(Thresholds::default()).unwrap();
        assert_eq!(v, serde_json::json!({"loadTimeoutS":120,"stopTimeoutS":30,"firstByteTimeoutS":90,"streamStallTimeoutS":30}));
    }
}
