//! Merges poll results and the in-flight table into a `Snapshot` for the UI.
//! Pure: callers pass `now` (monotonic) and `wall_ms` (Unix ms) in.

use std::collections::HashMap;
use std::time::Instant;

use serde::Serialize;

use crate::api::{ActivityEntry, Histogram, ModelEntry, RunningModel, StatsResponse};
use crate::compat::version_note;
use crate::events::{DecodeHealth, EventStream, InflightMsg, InflightTable};
use crate::poller::{Feature, PollOutcome};
use crate::state::{derive_state, ModelState, Thresholds};

pub const TOK_S_HISTORY: usize = 30;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Connection {
    Connecting,
    Connected { latency_ms: u64 },
    Unauthorized,
    Unreachable { message: String },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCard {
    pub id: String,
    pub name: String,
    pub description: String,
    pub state: ModelState,
    pub ttl_s: Option<u64>,
    /// Estimate: llama-swap does not expose the real unload deadline.
    pub ttl_remaining_s: Option<u64>,
    pub tok_s_history: Vec<f64>,
    pub last_request_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsSummary {
    pub total_requests: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_tokens: i64,
    pub gen: Option<Histogram>,
    pub prompt: Option<Histogram>,
}

/// Token counts of the newest finished request on any model, from `/api/metrics/activity`.
/// Shown as llama-swap reports them: depending on the upstream, `input_tokens` is either the
/// whole prompt (OpenAI `usage.prompt_tokens`, which counts cached tokens too) or only newly
/// processed tokens (llama.cpp `timings.prompt_n`), so `input_tokens + cache_tokens` is not
/// necessarily the prompt size.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastRequest {
    pub model: String,
    pub input_tokens: i64,
    pub cache_tokens: i64,
    pub output_tokens: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub host: String,
    pub connection: Connection,
    pub last_ok_ms: Option<i64>,
    pub version: Option<String>,
    /// Set when `version` parses to a release outside the tested range (see `compat`).
    pub version_note: Option<String>,
    pub models: Vec<ModelCard>,
    pub stats: Option<StatsSummary>,
    pub stats_available: bool,
    pub event_stream: EventStream,
    /// An event on this connection failed to decode since the last one that decoded. Lets the
    /// badge say "arrived but couldn't be read" instead of "nothing read yet". Only this flag
    /// leaves Rust, never payload or error text.
    pub event_read_failed: bool,
    pub last_request: Option<LastRequest>,
    /// Reply bytes received so far by requests still streaming; `None` when none are.
    pub streamed_bytes: Option<i64>,
    /// Loaded model ids for the Mini strip, busiest first (see `mini_order`).
    pub mini_order: Vec<String>,
}

#[derive(Debug)]
pub struct MonitorState {
    host: String,
    connection: Connection,
    last_ok_ms: Option<i64>,
    version: Option<String>,
    configured: Vec<ModelEntry>,
    running: Vec<RunningModel>,
    stats: Option<StatsResponse>,
    stats_available: bool,
    activity: Vec<ActivityEntry>,
    /// Activity id and counts of the newest request that reported tokens. Kept across polls so
    /// it survives scrolling out of the activity window behind token-less rows.
    last_request: Option<(i64, LastRequest)>,
    inflight: InflightTable,
    events: DecodeHealth,
    /// model id -> (llama-swap state, when we first saw it in that state)
    state_since: HashMap<String, (String, Instant)>,
}

impl MonitorState {
    pub fn new(host: &str) -> Self {
        Self {
            host: host.to_string(),
            connection: Connection::Connecting,
            last_ok_ms: None,
            version: None,
            configured: Vec::new(),
            running: Vec::new(),
            stats: None,
            stats_available: true,
            activity: Vec::new(),
            last_request: None,
            inflight: InflightTable::default(),
            events: DecodeHealth::default(),
            state_since: HashMap::new(),
        }
    }

    pub fn apply_poll(&mut self, outcome: PollOutcome, now: Instant, wall_ms: i64) {
        match outcome {
            PollOutcome::Ok(d) => {
                self.connection = Connection::Connected { latency_ms: d.latency.as_millis() as u64 };
                self.last_ok_ms = Some(wall_ms);
                match d.version {
                    Some(Feature::Available(v)) => self.version = Some(v.version),
                    Some(Feature::Unavailable) => self.version = None,
                    Some(Feature::Failed) | None => {}
                }
                if let Some(m) = d.models {
                    self.configured = m.into_iter().filter(ModelEntry::is_local_model).collect();
                }
                self.track_states(&d.running, now);
                self.running = d.running;
                match d.stats {
                    Feature::Available(s) => {
                        self.stats = Some(s);
                        self.stats_available = true;
                    }
                    Feature::Unavailable => {
                        self.stats = None;
                        self.stats_available = false;
                    }
                    Feature::Failed => {}
                }
                match d.activity {
                    Feature::Available(a) => {
                        self.activity = a;
                        self.update_last_request();
                    }
                    Feature::Unavailable => {
                        self.activity.clear();
                        self.last_request = None;
                    }
                    Feature::Failed => {}
                }
            }
            PollOutcome::Unauthorized => self.connection = Connection::Unauthorized,
            PollOutcome::Unreachable(message) => self.connection = Connection::Unreachable { message },
            PollOutcome::Error(message) => self.connection = Connection::Error { message },
        }
    }

    fn track_states(&mut self, running: &[RunningModel], now: Instant) {
        self.state_since.retain(|id, _| running.iter().any(|r| &r.model == id));
        for r in running {
            let unchanged = matches!(self.state_since.get(&r.model), Some((s, _)) if *s == r.state);
            if !unchanged {
                self.state_since.insert(r.model.clone(), (r.state.clone(), now));
            }
        }
    }

    pub fn apply_inflight(&mut self, msg: InflightMsg, now: Instant) {
        self.inflight.apply(msg, now);
    }

    /// The event stream (re)connected: decode counts start over, except after a quiet idle reconnect, which keeps them.
    pub fn events_connected(&mut self) {
        self.events.connect();
    }

    /// The event stream dropped or is unavailable. In-flight data can no longer be kept current.
    pub fn events_disconnected(&mut self) {
        self.events.disconnect();
        self.inflight.clear();
    }

    /// Records one decode attempt of an event the monitor reads. Returns whether what the
    /// snapshot shows may have changed (stream health or which ready models count as Idle).
    /// Entering `Unreadable` drops the in-flight table: its removals may have been lost.
    ///
    /// Known limit: after events recover, a request that was already running when the table was
    /// dropped is unknown until llama-swap sends an upsert for it (on output) or it finishes, so
    /// its model may show Idle meanwhile.
    pub fn record_decode(&mut self, ok: bool) -> bool {
        let look = |e: &DecodeHealth| (e.state(), e.sees_activity(), e.consecutive_failures() > 0);
        let before = look(&self.events);
        self.events.record(ok);
        let after = look(&self.events);
        if after.0 == EventStream::Unreadable && before.0 != EventStream::Unreadable {
            self.inflight.clear();
        }
        before != after
    }

    pub fn event_stream(&self) -> EventStream {
        self.events.state()
    }

    pub fn consecutive_decode_failures(&self) -> u32 {
        self.events.consecutive_failures()
    }

    pub fn snapshot(&self, now: Instant, wall_ms: i64, t: &Thresholds) -> Snapshot {
        let mut entries: Vec<(String, String, String)> =
            self.configured.iter().map(|m| (m.id.clone(), m.name.clone(), m.description.clone())).collect();
        for r in &self.running {
            if !entries.iter().any(|(id, _, _)| *id == r.model) {
                entries.push((r.model.clone(), r.name.clone(), r.description.clone()));
            }
        }
        let models: Vec<ModelCard> = entries
            .into_iter()
            .map(|(id, name, description)| self.card(id, name, description, now, wall_ms, t))
            .collect();
        Snapshot {
            host: self.host.clone(),
            connection: self.connection.clone(),
            last_ok_ms: self.last_ok_ms,
            version: self.version.clone(),
            version_note: self.version.as_deref().and_then(version_note),
            mini_order: mini_order(&models),
            models,
            stats: self.stats.as_ref().map(|s| StatsSummary {
                total_requests: s.total_requests,
                total_input_tokens: s.total_input_tokens,
                total_output_tokens: s.total_output_tokens,
                total_cache_tokens: s.total_cache_tokens,
                gen: s.gen_histogram.clone(),
                prompt: s.prompt_histogram.clone(),
            }),
            stats_available: self.stats_available,
            event_stream: self.events.state(),
            event_read_failed: self.events.consecutive_failures() > 0,
            last_request: self.last_request.as_ref().map(|(_, lr)| lr.clone()),
            streamed_bytes: self.inflight.streamed_bytes(),
        }
    }

    /// Moves `last_request` to the newest request in the activity window that reported any
    /// tokens. Rows without token data (failed requests, upstreams that report none) are skipped,
    /// and the kept request stays when only such rows are newer. Ids only grow while llama-swap
    /// runs, so llama-swap restarted when the window is empty, its newest id is below the kept
    /// one, or it spans the kept id without that row: the kept request is dropped and the new
    /// window decides. A restart is missed only when the new ids have all moved past the kept
    /// one, and then the kept request is still replaced by the next request with tokens. llama-swap sends -1 for
    /// an unknown count; it is shown as 0.
    fn update_last_request(&mut self) {
        let t = |a: &ActivityEntry| (a.tokens.input_tokens.max(0), a.tokens.cache_tokens.max(0), a.tokens.output_tokens.max(0));
        if let Some((kept, _)) = self.last_request {
            let oldest = self.activity.iter().map(|a| a.id).min();
            let newest = self.activity.iter().map(|a| a.id).max();
            let spans_kept = oldest.is_some_and(|o| o <= kept) && !self.activity.iter().any(|a| a.id == kept);
            if newest.is_none_or(|n| n < kept) || spans_kept {
                self.last_request = None;
            }
        }
        let found = self.activity.iter().filter(|a| t(a) != (0, 0, 0)).max_by_key(|a| a.id);
        if let Some(a) = found {
            if self.last_request.as_ref().is_none_or(|(kept, _)| a.id > *kept) {
                let (input_tokens, cache_tokens, output_tokens) = t(a);
                let lr = LastRequest { model: a.model.clone(), input_tokens, cache_tokens, output_tokens };
                self.last_request = Some((a.id, lr));
            }
        }
    }

    fn card(&self, id: String, name: String, description: String, now: Instant, wall_ms: i64, t: &Thresholds) -> ModelCard {
        let run = self.running.iter().find(|r| r.model == id);
        let since = self
            .state_since
            .get(&id)
            .map(|(_, at)| now.saturating_duration_since(*at))
            .unwrap_or_default();
        let views = self.inflight.views_for(&id, now);
        let state = derive_state(run.map(|r| r.state.as_str()), &views, since, self.events.sees_activity(), t);

        let mut history: Vec<&ActivityEntry> = self.activity.iter().filter(|a| a.model == id).collect();
        history.sort_by_key(|a| a.id);
        let last_request_at_ms = history.iter().filter_map(|a| a.timestamp_ms()).max();
        let speeds: Vec<f64> = history.iter().map(|a| a.tokens.tokens_per_second).filter(|v| *v > 0.0).collect();
        let tok_s_history = speeds[speeds.len().saturating_sub(TOK_S_HISTORY)..].to_vec();

        let ttl_s = run.map(|r| r.ttl).filter(|ttl| *ttl > 0).map(|ttl| ttl as u64);
        let ttl_remaining_s = match (&state, ttl_s) {
            (ModelState::Idle { .. }, Some(ttl)) => {
                let ready_wall_ms = wall_ms - since.as_millis() as i64;
                let reference = last_request_at_ms.map_or(ready_wall_ms, |l| l.max(ready_wall_ms));
                let idle_s = ((wall_ms - reference).max(0) / 1000) as u64;
                Some(ttl.saturating_sub(idle_s))
            }
            _ => None,
        };

        ModelCard {
            name: if name.is_empty() { id.clone() } else { name },
            id,
            description,
            state,
            ttl_s,
            ttl_remaining_s,
            tok_s_history,
            last_request_at_ms,
        }
    }
}

/// Loaded models for the Mini strip, busiest first: stalled, busy, loading, unloading, then
/// idle/loaded. Ties: most recent request first, then id. Not-loaded models are left out.
pub fn mini_order(models: &[ModelCard]) -> Vec<String> {
    let rank = |s: &ModelState| match s {
        ModelState::Stalled { .. } => 0,
        ModelState::Busy { .. } => 1,
        ModelState::Loading { .. } => 2,
        ModelState::Unloading => 3,
        ModelState::Idle { .. } | ModelState::Loaded => 4,
        ModelState::NotLoaded => 5,
    };
    let mut loaded: Vec<&ModelCard> = models.iter().filter(|m| !matches!(m.state, ModelState::NotLoaded)).collect();
    loaded.sort_by(|a, b| {
        rank(&a.state)
            .cmp(&rank(&b.state))
            .then_with(|| b.last_request_at_ms.unwrap_or(i64::MIN).cmp(&a.last_request_at_ms.unwrap_or(i64::MIN)))
            .then_with(|| a.id.cmp(&b.id))
    });
    loaded.into_iter().map(|m| m.id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::TokenMetrics;
    use crate::events::InflightEntry;
    use crate::poller::PollData;
    use std::time::Duration;

    const WALL: i64 = 1_790_000_000_000;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }
    fn running(id: &str, state: &str, ttl: i64) -> RunningModel {
        RunningModel { model: id.into(), state: state.into(), ttl, ..Default::default() }
    }
    fn model(id: &str) -> ModelEntry {
        ModelEntry { id: id.into(), ..Default::default() }
    }
    fn iso(ms: i64) -> String {
        chrono::DateTime::from_timestamp_millis(ms).unwrap().to_rfc3339()
    }
    fn activity(id: i64, model: &str, at_ms: i64, tps: f64) -> ActivityEntry {
        ActivityEntry {
            id,
            model: model.into(),
            timestamp: iso(at_ms),
            tokens: TokenMetrics { tokens_per_second: tps, ..Default::default() },
            ..Default::default()
        }
    }
    fn data(running: Vec<RunningModel>, models: Option<Vec<ModelEntry>>) -> Box<PollData> {
        Box::new(PollData {
            latency: Duration::from_millis(12),
            running,
            models,
            version: None,
            stats: Feature::Failed,
            activity: Feature::Failed,
        })
    }
    /// A monitor whose event stream is open, so ready models can show as Idle.
    fn watching(host: &str) -> MonitorState {
        let mut m = MonitorState::new(host);
        m.events_connected();
        m
    }
    fn card<'a>(s: &'a Snapshot, id: &str) -> &'a ModelCard {
        s.models.iter().find(|m| m.id == id).unwrap_or_else(|| panic!("no card {id}"))
    }

    #[test]
    fn cards_follow_configured_order_and_include_unlisted_running() {
        let t0 = Instant::now();
        let mut m = watching("http://box:8080");
        m.apply_poll(PollOutcome::Ok(data(vec![running("c", "ready", 0)], Some(vec![model("a"), model("b")]))), t0, WALL);
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        let ids: Vec<&str> = s.models.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(card(&s, "a").state, ModelState::NotLoaded);
        assert_eq!(card(&s, "c").state, ModelState::Idle { uptime_s: 0 });
        assert_eq!(card(&s, "a").name, "a", "empty name falls back to id");
        assert_eq!(s.connection, Connection::Connected { latency_ms: 12 });
        assert_eq!(s.last_ok_ms, Some(WALL));
    }

    #[test]
    fn alias_selector_and_peer_records_do_not_become_cards() {
        let t0 = Instant::now();
        let mut m = watching("h");
        let list: crate::api::ModelsResponse =
            serde_json::from_str(include_str!("../tests/fixtures/models_mixed.json")).unwrap();
        m.apply_poll(PollOutcome::Ok(data(vec![running("gemma-12b", "ready", 0)], Some(list.data))), t0, WALL);
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        let ids: Vec<&str> = s.models.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["gemma-12b", "legacy", "nometa"]);
        assert_eq!(card(&s, "gemma-12b").state, ModelState::Idle { uptime_s: 0 });
    }

    #[test]
    fn uptime_measured_from_observed_transition() {
        let t0 = Instant::now();
        let mut m = watching("h");
        m.apply_poll(PollOutcome::Ok(data(vec![running("a", "starting", 0)], Some(vec![model("a")]))), t0, WALL);
        let s = m.snapshot(t0 + secs(4), WALL, &Thresholds::default());
        assert_eq!(card(&s, "a").state, ModelState::Loading { elapsed_s: 4, slow: false });
        m.apply_poll(PollOutcome::Ok(data(vec![running("a", "ready", 0)], None)), t0 + secs(5), WALL);
        m.apply_poll(PollOutcome::Ok(data(vec![running("a", "ready", 0)], None)), t0 + secs(7), WALL);
        let s = m.snapshot(t0 + secs(15), WALL, &Thresholds::default());
        assert_eq!(card(&s, "a").state, ModelState::Idle { uptime_s: 10 });
    }

    #[test]
    fn tok_s_history_sorted_filtered_and_capped() {
        let t0 = Instant::now();
        let mut m = MonitorState::new("h");
        let mut acts: Vec<ActivityEntry> = (1..=35).rev().map(|i| activity(i, "a", WALL - 1000, i as f64)).collect();
        acts.push(activity(36, "a", WALL, 0.0)); // embeddings etc. report 0 tok/s
        acts.push(activity(37, "b", WALL, 99.0));
        let mut d = data(vec![running("a", "ready", 0)], Some(vec![model("a")]));
        d.activity = Feature::Available(acts);
        m.apply_poll(PollOutcome::Ok(d), t0, WALL);
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        let h = &card(&s, "a").tok_s_history;
        assert_eq!(h.len(), TOK_S_HISTORY);
        assert_eq!(h.first(), Some(&6.0));
        assert_eq!(h.last(), Some(&35.0));
        assert_eq!(card(&s, "a").last_request_at_ms, Some(WALL));
    }

    #[test]
    fn last_request_is_the_newest_id_across_models() {
        let t0 = Instant::now();
        let mut m = MonitorState::new("h");
        assert_eq!(m.snapshot(t0, WALL, &Thresholds::default()).last_request, None);
        let tokens = |input, cache, output| TokenMetrics { input_tokens: input, cache_tokens: cache, output_tokens: output, ..Default::default() };
        let mut d = data(vec![], None);
        d.activity = Feature::Available(vec![
            ActivityEntry { id: 7, model: "b".into(), tokens: tokens(120, 512, 80), ..Default::default() },
            ActivityEntry { id: 9, model: "a".into(), tokens: tokens(812, 0, 400), ..Default::default() },
            ActivityEntry { id: 8, model: "b".into(), tokens: tokens(1, 2, 3), ..Default::default() },
            ActivityEntry { id: 10, model: "c".into(), tokens: tokens(0, -1, 0), ..Default::default() },
        ]);
        m.apply_poll(PollOutcome::Ok(d), t0, WALL);
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        assert_eq!(
            s.last_request,
            Some(LastRequest { model: "a".into(), input_tokens: 812, cache_tokens: 0, output_tokens: 400 })
        );
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["lastRequest"], serde_json::json!({"model":"a","inputTokens":812,"cacheTokens":0,"outputTokens":400}));

        let mut d = data(vec![], None);
        d.activity = Feature::Available(vec![ActivityEntry { id: 11, model: "d".into(), tokens: tokens(812, -1, 5), ..Default::default() }]);
        m.apply_poll(PollOutcome::Ok(d), t0, WALL);
        let lr = m.snapshot(t0, WALL, &Thresholds::default()).last_request.unwrap();
        assert_eq!((lr.input_tokens, lr.cache_tokens), (812, 0), "-1 means unknown, never subtracted");
    }

    #[test]
    fn last_request_survives_token_less_polls_and_resets_on_restart() {
        let t0 = Instant::now();
        let mut m = MonitorState::new("h");
        let poll = |m: &mut MonitorState, rows: Vec<(i64, i64)>| {
            let mut d = data(vec![], None);
            d.activity = Feature::Available(
                rows.into_iter()
                    .map(|(id, out)| ActivityEntry {
                        id,
                        model: "a".into(),
                        tokens: TokenMetrics { output_tokens: out, ..Default::default() },
                        ..Default::default()
                    })
                    .collect(),
            );
            m.apply_poll(PollOutcome::Ok(d), t0, WALL);
            m.snapshot(t0, WALL, &Thresholds::default()).last_request.map(|lr| lr.output_tokens)
        };
        assert_eq!(poll(&mut m, vec![(5, 40)]), Some(40));
        assert_eq!(poll(&mut m, vec![(6, 0), (7, -1)]), Some(40), "kept while only token-less rows are in the window");
        assert_eq!(poll(&mut m, vec![(8, 9)]), Some(9));
        assert_eq!(poll(&mut m, vec![(8, 9)]), Some(9), "newest id equal to the kept one");
        let mut d = data(vec![], None);
        d.activity = Feature::Failed;
        m.apply_poll(PollOutcome::Ok(d), t0, WALL);
        assert_eq!(m.snapshot(t0, WALL, &Thresholds::default()).last_request.map(|lr| lr.output_tokens), Some(9));
        assert_eq!(poll(&mut m, vec![(8, 9), (9, 0)]), Some(9));
        assert_eq!(poll(&mut m, vec![(5, 6), (9, 0), (10, 0)]), Some(6), "window spans the kept id without it: restarted");
        assert_eq!(poll(&mut m, vec![(1, 0)]), None, "ids went backwards: llama-swap restarted");
        assert_eq!(poll(&mut m, vec![(2, 3)]), Some(3));
        assert_eq!(poll(&mut m, vec![]), None, "empty window after a restart");
        assert_eq!(poll(&mut m, vec![(1, 4)]), Some(4));
        let mut d = data(vec![], None);
        d.activity = Feature::Unavailable;
        m.apply_poll(PollOutcome::Ok(d), t0, WALL);
        assert_eq!(m.snapshot(t0, WALL, &Thresholds::default()).last_request, None);
    }

    #[test]
    fn streamed_bytes_follow_the_event_stream() {
        let (mut m, t0) = busy_on_live_stream();
        assert_eq!(m.snapshot(t0, WALL, &Thresholds::default()).streamed_bytes, None, "reply not started");
        let req = InflightEntry { id: "1".into(), model: "a".into(), resp_bytes: 4096, response_started: true, ..Default::default() };
        m.apply_inflight(InflightMsg { operation: "upsert".into(), request: Some(req), ..Default::default() }, t0);
        assert_eq!(m.snapshot(t0, WALL, &Thresholds::default()).streamed_bytes, Some(4096));
        m.events_disconnected();
        assert_eq!(m.snapshot(t0, WALL, &Thresholds::default()).streamed_bytes, None, "dropped with the table");
    }

    fn idle_with_request(ready_for: u64, last_request_ms: i64) -> Snapshot {
        let t0 = Instant::now();
        let mut m = watching("h");
        let mut d = data(vec![running("a", "ready", 300)], Some(vec![model("a")]));
        d.activity = Feature::Available(vec![activity(1, "a", last_request_ms, 40.0)]);
        m.apply_poll(PollOutcome::Ok(d), t0, WALL);
        m.snapshot(t0 + secs(ready_for), WALL, &Thresholds::default())
    }

    #[test]
    fn ttl_remaining_from_last_request() {
        let s = idle_with_request(200, WALL - 100_000);
        assert_eq!(card(&s, "a").ttl_s, Some(300));
        assert_eq!(card(&s, "a").ttl_remaining_s, Some(200));
    }

    #[test]
    fn ttl_remaining_clamped_when_server_clock_is_ahead() {
        let s = idle_with_request(10, WALL + 50_000);
        assert_eq!(card(&s, "a").ttl_remaining_s, Some(300));
    }

    #[test]
    fn ttl_remaining_counts_from_ready_when_last_request_predates_load() {
        let s = idle_with_request(20, WALL - 1_000_000);
        assert_eq!(card(&s, "a").ttl_remaining_s, Some(280));
    }

    #[test]
    fn ttl_zero_means_no_ttl() {
        let t0 = Instant::now();
        let mut m = watching("h");
        m.apply_poll(PollOutcome::Ok(data(vec![running("a", "ready", 0)], None)), t0, WALL);
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        assert_eq!(card(&s, "a").ttl_s, None);
        assert_eq!(card(&s, "a").ttl_remaining_s, None);
    }

    #[test]
    fn unreachable_keeps_last_data() {
        let t0 = Instant::now();
        let mut m = MonitorState::new("h");
        m.apply_poll(PollOutcome::Ok(data(vec![running("a", "ready", 0)], Some(vec![model("a")]))), t0, WALL);
        m.apply_poll(PollOutcome::Unreachable("timed out".into()), t0 + secs(2), WALL + 2000);
        let s = m.snapshot(t0 + secs(2), WALL + 2000, &Thresholds::default());
        assert_eq!(s.connection, Connection::Unreachable { message: "timed out".into() });
        assert_eq!(s.last_ok_ms, Some(WALL));
        assert_eq!(s.models.len(), 1);
    }

    #[test]
    fn stats_unavailable_clears_and_failed_keeps() {
        let t0 = Instant::now();
        let mut m = MonitorState::new("h");
        let mut d = data(vec![], None);
        d.stats = Feature::Available(StatsResponse { total_requests: 5, ..Default::default() });
        m.apply_poll(PollOutcome::Ok(d), t0, WALL);
        m.apply_poll(PollOutcome::Ok(data(vec![], None)), t0, WALL); // Failed keeps
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        assert_eq!(s.stats.as_ref().map(|x| x.total_requests), Some(5));
        let mut d = data(vec![], None);
        d.stats = Feature::Unavailable;
        m.apply_poll(PollOutcome::Ok(d), t0, WALL);
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        assert!(s.stats.is_none());
        assert!(!s.stats_available);
    }

    fn busy_on_live_stream() -> (MonitorState, Instant) {
        let t0 = Instant::now();
        let mut m = watching("h");
        m.apply_poll(PollOutcome::Ok(data(vec![running("a", "ready", 0)], None)), t0, WALL);
        let req = InflightEntry { id: "1".into(), model: "a".into(), resp_bytes: 10, ..Default::default() };
        m.apply_inflight(InflightMsg { operation: "upsert".into(), request: Some(req), ..Default::default() }, t0);
        assert!(m.record_decode(true), "connected -> live is a change");
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        assert!(matches!(card(&s, "a").state, ModelState::Busy { requests: 1, .. }));
        assert_eq!(s.event_stream, EventStream::Live);
        (m, t0)
    }

    #[test]
    fn dropping_live_events_clears_inflight_and_shows_loaded() {
        let (mut m, t0) = busy_on_live_stream();
        m.events_disconnected();
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        assert_eq!(card(&s, "a").state, ModelState::Loaded, "no live data: never Idle");
        assert_eq!(s.event_stream, EventStream::Offline);
        m.events_connected();
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        assert!(matches!(card(&s, "a").state, ModelState::Idle { .. }), "inflight was cleared");
        assert_eq!(s.event_stream, EventStream::Connected);
    }

    #[test]
    fn unreadable_events_show_loaded_and_drop_stale_inflight() {
        let (mut m, t0) = busy_on_live_stream();
        assert!(m.record_decode(false), "one stray bad frame only raises the failure flag");
        assert!(!m.record_decode(false), "a second changes nothing shown");
        assert!(matches!(card(&m.snapshot(t0, WALL, &Thresholds::default()), "a").state, ModelState::Busy { .. }));
        assert!(m.record_decode(false), "third failure in a row -> unreadable");
        assert_eq!(m.consecutive_decode_failures(), 3);
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        assert_eq!(s.event_stream, EventStream::Unreadable);
        assert_eq!(card(&s, "a").state, ModelState::Loaded);
        assert!(m.record_decode(true), "readable again");
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        assert_eq!(s.event_stream, EventStream::Live);
        assert!(matches!(card(&s, "a").state, ModelState::Idle { .. }), "stale request was dropped");
    }

    #[test]
    fn idle_needs_an_open_event_stream() {
        let t0 = Instant::now();
        let mut m = MonitorState::new("h");
        m.apply_poll(PollOutcome::Ok(data(vec![running("a", "ready", 300)], None)), t0, WALL);
        assert_eq!(card(&m.snapshot(t0, WALL, &Thresholds::default()), "a").state, ModelState::Loaded);
        m.events_connected();
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        assert!(matches!(card(&s, "a").state, ModelState::Idle { .. }));
        assert!(!s.event_read_failed);
        assert!(m.record_decode(false), "a failure before any success makes idle unbelievable");
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        assert_eq!(s.event_stream, EventStream::Connected);
        assert!(s.event_read_failed, "the badge must not say nothing has been read");
        assert_eq!(card(&s, "a").state, ModelState::Loaded);
        assert_eq!(card(&s, "a").ttl_remaining_s, None, "the unload estimate assumes idle");
        m.events_connected();
        assert!(!m.snapshot(t0, WALL, &Thresholds::default()).event_read_failed, "reset on reconnect");
    }

    #[test]
    fn a_stray_bad_frame_on_a_live_stream_is_reported() {
        let (mut m, t0) = busy_on_live_stream();
        assert!(m.record_decode(false), "the failure flag changes the snapshot");
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        assert_eq!(s.event_stream, EventStream::Live);
        assert!(s.event_read_failed);
        assert!(m.record_decode(true), "cleared by the next readable frame");
        assert!(!m.snapshot(t0, WALL, &Thresholds::default()).event_read_failed);
    }

    #[test]
    fn version_note_follows_reported_version() {
        let t0 = Instant::now();
        let mut m = MonitorState::new("h");
        let version = |v: &str| {
            let mut d = data(vec![], None);
            d.version = Some(Feature::Available(crate::api::VersionInfo { version: v.into(), ..Default::default() }));
            PollOutcome::Ok(d)
        };
        m.apply_poll(version("v270"), t0, WALL);
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        assert_eq!(s.version_note.as_deref(), Some("Tested with llama-swap v249–v262; this server reports v270"));
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["versionNote"], "Tested with llama-swap v249–v262; this server reports v270");
        m.apply_poll(version("v262"), t0, WALL);
        assert_eq!(m.snapshot(t0, WALL, &Thresholds::default()).version_note, None);
        m.apply_poll(version("garbage"), t0, WALL);
        assert_eq!(m.snapshot(t0, WALL, &Thresholds::default()).version_note, None);
    }

    #[test]
    fn snapshot_json_shape_matches_frontend_types() {
        let t0 = Instant::now();
        let mut m = MonitorState::new("http://box:8080");
        m.apply_poll(PollOutcome::Ok(data(vec![running("a", "ready", 300)], None)), t0, WALL);
        let v = serde_json::to_value(m.snapshot(t0, WALL, &Thresholds::default())).unwrap();
        for key in [
            "host", "connection", "lastOkMs", "version", "versionNote", "models", "stats", "statsAvailable", "eventStream",
            "eventReadFailed", "lastRequest", "streamedBytes", "miniOrder",
        ] {
            assert!(v.get(key).is_some(), "missing {key}: {v}");
        }
        assert_eq!(v["eventStream"], "offline");
        assert_eq!(v["eventReadFailed"], false);
        assert_eq!(v["versionNote"], serde_json::Value::Null);
        assert_eq!(v["models"][0]["state"], serde_json::json!({"kind":"loaded"}));
        assert_eq!(v["connection"], serde_json::json!({"kind":"connected","latencyMs":12}));
        let c = &v["models"][0];
        for key in ["id", "name", "description", "state", "ttlS", "ttlRemainingS", "tokSHistory", "lastRequestAtMs"] {
            assert!(c.get(key).is_some(), "missing card.{key}: {c}");
        }
    }

    /// A card for the mini-order tests: only the fields ranking looks at matter.
    fn mini_card(id: &str, state: ModelState, last: Option<i64>) -> ModelCard {
        ModelCard {
            id: id.into(),
            name: String::new(),
            description: String::new(),
            state,
            ttl_s: None,
            ttl_remaining_s: None,
            tok_s_history: vec![],
            last_request_at_ms: last,
        }
    }

    #[test]
    fn mini_order_ranks_busiest_first() {
        let busy = ModelState::Busy { requests: 1, oldest_elapsed_s: 1, streaming: 1, waiting_first_token: 0, awaiting_reply: 0, last_output_s: Some(0), first_token_long: false, awaiting_long: false };
        let models = vec![
            mini_card("idle", ModelState::Idle { uptime_s: 5 }, Some(100)),
            mini_card("off", ModelState::NotLoaded, Some(999)),
            mini_card("load", ModelState::Loading { elapsed_s: 1, slow: false }, None),
            mini_card("busy", busy, Some(1)),
            mini_card("stuck", ModelState::Stalled { reason: "x".into() }, None),
            mini_card("bye", ModelState::Unloading, None),
            mini_card("loaded", ModelState::Loaded, Some(200)),
        ];
        assert_eq!(mini_order(&models), ["stuck", "busy", "load", "bye", "loaded", "idle"]);
    }

    #[test]
    fn mini_order_ties_go_to_most_recent_then_id() {
        let models = vec![
            mini_card("b", ModelState::Idle { uptime_s: 1 }, None),
            mini_card("a", ModelState::Idle { uptime_s: 1 }, None),
            mini_card("c", ModelState::Idle { uptime_s: 1 }, Some(5)),
        ];
        assert_eq!(mini_order(&models), ["c", "a", "b"]);
    }

    #[test]
    fn mini_order_is_empty_when_nothing_is_loaded() {
        assert!(mini_order(&[mini_card("x", ModelState::NotLoaded, Some(1))]).is_empty());
    }
}
