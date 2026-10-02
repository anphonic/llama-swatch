# llama-swap Monitor Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A cross-platform Tauri 2 desktop app that connects to any llama-swap instance (URL + optional API key) and shows connection health, every model's live state (Not loaded / Loading / Idle / Busy / Stalled / Unloading) with animated visuals, and a stats summary.

**Architecture:** A Rust backend owns all network I/O. It polls llama-swap's REST endpoints every 2 s, holds one SSE stream to `/api/events` for in-flight requests, merges both into a `Snapshot` using a pure `derive_state` function, and emits that snapshot to the webview as a Tauri event. The vanilla TypeScript frontend renders snapshots with hand-written SVG/CSS components and never sees the API key.

**Tech Stack:** Tauri 2, Rust (reqwest 0.12 + rustls, tokio, serde, keyring 3, thiserror 2, chrono), wiremock 0.6 for tests, TypeScript 5.9 + Vite 8, no UI framework.

**Spec:** `docs/superpowers/specs/2026-10-02-llama-swap-monitor-design.md`

## Global Constraints

- Tauri **2.x** only (not 3.0 alpha). `@tauri-apps/cli` and `@tauri-apps/api` `^2.12.1`.
- Targets: Windows, macOS, Linux. Nothing may be platform-specific except bundling and CI.
- All network I/O happens in Rust. The webview never receives the API key.
- The API key lives in the OS keychain under service `llama-swap-monitor`, account = the normalized base URL. It never appears in `settings.json`, logs, emitted events, error strings, or `Debug` output.
- When a key is configured, send `Authorization: Bearer <key>` on every request.
- Settings are stored as `settings.json` in the Tauri app-config directory.
- Per-request timeout is 3 s. Default poll interval is 2000 ms, clamped to 500–60000 ms.
- Unreachable backoff: 2 → 4 → 8 → 16 → 30 s cap, reset on success.
- `/v1/models` is refreshed every 30 s. Version is fetched once per (re)connection. On 401, retry every 30 s.
- Threshold defaults: `load_timeout` 120 s, `stop_timeout` 30 s, `first_byte_timeout` 90 s, `stream_stall_timeout` 30 s. All are strict `>`.
- Every response struct uses `#[serde(default)]` and ignores unknown fields. JSON `null` arrays parse as empty.
- No frontend framework and no chart library. Server-supplied strings are rendered only via `textContent` / `append(string)`, never `innerHTML`.
- Every commit message ends with the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Run every command from the repo root (`J:\Projects\llama-swap-monitor`) in Git Bash.

## Review Focus

1. **User pastes the OpenAI-style URL** (`http://box:8080/v1/`, `box:8080`, trailing slashes, `?query`). Expected: it is normalized to `http://box:8080` and works. Pinned by Task 6, `normalize_base_url_cases`.
2. **A fresh llama-swap with no traffic returns `null` instead of `[]`** (`"data": null`, `"bins": null`). Expected: the app parses it as empty and the dashboard still renders. Pinned by Task 2, `null_arrays_parse_as_empty`.
3. **The user changes the URL in settings but leaves the key field blank.** Expected: the saved key moves to the new URL instead of being silently dropped, which would cause a surprise 401. Pinned by Task 11, `url_change_with_blank_key_migrates_key`.
4. **The URL points at something that isn't llama-swap** (another web app on :8080 returning HTML 200, or a 404 on `/health`). Expected: "doesn't look like llama-swap", not "bad API key" or a crash. Pinned by Task 7, `test_connection_rejects_non_llama_swap`.
5. **llama-swap restarts mid-request, so the SSE stream drops.** Expected: in-flight entries are cleared, so the model does not stay Busy, or turn Stalled, from ghost requests. Pinned by Task 10, `dropped_event_stream_clears_busy`.

---

## File Structure

```
llama-swap-monitor/
├── .github/workflows/{ci.yml,release.yml}      Task 14
├── .gitignore, app-icon.svg                    Task 1
├── index.html, package.json, package-lock.json,
│   tsconfig.json, vite.config.ts               Task 1
├── README.md                                   Task 14
├── src/                                        (frontend)
│   ├── main.ts            router: setup ⇄ dashboard, snapshot subscription   Task 12
│   ├── api.ts             typed invoke/listen wrappers                       Task 12
│   ├── types.ts           TS mirrors of Rust Snapshot/Settings/TestResult    Task 12
│   ├── dom.ts             h() / svg() element builders (textContent only)    Task 12
│   ├── format.ts          duration / count / "ago" formatting                Task 12
│   ├── styles.css         tokens, light/dark, all component styles           Task 12
│   └── views/
│       ├── setup.ts       settings form + test-before-save                   Task 12
│       ├── dashboard.ts   header, keyed model cards, stats                   Task 13
│       └── components/{ring,sparkline,ttlBar,histogram,statTile}.ts          Task 13
└── src-tauri/
    ├── Cargo.toml, build.rs, tauri.conf.json, capabilities/default.json, icons/   Task 1
    ├── src/
    │   ├── main.rs        calls lib::run()                                   Task 1
    │   ├── lib.rs         module list + Tauri builder                        Task 1, 11
    │   ├── api.rs         llama-swap wire types                              Task 2
    │   ├── sse.rs         byte-level SSE frame parser                        Task 3
    │   ├── events.rs      envelope decode + InflightTable                    Task 4
    │   ├── state.rs       Thresholds, ModelState, derive_state               Task 5
    │   ├── config.rs      Settings, URL normalization, SecretStore           Task 6
    │   ├── client.rs      LlamaSwapClient, ClientError, test_connection      Task 7
    │   ├── poller.rs      poll_once → PollOutcome                            Task 8
    │   ├── monitor.rs     MonitorState → Snapshot (pure)                     Task 9
    │   ├── backoff.rs     Backoff                                            Task 10
    │   ├── runtime.rs     poll + event loops, SnapshotSink, MonitorHandle    Task 10
    │   └── commands.rs    AppState, Tauri commands, settings update logic    Task 11
    └── tests/
        ├── fixtures/{version,running,models,stats,activity}.json             Task 2
        ├── common/mod.rs  wiremock helpers                                   Task 7
        ├── client.rs                                                         Task 7
        ├── poller.rs                                                         Task 8
        └── runtime.rs                                                        Task 10
```

Differences from the spec's unit list, made for focus:

- The spec's `events.rs` is split into `sse.rs` (framing) and `events.rs` (meaning).
- The spec's `monitor.rs` is split into `monitor.rs` (pure merge logic) and `runtime.rs` + `backoff.rs` (async loops).
- Wire types get their own `api.rs`.

---

### Task 1: Project scaffold

**Files:**
- Create: `.gitignore`, `package.json`, `tsconfig.json`, `vite.config.ts`, `index.html`, `src/main.ts`, `app-icon.svg`
- Create: `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`, `src-tauri/.gitignore`
- Generated: `package-lock.json`, `src-tauri/Cargo.lock`, `src-tauri/icons/*`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - Rust library crate `llama_swap_monitor_lib` with `pub fn run()`.
  - The full dependency set, which later tasks use without editing `Cargo.toml`.
  - npm scripts `dev`, `build`, `tauri`.
  - Window label `main`.

- [ ] **Step 1: Create a working branch**

```bash
git switch -c phase-1
```

- [ ] **Step 2: Write `package.json`**

```json
{
  "name": "llama-swap-monitor",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc && vite build",
    "preview": "vite preview",
    "tauri": "tauri"
  },
  "dependencies": {
    "@tauri-apps/api": "^2.12.1"
  },
  "devDependencies": {
    "@tauri-apps/cli": "^2.12.1",
    "typescript": "~5.9.3",
    "vite": "^8.3.0"
  }
}
```

- [ ] **Step 3: Install and generate the lockfile**

Run: `npm install`
Expected: `added N packages`. `package-lock.json` exists.

- [ ] **Step 4: Write `tsconfig.json`, `vite.config.ts`, `index.html`, `src/main.ts`**

`tsconfig.json`:
```json
{
  "compilerOptions": {
    "target": "ES2021",
    "useDefineForClassFields": true,
    "module": "ESNext",
    "lib": ["ES2021", "DOM", "DOM.Iterable"],
    "skipLibCheck": true,
    "moduleResolution": "bundler",
    "allowImportingTsExtensions": true,
    "isolatedModules": true,
    "noEmit": true,
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true
  },
  "include": ["src"]
}
```

`vite.config.ts`:
```ts
import { defineConfig } from "vite";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    target: "es2021",
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
```

`index.html`:
```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>llama-swap Monitor</title>
    <script type="module" src="/src/main.ts"></script>
  </head>
  <body>
    <div id="app"></div>
  </body>
</html>
```

`src/main.ts` (placeholder, replaced in Task 12):
```ts
document.getElementById("app")!.textContent = "llama-swap Monitor";
```

- [ ] **Step 5: Write `.gitignore` and `src-tauri/.gitignore`**

`.gitignore`:
```
node_modules/
dist/
.DS_Store
```

`src-tauri/.gitignore`:
```
/target/
/gen/schemas
```

- [ ] **Step 6: Write `src-tauri/Cargo.toml`**

```toml
[package]
name = "llama-swap-monitor"
version = "0.1.0"
description = "Desktop monitor for llama-swap"
edition = "2021"
rust-version = "1.77.2"

[lib]
name = "llama_swap_monitor_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = [] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.12", default-features = false, features = ["json", "stream", "rustls-tls"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "time", "sync"] }
futures-util = "0.3"
thiserror = "2"
chrono = { version = "0.4", default-features = false, features = ["std", "clock"] }
keyring = { version = "3", features = ["apple-native", "windows-native", "sync-secret-service", "crypto-rust"] }

[dev-dependencies]
wiremock = "0.6"
tempfile = "3"
```

> keyring 3 uses an in-memory mock store unless a platform feature is enabled. The four features above select the real store on each OS.

- [ ] **Step 7: Write `build.rs`, `main.rs`, `lib.rs`**

`src-tauri/build.rs`:
```rust
fn main() {
    tauri_build::build()
}
```

`src-tauri/src/main.rs`:
```rust
// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    llama_swap_monitor_lib::run()
}
```

`src-tauri/src/lib.rs`:
```rust
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 8: Write `tauri.conf.json` and the capability file**

`src-tauri/tauri.conf.json`:
```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "llama-swap Monitor",
  "version": "0.1.0",
  "identifier": "dev.llamaswap.monitor",
  "build": {
    "beforeDevCommand": "npm run dev",
    "devUrl": "http://localhost:1420",
    "beforeBuildCommand": "npm run build",
    "frontendDist": "../dist"
  },
  "app": {
    "windows": [
      {
        "label": "main",
        "title": "llama-swap Monitor",
        "width": 1100,
        "height": 760,
        "minWidth": 420,
        "minHeight": 400
      }
    ],
    "security": {
      "csp": "default-src 'self' ipc: http://ipc.localhost; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src ipc: http://ipc.localhost",
      "devCsp": null
    }
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ],
    "macOS": {
      "signingIdentity": "-"
    }
  }
}
```

> `signingIdentity: "-"` ad-hoc signs the macOS build. Apple Silicon refuses to run a fully unsigned arm64 binary at all.

`src-tauri/capabilities/default.json`:
```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Main window",
  "windows": ["main"],
  "permissions": ["core:default"]
}
```

- [ ] **Step 9: Create the app icon and generate the platform icons**

`app-icon.svg`:
```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">
  <rect width="1024" height="1024" rx="220" fill="#111827"/>
  <circle cx="512" cy="512" r="300" fill="none" stroke="#1f2937" stroke-width="90"/>
  <circle cx="512" cy="512" r="300" fill="none" stroke="#22c55e" stroke-width="90"
          stroke-linecap="round" stroke-dasharray="1300 1885" transform="rotate(-90 512 512)"/>
  <circle cx="512" cy="512" r="110" fill="#3b82f6"/>
</svg>
```

Run: `npx tauri icon app-icon.svg`
Expected: `src-tauri/icons/` contains `32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.icns`, `icon.ico` and more.

- [ ] **Step 10: Verify the backend and frontend both build**

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: `Finished` with no errors. The first build takes several minutes.

Run: `npm run build`
Expected: `tsc` passes and `vite` writes `dist/index.html`.

- [ ] **Step 11: Verify the window opens**

Run: `npm run tauri dev`
Expected: a window titled "llama-swap Monitor" shows the text "llama-swap Monitor". Close it.

- [ ] **Step 12: Commit**

```bash
git add .gitignore package.json package-lock.json tsconfig.json vite.config.ts index.html src app-icon.svg src-tauri
git commit -m "chore: scaffold Tauri 2 + Vite TypeScript app" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: llama-swap wire types

**Files:**
- Create: `src-tauri/src/api.rs`
- Create: `src-tauri/tests/fixtures/version.json`, `running.json`, `models.json`, `stats.json`, `activity.json`
- Modify: `src-tauri/src/lib.rs` (add `pub mod api;`)

**Interfaces:**
- Consumes: nothing.
- Produces (all derive `Debug, Clone, Default, PartialEq, Deserialize`; `Histogram` also derives `Serialize`):
  - `pub fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>` where `T: Default + Deserialize<'de>`.
  - `VersionInfo { version: String, commit: String, build_date: String }`
  - `RunningResponse { running: Vec<RunningModel> }`, `RunningModel { model, name, description, state: String, ttl: i64 }`
  - `ModelsResponse { data: Vec<ModelEntry> }`, `ModelEntry { id, name, description: String }`
  - `StatsResponse { total_requests, total_input_tokens, total_output_tokens, total_cache_tokens: i64, prompt_histogram: Option<Histogram>, gen_histogram: Option<Histogram> }`
  - `Histogram { bins: Vec<i64>, min, max, bin_size (JSON "binSize"), p50, p95, p99: f64 }`
  - `ActivityResponse { data: Vec<ActivityEntry>, total: i64 }`
  - `ActivityEntry { id: i64, timestamp: String, model, req_path: String, resp_status_code: i64, duration_ms: i64, tokens: TokenMetrics }` plus `fn timestamp_ms(&self) -> Option<i64>`
  - `TokenMetrics { input_tokens, output_tokens, cache_tokens: i64, prompt_per_second, tokens_per_second: f64 }`

- [ ] **Step 1: Write the fixtures**

`src-tauri/tests/fixtures/version.json`:
```json
{"version":"v188","commit":"abc1234","build_date":"2026-09-20T12:00:00Z"}
```

`src-tauri/tests/fixtures/running.json`:
```json
{"running":[
  {"model":"embed","name":"","description":"","state":"starting","ttl":0,"cmd":"llama-server --port 9002","proxy":"http://127.0.0.1:9002"},
  {"model":"qwen3-30b","name":"Qwen3 30B","description":"general chat","state":"ready","ttl":300,"cmd":"llama-server --port 9001 -m qwen.gguf","proxy":"http://127.0.0.1:9001"}
]}
```

`src-tauri/tests/fixtures/models.json`:
```json
{"object":"list","data":[
  {"id":"embed","object":"model","created":1759400000,"owned_by":"llama-swap","name":"","description":""},
  {"id":"gemma-12b","object":"model","created":1759400000,"owned_by":"llama-swap","name":"Gemma 12B","description":"vision"},
  {"id":"qwen3-30b","object":"model","created":1759400000,"owned_by":"llama-swap","name":"Qwen3 30B","description":"general chat","meta":{"llamaswap":{"aliases":["qwen"]}}}
]}
```

`src-tauri/tests/fixtures/stats.json`:
```json
{"total_requests":42,"total_input_tokens":51234,"total_output_tokens":20480,"total_cache_tokens":8000,
 "prompt_histogram":{"bins":[1,4,9,3],"min":100,"max":900,"binSize":200,"p50":420.5,"p95":810.2,"p99":880.0},
 "gen_histogram":{"bins":[2,7,12,5,1],"min":10,"max":60,"binSize":10,"p50":38.4,"p95":52.1,"p99":57.9}}
```

