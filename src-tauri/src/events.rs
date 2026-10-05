//! Decoding of llama-swap `/api/events` messages and the in-flight request table.
//!
//! Wire format per frame: `data:{"type":"inflight","data":"<JSON string>"}`.
//! The inner `data` is itself JSON encoded as a string.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Deserializer, Serialize};

use crate::api::null_default;

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    data: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct InflightEntry {
    pub id: String,
    /// Resolved model ID (never an alias).
    pub model: String,
    pub req_path: String,
    pub method: String,
    pub resp_bytes: i64,
    pub elapsed_ms: i64,
    /// llama-swap's `resp_headers` is non-empty: the proxied reply has sent its headers.
    /// Only this flag is kept; header names and values are dropped while decoding.
    #[serde(rename = "resp_headers", deserialize_with = "non_empty_object")]
    pub response_started: bool,
}

/// `true` for a non-empty JSON object. Missing (via `#[serde(default)]`), `null`, `{}` and
/// any unexpected shape are `false`, so an odd value never drops the whole entry.
fn non_empty_object<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(matches!(serde_json::Value::deserialize(d)?, serde_json::Value::Object(m) if !m.is_empty()))
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct InflightMsg {
    /// `snapshot`, `upsert` or `remove`.
    pub operation: String,
    #[serde(deserialize_with = "null_default")]
    pub requests: Vec<InflightEntry>,
    pub request: Option<InflightEntry>,
    pub id: String,
}

#[derive(Debug, PartialEq)]
pub enum StreamEvent {
    /// Some model changed state; the caller should re-poll `/running` now.
    ModelStatus,
    Inflight(InflightMsg),
    Other,
}

