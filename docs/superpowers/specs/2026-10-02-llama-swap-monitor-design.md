# llama-swap Monitor — Phase 1 Design

**Date:** 2026-10-02
**Status:** Approved in brainstorming, pending written-spec review

## Purpose

A small, installable desktop app that anyone running [llama-swap](https://github.com/mostlygeek/llama-swap)
can point at their instance — wherever it runs (localhost, a LAN box, behind a reverse
proxy) — to see at a glance:

- whether llama-swap is reachable and healthy,
- which models are loaded and what each one is doing (loading, idle, busy, stalled, …),
- a visual summary of current throughput and usage stats.

Phase 1 is read-only monitoring with a visually rich dashboard ("pretties to look at"),
not a control panel.

### Success criteria

1. A new user installs the app on Windows, macOS, or Linux, enters a URL (and optional
   API key), and sees live status within a minute.
2. Model state changes in llama-swap (load, request start/finish, unload) appear in the
   UI within ~2 seconds.
3. A model stuck loading or a request that stops producing output is shown as **Stalled**
   with a human-readable reason.
4. Losing the connection never crashes the app; it degrades visibly and recovers on its own.

### Out of scope (phase 1)

Unloading/controlling models, log viewer, tray icon, desktop notifications,
multi-server monitoring, code signing. All are phase 2 candidates.

## Stack

- **Tauri 2** — Rust backend, OS webview frontend. Small installers (~5–10 MB),
  low idle memory, cross-platform.
- **Frontend:** vanilla TypeScript + Vite, no framework. Hand-written SVG/CSS for rings,
  sparklines, bars, and histogram (no chart library).
- **Rust crates:** `tauri`, `reqwest` (rustls, streaming), `tokio`, `serde`/`serde_json`,
  `keyring`, `thiserror`; dev: `wiremock`, `tokio-test`.
- **Targets:** Windows (`.msi`/`.exe`), macOS (`.dmg`), Linux (`.AppImage`/`.deb`).

## llama-swap API surface used

Verified against llama-swap `main` source (`internal/server/server.go`, `api.go`,
`apigroup.go`, `internal/store/activity.go`, `internal/swaputil/events.go`).

| Endpoint | Auth | Used for |
|---|---|---|
| `GET /health` | none | Reachability ("OK") |
| `GET /api/version` | key | `{version, commit, build_date}`; also validates the key |
| `GET /running` | key | Loaded models: `model, name, description, state, ttl, cmd, proxy` |
| `GET /v1/models` | none* | All configured models (to show "Not loaded" cards) |
| `GET /api/metrics/stats` | key | `total_requests`, `total_input_tokens`, `total_output_tokens`, `total_cache_tokens`, `prompt_histogram`, `gen_histogram` (`bins, min, max, binSize, p50, p95, p99`) |
| `GET /api/metrics/activity` | key | Recent requests: `timestamp, model, req_path, resp_status_code, duration_ms, tokens{input_tokens, output_tokens, prompt_per_second, tokens_per_second}` |
| `GET /api/events` (SSE) | key | `modelStatus` and `inflight` messages (envelope `{type, data}` where `data` is a JSON string) |

\* `/v1/models` is reachable without a key only in some configurations; always send the key when configured.

**Auth:** when a key is configured, send `Authorization: Bearer <key>` on every request.

**Process states** from llama-swap: `stopped`, `starting`, `ready`, `stopping`, `shutdown`.

**In-flight entries** (`inflight` SSE message): `id, timestamp, model, req_path, method,
resp_bytes, elapsed_ms`. The message carries `operation` plus either a full `requests`
snapshot or a single `request`/`id` delta. A snapshot is sent on connect.

## Architecture

```
┌──────────────────────── Tauri app ────────────────────────┐
│  Rust (src-tauri/src)                    Webview (src/)   │
│                                                           │
│  config.rs ── settings JSON + keyring                     │
│  client.rs ── reqwest client, typed endpoints             │
│  poller.rs ── interval loop: health, running,             │
│               models, stats, activity                     │
│  events.rs ── SSE consumer for /api/events                │
│  state.rs  ── derive_state() pure function                │
│  monitor.rs ── merges poll + SSE into Snapshot ──emit──▶  main.ts
│  commands.rs ◀──invoke── get_settings / save_settings /   │  views/*.ts
│               test_connection                             │
└───────────────────────────────────────────────────────────┘
```

All network I/O happens in Rust. This avoids webview CORS restrictions entirely and
keeps the API key out of the frontend.

### Units

**`config.rs`** — `Settings { base_url, poll_interval_ms, thresholds: Thresholds }`
persisted as JSON in the OS app-config dir. API key stored separately via `keyring`
(service `llama-swap-monitor`, user = normalized base URL). `has_key()` exposed; the key
itself never crosses into the webview.

**`client.rs`** — `LlamaSwapClient { base_url, key: Option<String>, http: reqwest::Client }`.
3 s request timeout. One method per endpoint returning typed structs. All response
structs use `#[serde(default)]` and ignore unknown fields so different llama-swap
versions parse. Errors map to `ClientError::{Unreachable, Unauthorized, NotFound, Http(u16), Decode}`.

**`poller.rs`** — every `poll_interval_ms` (default 2000): `health`, `running`, `stats`,
`activity` concurrently; `models` every 30 s; `version` once per (re)connection.
Backoff on `Unreachable`: 2 → 4 → 8 → 16 → 30 s cap, reset on success.

**`events.rs`** — holds one streaming GET to `/api/events`, parses SSE frames,
decodes the envelope, and applies `modelStatus` / `inflight` updates to an in-memory
`InflightTable` (map id → entry, plus `last_bytes_change` instant per id). Ignores other
message types. Reconnects with the same backoff; on reconnect the snapshot replaces the
table.

**`state.rs`** — pure, synchronous, no I/O:

```rust
fn derive_state(
    model: &ModelInfo,             // from /v1/models + /running
    inflight: &[InflightView],     // entries for this model, with last_bytes_change
    since_state_change: Duration,  // how long the model has been in its current llama-swap state
    t: &Thresholds,
) -> ModelState
```

| `ModelState` | Rule (evaluated top to bottom) |
|---|---|
| `Stalled { reason }` | `starting` longer than `load_timeout` (default 120 s); or `stopping` longer than `stop_timeout` (30 s); or any in-flight request with `resp_bytes == 0` older than `first_byte_timeout` (90 s); or with `resp_bytes > 0` and no growth for `stream_stall_timeout` (30 s) |
| `Loading { elapsed }` | `starting` |
| `Unloading` | `stopping` |
| `Busy { requests, oldest_elapsed }` | `ready` and ≥1 in-flight |
| `Idle { uptime }` | `ready` and no in-flight |
| `NotLoaded` | not in `/running` (or `stopped`/`shutdown`) |

**`monitor.rs`** — owns the latest poll results, the in-flight table, and per-model
"state entered at" instants. On each poll tick or SSE update it builds a `Snapshot` and
emits it as Tauri event `snapshot`:

```rust
struct Snapshot {
    connection: Connection,        // Connected{latency_ms} | Unauthorized | Unreachable | Error(String)
    last_ok: Option<SystemTime>,
    version: Option<String>,
    models: Vec<ModelCard>,        // id, name, state: ModelState, ttl_s, tok_s_history: Vec<f32>, last_request_at
    stats: Option<StatsSummary>,   // totals + gen/prompt p50/p95/p99 + gen histogram bins
    stats_available: bool,         // false if endpoint 404s on this llama-swap version
}
```

`tok_s_history` per model = `tokens_per_second` from the last ~30 activity entries for
that model. TTL countdown for idle models = `ttl - (now - last_request_at)`, an estimate
labeled as such.

**`commands.rs`** — Tauri commands:
- `get_settings() -> SettingsView` (includes `has_key`, never the key)
- `save_settings(settings, api_key: Option<String>)` — restarts the monitor
- `test_connection(base_url, api_key) -> TestResult` — `/health` then `/api/version`;
  returns `Ok{version}` | `Unreachable` | `Unauthorized` | `NotLlamaSwap`

**Frontend (`src/`)**
- `main.ts` — routes between Setup and Dashboard; subscribes to `snapshot`.
- `views/setup.ts` — URL, API key, poll interval, Test connection. Save enabled only after a successful test.
- `views/dashboard.ts` — header (status dot, host, version, latency, settings gear),
  model card grid, stat tiles, histogram.
- `views/components/` — `ring.ts`, `sparkline.ts`, `ttlBar.ts`, `histogram.ts`,
  `statTile.ts`. Each is a pure function `(data) -> SVGElement | HTMLElement`.
- `styles.css` — CSS variables, light/dark via `prefers-color-scheme`.

### Visual language

| State | Ring | Pill | Card extras |
|---|---|---|---|
| Not loaded | thin grey outline | grey "Not loaded" | dimmed |
| Loading | amber dashed, pulsing; elapsed seconds in center | amber | "Starting for 38 s · timeout 120 s" |
| Idle | solid green | green | uptime, TTL bar |
| Busy | blue arc spinning; in-flight count in center | blue | tok/s sparkline, "2 requests · 41.8 tok/s · 6.2 s" |
| Stalled | solid red, "!" | red | red border, reason text |
| Unloading | grey fading | grey | — |

Disconnected: header dot changes color and text; cards keep last data, desaturated,
with "Last seen 12 s ago".

## Error handling

- Network failures never panic; every client call returns `Result`.
- Unreachable → backoff, dashboard shows stale state with age.
- 401 → `Unauthorized` banner with "Open settings" button; polling pauses until settings change.
- 404 on optional endpoints (`/api/metrics/*`, `/api/events`) → feature marked unavailable for that version; core status still works off `/running`. Without SSE, Busy/Stalled-by-request cannot be detected and the UI says so.
- Malformed JSON → logged (without secrets), that field treated as unavailable for the tick.
- The API key is never logged, never emitted, never stored in the settings JSON.

## Testing

- **Unit (Rust):** `derive_state` table-driven tests covering every row and boundary
  (exactly-at-threshold, multiple in-flight, zero-byte vs streaming stall). Parsing tests
  against fixture JSON captured from real llama-swap responses (`src-tauri/tests/fixtures/`).
  SSE frame parser tests (multi-line data, keep-alives, partial chunks).
- **Integration (Rust):** `wiremock` server simulating: healthy instance, 401, timeout,
  missing `/api/metrics/stats`, SSE stream that drops and reconnects.
- **Manual:** run against a real llama-swap; trigger load, request, long request, unload.
- No frontend test framework in phase 1.

## Distribution

GitHub Actions workflow using `tauri-apps/tauri-action` on tag push (`v*`), matrix
`windows-latest`, `macos-latest` (universal), `ubuntu-22.04`. Uploads installers to a
draft GitHub Release. Unsigned in phase 1; README documents SmartScreen/Gatekeeper
workarounds.

## Project layout

```
llama-swap-monitor/
├── src/                     # TS frontend
│   ├── main.ts
│   ├── styles.css
│   └── views/{setup,dashboard}.ts, views/components/*.ts
├── src-tauri/
│   ├── src/{main,lib,config,client,poller,events,state,monitor,commands}.rs
│   ├── tests/{fixtures/,integration.rs}
│   ├── Cargo.toml
│   └── tauri.conf.json
├── .github/workflows/release.yml
├── index.html, package.json, vite.config.ts, tsconfig.json
└── README.md
```