`src-tauri/tests/fixtures/activity.json`:
```json
{"data":[
  {"id":3,"timestamp":"2026-10-02T10:00:30.5-07:00","src":"127.0.0.1","model":"qwen3-30b","req_path":"/v1/chat/completions","resp_content_type":"text/event-stream","resp_status_code":200,"tokens":{"cache_tokens":0,"draft_tokens":0,"draft_acc_tokens":0,"input_tokens":812,"output_tokens":400,"prompt_per_second":610.2,"tokens_per_second":41.8},"duration_ms":10450,"has_capture":false},
  {"id":2,"timestamp":"2026-10-02T10:00:10.123456789-07:00","src":"127.0.0.1","model":"qwen3-30b","req_path":"/v1/chat/completions","resp_content_type":"application/json","resp_status_code":200,"tokens":{"cache_tokens":512,"draft_tokens":0,"draft_acc_tokens":0,"input_tokens":120,"output_tokens":80,"prompt_per_second":590.0,"tokens_per_second":39.5},"duration_ms":2100,"has_capture":false},
  {"id":1,"timestamp":"2026-10-02T09:59:00Z","src":"127.0.0.1","model":"embed","req_path":"/v1/embeddings","resp_content_type":"application/json","resp_status_code":200,"tokens":{"cache_tokens":0,"draft_tokens":0,"draft_acc_tokens":0,"input_tokens":64,"output_tokens":0,"prompt_per_second":0,"tokens_per_second":0},"duration_ms":80,"has_capture":false,"error_msg":""}
],"page":1,"limit":100,"total":3,"total_pages":1}
```

- [ ] **Step 2: Write the failing tests in `src-tauri/src/api.rs`**

```rust
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
```

Add `pub mod api;` to the top of `src-tauri/src/lib.rs`.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib api::`
Expected: FAIL to compile with errors like `cannot find type VersionInfo in this scope`.

- [ ] **Step 4: Implement the types above the test module in `api.rs`**

```rust
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
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib api::`
Expected: `test result: ok. 8 passed`

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/api.rs src-tauri/src/lib.rs src-tauri/tests/fixtures
git commit -m "feat: add llama-swap wire types with null-tolerant parsing" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: SSE frame parser

**Files:**
- Create: `src-tauri/src/sse.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod sse;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct SseParser` (Default).
  - `SseParser::new() -> Self`.
  - `SseParser::push(&mut self, chunk: &[u8]) -> Vec<String>`. Returns the joined `data` payload of every event completed by this chunk.

- [ ] **Step 1: Write the failing tests in `src-tauri/src/sse.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_llama_swap_frame() {
        let mut p = SseParser::new();
        let out = p.push(b"event:message\ndata:{\"type\":\"x\"}\n\n");
        assert_eq!(out, vec![r#"{"type":"x"}"#]);
    }

    #[test]
    fn two_frames_in_one_chunk() {
        let mut p = SseParser::new();
        let out = p.push(b"data:a\n\ndata:b\n\n");
        assert_eq!(out, vec!["a", "b"]);
    }

    #[test]
    fn frame_split_across_chunks() {
        let mut p = SseParser::new();
        assert!(p.push(b"event:mess").is_empty());
        assert!(p.push(b"age\ndata:hel").is_empty());
        assert!(p.push(b"lo\n").is_empty());
        assert_eq!(p.push(b"\n"), vec!["hello"]);
    }

    #[test]
    fn multibyte_utf8_split_across_chunks() {
        let bytes = "data:café\n\n".as_bytes();
        let split = bytes.iter().position(|&b| b == 0xC3).unwrap() + 1; // inside 'é'
        let mut p = SseParser::new();
        assert!(p.push(&bytes[..split]).is_empty());
        assert_eq!(p.push(&bytes[split..]), vec!["café"]);
    }

    #[test]
    fn crlf_line_endings() {
        let mut p = SseParser::new();
        assert_eq!(p.push(b"data: x\r\n\r\n"), vec!["x"]);
    }

    #[test]
    fn single_leading_space_is_stripped_only_once() {
        let mut p = SseParser::new();
        assert_eq!(p.push(b"data:  two\n\n"), vec![" two"]);
    }

    #[test]
    fn multi_line_data_is_joined_with_newline() {
        let mut p = SseParser::new();
        assert_eq!(p.push(b"data:line1\ndata:line2\n\n"), vec!["line1\nline2"]);
    }

    #[test]
    fn comments_and_keepalives_are_ignored() {
        let mut p = SseParser::new();
        assert!(p.push(b": keep-alive\n\n").is_empty());
        assert_eq!(p.push(b":ping\ndata:x\n\n"), vec!["x"]);
    }

    #[test]
    fn event_without_data_is_dropped() {
        let mut p = SseParser::new();
        assert!(p.push(b"event:message\nid:7\n\n").is_empty());
    }
}
```

Add `pub mod sse;` to `src-tauri/src/lib.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib sse::`
Expected: FAIL to compile with `cannot find struct SseParser`.

- [ ] **Step 3: Implement the parser above the tests**

```rust
//! Minimal Server-Sent Events parser.
//!
//! Feed raw bytes as they arrive from the network. Bytes are buffered until a
//! full line is available, so chunks may split lines or even UTF-8 code points.
//! Only the `data` field matters to us; `event`, `id`, `retry` and comments are ignored.

#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    data: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop(); // '\n'
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line);
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push(self.data.join("\n"));
                    self.data.clear();
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = match line.find(':') {
                Some(i) => {
                    let raw = &line[i + 1..];
                    (&line[..i], raw.strip_prefix(' ').unwrap_or(raw))
                }
                None => (&line[..], ""),
            };
            if field == "data" {
                self.data.push(value.to_string());
            }
        }
        out
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib sse::`
Expected: `test result: ok. 9 passed`

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/sse.rs src-tauri/src/lib.rs
git commit -m "feat: add byte-level SSE frame parser" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Event decoding and in-flight table

**Files:**
- Create: `src-tauri/src/events.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod events;`)

**Interfaces:**
- Consumes: `crate::api::null_default`.
- Produces:
  - `InflightEntry { id: String, model: String, req_path: String, method: String, resp_bytes: i64, elapsed_ms: i64 }` (Debug, Clone, Default, PartialEq, Deserialize).
  - `InflightMsg { operation: String, requests: Vec<InflightEntry>, request: Option<InflightEntry>, id: String }`.
  - `enum StreamEvent { ModelStatus, Inflight(InflightMsg), Other }`.
  - `pub fn decode_event(payload: &str) -> Result<StreamEvent, serde_json::Error>`.
  - `InflightView { elapsed: Duration, resp_bytes: i64, since_bytes_change: Duration }` (Debug, Clone, Copy, PartialEq).
  - `InflightTable` (Debug, Default) with:
    - `apply(&mut self, msg: InflightMsg, now: Instant)`
    - `views_for(&self, model: &str, now: Instant) -> Vec<InflightView>`
    - `clear(&mut self)`
    - `len(&self) -> usize`

- [ ] **Step 1: Write the failing tests in `src-tauri/src/events.rs`**

```rust
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
```

Add `pub mod events;` to `src-tauri/src/lib.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib events::`
Expected: FAIL to compile with `cannot find function decode_event`.

- [ ] **Step 3: Implement above the tests**

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib events::`
Expected: `test result: ok. 8 passed`

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/events.rs src-tauri/src/lib.rs
git commit -m "feat: decode llama-swap events and track in-flight requests" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Model state derivation

**Files:**
- Create: `src-tauri/src/state.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod state;`)

**Interfaces:**
- Consumes: `crate::events::InflightView`.
- Produces:
  - `Thresholds { load_timeout_s, stop_timeout_s, first_byte_timeout_s, stream_stall_timeout_s: u64 }`. Derives Debug, Clone, Copy, PartialEq, Serialize, Deserialize; camelCase; defaults 120/30/90/30.
  - `enum ModelState { NotLoaded, Loading { elapsed_s: u64 }, Idle { uptime_s: u64 }, Busy { requests: usize, oldest_elapsed_s: u64 }, Stalled { reason: String }, Unloading }`. Serialized as `{"kind":"busy","requests":2,"oldestElapsedS":9}`.
  - `pub fn derive_state(process_state: Option<&str>, inflight: &[InflightView], since_state_change: Duration, t: &Thresholds) -> ModelState`.

- [ ] **Step 1: Write the failing tests in `src-tauri/src/state.rs`**

```rust
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
```

Add `pub mod state;` to `src-tauri/src/lib.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib state::`
Expected: FAIL to compile with `cannot find function derive_state`.

- [ ] **Step 3: Implement above the tests**

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib state::`
Expected: `test result: ok. 3 passed`

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/state.rs src-tauri/src/lib.rs
git commit -m "feat: derive model state with stall detection" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Settings, URL normalization, secret store

**Files:**
- Create: `src-tauri/src/config.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod config;`)

**Interfaces:**
- Consumes: `crate::state::Thresholds`.
- Produces:
  - Constants: `KEYRING_SERVICE: &str = "llama-swap-monitor"`, `MIN_POLL_MS = 500`, `MAX_POLL_MS = 60_000`.
  - `enum ConfigError { InvalidUrl(String), Keychain(String), Io(io::Error) }` (thiserror, Display).
  - `Settings { base_url: String, poll_interval_ms: u64, thresholds: Thresholds }`. Derives Debug, Clone, PartialEq, Serialize, Deserialize; camelCase; default `http://localhost:8080` / 2000.
    - `Settings::sanitized(self) -> Result<Settings, ConfigError>`.
  - `pub fn normalize_base_url(input: &str) -> Result<String, ConfigError>`.
  - `pub fn load_settings(path: &Path) -> Option<Settings>`. Returns `None` when the file is missing, corrupt, or invalid.
  - `pub fn save_settings(path: &Path, s: &Settings) -> Result<(), ConfigError>`.
  - `pub trait SecretStore: Send + Sync` with:
    - `get(&self, account: &str) -> Result<Option<String>, ConfigError>`
    - `set(&self, account: &str, secret: &str) -> Result<(), ConfigError>`
    - `delete(&self, account: &str) -> Result<(), ConfigError>`
  - `pub struct KeyringStore;` and `#[derive(Default)] pub struct MemoryStore`.

- [ ] **Step 1: Write the failing tests in `src-tauri/src/config.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_base_url_cases() {
        let ok = [
            ("localhost:8080", "http://localhost:8080"),
            ("http://192.168.1.5:8080/", "http://192.168.1.5:8080"),
            ("  https://llm.example.com/v1  ", "https://llm.example.com"),
            ("http://box:8080/v1/", "http://box:8080"),
            ("https://example.com/llama-swap/", "https://example.com/llama-swap"),
            ("http://box:8080/?x=1#frag", "http://box:8080"),
            ("HTTP://Box:8080", "http://box:8080"),
            ("http://[::1]:8080", "http://[::1]:8080"),
        ];
        for (input, expected) in ok {
            assert_eq!(normalize_base_url(input).unwrap(), expected, "input: {input:?}");
        }
        for bad in ["", "   ", "ftp://box", "http://", "http://exa mple.com"] {
            assert!(normalize_base_url(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn sanitized_clamps_poll_interval() {
        let s = Settings { poll_interval_ms: 100, ..Default::default() }.sanitized().unwrap();
        assert_eq!(s.poll_interval_ms, MIN_POLL_MS);
        let s = Settings { poll_interval_ms: 9_999_999, ..Default::default() }.sanitized().unwrap();
        assert_eq!(s.poll_interval_ms, MAX_POLL_MS);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.json");
        let s = Settings { base_url: "http://box:8080".into(), poll_interval_ms: 3000, thresholds: Thresholds::default() };
        save_settings(&path, &s).unwrap();
        assert_eq!(load_settings(&path), Some(s));
        assert!(!path.with_extension("json.tmp").exists(), "temp file is renamed away");
    }

    #[test]
    fn missing_or_corrupt_file_is_unconfigured() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(load_settings(&path), None);
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_settings(&path), None);
        std::fs::write(&path, r#"{"baseUrl":"ftp://nope"}"#).unwrap();
        assert_eq!(load_settings(&path), None);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"baseUrl":"box:9000"}"#).unwrap();
        let s = load_settings(&path).unwrap();
        assert_eq!(s.base_url, "http://box:9000");
        assert_eq!(s.poll_interval_ms, 2000);
        assert_eq!(s.thresholds, Thresholds::default());
    }

    #[test]
    fn memory_store_get_set_delete() {
        let store = MemoryStore::default();
        assert_eq!(store.get("a").unwrap(), None);
        store.set("a", "k1").unwrap();
        assert_eq!(store.get("a").unwrap(), Some("k1".into()));
        store.delete("a").unwrap();
        store.delete("a").unwrap(); // deleting a missing key is fine
        assert_eq!(store.get("a").unwrap(), None);
    }
}
```

Add `pub mod config;` to `src-tauri/src/lib.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib config::`
Expected: FAIL to compile with `cannot find function normalize_base_url`.

- [ ] **Step 3: Implement above the tests**

```rust
//! User settings (JSON file) and API-key storage (OS keychain).

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::state::Thresholds;

pub const KEYRING_SERVICE: &str = "llama-swap-monitor";
pub const MIN_POLL_MS: u64 = 500;
pub const MAX_POLL_MS: u64 = 60_000;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("could not access the OS keychain: {0}")]
    Keychain(String),
    #[error("could not write settings: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub base_url: String,
    pub poll_interval_ms: u64,
    pub thresholds: Thresholds,
}

impl Default for Settings {
    fn default() -> Self {
        Self { base_url: "http://localhost:8080".into(), poll_interval_ms: 2000, thresholds: Thresholds::default() }
    }
}

impl Settings {
    pub fn sanitized(mut self) -> Result<Self, ConfigError> {
        self.base_url = normalize_base_url(&self.base_url)?;
        self.poll_interval_ms = self.poll_interval_ms.clamp(MIN_POLL_MS, MAX_POLL_MS);
        Ok(self)
    }
}

/// Accepts what people paste: missing scheme, trailing slashes, an
/// OpenAI-style `/v1` suffix, stray query strings. Keeps any reverse-proxy
/// path prefix. Returns `scheme://host[:port][/prefix]` with no trailing slash.
pub fn normalize_base_url(input: &str) -> Result<String, ConfigError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ConfigError::InvalidUrl("URL is empty".into()));
    }
    let with_scheme = if trimmed.contains("://") { trimmed.to_string() } else { format!("http://{trimmed}") };
    let mut url = reqwest::Url::parse(&with_scheme).map_err(|e| ConfigError::InvalidUrl(e.to_string()))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(ConfigError::InvalidUrl("use http:// or https://".into()));
    }
    if url.host_str().map_or(true, str::is_empty) {
        return Err(ConfigError::InvalidUrl("missing host".into()));
    }
    url.set_query(None);
    url.set_fragment(None);
    let mut path = url.path().trim_end_matches('/').to_string();
    if path.ends_with("/v1") {
        path.truncate(path.len() - 3);
    }
    url.set_path(&path);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

pub fn load_settings(path: &Path) -> Option<Settings> {
    let text = fs::read_to_string(path).ok()?;
    let settings: Settings = serde_json::from_str(&text).ok()?;
    settings.sanitized().ok()
}

/// Atomic write: temp file then rename, so a crash never leaves half a file.
pub fn save_settings(path: &Path, s: &Settings) -> Result<(), ConfigError> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(s).map_err(io::Error::other)?;
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

pub trait SecretStore: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<String>, ConfigError>;
    fn set(&self, account: &str, secret: &str) -> Result<(), ConfigError>;
    fn delete(&self, account: &str) -> Result<(), ConfigError>;
}

/// Windows Credential Manager, macOS Keychain, or Linux Secret Service.
pub struct KeyringStore;

fn keychain_err(e: keyring::Error) -> ConfigError {
    ConfigError::Keychain(e.to_string())
}

