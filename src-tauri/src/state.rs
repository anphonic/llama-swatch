//! Pure model-state derivation. No I/O and no clocks: callers pass durations in.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::events::InflightView;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Thresholds {
    /// Slow-load warning: a load past this many seconds is flagged `slow`. It never makes
    /// the model Stalled; llama-swap's own health-check timeout decides whether a load fails.
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
    /// `slow`: loading has lasted longer than `load_timeout_s`. A hint only.
    Loading { elapsed_s: u64, slow: bool },
    Idle { uptime_s: u64 },
    Busy {
        requests: usize,
        oldest_elapsed_s: u64,
        /// Requests that have produced at least one response byte.
        streaming: usize,
        /// Requests whose reply has started (headers sent) but has no output yet. Usually
        /// brief: llama.cpp's server sends headers only with its first result, even when
        /// streaming, so this group shows up e.g. with other backends.
        waiting_first_token: usize,
        /// Requests with no reply at all yet: a non-streaming chat (`stream: false`) or an
        /// embeddings request until it finishes, and with llama.cpp also a streaming request
        /// still processing its prompt. These never stall a model.
        awaiting_reply: usize,
        /// Seconds since any streaming request last grew; `None` while nothing streams.
        last_output_s: Option<u64>,
        /// Hint only: a request has waited for its first token over 10x the first-byte
        /// timeout while others stream. Never changes the state.
        first_token_long: bool,
        /// Hint only: a request has had no reply for longer than the first-byte timeout.
        /// Never changes the state.
        awaiting_long: bool,
    },
    Stalled { reason: String },
    Unloading,
}

/// A request waiting for its first token longer than this many first-byte timeouts, while
/// others stream, sets `first_token_long`.
const FIRST_TOKEN_LONG_FACTOR: u64 = 10;

/// Rules are evaluated top to bottom: Stalled (unloading past its timeout), Loading (never
/// Stalled, only flagged `slow`), Unloading, Busy (may be Stalled), Idle, NotLoaded.
pub fn derive_state(
    process_state: Option<&str>,
    inflight: &[InflightView],
    since_state_change: Duration,
    t: &Thresholds,
) -> ModelState {
    let in_state_s = since_state_change.as_secs();
    match process_state {
        Some("stopping") if in_state_s > t.stop_timeout_s => ModelState::Stalled {
            reason: format!("Unloading for {in_state_s} s (timeout {} s)", t.stop_timeout_s),
        },
        Some("starting") => ModelState::Loading { elapsed_s: in_state_s, slow: in_state_s > t.load_timeout_s },
        Some("stopping") => ModelState::Unloading,
        Some("ready") if !inflight.is_empty() => activity(inflight, since_state_change, t),
        Some("ready") => ModelState::Idle { uptime_s: in_state_s },
        _ => ModelState::NotLoaded,
    }
}

