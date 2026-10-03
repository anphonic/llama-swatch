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
    Busy {
        requests: usize,
        oldest_elapsed_s: u64,
        /// Requests that have produced at least one response byte.
        streaming: usize,
        /// Requests still waiting for their first byte (e.g. for a free slot).
        queued: usize,
        /// Seconds since any streaming request last grew; `None` while nothing streams.
        last_output_s: Option<u64>,
        /// Hint only: a queued request has waited over 10x the first-byte timeout while
        /// others stream. Never changes the state.
        queued_long: bool,
    },
    Stalled { reason: String },
    Unloading,
}

/// A queued request waiting longer than this many first-byte timeouts, while others
/// stream, sets `queued_long`.
const QUEUED_LONG_FACTOR: u64 = 10;

/// Rules are evaluated top to bottom: Stalled, Loading, Unloading, Busy, Idle, NotLoaded.
pub fn derive_state(
    process_state: Option<&str>,
    inflight: &[InflightView],
    since_state_change: Duration,
    t: &Thresholds,
) -> ModelState {
    let in_state_s = since_state_change.as_secs();
    match process_state {
        Some("starting") if in_state_s > t.load_timeout_s => ModelState::Stalled {
            reason: format!("Loading for {in_state_s} s (timeout {} s)", t.load_timeout_s),
        },
        Some("stopping") if in_state_s > t.stop_timeout_s => ModelState::Stalled {
            reason: format!("Unloading for {in_state_s} s (timeout {} s)", t.stop_timeout_s),
        },
        Some("starting") => ModelState::Loading { elapsed_s: in_state_s },
        Some("stopping") => ModelState::Unloading,
        Some("ready") if !inflight.is_empty() => activity(inflight, since_state_change, t),
        Some("ready") => ModelState::Idle { uptime_s: in_state_s },
        _ => ModelState::NotLoaded,
    }
}