impl SecretStore for KeyringStore {
    fn get(&self, account: &str) -> Result<Option<String>, ConfigError> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, account).map_err(keychain_err)?;
        match entry.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keychain_err(e)),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), ConfigError> {
        keyring::Entry::new(KEYRING_SERVICE, account)
            .and_then(|e| e.set_password(secret))
            .map_err(keychain_err)
    }

    fn delete(&self, account: &str) -> Result<(), ConfigError> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, account).map_err(keychain_err)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keychain_err(e)),
        }
    }
}

/// Test double for `KeyringStore`.
#[derive(Default)]
pub struct MemoryStore {
    map: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemoryStore {
    fn get(&self, account: &str) -> Result<Option<String>, ConfigError> {
        Ok(self.map.lock().unwrap().get(account).cloned())
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), ConfigError> {
        self.map.lock().unwrap().insert(account.into(), secret.into());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), ConfigError> {
        self.map.lock().unwrap().remove(account);
        Ok(())
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib config::`
Expected: `test result: ok. 6 passed`. If the `[::1]` case fails because the `url` crate formats IPv6 differently, check what `url.as_str()` produces and keep whatever bracketed form the crate emits, as long as the result is still a valid URL.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/config.rs src-tauri/src/lib.rs
git commit -m "feat: add settings persistence, URL normalization and keychain store" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: HTTP client and connection test

**Files:**
- Create: `src-tauri/src/client.rs`
- Create: `src-tauri/tests/common/mod.rs`, `src-tauri/tests/client.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod client;`)

**Interfaces:**
- Consumes: `crate::api::*` types.
- Produces:
  - `enum ClientError { Unreachable(String), Unauthorized, NotFound, Http(u16), Decode(String) }` (Debug, Clone, PartialEq, thiserror).
  - `LlamaSwapClient` (Clone, manual `Debug` that redacts the key) with:
    - `new(base_url: &str, key: Option<String>) -> Result<Self, ClientError>`
    - `base_url(&self) -> &str`
    - `async health(&self) -> Result<(String, Duration), ClientError>`
    - `async version(&self) -> Result<VersionInfo, ClientError>`
    - `async running(&self) -> Result<Vec<RunningModel>, ClientError>`
    - `async models(&self) -> Result<Vec<ModelEntry>, ClientError>`
    - `async stats(&self) -> Result<StatsResponse, ClientError>`
    - `async activity(&self, limit: u32) -> Result<Vec<ActivityEntry>, ClientError>`
    - `async events(&self) -> Result<reqwest::Response, ClientError>` (no total timeout; the stream stays open)
  - `enum TestResult { Ok { version: String }, Unreachable { message: String }, Unauthorized, NotLlamaSwap { message: String } }`. Serialized as `{"kind":"ok","version":…}` / `{"kind":"notLlamaSwap","message":…}`.
  - `pub async fn test_connection(client: &LlamaSwapClient) -> TestResult`.
  - Test helpers in `tests/common/mod.rs`: `mount_healthy(&MockServer)`, `sse_frame(kind, data) -> String`, `closed_port_url() -> String`, fixture consts.

- [ ] **Step 1: Write the shared test helpers `src-tauri/tests/common/mod.rs`**

```rust
#![allow(dead_code)]

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub const VERSION: &str = include_str!("../fixtures/version.json");
pub const RUNNING: &str = include_str!("../fixtures/running.json");
pub const MODELS: &str = include_str!("../fixtures/models.json");
pub const STATS: &str = include_str!("../fixtures/stats.json");
pub const ACTIVITY: &str = include_str!("../fixtures/activity.json");

pub fn json_body(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "application/json")
}

/// Mounts every polled endpoint with fixture data (default priority 5).
/// Tests override individual routes with `.with_priority(1)`.
pub async fn mount_healthy(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200).set_body_string("OK"))
        .mount(server)
        .await;
    for (p, body) in [
        ("/api/version", VERSION),
        ("/running", RUNNING),
        ("/v1/models", MODELS),
        ("/api/metrics/stats", STATS),
        ("/api/metrics/activity", ACTIVITY),
    ] {
        Mock::given(method("GET")).and(path(p)).respond_with(json_body(body)).mount(server).await;
    }
}

/// One llama-swap SSE frame: the inner `data` is JSON encoded as a string.
pub fn sse_frame(kind: &str, data: serde_json::Value) -> String {
    format!("event:message\ndata:{}\n\n", json!({ "type": kind, "data": data.to_string() }))
}

/// A localhost URL with nothing listening on it.
pub fn closed_port_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}
```

- [ ] **Step 2: Write the failing integration tests `src-tauri/tests/client.rs`**

```rust
mod common;

use std::time::Duration;

use common::*;
use llama_swap_monitor_lib::client::{test_connection, ClientError, LlamaSwapClient, TestResult};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn sends_bearer_key_and_parses_version() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .and(header("authorization", "Bearer s3cret"))
        .respond_with(json_body(VERSION))
        .mount(&server)
        .await;
    let client = LlamaSwapClient::new(&server.uri(), Some("s3cret".into())).unwrap();
    assert_eq!(client.version().await.unwrap().version, "v188");
}

#[tokio::test]
async fn parses_all_fixture_endpoints() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    let (body, _latency) = client.health().await.unwrap();
    assert_eq!(body, "OK");
    assert_eq!(client.running().await.unwrap().len(), 2);
    assert_eq!(client.models().await.unwrap().len(), 3);
    assert_eq!(client.stats().await.unwrap().total_requests, 42);
    assert_eq!(client.activity(100).await.unwrap().len(), 3);
}

#[tokio::test]
async fn status_codes_map_to_errors() {
    let server = MockServer::start().await;
    Mock::given(path("/running")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
    Mock::given(path("/v1/models")).respond_with(ResponseTemplate::new(500)).mount(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(client.running().await, Err(ClientError::Unauthorized));
    assert_eq!(client.stats().await, Err(ClientError::NotFound));
    assert_eq!(client.models().await, Err(ClientError::Http(500)));
}

#[tokio::test]
async fn html_body_is_a_decode_error() {
    let server = MockServer::start().await;
    Mock::given(path("/running"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>hi</html>"))
        .mount(&server)
        .await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert!(matches!(client.running().await, Err(ClientError::Decode(_))));
}

#[tokio::test]
async fn closed_port_is_unreachable() {
    let client = LlamaSwapClient::new(&closed_port_url(), None).unwrap();
    assert!(matches!(client.health().await, Err(ClientError::Unreachable(_))));
}

#[tokio::test]
async fn slow_server_times_out_as_unreachable() {
    let server = MockServer::start().await;
    Mock::given(path("/health"))
        .respond_with(ResponseTemplate::new(200).set_body_string("OK").set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(client.health().await, Err(ClientError::Unreachable("timed out".into())));
}

#[tokio::test]
async fn test_connection_ok() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(test_connection(&client).await, TestResult::Ok { version: "v188".into() });
}

#[tokio::test]
async fn test_connection_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(path("/api/version")).respond_with(ResponseTemplate::new(401)).with_priority(1).mount(&server).await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), Some("wrong".into())).unwrap();
    assert_eq!(test_connection(&client).await, TestResult::Unauthorized);
}

#[tokio::test]
async fn test_connection_unreachable() {
    let client = LlamaSwapClient::new(&closed_port_url(), None).unwrap();
    assert!(matches!(test_connection(&client).await, TestResult::Unreachable { .. }));
}

#[tokio::test]
async fn test_connection_rejects_non_llama_swap() {
    // Some other web app that answers 200 with HTML everywhere.
    let html = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<!doctype html><title>Router</title>"))
        .mount(&html)
        .await;
    let client = LlamaSwapClient::new(&html.uri(), None).unwrap();
    assert!(matches!(test_connection(&client).await, TestResult::NotLlamaSwap { .. }));

    // A server with no /health route at all.
    let empty = MockServer::start().await;
    let client = LlamaSwapClient::new(&empty.uri(), None).unwrap();
    assert!(matches!(test_connection(&client).await, TestResult::NotLlamaSwap { .. }));
}

#[tokio::test]
async fn test_connection_old_llama_swap_without_version_endpoint() {
    let server = MockServer::start().await;
    Mock::given(path("/api/version")).respond_with(ResponseTemplate::new(404)).with_priority(1).mount(&server).await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(test_connection(&client).await, TestResult::Ok { version: "unknown".into() });
}
```

Add `pub mod client;` to `src-tauri/src/lib.rs`.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test client`
Expected: FAIL to compile with `unresolved import llama_swap_monitor_lib::client`.

- [ ] **Step 4: Implement `src-tauri/src/client.rs`**

```rust
//! Typed HTTP client for llama-swap. Every call returns `Result`; nothing panics.

use std::time::{Duration, Instant};

use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::api::{
    ActivityEntry, ActivityResponse, ModelEntry, ModelsResponse, RunningModel, RunningResponse, StatsResponse,
    VersionInfo,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClientError {
    #[error("cannot reach llama-swap: {0}")]
    Unreachable(String),
    #[error("llama-swap rejected the API key")]
    Unauthorized,
    #[error("endpoint not found")]
    NotFound,
    #[error("HTTP {0}")]
    Http(u16),
    #[error("unexpected response: {0}")]
    Decode(String),
}

#[derive(Clone)]
pub struct LlamaSwapClient {
    base: String,
    key: Option<String>,
    http: Client,
}

impl std::fmt::Debug for LlamaSwapClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlamaSwapClient")
            .field("base", &self.base)
            .field("key", &self.key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

fn describe(e: reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".into()
    } else if e.is_connect() {
        "connection refused or host not found".into()
    } else {
        e.without_url().to_string()
    }
}

impl LlamaSwapClient {
    pub fn new(base_url: &str, key: Option<String>) -> Result<Self, ClientError> {
        let http = Client::builder()
            .connect_timeout(REQUEST_TIMEOUT)
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .map_err(|e| ClientError::Unreachable(describe(e)))?;
        Ok(Self {
            base: base_url.trim_end_matches('/').to_string(),
            key: key.filter(|k| !k.trim().is_empty()),
            http,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn get(&self, path: &str) -> RequestBuilder {
        let rb = self.http.get(format!("{}{}", self.base, path));
        match &self.key {
            Some(k) => rb.bearer_auth(k),
            None => rb,
        }
    }

    async fn send(rb: RequestBuilder) -> Result<Response, ClientError> {
        let resp = rb.send().await.map_err(|e| ClientError::Unreachable(describe(e)))?;
        match resp.status() {
            s if s.is_success() => Ok(resp),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(ClientError::Unauthorized),
            StatusCode::NOT_FOUND => Err(ClientError::NotFound),
            s => Err(ClientError::Http(s.as_u16())),
        }
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let resp = Self::send(self.get(path).timeout(REQUEST_TIMEOUT)).await?;
        let bytes = resp.bytes().await.map_err(|e| ClientError::Unreachable(describe(e)))?;
        serde_json::from_slice(&bytes).map_err(|e| ClientError::Decode(e.to_string()))
    }

    /// Body text and round-trip latency of `GET /health` (unauthenticated in llama-swap).
    pub async fn health(&self) -> Result<(String, Duration), ClientError> {
        let start = Instant::now();
        let resp = Self::send(self.get("/health").timeout(REQUEST_TIMEOUT)).await?;
        let text = resp.text().await.map_err(|e| ClientError::Unreachable(describe(e)))?;
        Ok((text, start.elapsed()))
    }

    pub async fn version(&self) -> Result<VersionInfo, ClientError> {
        self.get_json("/api/version").await
    }

    pub async fn running(&self) -> Result<Vec<RunningModel>, ClientError> {
        Ok(self.get_json::<RunningResponse>("/running").await?.running)
    }

    pub async fn models(&self) -> Result<Vec<ModelEntry>, ClientError> {
        Ok(self.get_json::<ModelsResponse>("/v1/models").await?.data)
    }

    pub async fn stats(&self) -> Result<StatsResponse, ClientError> {
        self.get_json("/api/metrics/stats").await
    }

    pub async fn activity(&self, limit: u32) -> Result<Vec<ActivityEntry>, ClientError> {
        let path = format!("/api/metrics/activity?limit={limit}");
        Ok(self.get_json::<ActivityResponse>(&path).await?.data)
    }

    /// Opens the SSE stream. Only the connect phase is time-limited; the body stays open.
    pub async fn events(&self) -> Result<Response, ClientError> {
        Self::send(self.get("/api/events").header("Accept", "text/event-stream")).await
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TestResult {
    Ok { version: String },
    Unreachable { message: String },
    Unauthorized,
    NotLlamaSwap { message: String },
}

/// `/health` tells "reachable" apart from "not there"; `/api/version` tells
/// "good key" apart from "bad key". Older llama-swap without `/api/version`
/// falls back to `/running`.
pub async fn test_connection(client: &LlamaSwapClient) -> TestResult {
    match client.health().await {
        Err(ClientError::Unreachable(message)) => return TestResult::Unreachable { message },
        Err(e) => return TestResult::NotLlamaSwap { message: format!("/health: {e}") },
        Ok((body, _)) if body.trim() != "OK" => {
            return TestResult::NotLlamaSwap { message: "/health did not answer OK".into() }
        }
        Ok(_) => {}
    }
    match client.version().await {
        Ok(v) if !v.version.is_empty() => TestResult::Ok { version: v.version },
        Ok(_) => TestResult::NotLlamaSwap { message: "/api/version has no version".into() },
        Err(ClientError::Unauthorized) => TestResult::Unauthorized,
        Err(ClientError::Unreachable(message)) => TestResult::Unreachable { message },
        Err(ClientError::NotFound) => match client.running().await {
            Ok(_) => TestResult::Ok { version: "unknown".into() },
            Err(ClientError::Unauthorized) => TestResult::Unauthorized,
            Err(e) => TestResult::NotLlamaSwap { message: format!("/running: {e}") },
        },
        Err(e) => TestResult::NotLlamaSwap { message: format!("/api/version: {e}") },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_redacts_key() {
        let c = LlamaSwapClient::new("http://box:8080", Some("super-secret".into())).unwrap();
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("super-secret"), "{dbg}");
        assert!(dbg.contains("<redacted>"));
    }

    #[test]
    fn blank_key_is_treated_as_none() {
        let c = LlamaSwapClient::new("http://box:8080/", Some("  ".into())).unwrap();
        assert!(c.key.is_none());
        assert_eq!(c.base_url(), "http://box:8080");
    }

    #[test]
    fn test_result_serializes_for_the_frontend() {
        let v = serde_json::to_value(TestResult::NotLlamaSwap { message: "x".into() }).unwrap();
        assert_eq!(v, serde_json::json!({"kind":"notLlamaSwap","message":"x"}));
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test client`
Expected: `test result: ok. 11 passed`. The timeout test takes about 3 s.

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib client::`
Expected: `test result: ok. 3 passed`

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/client.rs src-tauri/src/lib.rs src-tauri/tests/common src-tauri/tests/client.rs
git commit -m "feat: add llama-swap HTTP client and connection test" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Poller

**Files:**
- Create: `src-tauri/src/poller.rs`, `src-tauri/tests/poller.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod poller;`)

**Interfaces:**
- Consumes: `LlamaSwapClient`, `ClientError`, and the `api` types.
- Produces:
  - `pub const ACTIVITY_LIMIT: u32 = 100`.
  - `enum Feature<T> { Available(T), Unavailable, Failed }`. `Unavailable` means 404; `Failed` means a transient error, so the caller keeps its previous value.
  - `PollData { latency: Duration, running: Vec<RunningModel>, models: Option<Vec<ModelEntry>>, version: Option<VersionInfo>, stats: Feature<StatsResponse>, activity: Feature<Vec<ActivityEntry>> }`.
  - `enum PollOutcome { Ok(PollData), Unauthorized, Unreachable(String), Error(String) }`.
  - `pub async fn poll_once(client: &LlamaSwapClient, want_models: bool, want_version: bool) -> PollOutcome`.

- [ ] **Step 1: Write the failing tests `src-tauri/tests/poller.rs`**

```rust
mod common;

use common::*;
use llama_swap_monitor_lib::client::LlamaSwapClient;
use llama_swap_monitor_lib::poller::{poll_once, Feature, PollOutcome};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn unwrap_ok(o: PollOutcome) -> llama_swap_monitor_lib::poller::PollData {
    match o {
        PollOutcome::Ok(d) => d,
        other => panic!("expected Ok, got {other:?}"),
    }
}

#[tokio::test]
async fn full_poll_against_healthy_server() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    let d = unwrap_ok(poll_once(&client, true, true).await);
    assert_eq!(d.running.len(), 2);
    assert_eq!(d.models.unwrap().len(), 3);
    assert_eq!(d.version.unwrap().version, "v188");
    assert!(matches!(d.stats, Feature::Available(ref s) if s.total_requests == 42));
    assert!(matches!(d.activity, Feature::Available(ref a) if a.len() == 3));
}