/// Model-level stall rules, applied only while the model is `ready` with requests in flight.
/// Liveness is judged for the model, not per request: llama-swap may run several requests in
/// parallel while others wait for a free slot, so one quiet or waiting request alone never
/// stalls a model that is still producing output.
///
/// Requests split three ways: *streaming* (bytes received), *started* (reply headers sent, no
/// bytes yet) and *awaiting* (no headers, no bytes).
///
/// - Something streams: Stalled only when *every* streaming request has been quiet longer than
///   `stream_stall_timeout_s` (a byte on any of them counts as output).
/// - Nothing streams yet: Stalled when the longest *started* wait exceeds `first_byte_timeout_s`.
/// - *Awaiting* requests never stall: a non-streaming reply sends its headers and body together
///   when it is done, and llama.cpp holds back headers until its first result even when
///   streaming, so the monitor cannot tell one still working from one that is stuck. Past
///   `first_byte_timeout_s` they only set the `awaiting_long` hint.
///
/// So `first_byte_timeout_s` makes a model Stalled only for replies that have started; for
/// awaiting ones it is the hint threshold.
///
/// A request that queued during loading is timed from when the model became ready
/// (`ready_for`), not from when it arrived.
fn activity(inflight: &[InflightView], ready_for: Duration, t: &Thresholds) -> ModelState {
    let waited = |v: &InflightView| v.elapsed.min(ready_for).as_secs();
    let (streaming, waiting): (Vec<&InflightView>, Vec<&InflightView>) =
        inflight.iter().partition(|v| v.resp_bytes > 0);
    let (started, awaiting): (Vec<&InflightView>, Vec<&InflightView>) =
        waiting.into_iter().partition(|v| v.response_started);
    let last_output_s = streaming.iter().map(|v| v.since_bytes_change.as_secs()).min();
    let longest_started = started.iter().map(|v| waited(v)).max();
    let longest_awaiting = awaiting.iter().map(|v| waited(v)).max();
    match (last_output_s, longest_started) {
        (Some(quiet), _) if quiet > t.stream_stall_timeout_s => {
            return ModelState::Stalled { reason: format!("Output stopped {quiet} s ago") };
        }
        (None, Some(wait)) if wait > t.first_byte_timeout_s => {
            return ModelState::Stalled { reason: format!("No first token after {wait} s (reply started)") };
        }
        _ => {}
    }
    let long_limit = t.first_byte_timeout_s.saturating_mul(FIRST_TOKEN_LONG_FACTOR);
    ModelState::Busy {
        requests: inflight.len(),
        oldest_elapsed_s: inflight.iter().map(|v| v.elapsed.as_secs()).max().unwrap_or(0),
        streaming: streaming.len(),
        waiting_first_token: started.len(),
        awaiting_reply: awaiting.len(),
        last_output_s,
        first_token_long: last_output_s.is_some() && longest_started.is_some_and(|w| w > long_limit),
        awaiting_long: longest_awaiting.is_some_and(|w| w > t.first_byte_timeout_s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ModelState::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    /// A request with output (`bytes > 0` implies the reply headers were sent).
    fn view(elapsed: u64, bytes: i64, since_change: u64) -> InflightView {
        InflightView {
            elapsed: secs(elapsed),
            resp_bytes: bytes,
            since_bytes_change: secs(since_change),
            response_started: bytes > 0,
        }
    }

    /// Reply headers sent, no output yet: waiting for the first token.
    fn started(elapsed: u64) -> InflightView {
        InflightView { response_started: true, ..view(elapsed, 0, elapsed) }
    }

    /// No reply headers and no output: possibly a non-streaming request still generating.
    fn awaiting(elapsed: u64) -> InflightView {
        view(elapsed, 0, elapsed)
    }

    fn stalled(reason: &str) -> ModelState {
        Stalled { reason: reason.into() }
    }

    fn no_first_token(wait: u64) -> ModelState {
        stalled(&format!("No first token after {wait} s (reply started)"))
    }

    /// Busy with `requests = streaming + first + awaiting`.
    fn busy(
        oldest: u64,
        streaming: usize,
        first: usize,
        awaiting: usize,
        last_output_s: Option<u64>,
        first_long: bool,
        awaiting_long: bool,
    ) -> ModelState {
        Busy {
            requests: streaming + first + awaiting,
            oldest_elapsed_s: oldest,
            streaming,
            waiting_first_token: first,
            awaiting_reply: awaiting,
            last_output_s,
            first_token_long: first_long,
            awaiting_long,
        }
    }

    /// (name, process state, in-flight requests, seconds in state, expected).
    type Case = (&'static str, Option<&'static str>, Vec<InflightView>, u64, ModelState);

    #[test]
    fn table() {
        let t = Thresholds::default();
        let cases: Vec<Case> = vec![
            ("absent", None, vec![], 0, NotLoaded),
            ("stopped", Some("stopped"), vec![], 5, NotLoaded),
            ("shutdown", Some("shutdown"), vec![], 5, NotLoaded),
            ("unknown state", Some("weird"), vec![], 5, NotLoaded),
            ("loading", Some("starting"), vec![], 10, Loading { elapsed_s: 10, slow: false }),
            ("loading at threshold", Some("starting"), vec![], 120, Loading { elapsed_s: 120, slow: false }),
            ("loading over", Some("starting"), vec![], 121, Loading { elapsed_s: 121, slow: true }),
            ("loading ignores waiting request", Some("starting"), vec![awaiting(200)], 100, Loading { elapsed_s: 100, slow: false }),
            ("unloading", Some("stopping"), vec![], 5, Unloading),
            ("unloading over", Some("stopping"), vec![], 31, stalled("Unloading for 31 s (timeout 30 s)")),
            ("unloading ignores requests", Some("stopping"), vec![awaiting(500)], 5, Unloading),
            ("idle", Some("ready"), vec![], 42, Idle { uptime_s: 42 }),
            ("busy one", Some("ready"), vec![view(6, 100, 0)], 300, busy(6, 1, 0, 0, Some(0), false, false)),
            ("busy two oldest", Some("ready"), vec![view(3, 10, 0), view(9, 50, 1)], 300, busy(9, 2, 0, 0, Some(0), false, false)),
            // Reply started (headers sent), no token yet.
            ("first token at threshold", Some("ready"), vec![started(90)], 300, busy(90, 0, 1, 0, None, false, false)),
            ("no first token", Some("ready"), vec![started(91)], 300, no_first_token(91)),
            ("first token timed from ready", Some("ready"), vec![started(150)], 40, busy(150, 0, 1, 0, None, false, false)),
            // No headers and no bytes: a stream:false chat or an embeddings request sends nothing
            // until it is done, so a long wait is only a hint, never Stalled.
            ("awaiting reply", Some("ready"), vec![awaiting(30)], 300, busy(30, 0, 0, 1, None, false, false)),
            ("awaiting at threshold", Some("ready"), vec![awaiting(90)], 300, busy(90, 0, 0, 1, None, false, false)),
            ("awaiting past threshold is a hint", Some("ready"), vec![awaiting(91)], 300, busy(91, 0, 0, 1, None, false, true)),
            ("awaiting very long is still busy", Some("ready"), vec![awaiting(5_000)], 9_000, busy(5_000, 0, 0, 1, None, false, true)),
            ("awaiting timed from ready", Some("ready"), vec![awaiting(150)], 40, busy(150, 0, 0, 1, None, false, false)),
            ("stream at threshold", Some("ready"), vec![view(60, 500, 30)], 300, busy(60, 1, 0, 0, Some(30), false, false)),
            ("stream stalled", Some("ready"), vec![view(60, 500, 31)], 300, stalled("Output stopped 31 s ago")),
            // A request waiting for its first token does not stall a model that is still streaming.
            ("waiting among streaming is busy", Some("ready"), vec![view(5, 100, 0), started(100)], 300, busy(100, 1, 1, 0, Some(0), false, false)),
            (
                "1 streaming + 2 waiting at 200 s",
                Some("ready"),
                vec![view(250, 4_000, 1), started(200), started(200)],
                300,
                busy(250, 1, 2, 0, Some(1), false, false),
            ),
            (
                "streaming + awaiting past threshold",
                Some("ready"),
                vec![view(20, 4_000, 1), awaiting(120)],
                300,
                busy(120, 1, 0, 1, Some(1), false, true),
            ),
            (
                "streaming + started + awaiting",
                Some("ready"),
                vec![view(20, 4_000, 2), started(10), awaiting(40)],
                300,
                busy(40, 1, 1, 1, Some(2), false, false),
            ),
            (
                "all streams quiet",
                Some("ready"),
                vec![view(60, 100, 31), view(70, 200, 40), started(200)],
                300,
                stalled("Output stopped 31 s ago"),
            ),
            (
                "quiet streams stall even beside an awaiting reply",
                Some("ready"),
                vec![view(60, 100, 31), awaiting(40)],
                300,
                stalled("Output stopped 31 s ago"),
            ),
            (
                "nothing streams, longest first-token wait over",
                Some("ready"),
                vec![started(50), started(91)],
                300,
                no_first_token(91),
            ),
            (
                "started over threshold stalls beside an awaiting reply",
                Some("ready"),
                vec![started(91), awaiting(500)],
                600,
                no_first_token(91),
            ),
            (
                "awaiting over threshold does not stall beside a waiting start",
                Some("ready"),
                vec![started(50), awaiting(500)],
                600,
                busy(500, 0, 1, 1, None, false, true),
            ),
            (
                "one fresh stream keeps a quiet one alive",
                Some("ready"),
                vec![view(120, 100, 0), view(120, 500, 100)],
                300,
                busy(120, 2, 0, 0, Some(0), false, false),
            ),
            (
                "last output is the min over streams",
                Some("ready"),
                vec![view(30, 100, 12), view(40, 200, 5), view(50, 300, 20), started(10)],
                300,
                busy(50, 3, 1, 0, Some(5), false, false),
            ),
            (
                "multi-stream at threshold",
                Some("ready"),
                vec![view(60, 100, 30), view(80, 200, 45)],
                300,
                busy(80, 2, 0, 0, Some(30), false, false),
            ),
            ("first token long", Some("ready"), vec![view(1000, 100, 2), started(901)], 2000, busy(1000, 1, 1, 0, Some(2), true, false)),
            ("first token long at threshold", Some("ready"), vec![view(1000, 100, 2), started(900)], 2000, busy(1000, 1, 1, 0, Some(2), false, false)),
            ("first token long timed from ready", Some("ready"), vec![view(1000, 100, 2), started(1000)], 500, busy(1000, 1, 1, 0, Some(2), false, false)),
        ];
        for (name, ps, inflight, since, expected) in cases {
            assert_eq!(derive_state(ps, &inflight, secs(since), &t), expected, "case: {name}");
        }
    }

    #[test]
    fn custom_thresholds_are_respected() {
        let t = Thresholds { load_timeout_s: 10, stop_timeout_s: 5, first_byte_timeout_s: 3, stream_stall_timeout_s: 2 };
        assert_eq!(derive_state(Some("starting"), &[], secs(11), &t), Loading { elapsed_s: 11, slow: true });
        assert_eq!(derive_state(Some("stopping"), &[], secs(6), &t), stalled("Unloading for 6 s (timeout 5 s)"));
        assert_eq!(derive_state(Some("ready"), &[started(4)], secs(60), &t), no_first_token(4));
        assert_eq!(derive_state(Some("ready"), &[awaiting(4)], secs(60), &t), busy(4, 0, 0, 1, None, false, true));
        assert_eq!(derive_state(Some("ready"), &[view(9, 5, 3)], secs(60), &t), stalled("Output stopped 3 s ago"));
    }

    #[test]
    fn first_token_long_scales_with_first_byte_timeout() {
        // 10 x 20 s = 200 s, not the default 900 s.
        let t = Thresholds { first_byte_timeout_s: 20, ..Thresholds::default() };
        let long = derive_state(Some("ready"), &[view(500, 100, 1), started(201)], secs(1000), &t);
        assert_eq!(long, busy(500, 1, 1, 0, Some(1), true, false));
        let not_yet = derive_state(Some("ready"), &[view(500, 100, 1), started(200)], secs(1000), &t);
        assert_eq!(not_yet, busy(500, 1, 1, 0, Some(1), false, false));
    }

    #[test]
    fn serializes_for_the_frontend() {
        let v = serde_json::to_value(busy(9, 1, 1, 1, Some(3), true, true)).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind":"busy","requests":3,"oldestElapsedS":9,"streaming":1,"waitingFirstToken":1,"awaitingReply":1,"lastOutputS":3,"firstTokenLong":true,"awaitingLong":true})
        );
        let v = serde_json::to_value(busy(4, 0, 2, 0, None, false, false)).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind":"busy","requests":2,"oldestElapsedS":4,"streaming":0,"waitingFirstToken":2,"awaitingReply":0,"lastOutputS":null,"firstTokenLong":false,"awaitingLong":false})
        );
        let v = serde_json::to_value(Loading { elapsed_s: 121, slow: true }).unwrap();
        assert_eq!(v, serde_json::json!({"kind":"loading","elapsedS":121,"slow":true}));
        let v = serde_json::to_value(NotLoaded).unwrap();
        assert_eq!(v, serde_json::json!({"kind":"notLoaded"}));
        let v = serde_json::to_value(Thresholds::default()).unwrap();
        assert_eq!(v, serde_json::json!({"loadTimeoutS":120,"stopTimeoutS":30,"firstByteTimeoutS":90,"streamStallTimeoutS":30}));
    }
}