pub fn decode_event(payload: &str) -> Result<StreamEvent, serde_json::Error> {
    let env: Envelope = serde_json::from_str(payload)?;
    Ok(match env.kind.as_str() {
        "modelStatus" => StreamEvent::ModelStatus,
        "inflight" => StreamEvent::Inflight(serde_json::from_str(&env.data)?),
        _ => StreamEvent::Other,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InflightView {
    pub elapsed: Duration,
    pub resp_bytes: i64,
    pub since_bytes_change: Duration,
    /// Reply headers have been seen (see `InflightEntry::response_started`).
    pub response_started: bool,
}

#[derive(Debug, Clone)]
struct Tracked {
    entry: InflightEntry,
    started: Instant,
    last_bytes_change: Instant,
}

/// In-flight requests keyed by llama-swap request ID. Times are local
/// `Instant`s, so server/client clock skew never matters.
#[derive(Debug, Default)]
pub struct InflightTable {
    entries: HashMap<String, Tracked>,
}

impl InflightTable {
    pub fn apply(&mut self, msg: InflightMsg, now: Instant) {
        match msg.operation.as_str() {
            "snapshot" => {
                let old = std::mem::take(&mut self.entries);
                for e in msg.requests {
                    let prev = old.get(&e.id).cloned();
                    self.upsert(e, prev, now);
                }
            }
            "upsert" => {
                if let Some(e) = msg.request {
                    let prev = self.entries.remove(&e.id);
                    self.upsert(e, prev, now);
                }
            }
            "remove" => {
                self.entries.remove(&msg.id);
            }
            _ => {}
        }
    }

    fn upsert(&mut self, e: InflightEntry, prev: Option<Tracked>, now: Instant) {
        let tracked = match prev {
            Some(p) => Tracked {
                last_bytes_change: if e.resp_bytes != p.entry.resp_bytes { now } else { p.last_bytes_change },
                started: p.started,
                entry: e,
            },
            None => {
                let age = Duration::from_millis(e.elapsed_ms.max(0) as u64);
                Tracked { started: now.checked_sub(age).unwrap_or(now), last_bytes_change: now, entry: e }
            }
        };
        self.entries.insert(tracked.entry.id.clone(), tracked);
    }

    pub fn views_for(&self, model: &str, now: Instant) -> Vec<InflightView> {
        self.entries
            .values()
            .filter(|t| t.entry.model == model)
            .map(|t| InflightView {
                elapsed: now.saturating_duration_since(t.started),
                resp_bytes: t.entry.resp_bytes,
                since_bytes_change: now.saturating_duration_since(t.last_bytes_change),
                response_started: t.entry.response_started,
            })
            .collect()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// What one frame says about whether this monitor can read the in-flight data the cards need:
/// `Some(true)` proves it can, `Some(false)` is a decode failure, `None` proves nothing.
///
/// Only an inflight `snapshot`, or an `upsert` carrying an entry, counts as success: those are
/// the frames whose entries had to decode. `remove` (just an id), `modelStatus` and other event
/// types are neutral. If they counted, a llama-swap that changed only the entry shape would keep
/// the stream `Live` on removes alone, with an empty table showing Idle while models serve.
pub fn health_signal(r: &Result<StreamEvent, serde_json::Error>) -> Option<bool> {
    match r {
        Ok(StreamEvent::Inflight(m)) => match m.operation.as_str() {
            "snapshot" => Some(true),
            "upsert" if m.request.is_some() => Some(true),
            _ => None,
        },
        Ok(StreamEvent::ModelStatus | StreamEvent::Other) => None,
        Err(_) => Some(false),
    }
}

/// Health of the `/api/events` stream, as shown in the header badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EventStream {
    /// Not connected, or the server has no `/api/events`.
    Offline,
    /// Stream open, no in-flight snapshot or upsert decoded yet on this connection.
    Connected,
    /// At least one in-flight snapshot or upsert decoded on this connection, and fewer than
    /// [`UNREADABLE_AFTER`] decode failures since the last success.
    Live,
    /// [`UNREADABLE_AFTER`] or more events in a row on this connection failed to decode.
    Unreadable,
}

/// Consecutive decode failures that make the stream `Unreadable`. One or two stray bad frames
/// change nothing; a third in a row means this llama-swap sends events we cannot read.
pub const UNREADABLE_AFTER: u32 = 3;

/// Per-connection decode bookkeeping behind [`EventStream`]. Holds only counts: payload and
/// error text never reach it (payloads can carry request headers).
///
/// Fed by [`health_signal`]: a success is an inflight `snapshot` or `upsert` whose entries
/// decoded; a failure is a frame that did not decode. Everything else (`remove`, `modelStatus`,
/// logs, metrics) is neutral: it neither resets nor adds to the failure streak, because it
/// proves nothing about the entry data the cards need.
#[derive(Debug, Default)]
pub struct DecodeHealth {
    connected: bool,
    decoded: bool,
    consecutive_failures: u32,
}

impl DecodeHealth {
    /// A new connection: all counts start over.
    pub fn connect(&mut self) {
        *self = Self { connected: true, ..Self::default() };
    }

    pub fn disconnect(&mut self) {
        *self = Self::default();
    }

    /// Records one decode attempt of an event the monitor reads.
    pub fn record(&mut self, ok: bool) {
        if ok {
            self.decoded = true;
            self.consecutive_failures = 0;
        } else {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        }
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    pub fn state(&self) -> EventStream {
        if !self.connected {
            EventStream::Offline
        } else if self.consecutive_failures >= UNREADABLE_AFTER {
            EventStream::Unreadable
        } else if self.decoded {
            EventStream::Live
        } else {
            EventStream::Connected
        }
    }

    /// Whether "no requests in flight" can be believed, i.e. a ready model may show as Idle.
    /// `Connected` counts while nothing has failed to decode: llama-swap may send nothing while
    /// idle, and the first request's events either decode (then `Live`) or fail (then this turns
    /// false at once, before `Unreadable`). Offline and Unreadable never do.
    pub fn sees_activity(&self) -> bool {
        match self.state() {
            EventStream::Live => true,
            EventStream::Connected => self.consecutive_failures == 0,
            EventStream::Offline | EventStream::Unreadable => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn envelope(kind: &str, data: serde_json::Value) -> String {
        json!({ "type": kind, "data": data.to_string() }).to_string()
    }

    fn entry(id: &str, model: &str, bytes: i64, elapsed_ms: i64) -> InflightEntry {
        InflightEntry { id: id.into(), model: model.into(), resp_bytes: bytes, elapsed_ms, ..Default::default() }
    }

    fn upsert(e: InflightEntry) -> InflightMsg {
        InflightMsg { operation: "upsert".into(), request: Some(e), ..Default::default() }
    }

    #[test]
    fn decodes_inflight_snapshot_with_string_encoded_data() {
        let payload = envelope(
            "inflight",
            json!({"operation":"snapshot","requests":[{"id":"1","model":"qwen","req_path":"/v1/chat/completions","method":"POST","resp_bytes":0,"elapsed_ms":1200,"req_headers":{}}]}),
        );
        match decode_event(&payload).unwrap() {
            StreamEvent::Inflight(m) => {
                assert_eq!(m.operation, "snapshot");
                assert_eq!(m.requests, vec![InflightEntry {
                    id: "1".into(), model: "qwen".into(), req_path: "/v1/chat/completions".into(),
                    method: "POST".into(), resp_bytes: 0, elapsed_ms: 1200, response_started: false,
                }]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    /// A realistic llama-swap entry, as sent in an `upsert`.
    fn wire_entry(resp_headers: Option<serde_json::Value>) -> serde_json::Value {
        let mut e = json!({
            "id": "7", "timestamp": "2026-10-05T12:00:00Z", "model": "qwen",
            "req_path": "/v1/chat/completions", "method": "POST",
            "req_headers": {"Content-Type": "application/json"}, "remote_ip": "127.0.0.1",
            "resp_bytes": 0, "elapsed_ms": 3000, "metadata": {}
        });
        if let Some(h) = resp_headers {
            e["resp_headers"] = h;
        }
        e
    }

    fn decode_upsert(entry: serde_json::Value) -> InflightEntry {
        let payload = envelope("inflight", json!({"operation":"upsert","request":entry}));
        match decode_event(&payload).unwrap() {
            StreamEvent::Inflight(m) => m.request.expect("request"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn empty_missing_or_null_resp_headers_mean_not_started() {
        assert!(!decode_upsert(wire_entry(Some(json!({})))).response_started);
        assert!(!decode_upsert(wire_entry(None)).response_started);
        assert!(!decode_upsert(wire_entry(Some(serde_json::Value::Null))).response_started);
    }

    #[test]
    fn non_empty_resp_headers_mean_started() {
        let e = decode_upsert(wire_entry(Some(json!({"Content-Type": "text/event-stream", "Cache-Control": "no-cache"}))));
        assert!(e.response_started);
        assert_eq!((e.id.as_str(), e.model.as_str(), e.elapsed_ms), ("7", "qwen", 3000));
    }

    #[test]
    fn unexpected_resp_headers_shape_does_not_fail_the_entry() {
        let e = decode_upsert(wire_entry(Some(json!("text/event-stream"))));
        assert!(!e.response_started);
        assert_eq!(e.id, "7");
    }

    #[test]
    fn upsert_with_headers_flips_the_view_to_started() {
        let t0 = Instant::now();
        let mut t = InflightTable::default();
        t.apply(upsert(decode_upsert(wire_entry(Some(json!({}))))), t0);
        assert!(!t.views_for("qwen", t0)[0].response_started);
        t.apply(upsert(decode_upsert(wire_entry(Some(json!({"Content-Type": "text/event-stream"}))))), t0 + Duration::from_secs(2));
        let v = t.views_for("qwen", t0 + Duration::from_secs(2));
        assert!(v[0].response_started);
        assert_eq!(v[0].resp_bytes, 0);
        assert_eq!(v[0].elapsed.as_secs(), 5, "start time is kept from first sighting");

        // A snapshot (e.g. after a reconnect) also flips an entry already in the table.
        let mut other = wire_entry(Some(json!({})));
        other["id"] = json!("8");
        other["model"] = json!("gemma");
        t.apply(upsert(decode_upsert(other.clone())), t0);
        assert!(!t.views_for("gemma", t0)[0].response_started);
        other["resp_headers"] = json!({"Content-Type": "text/event-stream"});
        let snap = envelope("inflight", json!({"operation":"snapshot","requests":[other]}));
        match decode_event(&snap).unwrap() {
            StreamEvent::Inflight(m) => t.apply(m, t0 + Duration::from_secs(4)),
            e => panic!("unexpected {e:?}"),
        }
        let v = t.views_for("gemma", t0 + Duration::from_secs(4));
        assert!(v[0].response_started);
        assert_eq!(v[0].elapsed.as_secs(), 7, "snapshot keeps the survivor's start");
    }

    #[test]
    fn decodes_remove_and_null_requests() {
        let payload = envelope("inflight", json!({"operation":"remove","id":"9","requests":null}));
        match decode_event(&payload).unwrap() {
            StreamEvent::Inflight(m) => {
                assert_eq!(m.id, "9");
                assert!(m.requests.is_empty());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn classifies_other_types() {
        assert_eq!(decode_event(&envelope("modelStatus", json!([]))).unwrap(), StreamEvent::ModelStatus);
        assert_eq!(decode_event(&envelope("logData", json!({"source":"proxy"}))).unwrap(), StreamEvent::Other);
        assert!(decode_event("not json").is_err());
    }

    #[test]
    fn new_entry_age_comes_from_elapsed_ms() {
        let now = Instant::now();
        let mut t = InflightTable::default();
        t.apply(upsert(entry("1", "qwen", 0, 5_000)), now);
        let v = t.views_for("qwen", now + Duration::from_secs(1));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].elapsed.as_secs(), 6);
        assert_eq!(v[0].since_bytes_change.as_secs(), 1);
    }

    #[test]
    fn byte_growth_resets_since_bytes_change_and_same_bytes_does_not() {
        let t0 = Instant::now();
        let mut t = InflightTable::default();
        t.apply(upsert(entry("1", "qwen", 10, 0)), t0);
        t.apply(upsert(entry("1", "qwen", 50, 0)), t0 + Duration::from_secs(4));
        t.apply(upsert(entry("1", "qwen", 50, 0)), t0 + Duration::from_secs(9));
        let v = t.views_for("qwen", t0 + Duration::from_secs(10));
        assert_eq!(v[0].resp_bytes, 50);
        assert_eq!(v[0].since_bytes_change.as_secs(), 6);
        assert_eq!(v[0].elapsed.as_secs(), 10, "start time is kept from first sighting");
    }

    #[test]
    fn remove_deletes_entry() {
        let now = Instant::now();
        let mut t = InflightTable::default();
        t.apply(upsert(entry("1", "qwen", 0, 0)), now);
        t.apply(InflightMsg { operation: "remove".into(), id: "1".into(), ..Default::default() }, now);
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn snapshot_replaces_table_but_keeps_history_of_survivors() {
        let t0 = Instant::now();
        let mut t = InflightTable::default();
        t.apply(upsert(entry("1", "qwen", 10, 0)), t0);
        t.apply(upsert(entry("2", "qwen", 0, 0)), t0);
        let snap = InflightMsg { operation: "snapshot".into(), requests: vec![entry("1", "qwen", 10, 99_000)], ..Default::default() };
        t.apply(snap, t0 + Duration::from_secs(5));
        assert_eq!(t.len(), 1);
        let v = t.views_for("qwen", t0 + Duration::from_secs(5));
        assert_eq!(v[0].elapsed.as_secs(), 5, "survivor keeps its original start");
        assert_eq!(v[0].since_bytes_change.as_secs(), 5);
    }

    #[test]
    fn views_filter_by_model_and_clear_empties() {
        let now = Instant::now();
        let mut t = InflightTable::default();
        t.apply(upsert(entry("1", "qwen", 0, 0)), now);
        t.apply(upsert(entry("2", "gemma", 0, 0)), now);
        assert_eq!(t.views_for("qwen", now).len(), 1);
        assert_eq!(t.views_for("nope", now).len(), 0);
        t.clear();
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn decode_health_states() {
        use EventStream::*;
        let mut h = DecodeHealth::default();
        assert_eq!(h.state(), Offline);
        assert!(!h.sees_activity());
        h.connect();
        assert_eq!(h.state(), Connected);
        assert!(h.sees_activity(), "connected, nothing failed: idle is believable");
        h.record(true);
        assert_eq!(h.state(), Live);
        assert!(h.sees_activity());
        h.disconnect();
        assert_eq!(h.state(), Offline);
        assert!(!h.sees_activity());
    }

    #[test]
    fn decode_health_tolerates_stray_bad_frames() {
        use EventStream::*;
        let mut h = DecodeHealth::default();
        h.connect();
        h.record(true);
        h.record(false);
        h.record(false);
        assert_eq!(h.state(), Live, "two stray failures after a success");
        assert!(h.sees_activity());
        h.record(true);
        h.record(false);
        h.record(false);
        assert_eq!(h.state(), Live, "a success resets the run of failures");
        h.record(false);
        assert_eq!(h.state(), Unreadable);
        assert_eq!(h.consecutive_failures(), UNREADABLE_AFTER);
        assert!(!h.sees_activity());
        h.record(true);
        assert_eq!(h.state(), Live, "readable again after a success");
    }

    #[test]
    fn decode_health_failures_before_any_success() {
        use EventStream::*;
        let mut h = DecodeHealth::default();
        h.connect();
        h.record(false);
        assert_eq!(h.state(), Connected);
        assert!(!h.sees_activity(), "a failure before any success: idle is not believable");
        h.record(false);
        h.record(false);
        assert_eq!(h.state(), Unreadable);
    }

    #[test]
    fn decode_health_resets_on_reconnect() {
        use EventStream::*;
        let mut h = DecodeHealth::default();
        h.connect();
        for _ in 0..5 {
            h.record(false);
        }
        assert_eq!(h.state(), Unreadable);
        h.connect();
        assert_eq!(h.state(), Connected);
        assert_eq!(h.consecutive_failures(), 0);
        assert!(h.sees_activity());
        h.record(true);
        h.disconnect();
        h.connect();
        assert_eq!(h.state(), Connected, "a success on an earlier connection does not carry over");
    }

    #[test]
    fn event_stream_serializes_camel_case() {
        let v: Vec<_> = [EventStream::Offline, EventStream::Connected, EventStream::Live, EventStream::Unreadable]
            .iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .collect();
        assert_eq!(v, [json!("offline"), json!("connected"), json!("live"), json!("unreadable")]);
    }

    /// Feeds raw frames through `decode_event` and `health_signal` the way the runtime does.
    fn feed(h: &mut DecodeHealth, payloads: &[String]) {
        for p in payloads {
            if let Some(ok) = health_signal(&decode_event(p)) {
                h.record(ok);
            }
        }
    }

    #[test]
    fn only_snapshot_and_upsert_prove_the_stream_readable() {
        let good_upsert = envelope("inflight", json!({"operation":"upsert","request":wire_entry(None)}));
        let snapshot = envelope("inflight", json!({"operation":"snapshot","requests":[wire_entry(None)]}));
        let empty_snapshot = envelope("inflight", json!({"operation":"snapshot","requests":null}));
        let remove = envelope("inflight", json!({"operation":"remove","id":"7"}));
        let upsert_without_entry = envelope("inflight", json!({"operation":"upsert"}));
        let model_status = envelope("modelStatus", json!({}));
        let log = envelope("logData", json!("line"));
        assert_eq!(health_signal(&decode_event(&good_upsert)), Some(true));
        assert_eq!(health_signal(&decode_event(&snapshot)), Some(true));
        assert_eq!(health_signal(&decode_event(&empty_snapshot)), Some(true));
        assert_eq!(health_signal(&decode_event(&remove)), None);
        assert_eq!(health_signal(&decode_event(&upsert_without_entry)), None);
        assert_eq!(health_signal(&decode_event(&model_status)), None);
        assert_eq!(health_signal(&decode_event(&log)), None);
        assert_eq!(health_signal(&decode_event("{not json")), Some(false));
    }

    #[test]
    fn broken_upserts_are_not_masked_by_removes_and_model_status() {
        use EventStream::*;
        // A newer llama-swap where only the upsert entry shape changed.
        let mut bad_entry = wire_entry(None);
        bad_entry["elapsed_ms"] = json!("3000");
        let bad_upsert = envelope("inflight", json!({"operation":"upsert","request":bad_entry}));
        let remove = envelope("inflight", json!({"operation":"remove","id":"7"}));
        let model_status = envelope("modelStatus", json!({}));
        assert!(decode_event(&bad_upsert).is_err(), "test frame must fail to decode");

        let mut h = DecodeHealth::default();
        h.connect();
        for _ in 0..5 {
            feed(&mut h, &[bad_upsert.clone()]);
            assert_ne!(h.state(), Live);
            assert!(!h.sees_activity(), "a model must not show Idle while upserts fail");
            feed(&mut h, &[remove.clone(), model_status.clone()]);
            assert_ne!(h.state(), Live);
            assert!(!h.sees_activity());
        }
        assert_eq!(h.state(), Unreadable);

        let good_upsert = envelope("inflight", json!({"operation":"upsert","request":wire_entry(None)}));
        feed(&mut h, &[good_upsert]);
        assert_eq!(h.state(), Live);
        assert!(h.sees_activity());
    }
}