#[tokio::test]
async fn skips_models_and_version_when_not_wanted() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    let d = unwrap_ok(poll_once(&client, false, false).await);
    assert!(d.models.is_none());
    assert!(d.version.is_none());
    let requests = server.received_requests().await.unwrap();
    assert!(!requests.iter().any(|r| r.url.path() == "/v1/models" || r.url.path() == "/api/version"));
}

#[tokio::test]
async fn missing_metrics_endpoints_are_unavailable() {
    let server = MockServer::start().await;
    Mock::given(path("/health")).respond_with(ResponseTemplate::new(200).set_body_string("OK")).mount(&server).await;
    Mock::given(path("/running")).respond_with(json_body(RUNNING)).mount(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    let d = unwrap_ok(poll_once(&client, false, false).await);
    assert_eq!(d.stats, Feature::Unavailable);
    assert_eq!(d.activity, Feature::Unavailable);
}

#[tokio::test]
async fn transient_stats_error_is_failed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/metrics/stats"))
        .respond_with(ResponseTemplate::new(500))
        .with_priority(1)
        .mount(&server)
        .await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    let d = unwrap_ok(poll_once(&client, false, false).await);
    assert_eq!(d.stats, Feature::Failed);
}

#[tokio::test]
async fn unauthorized_running_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(path("/running")).respond_with(ResponseTemplate::new(401)).with_priority(1).mount(&server).await;
    mount_healthy(&server).await;
    let client = LlamaSwapClient::new(&server.uri(), None).unwrap();
    assert_eq!(poll_once(&client, true, true).await, PollOutcome::Unauthorized);
}

#[tokio::test]
async fn down_server_is_unreachable() {
    let client = LlamaSwapClient::new(&closed_port_url(), None).unwrap();
    assert!(matches!(poll_once(&client, true, true).await, PollOutcome::Unreachable(_)));
}
```

Add `pub mod poller;` to `src-tauri/src/lib.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test poller`
Expected: FAIL to compile with `unresolved import llama_swap_monitor_lib::poller`.

- [ ] **Step 3: Implement `src-tauri/src/poller.rs`**

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test poller`
Expected: `test result: ok. 6 passed`

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/poller.rs src-tauri/src/lib.rs src-tauri/tests/poller.rs
git commit -m "feat: add concurrent poll tick with per-feature availability" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Monitor state and snapshot building

**Files:**
- Create: `src-tauri/src/monitor.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod monitor;`)

**Interfaces:**
- Consumes:
  - `api::{ActivityEntry, Histogram, ModelEntry, RunningModel, StatsResponse}`
  - `events::{InflightMsg, InflightTable}`
  - `poller::{Feature, PollOutcome, PollData}`
  - `state::{derive_state, ModelState, Thresholds}`
- Produces (all derive `Debug, Clone, PartialEq, Serialize`, camelCase):
  - `pub const TOK_S_HISTORY: usize = 30`.
  - `enum Connection { Connecting, Connected { latency_ms: u64 }, Unauthorized, Unreachable { message: String }, Error { message: String } }`. Tagged as `kind`.
  - `ModelCard { id, name, description: String, state: ModelState, ttl_s: Option<u64>, ttl_remaining_s: Option<u64>, tok_s_history: Vec<f64>, last_request_at_ms: Option<i64> }`.
  - `StatsSummary { total_requests, total_input_tokens, total_output_tokens, total_cache_tokens: i64, gen: Option<Histogram>, prompt: Option<Histogram> }`.
  - `Snapshot { host: String, connection: Connection, last_ok_ms: Option<i64>, version: Option<String>, models: Vec<ModelCard>, stats: Option<StatsSummary>, stats_available: bool, live_events: bool }`.
  - `MonitorState` with:
    - `new(host: &str)`
    - `apply_poll(&mut self, outcome: PollOutcome, now: Instant, wall_ms: i64)`
    - `apply_inflight(&mut self, msg: InflightMsg, now: Instant)`
    - `set_live_events(&mut self, live: bool)` (clears in-flight requests when `false`)
    - `snapshot(&self, now: Instant, wall_ms: i64, t: &Thresholds) -> Snapshot`

- [ ] **Step 1: Write the failing tests in `src-tauri/src/monitor.rs`**

```rust
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
    fn data(running: Vec<RunningModel>, models: Option<Vec<ModelEntry>>) -> PollData {
        PollData {
            latency: Duration::from_millis(12),
            running,
            models,
            version: None,
            stats: Feature::Failed,
            activity: Feature::Failed,
        }
    }
    fn card<'a>(s: &'a Snapshot, id: &str) -> &'a ModelCard {
        s.models.iter().find(|m| m.id == id).unwrap_or_else(|| panic!("no card {id}"))
    }

    #[test]
    fn cards_follow_configured_order_and_include_unlisted_running() {
        let t0 = Instant::now();
        let mut m = MonitorState::new("http://box:8080");
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
    fn uptime_measured_from_observed_transition() {
        let t0 = Instant::now();
        let mut m = MonitorState::new("h");
        m.apply_poll(PollOutcome::Ok(data(vec![running("a", "starting", 0)], Some(vec![model("a")]))), t0, WALL);
        let s = m.snapshot(t0 + secs(4), WALL, &Thresholds::default());
        assert_eq!(card(&s, "a").state, ModelState::Loading { elapsed_s: 4 });
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

    fn idle_with_request(ready_for: u64, last_request_ms: i64) -> Snapshot {
        let t0 = Instant::now();
        let mut m = MonitorState::new("h");
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
        let mut m = MonitorState::new("h");
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

    #[test]
    fn dropping_live_events_clears_inflight() {
        let t0 = Instant::now();
        let mut m = MonitorState::new("h");
        m.apply_poll(PollOutcome::Ok(data(vec![running("a", "ready", 0)], None)), t0, WALL);
        m.set_live_events(true);
        let req = InflightEntry { id: "1".into(), model: "a".into(), resp_bytes: 10, ..Default::default() };
        m.apply_inflight(InflightMsg { operation: "upsert".into(), request: Some(req), ..Default::default() }, t0);
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        assert!(matches!(card(&s, "a").state, ModelState::Busy { requests: 1, .. }));
        assert!(s.live_events);
        m.set_live_events(false);
        let s = m.snapshot(t0, WALL, &Thresholds::default());
        assert!(matches!(card(&s, "a").state, ModelState::Idle { .. }));
        assert!(!s.live_events);
    }

    #[test]
    fn snapshot_json_shape_matches_frontend_types() {
        let t0 = Instant::now();
        let mut m = MonitorState::new("http://box:8080");
        m.apply_poll(PollOutcome::Ok(data(vec![running("a", "ready", 300)], None)), t0, WALL);
        let v = serde_json::to_value(m.snapshot(t0, WALL, &Thresholds::default())).unwrap();
        for key in ["host", "connection", "lastOkMs", "version", "models", "stats", "statsAvailable", "liveEvents"] {
            assert!(v.get(key).is_some(), "missing {key}: {v}");
        }
        assert_eq!(v["connection"], serde_json::json!({"kind":"connected","latencyMs":12}));
        let c = &v["models"][0];
        for key in ["id", "name", "description", "state", "ttlS", "ttlRemainingS", "tokSHistory", "lastRequestAtMs"] {
            assert!(c.get(key).is_some(), "missing card.{key}: {c}");
        }
    }
}
```

Add `pub mod monitor;` to `src-tauri/src/lib.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib monitor::`
Expected: FAIL to compile with `cannot find struct MonitorState`.

- [ ] **Step 3: Implement above the tests**

```rust
//! Merges poll results and the in-flight table into a `Snapshot` for the UI.
//! Pure: callers pass `now` (monotonic) and `wall_ms` (Unix ms) in.

use std::collections::HashMap;
use std::time::Instant;

use serde::Serialize;

use crate::api::{ActivityEntry, Histogram, ModelEntry, RunningModel, StatsResponse};
use crate::events::{InflightMsg, InflightTable};
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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub host: String,
    pub connection: Connection,
    pub last_ok_ms: Option<i64>,
    pub version: Option<String>,
    pub models: Vec<ModelCard>,
    pub stats: Option<StatsSummary>,
    pub stats_available: bool,
    pub live_events: bool,
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
    inflight: InflightTable,
    live_events: bool,
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
            inflight: InflightTable::default(),
            live_events: false,
            state_since: HashMap::new(),
        }
    }

    pub fn apply_poll(&mut self, outcome: PollOutcome, now: Instant, wall_ms: i64) {
        match outcome {
            PollOutcome::Ok(d) => {
                self.connection = Connection::Connected { latency_ms: d.latency.as_millis() as u64 };
                self.last_ok_ms = Some(wall_ms);
                if let Some(v) = d.version {
                    self.version = Some(v.version);
                }
                if let Some(m) = d.models {
                    self.configured = m;
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
                    Feature::Available(a) => self.activity = a,
                    Feature::Unavailable => self.activity.clear(),
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

    pub fn set_live_events(&mut self, live: bool) {
        self.live_events = live;
        if !live {
            self.inflight.clear();
        }
    }

    pub fn snapshot(&self, now: Instant, wall_ms: i64, t: &Thresholds) -> Snapshot {
        let mut entries: Vec<(String, String, String)> =
            self.configured.iter().map(|m| (m.id.clone(), m.name.clone(), m.description.clone())).collect();
        for r in &self.running {
            if !entries.iter().any(|(id, _, _)| *id == r.model) {
                entries.push((r.model.clone(), r.name.clone(), r.description.clone()));
            }
        }
        let models = entries
            .into_iter()
            .map(|(id, name, description)| self.card(id, name, description, now, wall_ms, t))
            .collect();
        Snapshot {
            host: self.host.clone(),
            connection: self.connection.clone(),
            last_ok_ms: self.last_ok_ms,
            version: self.version.clone(),
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
            live_events: self.live_events,
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
        let state = derive_state(run.map(|r| r.state.as_str()), &views, since, t);

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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib monitor::`
Expected: `test result: ok. 11 passed`

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/monitor.rs src-tauri/src/lib.rs
git commit -m "feat: merge poll and event data into UI snapshots" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Backoff and the monitor runtime

**Files:**
- Create: `src-tauri/src/backoff.rs`, `src-tauri/src/runtime.rs`, `src-tauri/tests/runtime.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod backoff;` and `pub mod runtime;`)

**Interfaces:**
- Consumes:
  - `LlamaSwapClient`, `ClientError`
  - `poll_once`, `PollOutcome`
  - `SseParser`, `decode_event`, `StreamEvent`
  - `MonitorState`, `Snapshot`, `Thresholds`
- Produces:
  - `Backoff::new(base: Duration, cap: Duration)`, `.next_delay(&mut self) -> Duration`, `.reset(&mut self)`.
  - `pub trait SnapshotSink: Send + Sync + 'static { fn emit(&self, snapshot: &Snapshot); }`
  - `MonitorConfig { base_url: String, api_key: Option<String>, poll_interval: Duration, thresholds: Thresholds }`.
  - `MonitorHandle` with `stop(&self)`. Also stops on `Drop`.
  - `pub fn start(cfg: MonitorConfig, sink: Arc<dyn SnapshotSink>) -> Result<MonitorHandle, ClientError>`.
  - `pub fn now_ms() -> i64`.

- [ ] **Step 1: Write the failing backoff test in `src-tauri/src/backoff.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_to_cap_and_resets() {
        let mut b = Backoff::new(Duration::from_secs(2), Duration::from_secs(30));
        let seq: Vec<u64> = (0..6).map(|_| b.next_delay().as_secs()).collect();
        assert_eq!(seq, [2, 4, 8, 16, 30, 30]);
        b.reset();
        assert_eq!(b.next_delay().as_secs(), 2);
    }
}
```

Add `pub mod backoff;` to `src-tauri/src/lib.rs`.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib backoff::`
Expected: FAIL to compile with `cannot find struct Backoff`.

- [ ] **Step 3: Implement `Backoff` above the test**

```rust
//! Exponential backoff: base, 2×base, 4×base, … capped.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    cap: Duration,
    next: Duration,
}

impl Backoff {
    pub fn new(base: Duration, cap: Duration) -> Self {
        Self { base, cap, next: base }
    }

    pub fn next_delay(&mut self) -> Duration {
        let d = self.next;
        self.next = (self.next * 2).min(self.cap);
        d
    }

    pub fn reset(&mut self) {
        self.next = self.base;
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib backoff::`
Expected: `test result: ok. 1 passed`

- [ ] **Step 4: Write the failing runtime integration tests `src-tauri/tests/runtime.rs`**

```rust
mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use llama_swap_monitor_lib::monitor::{Connection, Snapshot};
use llama_swap_monitor_lib::runtime::{start, MonitorConfig, SnapshotSink};
use llama_swap_monitor_lib::state::{ModelState, Thresholds};
use serde_json::json;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

struct ChanSink(UnboundedSender<Snapshot>);

impl SnapshotSink for ChanSink {
    fn emit(&self, s: &Snapshot) {
        let _ = self.0.send(s.clone());
    }
}

fn config(url: String) -> MonitorConfig {
    MonitorConfig { base_url: url, api_key: None, poll_interval: Duration::from_millis(200), thresholds: Thresholds::default() }
}

async fn wait_for(rx: &mut UnboundedReceiver<Snapshot>, what: &str, pred: impl Fn(&Snapshot) -> bool) -> Snapshot {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let s = rx.recv().await.expect("sink closed");
            if pred(&s) {
                return s;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for: {what}"))
}

fn state_of<'a>(s: &'a Snapshot, id: &str) -> Option<&'a ModelState> {
    s.models.iter().find(|m| m.id == id).map(|m| &m.state)
}

#[tokio::test]
async fn healthy_server_produces_connected_snapshot() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    let (tx, mut rx) = unbounded_channel();
    let _handle = start(config(server.uri()), Arc::new(ChanSink(tx))).unwrap();
    let first = rx.recv().await.unwrap();
    assert_eq!(first.connection, Connection::Connecting, "an initial snapshot is published immediately");
    let s = wait_for(&mut rx, "connected", |s| matches!(s.connection, Connection::Connected { .. })).await;
    assert_eq!(s.version.as_deref(), Some("v188"));
    assert_eq!(s.models.len(), 3);
    assert!(s.stats.is_some());
    assert!(!s.live_events, "no /api/events route mounted");
}

#[tokio::test]
async fn unauthorized_is_reported() {
    let server = MockServer::start().await;
    Mock::given(path("/running")).respond_with(ResponseTemplate::new(401)).with_priority(1).mount(&server).await;
    mount_healthy(&server).await;
    let (tx, mut rx) = unbounded_channel();
    let _handle = start(config(server.uri()), Arc::new(ChanSink(tx))).unwrap();
    wait_for(&mut rx, "unauthorized", |s| s.connection == Connection::Unauthorized).await;
}