/// Model-level stall rules, applied only while the model is `ready` with requests in flight.
/// Liveness is judged for the model, not per request: llama-swap may run several requests in
/// parallel while others wait for a free slot, so one quiet or queued request alone never
/// stalls a model that is still producing output.
///
/// - Something streams: Stalled only when *every* streaming request has been quiet longer than
///   `stream_stall_timeout_s` (a byte on any of them counts as output).
/// - Nothing streams yet: Stalled when the longest wait exceeds `first_byte_timeout_s`.
///
/// A request that queued during loading is timed from when the model became ready
/// (`ready_for`), not from when it arrived.
fn activity(inflight: &[InflightView], ready_for: Duration, t: &Thresholds) -> ModelState {
    let waited = |v: &InflightView| v.elapsed.min(ready_for).as_secs();
    let (streaming, queued): (Vec<&InflightView>, Vec<&InflightView>) =
        inflight.iter().partition(|v| v.resp_bytes > 0);
    let last_output_s = streaming.iter().map(|v| v.since_bytes_change.as_secs()).min();
    let longest_wait = queued.iter().map(|v| waited(v)).max();
    match (last_output_s, longest_wait) {
        (Some(quiet), _) if quiet > t.stream_stall_timeout_s => {
            return ModelState::Stalled { reason: format!("Output stopped {quiet} s ago") };
        }
        (None, Some(wait)) if wait > t.first_byte_timeout_s => {
            return ModelState::Stalled { reason: format!("No response for {wait} s") };
        }
        _ => {}
    }
    let long_limit = t.first_byte_timeout_s.saturating_mul(QUEUED_LONG_FACTOR);
    ModelState::Busy {
        requests: inflight.len(),
        oldest_elapsed_s: inflight.iter().map(|v| v.elapsed.as_secs()).max().unwrap_or(0),
        streaming: streaming.len(),
        queued: queued.len(),
        last_output_s,
        queued_long: last_output_s.is_some() && longest_wait.is_some_and(|w| w > long_limit),
    }
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

    /// Busy with `requests = streaming + queued`.
    fn busy(oldest: u64, streaming: usize, queued: usize, last_output_s: Option<u64>, queued_long: bool) -> ModelState {
        Busy { requests: streaming + queued, oldest_elapsed_s: oldest, streaming, queued, last_output_s, queued_long }
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
            ("unloading ignores requests", Some("stopping"), vec![view(500, 0, 500)], 5, Unloading),
            ("idle", Some("ready"), vec![], 42, Idle { uptime_s: 42 }),
            ("busy one", Some("ready"), vec![view(6, 100, 0)], 300, busy(6, 1, 0, Some(0), false)),
            ("busy two oldest", Some("ready"), vec![view(3, 10, 0), view(9, 50, 1)], 300, busy(9, 2, 0, Some(0), false)),
            ("first byte at threshold", Some("ready"), vec![view(90, 0, 90)], 300, busy(90, 0, 1, None, false)),
            ("no first byte", Some("ready"), vec![view(91, 0, 91)], 300, stalled("No response for 91 s")),
            ("first byte timed from ready", Some("ready"), vec![view(150, 0, 150)], 40, busy(150, 0, 1, None, false)),
            ("stream at threshold", Some("ready"), vec![view(60, 500, 30)], 300, busy(60, 1, 0, Some(30), false)),
            ("stream stalled", Some("ready"), vec![view(60, 500, 31)], 300, stalled("Output stopped 31 s ago")),
            // Was Stalled "No response for 100 s" under per-request rules: a queued request no
            // longer stalls a model that is still streaming.
            ("queued among streaming is busy", Some("ready"), vec![view(5, 100, 0), view(100, 0, 100)], 300, busy(100, 1, 1, Some(0), false)),
            (
                "1 streaming + 2 queued at 200 s",
                Some("ready"),
                vec![view(250, 4_000, 1), view(200, 0, 200), view(200, 0, 200)],
                300,
                busy(250, 1, 2, Some(1), false),
            ),
            (
                "all streams quiet",
                Some("ready"),
                vec![view(60, 100, 31), view(70, 200, 40), view(200, 0, 200)],
                300,
                stalled("Output stopped 31 s ago"),
            ),
            (
                "nothing streams, longest queue over",
                Some("ready"),
                vec![view(50, 0, 50), view(91, 0, 91)],
                300,
                stalled("No response for 91 s"),
            ),
            (
                "one fresh stream keeps a quiet one alive",
                Some("ready"),
                vec![view(120, 100, 0), view(120, 500, 100)],
                300,
                busy(120, 2, 0, Some(0), false),
            ),
            (
                "last output is the min over streams",
                Some("ready"),
                vec![view(30, 100, 12), view(40, 200, 5), view(50, 300, 20), view(10, 0, 10)],
                300,
                busy(50, 3, 1, Some(5), false),
            ),
            (
                "multi-stream at threshold",
                Some("ready"),
                vec![view(60, 100, 30), view(80, 200, 45)],
                300,
                busy(80, 2, 0, Some(30), false),
            ),
            ("queued long", Some("ready"), vec![view(1000, 100, 2), view(901, 0, 901)], 2000, busy(1000, 1, 1, Some(2), true)),
            ("queued long at threshold", Some("ready"), vec![view(1000, 100, 2), view(900, 0, 900)], 2000, busy(1000, 1, 1, Some(2), false)),
            ("queued long timed from ready", Some("ready"), vec![view(1000, 100, 2), view(1000, 0, 1000)], 500, busy(1000, 1, 1, Some(2), false)),
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
    fn queued_long_scales_with_first_byte_timeout() {
        // 10 x 20 s = 200 s, not the default 900 s.
        let t = Thresholds { first_byte_timeout_s: 20, ..Thresholds::default() };
        let long = derive_state(Some("ready"), &[view(500, 100, 1), view(201, 0, 201)], secs(1000), &t);
        assert_eq!(long, busy(500, 1, 1, Some(1), true));
        let not_yet = derive_state(Some("ready"), &[view(500, 100, 1), view(200, 0, 200)], secs(1000), &t);
        assert_eq!(not_yet, busy(500, 1, 1, Some(1), false));
    }

    #[test]
    fn serializes_for_the_frontend() {
        let v = serde_json::to_value(busy(9, 1, 1, Some(3), true)).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind":"busy","requests":2,"oldestElapsedS":9,"streaming":1,"queued":1,"lastOutputS":3,"queuedLong":true})
        );
        let v = serde_json::to_value(busy(4, 0, 2, None, false)).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind":"busy","requests":2,"oldestElapsedS":4,"streaming":0,"queued":2,"lastOutputS":null,"queuedLong":false})
        );
        let v = serde_json::to_value(NotLoaded).unwrap();
        assert_eq!(v, serde_json::json!({"kind":"notLoaded"}));
        let v = serde_json::to_value(Thresholds::default()).unwrap();
        assert_eq!(v, serde_json::json!({"loadTimeoutS":120,"stopTimeoutS":30,"firstByteTimeoutS":90,"streamStallTimeoutS":30}));
    }
}
