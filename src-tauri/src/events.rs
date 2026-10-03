//! Decoding of llama-swap `/api/events` messages and the in-flight request table.
//!
//! Wire format per frame: `data:{"type":"inflight","data":"<JSON string>"}`.
//! The inner `data` is itself JSON encoded as a string.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Deserialize;

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
                    method: "POST".into(), resp_bytes: 0, elapsed_ms: 1200,
                }]);
            }
            other => panic!("unexpected {other:?}"),
        }
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
}