#[tokio::test]
async fn unreachable_is_reported() {
    let (tx, mut rx) = unbounded_channel();
    let _handle = start(config(closed_port_url()), Arc::new(ChanSink(tx))).unwrap();
    wait_for(&mut rx, "unreachable", |s| matches!(s.connection, Connection::Unreachable { .. })).await;
}

#[tokio::test]
async fn dropped_event_stream_clears_busy() {
    let server = MockServer::start().await;
    let body = sse_frame(
        "inflight",
        json!({"operation":"snapshot","requests":[{"id":"7","model":"qwen3-30b","resp_bytes":128,"elapsed_ms":2000}]}),
    );
    // Delay so the first poll has marked qwen3-30b ready before the event lands.
    // The response body then ends, which is exactly what a llama-swap restart looks like.
    Mock::given(path("/api/events"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(body.into_bytes(), "text/event-stream")
                .set_delay(Duration::from_millis(500)),
        )
        .mount(&server)
        .await;
    mount_healthy(&server).await;
    let (tx, mut rx) = unbounded_channel();
    let _handle = start(config(server.uri()), Arc::new(ChanSink(tx))).unwrap();

    wait_for(&mut rx, "busy from live event", |s| {
        s.live_events && matches!(state_of(s, "qwen3-30b"), Some(ModelState::Busy { requests: 1, .. }))
    })
    .await;
    wait_for(&mut rx, "idle after stream drop", |s| {
        !s.live_events && matches!(state_of(s, "qwen3-30b"), Some(ModelState::Idle { .. }))
    })
    .await;
}

#[tokio::test]
async fn stopping_the_handle_stops_snapshots() {
    let server = MockServer::start().await;
    mount_healthy(&server).await;
    let (tx, mut rx) = unbounded_channel();
    let handle = start(config(server.uri()), Arc::new(ChanSink(tx))).unwrap();
    wait_for(&mut rx, "connected", |s| matches!(s.connection, Connection::Connected { .. })).await;
    handle.stop();
    tokio::time::sleep(Duration::from_millis(300)).await;
    while rx.try_recv().is_ok() {} // drain anything emitted before the abort landed
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(rx.try_recv().is_err(), "no snapshots after stop");
}
```

Add `pub mod runtime;` to `src-tauri/src/lib.rs`.

- [ ] **Step 5: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test runtime`
Expected: FAIL to compile with `unresolved import llama_swap_monitor_lib::runtime`.

- [ ] **Step 6: Implement `src-tauri/src/runtime.rs`**

```rust
//! The two background loops: REST polling and the `/api/events` stream.
//! Both write into one `MonitorState` and publish a fresh `Snapshot` after every change.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use tauri::async_runtime::{spawn, JoinHandle};
use tokio::sync::Notify;

use crate::backoff::Backoff;
use crate::client::{ClientError, LlamaSwapClient};
use crate::events::{decode_event, StreamEvent};
use crate::monitor::{MonitorState, Snapshot};
use crate::poller::{poll_once, PollOutcome};
use crate::sse::SseParser;
use crate::state::Thresholds;

const MODELS_REFRESH: Duration = Duration::from_secs(30);
const UNAUTHORIZED_RETRY: Duration = Duration::from_secs(30);
const EVENTS_UNAVAILABLE_RETRY: Duration = Duration::from_secs(60);
const BACKOFF_BASE: Duration = Duration::from_secs(2);
const BACKOFF_CAP: Duration = Duration::from_secs(30);

pub trait SnapshotSink: Send + Sync + 'static {
    fn emit(&self, snapshot: &Snapshot);
}

pub struct MonitorConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub poll_interval: Duration,
    pub thresholds: Thresholds,
}

pub struct MonitorHandle {
    tasks: Vec<JoinHandle<()>>,
}

impl MonitorHandle {
    pub fn stop(&self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl Drop for MonitorHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

struct Shared {
    state: Mutex<MonitorState>,
    sink: Arc<dyn SnapshotSink>,
    thresholds: Thresholds,
    /// Poked by `modelStatus` events so the poll loop refreshes immediately.
    wake: Notify,
}

impl Shared {
    fn with_state(&self, f: impl FnOnce(&mut MonitorState)) {
        f(&mut self.state.lock().expect("monitor state poisoned"));
    }

    fn publish(&self) {
        let snapshot = self
            .state
            .lock()
            .expect("monitor state poisoned")
            .snapshot(Instant::now(), now_ms(), &self.thresholds);
        self.sink.emit(&snapshot);
    }
}

pub fn start(cfg: MonitorConfig, sink: Arc<dyn SnapshotSink>) -> Result<MonitorHandle, ClientError> {
    let client = LlamaSwapClient::new(&cfg.base_url, cfg.api_key)?;
    let shared = Arc::new(Shared {
        state: Mutex::new(MonitorState::new(client.base_url())),
        sink,
        thresholds: cfg.thresholds,
        wake: Notify::new(),
    });
    shared.publish();
    let poll = spawn(poll_loop(client.clone(), shared.clone(), cfg.poll_interval));
    let events = spawn(event_loop(client, shared));
    Ok(MonitorHandle { tasks: vec![poll, events] })
}

async fn poll_loop(client: LlamaSwapClient, shared: Arc<Shared>, interval: Duration) {
    let mut backoff = Backoff::new(BACKOFF_BASE, BACKOFF_CAP);
    let mut need_version = true;
    let mut models_fetched_at: Option<Instant> = None;
    loop {
        let want_models = models_fetched_at.map_or(true, |t| t.elapsed() >= MODELS_REFRESH);
        let outcome = poll_once(&client, want_models, need_version).await;
        let delay = match &outcome {
            PollOutcome::Ok(d) => {
                backoff.reset();
                if d.version.is_some() {
                    need_version = false;
                }
                if d.models.is_some() {
                    models_fetched_at = Some(Instant::now());
                }
                interval
            }
            PollOutcome::Unauthorized => {
                need_version = true;
                UNAUTHORIZED_RETRY
            }
            PollOutcome::Unreachable(_) | PollOutcome::Error(_) => {
                if let PollOutcome::Error(m) = &outcome {
                    eprintln!("llama-swap poll error: {m}");
                }
                need_version = true;
                backoff.next_delay()
            }
        };
        shared.with_state(|s| s.apply_poll(outcome, Instant::now(), now_ms()));
        shared.publish();
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shared.wake.notified() => {}
        }
    }
}

async fn event_loop(client: LlamaSwapClient, shared: Arc<Shared>) {
    let mut backoff = Backoff::new(BACKOFF_BASE, BACKOFF_CAP);
    loop {
        let delay = match client.events().await {
            Ok(resp) => {
                backoff.reset();
                shared.with_state(|s| s.set_live_events(true));
                shared.publish();
                let mut parser = SseParser::new();
                let mut stream = resp.bytes_stream();
                while let Some(Ok(chunk)) = stream.next().await {
                    let mut changed = false;
                    for payload in parser.push(&chunk) {
                        match decode_event(&payload) {
                            Ok(StreamEvent::Inflight(msg)) => {
                                shared.with_state(|s| s.apply_inflight(msg, Instant::now()));
                                changed = true;
                            }
                            Ok(StreamEvent::ModelStatus) => shared.wake.notify_one(),
                            Ok(StreamEvent::Other) | Err(_) => {}
                        }
                    }
                    if changed {
                        shared.publish();
                    }
                }
                shared.with_state(|s| s.set_live_events(false));
                shared.publish();
                backoff.next_delay()
            }
            Err(ClientError::NotFound) => EVENTS_UNAVAILABLE_RETRY,
            Err(_) => backoff.next_delay(),
        };
        tokio::time::sleep(delay).await;
    }
}
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test runtime`
Expected: `test result: ok. 5 passed`

Run the whole suite to make sure nothing regressed: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: every test binary reports `ok`.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/backoff.rs src-tauri/src/runtime.rs src-tauri/src/lib.rs src-tauri/tests/runtime.rs
git commit -m "feat: run poll and event-stream loops that publish snapshots" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Tauri commands and app wiring

**Files:**
- Create: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod commands;` and replace `run()`)

**Interfaces:**
- Consumes:
  - `config::{Settings, SecretStore, KeyringStore, ConfigError, load_settings, save_settings, normalize_base_url}`
  - `client::{LlamaSwapClient, TestResult, test_connection}`
  - `runtime::{start, MonitorConfig, MonitorHandle, SnapshotSink}`
  - `monitor::Snapshot`
- Produces:
  - `AppState { config_path: PathBuf, secrets: Arc<dyn SecretStore>, monitor: Mutex<Option<MonitorHandle>>, latest: Arc<Mutex<Option<Snapshot>>>, generation: Arc<AtomicU64> }` and `AppState::new(config_path, secrets)`.
  - `SettingsView { configured: bool, settings: Settings, has_key: bool }` (Serialize, camelCase).
  - `pub fn resolve_key(secrets: &dyn SecretStore, current: Option<&Settings>, api_key: Option<String>) -> Result<Option<String>, ConfigError>`.
    - Key semantics: `None` keeps the saved key, `Some("")` removes it, `Some(k)` replaces it.
  - `pub fn apply_settings(path: &Path, secrets: &dyn SecretStore, settings: Settings, api_key: Option<String>) -> Result<(Settings, Option<String>), ConfigError>`.
  - `pub fn restart_monitor(app: &AppHandle, state: &AppState, settings: &Settings, key: Option<String>) -> Result<(), String>`.
  - Tauri commands, invoked by the frontend with camelCase args (`baseUrl`, `apiKey`):
    - `get_settings() -> SettingsView`
    - `save_settings(settings, apiKey) -> SettingsView`
    - `test_connection(baseUrl, apiKey) -> TestResult`
    - `get_snapshot() -> Option<Snapshot>`
  - Tauri event `"snapshot"` with a `Snapshot` payload.

- [ ] **Step 1: Write the failing tests in `src-tauri/src/commands.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MemoryStore;

    fn settings(url: &str) -> Settings {
        Settings { base_url: url.into(), ..Default::default() }
    }

    #[test]
    fn new_key_is_stored_under_normalized_url_and_not_in_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = MemoryStore::default();
        let (saved, key) = apply_settings(&path, &store, settings("box:8080/v1"), Some("s3cret".into())).unwrap();
        assert_eq!(saved.base_url, "http://box:8080");
        assert_eq!(key.as_deref(), Some("s3cret"));
        assert_eq!(store.get("http://box:8080").unwrap().as_deref(), Some("s3cret"));
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(!file.contains("s3cret"), "key leaked into settings.json: {file}");
    }

    #[test]
    fn blank_key_keeps_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = MemoryStore::default();
        apply_settings(&path, &store, settings("http://box:8080"), Some("k1".into())).unwrap();
        let (_, key) = apply_settings(&path, &store, settings("http://box:8080"), None).unwrap();
        assert_eq!(key.as_deref(), Some("k1"));
        assert_eq!(store.get("http://box:8080").unwrap().as_deref(), Some("k1"));
    }

    #[test]
    fn empty_string_removes_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = MemoryStore::default();
        apply_settings(&path, &store, settings("http://box:8080"), Some("k1".into())).unwrap();
        let (_, key) = apply_settings(&path, &store, settings("http://box:8080"), Some(String::new())).unwrap();
        assert_eq!(key, None);
        assert_eq!(store.get("http://box:8080").unwrap(), None);
    }

    #[test]
    fn url_change_with_blank_key_migrates_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = MemoryStore::default();
        apply_settings(&path, &store, settings("http://old:8080"), Some("k1".into())).unwrap();
        let (saved, key) = apply_settings(&path, &store, settings("http://new:8080"), None).unwrap();
        assert_eq!(saved.base_url, "http://new:8080");
        assert_eq!(key.as_deref(), Some("k1"));
        assert_eq!(store.get("http://new:8080").unwrap().as_deref(), Some("k1"));
        assert_eq!(store.get("http://old:8080").unwrap(), None, "old entry is cleaned up");
    }

    #[test]
    fn invalid_url_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = MemoryStore::default();
        assert!(apply_settings(&path, &store, settings("ftp://nope"), Some("k".into())).is_err());
        assert!(!path.exists());
        assert_eq!(store.get("ftp://nope").unwrap(), None);
    }

    #[test]
    fn resolve_key_for_test_connection_uses_saved_key_when_blank() {
        let store = MemoryStore::default();
        store.set("http://box:8080", "k1").unwrap();
        let current = settings("http://box:8080");
        assert_eq!(resolve_key(&store, Some(&current), None).unwrap().as_deref(), Some("k1"));
        assert_eq!(resolve_key(&store, Some(&current), Some(" new ".into())).unwrap().as_deref(), Some("new"));
        assert_eq!(resolve_key(&store, Some(&current), Some("".into())).unwrap(), None);
        assert_eq!(resolve_key(&store, None, None).unwrap(), None);
    }
}
```

Add `pub mod commands;` to `src-tauri/src/lib.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib commands::`
Expected: FAIL to compile with `cannot find function apply_settings`.

- [ ] **Step 3: Implement above the tests**

```rust
//! Tauri commands and the glue between settings, keychain and the monitor.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::client::{self, LlamaSwapClient, TestResult};
use crate::config::{self, load_settings, normalize_base_url, ConfigError, SecretStore, Settings};
use crate::monitor::Snapshot;
use crate::runtime::{self, MonitorConfig, MonitorHandle, SnapshotSink};

pub struct AppState {
    pub config_path: PathBuf,
    pub secrets: Arc<dyn SecretStore>,
    pub monitor: Mutex<Option<MonitorHandle>>,
    pub latest: Arc<Mutex<Option<Snapshot>>>,
    /// Bumped on every monitor restart so a stopping monitor can't publish stale data.
    pub generation: Arc<AtomicU64>,
}

