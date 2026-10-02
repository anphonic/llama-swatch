//! Wire types for llama-swap's HTTP API.
//!
//! Every struct defaults missing fields and ignores unknown ones so that older
//! and newer llama-swap versions both parse. Go marshals nil slices as `null`,
//! so every `Vec` goes through `null_default`.

use serde::{Deserialize, Deserializer, Serialize};

/// Deserialize `null` as `T::default()`. Missing fields are handled by the
/// struct-level `#[serde(default)]`.
pub fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct VersionInfo {
    pub version: String,
    pub commit: String,
    pub build_date: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RunningResponse {
    #[serde(deserialize_with = "null_default")]
    pub running: Vec<RunningModel>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RunningModel {
    pub model: String,
    pub name: String,
    pub description: String,
    /// One of `stopped`, `starting`, `ready`, `stopping`, `shutdown`.
    pub state: String,
    /// Seconds of idleness before llama-swap unloads the model; 0 = never.
    pub ttl: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ModelsResponse {
    #[serde(deserialize_with = "null_default")]
    pub data: Vec<ModelEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct StatsResponse {
    pub total_requests: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_tokens: i64,
    pub prompt_histogram: Option<Histogram>,
    pub gen_histogram: Option<Histogram>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Histogram {
    #[serde(deserialize_with = "null_default")]
    pub bins: Vec<i64>,
    pub min: f64,
    pub max: f64,
    #[serde(rename = "binSize")]
    pub bin_size: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ActivityResponse {
    #[serde(deserialize_with = "null_default")]
    pub data: Vec<ActivityEntry>,
    pub total: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ActivityEntry {
    pub id: i64,
    /// RFC 3339 with arbitrary offset and up to nanosecond precision (Go `time.Time`).
    pub timestamp: String,
    pub model: String,
    pub req_path: String,
    pub resp_status_code: i64,
    pub duration_ms: i64,
    pub tokens: TokenMetrics,
}

impl ActivityEntry {
    /// Unix milliseconds, or `None` if the timestamp is not valid RFC 3339.
    pub fn timestamp_ms(&self) -> Option<i64> {
        chrono::DateTime::parse_from_rfc3339(&self.timestamp)
            .ok()
            .map(|t| t.timestamp_millis())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TokenMetrics {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_tokens: i64,
    pub prompt_per_second: f64,
    pub tokens_per_second: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_fixture() {
        let v: VersionInfo = serde_json::from_str(include_str!("../tests/fixtures/version.json")).unwrap();
        assert_eq!(v.version, "v188");
        assert_eq!(v.commit, "abc1234");
    }

    #[test]
    fn parses_running_fixture_ignoring_unknown_fields() {
        let r: RunningResponse = serde_json::from_str(include_str!("../tests/fixtures/running.json")).unwrap();
        assert_eq!(r.running.len(), 2);
        assert_eq!(r.running[1].model, "qwen3-30b");
        assert_eq!(r.running[1].state, "ready");
        assert_eq!(r.running[1].ttl, 300);
    }

    #[test]
    fn parses_models_fixture() {
        let m: ModelsResponse = serde_json::from_str(include_str!("../tests/fixtures/models.json")).unwrap();
        let ids: Vec<&str> = m.data.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["embed", "gemma-12b", "qwen3-30b"]);
        assert_eq!(m.data[1].name, "Gemma 12B");
    }

    #[test]
    fn parses_stats_fixture() {
        let s: StatsResponse = serde_json::from_str(include_str!("../tests/fixtures/stats.json")).unwrap();
        assert_eq!(s.total_requests, 42);
        let g = s.gen_histogram.unwrap();
        assert_eq!(g.bins, vec![2, 7, 12, 5, 1]);
        assert_eq!(g.bin_size, 10.0);
        assert_eq!(g.p50, 38.4);
    }

    #[test]
    fn parses_activity_fixture_and_timestamps_with_offsets() {
        let a: ActivityResponse = serde_json::from_str(include_str!("../tests/fixtures/activity.json")).unwrap();
        assert_eq!(a.data.len(), 3);
        assert_eq!(a.data[0].tokens.tokens_per_second, 41.8);
        // 10:00:30.5 at -07:00 is 17:00:30.5 UTC
        let expected = chrono::DateTime::parse_from_rfc3339("2026-10-02T17:00:30.5Z").unwrap().timestamp_millis();
        assert_eq!(a.data[0].timestamp_ms(), Some(expected));
        assert!(a.data[1].timestamp_ms().is_some(), "nanosecond precision must parse");
    }

    #[test]
    fn bad_timestamp_is_none_not_an_error() {
        let e = ActivityEntry { timestamp: "yesterday".into(), ..Default::default() };
        assert_eq!(e.timestamp_ms(), None);
    }

    #[test]
    fn null_arrays_parse_as_empty() {
        let a: ActivityResponse =
            serde_json::from_str(r#"{"data":null,"page":1,"limit":100,"total":0,"total_pages":0}"#).unwrap();
        assert!(a.data.is_empty());
        let r: RunningResponse = serde_json::from_str(r#"{"running":null}"#).unwrap();
        assert!(r.running.is_empty());
        let m: ModelsResponse = serde_json::from_str(r#"{"object":"list","data":null}"#).unwrap();
        assert!(m.data.is_empty());
        let s: StatsResponse = serde_json::from_str(
            r#"{"total_requests":0,"prompt_histogram":null,"gen_histogram":{"bins":null,"min":0,"max":0,"binSize":0,"p50":0,"p95":0,"p99":0}}"#,
        )
        .unwrap();
        assert!(s.prompt_histogram.is_none());
        assert!(s.gen_histogram.unwrap().bins.is_empty());
    }

    #[test]
    fn missing_fields_default() {
        let r: RunningResponse = serde_json::from_str(r#"{"running":[{"model":"x"}]}"#).unwrap();
        assert_eq!(r.running[0].state, "");
        assert_eq!(r.running[0].ttl, 0);
    }
}