impl AppState {
    pub fn new(config_path: PathBuf, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            config_path,
            secrets,
            monitor: Mutex::new(None),
            latest: Arc::new(Mutex::new(None)),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub configured: bool,
    pub settings: Settings,
    pub has_key: bool,
}

/// `None` = keep the saved key (looked up under the *current* URL),
/// `Some("")` = no key, `Some(k)` = use `k`.
pub fn resolve_key(
    secrets: &dyn SecretStore,
    current: Option<&Settings>,
    api_key: Option<String>,
) -> Result<Option<String>, ConfigError> {
    match api_key {
        Some(k) if k.trim().is_empty() => Ok(None),
        Some(k) => Ok(Some(k.trim().to_string())),
        None => match current {
            Some(c) => secrets.get(&c.base_url),
            None => Ok(None),
        },
    }
}

/// Validates, stores the key in the keychain (migrating it if the URL
/// changed), then writes the settings file. Returns what was saved.
pub fn apply_settings(
    path: &Path,
    secrets: &dyn SecretStore,
    settings: Settings,
    api_key: Option<String>,
) -> Result<(Settings, Option<String>), ConfigError> {
    let current = load_settings(path);
    let settings = settings.sanitized()?;
    let key = resolve_key(secrets, current.as_ref(), api_key)?;
    match &key {
        Some(k) => secrets.set(&settings.base_url, k)?,
        None => secrets.delete(&settings.base_url)?,
    }
    if let Some(old) = &current {
        if old.base_url != settings.base_url {
            secrets.delete(&old.base_url)?;
        }
    }
    config::save_settings(path, &settings)?;
    Ok((settings, key))
}

struct TauriSink {
    app: AppHandle,
    latest: Arc<Mutex<Option<Snapshot>>>,
    generation: Arc<AtomicU64>,
    mine: u64,
}

impl SnapshotSink for TauriSink {
    fn emit(&self, snapshot: &Snapshot) {
        if self.generation.load(Ordering::SeqCst) != self.mine {
            return;
        }
        *self.latest.lock().expect("latest poisoned") = Some(snapshot.clone());
        let _ = self.app.emit("snapshot", snapshot);
    }
}

pub fn restart_monitor(app: &AppHandle, state: &AppState, settings: &Settings, key: Option<String>) -> Result<(), String> {
    let mut guard = state.monitor.lock().expect("monitor poisoned");
    if let Some(old) = guard.take() {
        old.stop();
    }
    let mine = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    *state.latest.lock().expect("latest poisoned") = None;
    let sink = Arc::new(TauriSink { app: app.clone(), latest: state.latest.clone(), generation: state.generation.clone(), mine });
    let handle = runtime::start(
        MonitorConfig {
            base_url: settings.base_url.clone(),
            api_key: key,
            poll_interval: Duration::from_millis(settings.poll_interval_ms),
            thresholds: settings.thresholds,
        },
        sink,
    )
    .map_err(|e| e.to_string())?;
    *guard = Some(handle);
    Ok(())
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> SettingsView {
    let current = load_settings(&state.config_path);
    // A broken keychain must not block the UI: treat it as "no key".
    let has_key = current
        .as_ref()
        .map(|s| matches!(state.secrets.get(&s.base_url), Ok(Some(_))))
        .unwrap_or(false);
    SettingsView { configured: current.is_some(), settings: current.unwrap_or_default(), has_key }
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
    api_key: Option<String>,
) -> Result<SettingsView, String> {
    let (settings, key) =
        apply_settings(&state.config_path, state.secrets.as_ref(), settings, api_key).map_err(|e| e.to_string())?;
    let has_key = key.is_some();
    restart_monitor(&app, &state, &settings, key)?;
    Ok(SettingsView { configured: true, settings, has_key })
}

#[tauri::command]
pub async fn test_connection(
    state: State<'_, AppState>,
    base_url: String,
    api_key: Option<String>,
) -> Result<TestResult, String> {
    let url = normalize_base_url(&base_url).map_err(|e| e.to_string())?;
    let current = load_settings(&state.config_path);
    let key = resolve_key(state.secrets.as_ref(), current.as_ref(), api_key).map_err(|e| e.to_string())?;
    let client = LlamaSwapClient::new(&url, key).map_err(|e| e.to_string())?;
    Ok(client::test_connection(&client).await)
}

#[tauri::command]
pub fn get_snapshot(state: State<'_, AppState>) -> Option<Snapshot> {
    state.latest.lock().expect("latest poisoned").clone()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib commands::`
Expected: `test result: ok. 6 passed`

- [ ] **Step 5: Wire everything in `src-tauri/src/lib.rs`**

Replace the whole file with:
```rust
pub mod api;
pub mod backoff;
pub mod client;
pub mod commands;
pub mod config;
pub mod events;
pub mod monitor;
pub mod poller;
pub mod runtime;
pub mod sse;
pub mod state;

use std::sync::Arc;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            let state = commands::AppState::new(dir.join("settings.json"), Arc::new(config::KeyringStore));
            if let Some(settings) = config::load_settings(&state.config_path) {
                let key = state.secrets.get(&settings.base_url).ok().flatten();
                if let Err(e) = commands::restart_monitor(app.handle(), &state, &settings, key) {
                    eprintln!("monitor failed to start: {e}");
                }
            }
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::test_connection,
            commands::get_snapshot
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 6: Verify the full suite and build**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: all test binaries `ok`.

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: `Finished` with no warnings about unused modules.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/commands.rs src-tauri/src/lib.rs
git commit -m "feat: expose settings, connection test and snapshots to the webview" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: Frontend foundation and setup view

**Files:**
- Create: `src/types.ts`, `src/api.ts`, `src/dom.ts`, `src/format.ts`, `src/styles.css`, `src/views/setup.ts`
- Modify: `src/main.ts` (replace the placeholder)
- Create: `src/views/dashboard.ts`. This task writes a minimal version; Task 13 replaces it with the full one.

**Interfaces:**
- Consumes: the Tauri commands and the `snapshot` event from Task 11, plus the JSON shapes pinned by the serialization tests in Tasks 5, 7 and 9.
- Produces:
  - TS types `ModelState`, `Connection`, `ModelCard`, `Histogram`, `StatsSummary`, `Snapshot`, `Thresholds`, `Settings`, `SettingsView`, `TestResult`.
  - `api.ts`: `getSettings()`, `saveSettings(settings, apiKey: string | null)`, `testConnection(baseUrl, apiKey: string | null)`, `getSnapshot()`, `onSnapshot(cb)`.
  - `dom.ts`: `h(tag, attrs?, ...children)`, `svg(tag, attrs?, ...children)`.
  - `format.ts`: `formatDuration(s)`, `formatCount(n)`, `formatAgo(ms, now?)`.
  - `renderSetup(view, onSaved, onCancel?) -> HTMLElement`.
  - `Dashboard` interface `{ element; update(s: Snapshot); destroy() }` and `createDashboard(settings, onOpenSettings) -> Dashboard`.

No frontend test framework in phase 1 (per spec). Verification is `npm run build` type-checking plus a manual run.

- [ ] **Step 1: Write `src/types.ts`**

```ts
// Mirrors the Rust types serialized in monitor.rs, state.rs, client.rs, config.rs, commands.rs.

export type ModelState =
  | { kind: "notLoaded" }
  | { kind: "loading"; elapsedS: number }
  | { kind: "idle"; uptimeS: number }
  | { kind: "busy"; requests: number; oldestElapsedS: number }
  | { kind: "stalled"; reason: string }
  | { kind: "unloading" };

export type Connection =
  | { kind: "connecting" }
  | { kind: "connected"; latencyMs: number }
  | { kind: "unauthorized" }
  | { kind: "unreachable"; message: string }
  | { kind: "error"; message: string };

export interface ModelCard {
  id: string;
  name: string;
  description: string;
  state: ModelState;
  ttlS: number | null;
  ttlRemainingS: number | null;
  tokSHistory: number[];
  lastRequestAtMs: number | null;
}

export interface Histogram {
  bins: number[];
  min: number;
  max: number;
  binSize: number;
  p50: number;
  p95: number;
  p99: number;
}

export interface StatsSummary {
  totalRequests: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCacheTokens: number;
  gen: Histogram | null;
  prompt: Histogram | null;
}

export interface Snapshot {
  host: string;
  connection: Connection;
  lastOkMs: number | null;
  version: string | null;
  models: ModelCard[];
  stats: StatsSummary | null;
  statsAvailable: boolean;
  liveEvents: boolean;
}

export interface Thresholds {
  loadTimeoutS: number;
  stopTimeoutS: number;
  firstByteTimeoutS: number;
  streamStallTimeoutS: number;
}

export interface Settings {
  baseUrl: string;
  pollIntervalMs: number;
  thresholds: Thresholds;
}

export interface SettingsView {
  configured: boolean;
  settings: Settings;
  hasKey: boolean;
}

export type TestResult =
  | { kind: "ok"; version: string }
  | { kind: "unreachable"; message: string }
  | { kind: "unauthorized" }
  | { kind: "notLlamaSwap"; message: string };
```

- [ ] **Step 2: Write `src/api.ts`, `src/dom.ts`, `src/format.ts`**

`src/api.ts`:
```ts
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Settings, SettingsView, Snapshot, TestResult } from "./types";

export const getSettings = () => invoke<SettingsView>("get_settings");

/** apiKey: null keeps the saved key, "" removes it, anything else replaces it. */
export const saveSettings = (settings: Settings, apiKey: string | null) =>
  invoke<SettingsView>("save_settings", { settings, apiKey });

export const testConnection = (baseUrl: string, apiKey: string | null) =>
  invoke<TestResult>("test_connection", { baseUrl, apiKey });

export const getSnapshot = () => invoke<Snapshot | null>("get_snapshot");

export const onSnapshot = (cb: (s: Snapshot) => void): Promise<UnlistenFn> =>
  listen<Snapshot>("snapshot", (e) => cb(e.payload));
```

`src/dom.ts`:
```ts
// Tiny element builders. Strings are appended as text nodes, never parsed as
// HTML, so model names from the server can't inject markup.

type Child = Node | string | null | undefined | false;

function appendChildren(el: Element, children: Child[]) {
  for (const c of children) if (c !== null && c !== undefined && c !== false) el.append(c);
}

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Record<string, string> = {},
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
  appendChildren(el, children);
  return el;
}

const SVG_NS = "http://www.w3.org/2000/svg";

export function svg<K extends keyof SVGElementTagNameMap>(
  tag: K,
  attrs: Record<string, string | number> = {},
  ...children: Child[]
): SVGElementTagNameMap[K] {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, String(v));
  appendChildren(el, children);
  return el;
}
```

`src/format.ts`:
```ts
export function formatDuration(totalSeconds: number): string {
  const s = Math.max(0, Math.round(totalSeconds));
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return s % 60 ? `${m} m ${s % 60} s` : `${m} m`;
  const hr = Math.floor(m / 60);
  if (hr < 24) return m % 60 ? `${hr} h ${m % 60} m` : `${hr} h`;
  const d = Math.floor(hr / 24);
  return hr % 24 ? `${d} d ${hr % 24} h` : `${d} d`;
}

export function formatCount(n: number): string {
  const units: [string, number][] = [["B", 1e9], ["M", 1e6], ["k", 1e3]];
  for (const [unit, size] of units) {
    if (n >= size) {
      const x = n / size;
      return `${x >= 100 ? x.toFixed(0) : x.toFixed(1)}${unit}`;
    }
  }
  return String(n);
}

export function formatAgo(ms: number, now: number = Date.now()): string {
  return `${formatDuration((now - ms) / 1000)} ago`;
}
```

- [ ] **Step 3: Write `src/styles.css`**

```css
:root {
  color-scheme: light dark;
  --bg: #f6f7f9;
  --panel: #ffffff;
  --text: #1b1f24;
  --muted: #6b7280;
  --border: #e3e6ea;
  --track: #e5e7eb;
  --grey: #9ca3af;
  --green: #16a34a;
  --amber: #d97706;
  --blue: #2563eb;
  --red: #dc2626;
  font-family: system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
  font-size: 14px;
}

@media (prefers-color-scheme: dark) {
  :root {
    --bg: #0f1115;
    --panel: #171a21;
    --text: #e6e8eb;
    --muted: #9aa3ad;
    --border: #262b35;
    --track: #262b35;
    --grey: #6b7280;
    --green: #22c55e;
    --amber: #f59e0b;
    --blue: #60a5fa;
    --red: #f87171;
  }
}

* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--text); }
button {
  font: inherit; color: inherit; background: var(--panel);
  border: 1px solid var(--border); border-radius: 8px; padding: 6px 12px; cursor: pointer;
}
button.primary { background: var(--blue); border-color: var(--blue); color: #fff; }
button:disabled { opacity: 0.6; cursor: default; }
input {
  font: inherit; color: inherit; background: var(--panel);
  border: 1px solid var(--border); border-radius: 8px; padding: 7px 10px; width: 100%;
}
.muted { color: var(--muted); }

/* ---- dashboard header ---- */
.dashboard { padding: 16px 20px 28px; display: flex; flex-direction: column; gap: 16px; }
.header { display: flex; align-items: center; gap: 12px; }
.header-main { flex: 1; min-width: 0; }
.header-line { display: flex; gap: 10px; align-items: baseline; flex-wrap: wrap; }
.status-text { font-weight: 600; font-size: 16px; }
.host { color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.meta { color: var(--muted); }
.dot { width: 12px; height: 12px; border-radius: 50%; background: var(--grey); flex: none; }
[data-conn="connected"] .dot { background: var(--green); box-shadow: 0 0 0 4px color-mix(in srgb, var(--green) 25%, transparent); }
[data-conn="unreachable"] .dot { background: var(--amber); }
[data-conn="unauthorized"] .dot, [data-conn="error"] .dot { background: var(--red); }
[data-conn="connecting"] .dot { animation: pulse 1.2s ease-in-out infinite; }
.badge {
  font-size: 11px; text-transform: uppercase; letter-spacing: 0.06em;
  padding: 2px 6px; border-radius: 999px; border: 1px solid var(--border); color: var(--muted);
}
.badge-live { color: var(--green); border-color: color-mix(in srgb, var(--green) 50%, transparent); }
.icon-button { font-size: 18px; padding: 4px 10px; }
.banner {
  display: flex; gap: 12px; align-items: center; justify-content: space-between;
  padding: 10px 14px; border-radius: 10px;
  background: color-mix(in srgb, var(--amber) 14%, var(--panel));
  border: 1px solid color-mix(in srgb, var(--amber) 40%, transparent);
}
[data-conn="unauthorized"] .banner {
  background: color-mix(in srgb, var(--red) 12%, var(--panel));
  border-color: color-mix(in srgb, var(--red) 40%, transparent);
}
.banner[hidden] { display: none; }

/* ---- model cards ---- */
.grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 14px; }
.card {
  --pill: var(--grey); --ring: var(--grey);
  background: var(--panel); border: 1px solid var(--border); border-radius: 14px; padding: 14px;
  display: flex; flex-direction: column; gap: 10px; transition: opacity 0.3s, border-color 0.3s;
}
.card[data-state="loading"] { --pill: var(--amber); --ring: var(--amber); }
.card[data-state="idle"] { --pill: var(--green); --ring: var(--green); }
.card[data-state="busy"] { --pill: var(--blue); --ring: var(--blue); }
.card[data-state="stalled"] { --pill: var(--red); --ring: var(--red); border-color: var(--red); box-shadow: 0 0 0 1px var(--red) inset; }
.card[data-state="notLoaded"] { opacity: 0.55; }
.card-head { display: flex; gap: 12px; align-items: center; }
.card-heading { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
.card-title { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.card-detail { color: var(--muted); min-height: 1.2em; }
.card[data-state="stalled"] .card-detail { color: var(--red); }
.card-extras { display: flex; flex-direction: column; gap: 8px; }
.card-extras:empty { display: none; }
.pill {
  align-self: flex-start; font-size: 12px; font-weight: 600; padding: 2px 8px; border-radius: 999px;
  color: var(--pill); background: color-mix(in srgb, var(--pill) 15%, transparent);
}

/* ---- ring ---- */
.ring { flex: none; }
.ring-track { fill: none; stroke: var(--track); stroke-width: 6; }
.ring-arc {
  fill: none; stroke: var(--ring); stroke-width: 6; stroke-linecap: round;
  transform: rotate(-90deg); transform-box: fill-box; transform-origin: center;
  transition: stroke-dasharray 0.6s ease;
}
.ring-label { fill: var(--text); font-size: 15px; font-weight: 600; }
.ring[data-state="notLoaded"] .ring-track,
.ring[data-state="notLoaded"] .ring-arc { stroke-width: 2; }
.ring[data-state="loading"] .ring-track { stroke: color-mix(in srgb, var(--amber) 35%, transparent); stroke-dasharray: 4 5; }
.ring[data-state="loading"] .ring-arc { animation: pulse 1.4s ease-in-out infinite; }
.ring[data-state="busy"] .ring-arc { animation: spin 1.1s linear infinite; }
.ring[data-state="unloading"] .ring-arc { animation: fade 1.6s ease-in-out infinite; }
.ring[data-state="stalled"] .ring-label { fill: var(--red); font-size: 24px; }
@keyframes spin { from { transform: rotate(-90deg); } to { transform: rotate(270deg); } }
@keyframes pulse { 50% { opacity: 0.35; } }
@keyframes fade { 50% { opacity: 0.15; } }
@media (prefers-reduced-motion: reduce) {
  .ring-arc, .dot { animation: none !important; }
}

/* ---- sparkline, ttl ---- */
.sparkline-line { fill: none; stroke: var(--ring); stroke-width: 1.5; }
.sparkline-fill { fill: color-mix(in srgb, var(--ring) 15%, transparent); stroke: none; }
.ttl { display: flex; flex-direction: column; gap: 4px; }
.ttl-track { height: 6px; border-radius: 3px; background: var(--track); overflow: hidden; }
.ttl-fill { height: 100%; background: var(--green); transition: width 1s linear; }
.ttl-text { font-size: 12px; color: var(--muted); }

/* ---- stats ---- */
.stats { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1.2fr); gap: 14px; }
@media (max-width: 720px) { .stats { grid-template-columns: 1fr; } }
.tiles { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 10px; }
.tile, .hist-box { background: var(--panel); border: 1px solid var(--border); border-radius: 14px; padding: 12px 14px; }
.tile-label, .hist-title { font-size: 12px; color: var(--muted); text-transform: uppercase; letter-spacing: 0.05em; }
.tile-value { font-size: 22px; font-weight: 650; margin-top: 4px; font-variant-numeric: tabular-nums; }
.tile-sub { font-size: 12px; color: var(--muted); min-height: 1.2em; }
.histogram { display: block; margin-top: 8px; }
.hist-bar { fill: color-mix(in srgb, var(--blue) 70%, transparent); }
.hist-mark { stroke: var(--text); stroke-width: 1; stroke-dasharray: 3 3; opacity: 0.6; }
.hist-label { fill: var(--muted); font-size: 10px; }

/* ---- disconnected ---- */
.dashboard.stale .grid, .dashboard.stale .stats { filter: saturate(0.15); opacity: 0.7; }
.empty { color: var(--muted); }

/* ---- setup ---- */
.setup { max-width: 520px; margin: 40px auto; padding: 0 20px; }
.setup h1 { font-size: 20px; margin: 0 0 4px; }
.setup form { display: flex; flex-direction: column; gap: 14px; margin-top: 20px; }
.field { display: flex; flex-direction: column; gap: 6px; }
.field label { font-weight: 600; }
.hint { font-size: 12px; color: var(--muted); }
.check { display: flex; gap: 8px; align-items: center; font-size: 13px; }
.check input { width: auto; }
.thresholds { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; margin-top: 10px; }
.actions { display: flex; gap: 8px; justify-content: flex-end; }
.form-status { min-height: 1.4em; margin: 0; }
.form-status[data-kind="ok"] { color: var(--green); }
.form-status[data-kind="error"] { color: var(--red); }
```

- [ ] **Step 4: Write `src/views/setup.ts`**

```ts
import { saveSettings, testConnection } from "../api";
import { h } from "../dom";
import type { Settings, SettingsView, TestResult } from "../types";

export function renderSetup(
  view: SettingsView,
  onSaved: (saved: SettingsView) => void,
  onCancel?: () => void,
): HTMLElement {
  const s = view.settings;
  const url = h("input", { id: "url", type: "text", value: s.baseUrl, placeholder: "http://localhost:8080", autocomplete: "off", spellcheck: "false" });
  const key = h("input", {
    id: "key",
    type: "password",
    autocomplete: "off",
    placeholder: view.hasKey ? "Saved — leave blank to keep it" : "Leave blank if llama-swap has no apiKeys",
  });
  const removeKey = h("input", { id: "remove-key", type: "checkbox" });
  const poll = numberInput("poll", s.pollIntervalMs / 1000, 0.5, 60, 0.5);
  const load = numberInput("load", s.thresholds.loadTimeoutS, 5, 3600, 1);
  const stop = numberInput("stop", s.thresholds.stopTimeoutS, 5, 600, 1);
  const firstByte = numberInput("first-byte", s.thresholds.firstByteTimeoutS, 5, 3600, 1);
  const stall = numberInput("stall", s.thresholds.streamStallTimeoutS, 5, 600, 1);
  const status = h("p", { class: "form-status", role: "status" });
  const testBtn = h("button", { type: "button" }, "Test connection");
  const saveBtn = h("button", { type: "submit", class: "primary" }, "Save");
  const cancelBtn = onCancel ? h("button", { type: "button" }, "Cancel") : null;

  const form = h(
    "form",
    {},
    field("url", "llama-swap URL", url, "Where llama-swap listens, e.g. http://192.168.1.20:8080. A trailing /v1 is removed."),
    field("key", "API key", key, "Only needed if llama-swap's config sets apiKeys. Stored in your OS keychain, never in a file."),
    view.hasKey ? h("label", { class: "check" }, removeKey, "Remove the saved key") : null,
    field("poll", "Refresh every (seconds)", poll),
    h(
      "details",
      {},
      h("summary", {}, "Stall detection"),
      h(
        "div",
        { class: "thresholds" },
        field("load", "Loading timeout (s)", load),
        field("stop", "Unloading timeout (s)", stop),
        field("first-byte", "No first token after (s)", firstByte),
        field("stall", "Output stopped for (s)", stall),
      ),
    ),
    status,
    h("div", { class: "actions" }, cancelBtn, testBtn, saveBtn),
  );

  const apiKeyValue = (): string | null => {
    if (removeKey.checked) return "";
    const v = key.value.trim();
    return v ? v : null;
  };

  const collect = (): Settings => ({
    baseUrl: url.value,
    pollIntervalMs: Math.round(readNumber(poll, 2) * 1000),
    thresholds: {
      loadTimeoutS: readNumber(load, 120),
      stopTimeoutS: readNumber(stop, 30),
      firstByteTimeoutS: readNumber(firstByte, 90),
      streamStallTimeoutS: readNumber(stall, 30),
    },
  });

  const setBusy = (busy: boolean) => {
    testBtn.disabled = busy;
    saveBtn.disabled = busy;
  };

  const showStatus = (text: string, kind: "ok" | "error" | "pending") => {
    status.textContent = text;
    status.dataset.kind = kind;
  };

  const runTest = async (): Promise<boolean> => {
    setBusy(true);
    showStatus("Testing…", "pending");
    try {
      const result = await testConnection(url.value, apiKeyValue());
      const { ok, text } = describeResult(result, view.hasKey && apiKeyValue() === null);
      showStatus(text, ok ? "ok" : "error");
      return ok;
    } catch (e) {
      showStatus(String(e), "error");
      return false;
    } finally {
      setBusy(false);
    }
  };

  testBtn.addEventListener("click", () => void runTest());
  cancelBtn?.addEventListener("click", () => onCancel?.());
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    if (!(await runTest())) return;
    setBusy(true);
    try {
      onSaved(await saveSettings(collect(), apiKeyValue()));
    } catch (err) {
      showStatus(String(err), "error");
    } finally {
      setBusy(false);
    }
  });

  return h(
    "section",
    { class: "setup" },
    h("h1", {}, "Connect to llama-swap"),
    h("p", { class: "muted" }, "Point the monitor at your llama-swap instance. Save checks the connection first."),
    form,
  );
}

function describeResult(r: TestResult, usingSavedKey: boolean): { ok: boolean; text: string } {
  switch (r.kind) {
    case "ok":
      return { ok: true, text: `Connected — llama-swap ${r.version}` };
    case "unreachable":
      return { ok: false, text: `Can't reach that address: ${r.message}` };
    case "unauthorized":
      return {
        ok: false,
        text: usingSavedKey
          ? "llama-swap rejected the saved API key."
          : "llama-swap needs a valid API key (see apiKeys in its config).",
      };
    case "notLlamaSwap":
      return { ok: false, text: `That server doesn't look like llama-swap: ${r.message}` };
  }
}

function numberInput(id: string, value: number, min: number, max: number, step: number): HTMLInputElement {
  return h("input", { id, type: "number", value: String(value), min: String(min), max: String(max), step: String(step) });
}

function field(id: string, label: string, input: HTMLElement, hint?: string): HTMLElement {
  return h("div", { class: "field" }, h("label", { for: id }, label), input, hint ? h("span", { class: "hint" }, hint) : null);
}

function readNumber(input: HTMLInputElement, fallback: number): number {
  const n = Number(input.value);
  return Number.isFinite(n) && n > 0 ? n : fallback;
}
```

- [ ] **Step 5: Write a minimal `src/views/dashboard.ts` (Task 13 replaces it)**

```ts
import { h } from "../dom";
import type { Settings, Snapshot } from "../types";

export interface Dashboard {
  element: HTMLElement;
  update(s: Snapshot): void;
  destroy(): void;
}

export function createDashboard(settings: Settings, onOpenSettings: () => void): Dashboard {
  const pre = h("pre", {});
  const gear = h("button", { type: "button" }, "Settings");
  gear.addEventListener("click", onOpenSettings);
  const element = h("main", { class: "dashboard" }, h("div", {}, settings.baseUrl, " ", gear), pre);
  return {
    element,
    update(s) {
      pre.textContent = JSON.stringify(s, null, 2);
    },
    destroy() {},
  };
}
```

- [ ] **Step 6: Replace `src/main.ts`**

```ts
import "./styles.css";
import { getSettings, getSnapshot, onSnapshot } from "./api";
import type { SettingsView } from "./types";
import { createDashboard, type Dashboard } from "./views/dashboard";
import { renderSetup } from "./views/setup";

const root = document.getElementById("app")!;
let dashboard: Dashboard | null = null;

function showSetup(view: SettingsView) {
  dashboard?.destroy();
  dashboard = null;
  const cancel = view.configured ? () => void showDashboard(view) : undefined;
  root.replaceChildren(renderSetup(view, (saved) => void showDashboard(saved), cancel));
}

async function showDashboard(view: SettingsView) {
  dashboard?.destroy();
  const current = createDashboard(view.settings, async () => showSetup(await getSettings()));
  dashboard = current;
  root.replaceChildren(current.element);
  // The first snapshot may have been emitted before we subscribed; fetch it.
  const snap = await getSnapshot();
  if (snap && dashboard === current) current.update(snap);
}

async function boot() {
  await onSnapshot((s) => dashboard?.update(s));
  const view = await getSettings();
  if (view.configured) await showDashboard(view);
  else showSetup(view);
}

boot().catch((e) => {
  root.textContent = `Failed to start: ${e}`;
});
```

- [ ] **Step 7: Type-check and build**

Run: `npm run build`
Expected: `tsc` reports no errors and `vite build` writes `dist/`.

- [ ] **Step 8: Manual check against a real llama-swap**

Run: `npm run tauri dev`

Expected:
1. First launch shows "Connect to llama-swap".
2. A wrong port gives "Can't reach that address…".
3. If your llama-swap has `apiKeys`, a wrong key gives "llama-swap needs a valid API key…".
4. The correct URL and key give "Connected — llama-swap vNNN".
5. Save switches to a JSON dump of live snapshots.
6. Settings → change nothing → Save still works and the key is kept: the placeholder reads "Saved — leave blank to keep it".

Close the app.

- [ ] **Step 9: Commit**

```bash
git add src
git commit -m "feat: add setup view, typed IPC layer and styles" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: Dashboard and visual components

**Files:**
- Create: `src/views/components/ring.ts`, `sparkline.ts`, `ttlBar.ts`, `histogram.ts`, `statTile.ts`
- Modify: `src/views/dashboard.ts` (replace the whole file)

**Interfaces:**
- Consumes:
  - `h`, `svg` from `src/dom.ts`
  - `formatDuration`, `formatCount`, `formatAgo` from `src/format.ts`
  - Types from `src/types.ts`
- Produces:
  - `createRing(): Ring`, where `Ring = { el: SVGSVGElement; update(state: ModelState, loadTimeoutS: number): void }`
  - `renderSparkline(values: number[], width?, height?) -> SVGSVGElement`
  - `renderTtlBar(remainingS: number, totalS: number) -> HTMLElement`
  - `renderHistogram(hist: Histogram, width?, height?) -> SVGSVGElement`
  - `createStatTile(label: string): StatTile`, where `StatTile = { el: HTMLElement; update(value: string, sub?: string): void }`
  - The same `Dashboard` / `createDashboard` signature as Task 12.

Animations (spinning busy arc, pulsing loading ring) restart whenever their element is detached and reattached. So rings and cards are **persistent keyed elements** that are updated in place. They are re-inserted only when the model order actually changes.

- [ ] **Step 1: Write `src/views/components/ring.ts`**

```ts
import { svg } from "../../dom";
import type { ModelState } from "../../types";

const R = 26;
const C = 2 * Math.PI * R;

export interface Ring {
  el: SVGSVGElement;
  update(state: ModelState, loadTimeoutS: number): void;
}

export function createRing(): Ring {
  const track = svg("circle", { class: "ring-track", cx: 32, cy: 32, r: R });
  const arc = svg("circle", { class: "ring-arc", cx: 32, cy: 32, r: R, "stroke-dasharray": `${C} ${C}` });
  const label = svg("text", { class: "ring-label", x: 32, y: 32, "text-anchor": "middle", "dominant-baseline": "central" });
  const el = svg("svg", { class: "ring", viewBox: "0 0 64 64", width: 64, height: 64, role: "img" }, track, arc, label);

  return {
    el,
    update(state, loadTimeoutS) {
      let fraction = 1;
      let text = "";
      switch (state.kind) {
        case "loading":
          fraction = Math.min(1, state.elapsedS / Math.max(1, loadTimeoutS));
          text = `${state.elapsedS}s`;
          break;
        case "busy":
          fraction = 0.3;
          text = String(state.requests);
          break;
        case "stalled":
          text = "!";
          break;
        case "notLoaded":
        case "idle":
        case "unloading":
          break;
      }
      // Only touch attributes that changed, so running CSS animations are not disturbed.
      if (el.dataset.state !== state.kind) el.dataset.state = state.kind;
      arc.setAttribute("stroke-dasharray", `${(C * fraction).toFixed(1)} ${C.toFixed(1)}`);
      if (label.textContent !== text) label.textContent = text;
      el.setAttribute("aria-label", state.kind);
    },
  };
}
```

- [ ] **Step 2: Write `sparkline.ts`, `ttlBar.ts`, `histogram.ts`, `statTile.ts`**

`src/views/components/sparkline.ts`:
```ts
import { svg } from "../../dom";

export function renderSparkline(values: number[], width = 220, height = 32): SVGSVGElement {
  const el = svg("svg", { class: "sparkline", viewBox: `0 0 ${width} ${height}`, width: "100%", height, "aria-hidden": "true", preserveAspectRatio: "none" });
  if (values.length < 2) return el;
  const max = Math.max(...values);
  const min = Math.min(...values);
  const span = max - min || 1;
  const step = width / (values.length - 1);
  const pts = values.map((v, i) => `${(i * step).toFixed(1)},${(height - 2 - ((v - min) / span) * (height - 4)).toFixed(1)}`);
  el.append(
    svg("polygon", { class: "sparkline-fill", points: `0,${height} ${pts.join(" ")} ${width},${height}` }),
    svg("polyline", { class: "sparkline-line", points: pts.join(" ") }),
  );
  return el;
}
```

`src/views/components/ttlBar.ts`:
```ts
import { h } from "../../dom";
import { formatDuration } from "../../format";

export function renderTtlBar(remainingS: number, totalS: number): HTMLElement {
  const pct = totalS > 0 ? Math.max(0, Math.min(100, (remainingS / totalS) * 100)) : 0;
  const fill = h("div", { class: "ttl-fill" });
  fill.style.width = `${pct}%`;
  return h(
    "div",
    { class: "ttl" },
    h("div", { class: "ttl-track" }, fill),
    h("span", { class: "ttl-text" }, `Unloads in ~${formatDuration(remainingS)} (estimate)`),
  );
}
```

`src/views/components/histogram.ts`:
```ts
import { svg } from "../../dom";
import type { Histogram } from "../../types";

export function renderHistogram(hist: Histogram, width = 360, height = 120): SVGSVGElement {
  const el = svg("svg", { class: "histogram", viewBox: `0 0 ${width} ${height}`, width: "100%", height, role: "img", "aria-label": `p50 ${hist.p50.toFixed(1)}, p95 ${hist.p95.toFixed(1)}` });
  if (!hist.bins.length) return el;
  const plotH = height - 18;
  const maxCount = Math.max(1, ...hist.bins);
  const barW = width / hist.bins.length;
  hist.bins.forEach((count, i) => {
    const bh = (count / maxCount) * plotH;
    el.append(svg("rect", {
      class: "hist-bar",
      x: (i * barW + 1).toFixed(1),
      y: (plotH - bh).toFixed(1),
      width: Math.max(1, barW - 2).toFixed(1),
      height: bh.toFixed(1),
    }));
  });
  const range = hist.max - hist.min || 1;
  const marks: [string, number][] = [["p50", hist.p50], ["p95", hist.p95]];
  for (const [name, value] of marks) {
    const x = Math.max(0, Math.min(width, ((value - hist.min) / range) * width));
    el.append(
      svg("line", { class: "hist-mark", x1: x.toFixed(1), x2: x.toFixed(1), y1: 0, y2: plotH }),
      svg("text", { class: "hist-label", x: x.toFixed(1), y: height - 4, "text-anchor": "middle" }, `${name} ${value.toFixed(1)}`),
    );
  }
  return el;
}
```

`src/views/components/statTile.ts`:
```ts
import { h } from "../../dom";

export interface StatTile {
  el: HTMLElement;
  update(value: string, sub?: string): void;
}

export function createStatTile(label: string): StatTile {
  const value = h("div", { class: "tile-value" }, "—");
  const sub = h("div", { class: "tile-sub" });
  const el = h("div", { class: "tile" }, h("div", { class: "tile-label" }, label), value, sub);
  return {
    el,
    update(v, s = "") {
      value.textContent = v;
      sub.textContent = s;
    },
  };
}
```

- [ ] **Step 3: Replace `src/views/dashboard.ts`**

```ts
import { h } from "../dom";
import { formatAgo, formatCount, formatDuration } from "../format";
import type { Connection, ModelCard, ModelState, Settings, Snapshot } from "../types";
import { renderHistogram } from "./components/histogram";
import { createRing, type Ring } from "./components/ring";
import { renderSparkline } from "./components/sparkline";
import { createStatTile } from "./components/statTile";
import { renderTtlBar } from "./components/ttlBar";

export interface Dashboard {
  element: HTMLElement;
  update(s: Snapshot): void;
  destroy(): void;
}

const PILL: Record<ModelState["kind"], string> = {
  notLoaded: "Not loaded",
  loading: "Loading",
  idle: "Idle",
  busy: "Busy",
  stalled: "Stalled",
  unloading: "Unloading",
};

const CONN_TEXT: Record<Connection["kind"], string> = {
  connecting: "Connecting…",
  connected: "Connected",
  unauthorized: "Unauthorized",
  unreachable: "Unreachable",
  error: "Error",
};

function lastOf(values: number[]): number | undefined {
  return values.length ? values[values.length - 1] : undefined;
}

function describe(card: ModelCard, settings: Settings): string {
  const s = card.state;
  const tok = lastOf(card.tokSHistory);
  switch (s.kind) {
    case "notLoaded":
      return card.lastRequestAtMs ? `Last used ${formatAgo(card.lastRequestAtMs)}` : "";
    case "loading":
      return `Starting for ${formatDuration(s.elapsedS)} · timeout ${formatDuration(settings.thresholds.loadTimeoutS)}`;
    case "idle":
      return `Up ${formatDuration(s.uptimeS)}${tok ? ` · last ${tok.toFixed(1)} tok/s` : ""}`;
    case "busy":
      return [
        `${s.requests} request${s.requests === 1 ? "" : "s"}`,
        tok ? `${tok.toFixed(1)} tok/s` : null,
        formatDuration(s.oldestElapsedS),
      ].filter(Boolean).join(" · ");
    case "stalled":
      return s.reason;
    case "unloading":
      return "Stopping…";
  }
}

class CardView {
  readonly el: HTMLElement;
  private readonly ring: Ring = createRing();
  private readonly title = h("div", { class: "card-title" });
  private readonly pill = h("span", { class: "pill" });
  private readonly detail = h("div", { class: "card-detail" });
  private readonly extras = h("div", { class: "card-extras" });

  constructor(private readonly settings: Settings) {
    this.el = h(
      "article",
      { class: "card" },
      h("div", { class: "card-head" }, this.ring.el, h("div", { class: "card-heading" }, this.title, this.pill)),
      this.detail,
      this.extras,
    );
  }

  update(card: ModelCard) {
    const s = card.state;
    if (this.el.dataset.state !== s.kind) this.el.dataset.state = s.kind;
    this.title.textContent = card.name;
    this.title.title = card.description ? `${card.id} — ${card.description}` : card.id;
    this.ring.update(s, this.settings.thresholds.loadTimeoutS);
    this.pill.textContent = PILL[s.kind];
    this.detail.textContent = describe(card, this.settings);
    const extras: Node[] = [];
    if ((s.kind === "busy" || s.kind === "idle") && card.tokSHistory.length >= 2) {
      extras.push(renderSparkline(card.tokSHistory));
    }
    if (s.kind === "idle" && card.ttlS !== null && card.ttlRemainingS !== null) {
      extras.push(renderTtlBar(card.ttlRemainingS, card.ttlS));
    }
    this.extras.replaceChildren(...extras);
  }
}

export function createDashboard(settings: Settings, onOpenSettings: () => void): Dashboard {
  const dot = h("span", { class: "dot" });
  const statusText = h("span", { class: "status-text" }, "Connecting…");
  const live = h("span", { class: "badge" });
  const host = h("span", { class: "host" }, settings.baseUrl);
  const meta = h("span", { class: "meta" });
  const gear = h("button", { type: "button", class: "icon-button", "aria-label": "Settings", title: "Settings" }, "⚙");
  gear.addEventListener("click", onOpenSettings);
  const header = h(
    "header",
    { class: "header" },
    dot,
    h("div", { class: "header-main" }, h("div", { class: "header-line" }, statusText, live), h("div", { class: "header-line" }, host, meta)),
    gear,
  );

  const banner = h("div", { class: "banner" });
  banner.hidden = true;
  const openSettings = h("button", { type: "button" }, "Open settings");
  openSettings.addEventListener("click", onOpenSettings);

  const grid = h("section", { class: "grid" });
  const empty = h("p", { class: "empty" }, "No models reported yet.");
  const tiles = {
    requests: createStatTile("Requests"),
    input: createStatTile("Input tokens"),
    output: createStatTile("Output tokens"),
    speed: createStatTile("Generation p50"),
  };
  const histBox = h("div", { class: "hist-box" });
  const stats = h(
    "section",
    { class: "stats" },
    h("div", { class: "tiles" }, tiles.requests.el, tiles.input.el, tiles.output.el, tiles.speed.el),
    histBox,
  );
  const element = h("main", { class: "dashboard" }, header, banner, grid, stats);

  const cards = new Map<string, CardView>();
  let last: Snapshot | null = null;

  function showBanner(text: string, withSettingsButton: boolean) {
    banner.replaceChildren(h("span", {}, text), withSettingsButton ? openSettings : null);
    banner.hidden = false;
  }

  function renderHeader(s: Snapshot) {
    const c = s.connection;
    element.dataset.conn = c.kind;
    element.classList.toggle("stale", c.kind !== "connected" && c.kind !== "connecting");
    statusText.textContent = CONN_TEXT[c.kind];
    meta.textContent = [s.version ? `llama-swap ${s.version}` : null, c.kind === "connected" ? `${c.latencyMs} ms` : null]
      .filter(Boolean)
      .join(" · ");
    live.textContent = s.liveEvents ? "live" : "polling";
    live.title = s.liveEvents
      ? "Receiving live request events"
      : "Live events unavailable: Busy and stalled-request detection are off";
    live.classList.toggle("badge-live", s.liveEvents);

    const seen = s.lastOkMs ? ` · last seen ${formatAgo(s.lastOkMs)}` : "";
    if (c.kind === "unauthorized") showBanner("llama-swap rejected the API key.", true);
    else if (c.kind === "unreachable" || c.kind === "error") showBanner(`${c.message}${seen}`, false);
    else banner.hidden = true;
  }

  function renderCards(s: Snapshot) {
    const ordered: HTMLElement[] = [];
    const ids = new Set<string>();
    for (const m of s.models) {
      let view = cards.get(m.id);
      if (!view) {
        view = new CardView(settings);
        cards.set(m.id, view);
      }
      view.update(m);
      ids.add(m.id);
      ordered.push(view.el);
    }
    for (const id of [...cards.keys()]) if (!ids.has(id)) cards.delete(id);
    const wanted = ordered.length ? ordered : [empty];
    const same = wanted.length === grid.children.length && wanted.every((el, i) => grid.children[i] === el);
    if (!same) grid.replaceChildren(...wanted);
  }

  function renderStats(s: Snapshot) {
    const st = s.stats;
    if (!st) {
      for (const t of Object.values(tiles)) t.update("—");
      histBox.replaceChildren(
        h("p", { class: "muted" }, s.statsAvailable ? "Waiting for metrics…" : "This llama-swap version does not report metrics."),
      );
      return;
    }
    tiles.requests.update(formatCount(st.totalRequests));
    tiles.input.update(formatCount(st.totalInputTokens), st.totalCacheTokens ? `${formatCount(st.totalCacheTokens)} cached` : "");
    tiles.output.update(formatCount(st.totalOutputTokens));
    if (st.gen && st.gen.bins.length) {
      tiles.speed.update(`${st.gen.p50.toFixed(1)} tok/s`, `p95 ${st.gen.p95.toFixed(1)} · p99 ${st.gen.p99.toFixed(1)}`);
      histBox.replaceChildren(h("div", { class: "hist-title" }, "Generation speed (tok/s)"), renderHistogram(st.gen));
    } else {
      tiles.speed.update("—");
      histBox.replaceChildren(h("p", { class: "muted" }, "No generation requests yet."));
    }
  }

  function render() {
    if (!last) return;
    renderHeader(last);
    renderCards(last);
    renderStats(last);
  }

  // Re-render every second so "last seen … ago" stays current between snapshots.
  const timer = window.setInterval(render, 1000);

  return {
    element,
    update(s) {
      last = s;
      render();
    },
    destroy() {
      window.clearInterval(timer);
    },
  };
}
```

- [ ] **Step 4: Type-check and build**

Run: `npm run build`
Expected: no `tsc` errors and `dist/` is written.

- [ ] **Step 5: Manual visual check against a real llama-swap**

Run: `npm run tauri dev`

Then check each case and its expected result:
1. **Configured but unloaded models:** dimmed grey cards with a thin ring and a "Not loaded" pill.
2. **Trigger a model load** (send a request to an unloaded model): amber dashed pulsing ring with elapsed seconds in the center, and "Starting for N s · timeout 2 m".
3. **While a response streams:** blue spinning arc with the request count in the center, a "Busy" pill, and the detail "1 request · NN tok/s · N s". The badge says `live`.
4. **After it finishes:** solid green ring, "Idle", "Up …", a tok/s sparkline (after ≥2 requests), and a TTL bar when the model has a `ttl`.
5. **Stop llama-swap:** the header dot turns amber with "Unreachable", the banner shows "… · last seen N s ago", and the cards desaturate but keep their last state. Restart llama-swap and it recovers by itself within 30 s.
6. **Stats row:** shows totals and a generation-speed histogram with p50/p95 markers.
7. **Busy for several seconds:** the spinning arc stays smooth and does not restart every second.

Close the app.

- [ ] **Step 6: Commit**

```bash
git add src/views
git commit -m "feat: add dashboard with animated model cards and stats" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: CI, release workflow, README

**Files:**
- Create: `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `README.md`

**Interfaces:**
- Consumes: the npm scripts and `cargo test` from earlier tasks.
- Produces: tests on every push/PR, and draft GitHub Releases with installers on `v*` tags.

- [ ] **Step 1: Write `.github/workflows/ci.yml`**

```yaml
name: ci
on:
  push:
    branches: [main, master]
  pull_request:

jobs:
  test:
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v4
      - name: Install Linux dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf libdbus-1-dev pkg-config
      - uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: npm
      - uses: dtolnay/rust-toolchain@stable
      - uses: swatinem/rust-cache@v2
        with:
          workspaces: "./src-tauri -> target"
      - run: npm ci
      - run: npm run build
      - run: cargo test --manifest-path src-tauri/Cargo.toml
```

- [ ] **Step 2: Write `.github/workflows/release.yml`**

```yaml
name: release
on:
  push:
    tags: ["v*"]

permissions:
  contents: write

jobs:
  build:
    strategy:
      fail-fast: false
      matrix:
        include:
          - platform: windows-latest
            args: ""
          - platform: macos-latest
            args: "--target universal-apple-darwin"
          - platform: ubuntu-22.04
            args: ""
    runs-on: ${{ matrix.platform }}
    steps:
      - uses: actions/checkout@v4
      - name: Install Linux dependencies
        if: matrix.platform == 'ubuntu-22.04'
        run: |
          sudo apt-get update
          sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf libdbus-1-dev pkg-config
      - uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: npm
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.platform == 'macos-latest' && 'aarch64-apple-darwin,x86_64-apple-darwin' || '' }}
      - uses: swatinem/rust-cache@v2
        with:
          workspaces: "./src-tauri -> target"
      - run: npm ci
      - uses: tauri-apps/tauri-action@v0
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        with:
          tagName: ${{ github.ref_name }}
          releaseName: "llama-swap Monitor ${{ github.ref_name }}"
          releaseBody: "Unsigned build. See the README for first-launch steps on each OS."
          releaseDraft: true
          prerelease: false
          args: ${{ matrix.args }}
```

- [ ] **Step 3: Write `README.md`**

````markdown
# llama-swap Monitor

A small desktop app that shows what your [llama-swap](https://github.com/mostlygeek/llama-swap)
instance is doing: whether it's reachable, which models are loaded, which are busy or stuck,
and a summary of throughput.

Works with llama-swap running anywhere you can reach over HTTP: the same machine, a box
on your LAN, or behind a reverse proxy.

## What the states mean

| State | Looks like | Meaning |
|---|---|---|
| Not loaded | dimmed grey ring | Configured in llama-swap, not running |
| Loading | amber pulsing ring, seconds in the middle | llama-swap is starting the model |
| Idle | solid green ring, TTL bar | Loaded, no requests in flight |
| Busy | spinning blue arc, request count | At least one request in flight |
| Stalled | red ring with `!` | Loading/unloading took too long, or a request stopped producing output |
| Unloading | fading grey ring | llama-swap is stopping the model |

Busy and request-level Stalled detection need llama-swap's live event stream (`/api/events`).
The header badge shows `live` when it's connected and `polling` when it isn't.

## Install

Download the installer for your OS from the [Releases](../../releases) page.

These builds are **not code-signed**, so each OS warns on first launch:

- **Windows:** SmartScreen says "Windows protected your PC". Click **More info → Run anyway**.
- **macOS:** right-click the app → **Open** → **Open**. If macOS says the app is damaged, run
  `xattr -dr com.apple.quarantine "/Applications/llama-swap Monitor.app"`.
- **Linux:** use the `.deb`, or `chmod +x` the `.AppImage` and run it. The API key is stored
  through the Secret Service (GNOME Keyring or KWallet), so one of those must be running.

## Connect

1. Enter llama-swap's URL, e.g. `http://192.168.1.20:8080`. Paste the OpenAI-style `.../v1`
   URL if that's what you have; the `/v1` is removed.
2. If your llama-swap config has `apiKeys:`, paste one of those keys. It is stored in your OS
   keychain (Windows Credential Manager, macOS Keychain, Linux Secret Service), never in a
   file, and the UI never sees it again.
3. **Save** tests the connection first and only saves if it works.

Stall thresholds (loading timeout, no-first-token timeout, and so on) are under
**Stall detection** in settings.

## Develop

Requires Node 22+, Rust stable, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/)
for your OS.

```bash
npm install
npm run tauri dev
```

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

Releases: push a `v*` tag. GitHub Actions builds Windows, macOS (universal) and Linux
installers into a draft release.
````

- [ ] **Step 4: Verify the workflows parse and the suite still passes**

Run: `cargo test --manifest-path src-tauri/Cargo.toml && npm run build`
Expected: all Rust tests pass and the frontend builds.

Optional, if `actionlint` is installed: `actionlint .github/workflows/*.yml`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add .github README.md
git commit -m "ci: add test workflow, cross-platform release workflow and README" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
