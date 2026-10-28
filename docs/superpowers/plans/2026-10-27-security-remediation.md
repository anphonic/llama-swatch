# Security Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the Medium and Low findings from the October 2026 security audit, in four small PRs, without changing what the app shows a user whose server behaves normally.

**Architecture:** Every fix is local to the module the finding names. Network input gets hard size and time limits (body reads, SSE lines and events, stream idleness, in-flight table). Settings writes are serialized and made durable. Skin files are read once, with a bound. The setup form warns before it sends a key over plain HTTP. CI actions are pinned to commit SHAs.

**Tech Stack:** Rust (Tauri 2, reqwest, tokio, futures-util, serde_json; wiremock and tempfile for tests), TypeScript views (no frontend test runner), GitHub Actions.

**Spec:** The audit report. Its findings are restated in each task below, so the implementer needs only the task brief. Finding IDs (M1–M3, L1–L6, I1–I9) refer to that report.

## Global Constraints

- The API key is never logged, emitted, stored in settings JSON, or returned to the webview. No new code path may print it, and no new error string may contain server bytes.
- The saved key goes only to the saved URL. Redirects stay disabled. Certificate verification stays on.
- Subagents never contact any network server and never launch the GUI. Tests use wiremock or local listeners on 127.0.0.1 only.
- No real LAN hostnames or IPs in code, docs, tests or commit messages. Use `llama.example.test` where an example host is needed.
- Error strings shown to the user stay short and plain, like the existing ones ("couldn't be read", "larger than 16 KB").
- No new dependencies. Everything needed is already in `src-tauri/Cargo.toml`.
- Every Rust change passes: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, run from `src-tauri/`. TypeScript changes pass `npm run build`.
- Commits use `git -c user.name=Andrew -c user.email=5176615+anphonic@users.noreply.github.com commit` and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Match the surrounding code's style: short doc comments that say why, the same naming and error idioms.

## PR layout

| PR | Branch (off `master`) | Tasks | Findings |
|---|---|---|---|
| A | `fix/network-input-limits` | 1–4 | M1, M2, M3, L1 |
| B | `fix/settings-writes` | 5–6 | L2, L5, L6 |
| C | `fix/skin-file-reads` | 7 | L4 |
| D | `chore/plain-http-warning-and-ci-pins` | 8–9 | L3, I8 |

Accepted with no code change (listed in PR A's description): I1 `test_connection` probes an arbitrary URL but sends no key; I2 serde error text only reaches local logs, never with payload bytes; I3 local paths in history; I4 CSP `style-src 'unsafe-inline'` (needed for runtime skin colours, no script injection path); I5 `gsettings` found via PATH; I6 DBus sender unchecked (read-only theme signal); I7 unsigned builds (documented in the README); I9 glib RUSTSEC-2024-0429 (dismissed, waiting on upstream). The I8 build-job `contents: write` is required by `tauri-action` to upload release assets, so only the SHA pinning half of I8 is done.

## Review Focus

- A normal llama-swap that is quiet for minutes (no requests, no log lines) must not make the UI flicker to offline. The idle reconnect is silent unless the reconnect itself fails. (Task 3)
- A real `/v1/models` or `/running` reply from a server with many models (hundreds of KB) must still decode. Caps are set well above real replies. (Task 1)
- A legitimate large SSE frame, such as a long log line from llama-swap, must not drop the stream. The cap is far above normal frames, and only the oversized frame is lost. (Task 2)
- Two settings changes made at the same moment (pin toggle while the window moves) must both land, and the file must always parse. (Task 5)
- A settings file written by an older version, whose `skin` is an id this version does not know, must still load. Validation applies to `set_skin` input only. (Task 5)

---

## PR A: network input limits (`fix/network-input-limits`)

### Task 1: Cap response bodies in `get_json` and `health` (M1)

**Files:**
- Modify: `src-tauri/src/client.rs` (`get_json` ~line 148, `health` ~line 155, add constants near line 14, tests in the existing `mod tests`)

**Interfaces:**
- Produces: `async fn read_limited(resp: Response, cap: usize) -> Result<Vec<u8>, ClientError>`, private to `client.rs`. Over cap: `Err(ClientError::Decode(format!("reply larger than {} KB", cap / 1024)))`. Read error: `Err(ClientError::Unreachable(describe(e)))`.
- Constants: `const JSON_BODY_CAP: usize = 4 * 1024 * 1024;` and `const HEALTH_BODY_CAP: usize = 64 * 1024;`

- [ ] **Step 1: Write the failing tests** in `client.rs` tests, using wiremock like the neighbouring tests:
  - `get_json_rejects_body_over_cap`: mount `/running` returning `200` with a body of `JSON_BODY_CAP + 1` bytes. Call the public method that uses `get_json` for `/running`. Assert `matches!(err, ClientError::Decode(m) if m.contains("larger than"))`.
  - `health_rejects_body_over_cap`: mount `/health` returning `"x".repeat(HEALTH_BODY_CAP + 1)`. Assert `health()` returns `Err(ClientError::Decode(_))`.
  - `get_json_accepts_body_at_cap_boundary`: a valid JSON reply padded with spaces to exactly `JSON_BODY_CAP` bytes decodes `Ok`.
- [ ] **Step 2: Run** `cargo test get_json_ health_rejects` in `src-tauri/`. Expected: the two reject tests FAIL.
- [ ] **Step 3: Implement `read_limited`.** Loop on `resp.chunk()` and fail as soon as `buf.len() + chunk.len() > cap`, so no more than `cap` bytes are ever held. Do not reuse `read_capped`: its truncation flag mixes "over cap" with "read error". Switch `get_json` to `read_limited(resp, JSON_BODY_CAP)` and `health` to `read_limited(resp, HEALTH_BODY_CAP)` plus `String::from_utf8_lossy`.
- [ ] **Step 4: Run** the full `cargo test`, clippy and fmt. Expected: all pass.
- [ ] **Step 5: Commit** `fix: cap reply bodies the client reads from llama-swap`

### Task 2: Bound SSE line and event size (M2)

**Files:**
- Modify: `src-tauri/src/sse.rs` (struct, `push`, tests)
- Modify: `src-tauri/src/runtime.rs` `event_loop` (react to overflow)

**Interfaces:**
- Produces: `pub const MAX_LINE_BYTES: usize = 1024 * 1024;` and `pub const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;` in `sse.rs`.
- Produces: `SseParser::push(&mut self, chunk: &[u8]) -> Vec<String>` (signature unchanged), plus `pub fn overflowed(&self) -> bool`. It is true after any line or event was dropped for size since the parser was created.
- Behaviour: a line longer than `MAX_LINE_BYTES` is discarded up to its next `\n`, and the event it belongs to is dropped. An event whose `data` lines total more than `MAX_EVENT_BYTES` is dropped at its blank line. Parsing resumes normally with the next event.
- Performance: remember how far the buffer has already been scanned for `\n`, so each byte is scanned once (no quadratic rescans when a line arrives in many chunks).

- [ ] **Step 1: Write the failing tests** in `sse.rs`:
  - `oversized_line_is_dropped_and_parsing_resumes`: push `b"data:"`, then `MAX_LINE_BYTES + 10` bytes of `b'a'` in 64 KB chunks, then `b"\n\ndata:ok\n\n"`. Assert the collected output is `vec!["ok"]` and `p.overflowed()`.
  - `buffer_never_exceeds_line_cap`: after pushing `3 * MAX_LINE_BYTES` bytes with no newline, assert `p.buffered_len() <= MAX_LINE_BYTES`. Add `#[cfg(test)] fn buffered_len(&self) -> usize`.
  - `oversized_event_is_dropped`: push enough `data:` lines of 512 KB each to pass `MAX_EVENT_BYTES`, then `\n`, then `data:ok\n\n`. Assert output `vec!["ok"]`.
  - `normal_frames_unchanged`: the existing tests still pass unchanged.
- [ ] **Step 2: Run** `cargo test sse::`. Expected: the new tests FAIL.
- [ ] **Step 3: Implement** the caps and the scan offset in `SseParser`. In `runtime.rs` `event_loop`, no reconnect is needed for overflow: the parser recovers by itself. Record one failed decode (`record_decode(&shared, false)`) the first time `overflowed()` turns true on a connection, so a server that always sends huge frames shows as unreadable instead of silently live.
- [ ] **Step 4: Run** `cargo test`, clippy and fmt. Expected: pass.
- [ ] **Step 5: Commit** `fix: bound SSE line and event size`

### Task 3: Idle timeout on the event stream (M3)

**Files:**
- Modify: `src-tauri/src/runtime.rs` (`event_loop`, constants near line 23, tests)

**Interfaces:**
- Produces: `const EVENTS_IDLE_TIMEOUT: Duration = Duration::from_secs(120);`
- Produces: a private helper `async fn next_chunk<S>(stream: &mut S, idle: Duration) -> Next`, where `S: Stream<Item = reqwest::Result<Bytes>> + Unpin` (make it generic over the error type if that makes the test simpler) and `enum Next { Chunk(Bytes), Ended, Idle }`. `Ended` covers both end of stream and a stream error.

- [ ] **Step 1: Write the failing tests** in `runtime.rs` tests with `#[tokio::test(start_paused = true)]`:
  - `next_chunk_reports_idle_after_timeout`: `futures_util::stream::pending()` gives `Next::Idle` after `EVENTS_IDLE_TIMEOUT`.
  - `next_chunk_passes_chunks_and_end`: `stream::iter([Ok(Bytes::from_static(b"a"))])` gives `Chunk`, then `Ended`.
- [ ] **Step 2: Run** `cargo test next_chunk`. Expected: FAIL (helper missing).
- [ ] **Step 3: Implement** `next_chunk` with `tokio::time::timeout(idle, stream.next())`. In `event_loop`, replace `while let Some(Ok(chunk)) = stream.next().await` with a loop over `next_chunk`.
  - On `Ended`, keep today's behaviour: `events_disconnected`, publish, backoff.
  - On `Idle`, reconnect quietly: don't call `events_disconnected`, sleep zero, and don't call `events_connected` again for that reconnect. Decode health and the in-flight table stay as they are, and the next snapshot from the new connection replaces the table. If the quiet reconnect fails to connect, take the normal failure path (`events_disconnected`, publish, backoff).
  - Track this with a local `quiet: bool` in the loop. Comment why: llama-swap may send nothing for minutes when idle, and that must not flicker the UI.
- [ ] **Step 4: Run** `cargo test`, clippy and fmt. Expected: pass.
- [ ] **Step 5: Commit** `fix: reconnect an event stream that goes silent`

### Task 4: Cap the in-flight table (L1)

**Files:**
- Modify: `src-tauri/src/events.rs` (`InflightTable::apply`/`upsert`, `streamed_bytes`, tests)

**Interfaces:**
- Produces: `pub const MAX_INFLIGHT: usize = 1024;`
- Behaviour: once the table holds `MAX_INFLIGHT` entries, an `upsert` for an unknown id is ignored, while updates to known ids still apply. A `snapshot` keeps at most the first `MAX_INFLIGHT` entries.
- `streamed_bytes` sums with `saturating_add` (folds L6's events.rs half in here).

- [ ] **Step 1: Write the failing tests:**
  - `upsert_ignores_new_ids_when_full`: fill to `MAX_INFLIGHT` with distinct ids, upsert a new id, assert the table size is still `MAX_INFLIGHT` and the new id is absent from `views_for`.
  - `snapshot_is_truncated_to_cap`: a snapshot of `MAX_INFLIGHT + 5` entries leaves `MAX_INFLIGHT`.
  - `streamed_bytes_saturates`: two started entries with `resp_bytes: i64::MAX` give `Some(i64::MAX)`.
  - Add `#[cfg(test)] fn len(&self) -> usize` if needed.
- [ ] **Step 2: Run** `cargo test events::`. Expected: new tests FAIL (the saturating one panics in debug).
- [ ] **Step 3: Implement** the caps and the saturating sum.
- [ ] **Step 4: Run** `cargo test`, clippy and fmt. Expected: pass.
- [ ] **Step 5: Commit** `fix: cap the in-flight table and saturate its byte sum`

---

## PR B: settings writes (`fix/settings-writes`)

### Task 5: Serialize, harden and validate settings writes (L2, L5)

**Files:**
- Modify: `src-tauri/src/config.rs` (`save_settings` ~line 198, the `save_*` helpers, `save_skin` ~line 230, tests)
- Modify: `src-tauri/src/commands.rs` `set_skin` (~line 220)
- Read: `src-tauri/src/skins.rs` `builtins()` for the built-in ids

**Interfaces:**
- Produces: `static SETTINGS_WRITE: Mutex<()>` in `config.rs`. Every `save_*` helper holds it across its whole load-modify-save, and so does `commands::save_settings`'s call into `config::save_settings`. Recover from poisoning with `lock().unwrap_or_else(PoisonError::into_inner)`.
- `save_settings` writes to a unique temp file in the same directory (`settings.json.<pid>.<n>.tmp`, with `n` from a static `AtomicU64`), calls `sync_all()` on it before `rename`, and removes the temp file if the rename fails.
- Produces: `pub fn is_valid_skin_id(id: &str) -> bool` in `skins.rs`. It is true for any id in `builtins()`, or for `file:<name>` where `<name>` ends in `.json` (ASCII case-insensitive), is 1–255 bytes, and contains no `/`, `\`, `..`, `:` or control characters.
- `commands::set_skin` returns `Err("unknown skin".into())` when `!is_valid_skin_id(&skin)`, before touching the file. `load_settings` does NOT validate the skin (old files must still load).

- [ ] **Step 1: Write the failing tests:**
  - `config.rs` `concurrent_saves_all_land`: save defaults, then spawn 8 threads that each run `save_always_on_top`/`save_check_for_updates`/`save_skin` 50 times. Join them, then assert `load_settings` is `Some` and the last value written by each single-writer thread is present (give each field one thread).
  - `config.rs` `save_leaves_no_temp_files`: after several saves, the directory holds only `settings.json`.
  - `skins.rs` `skin_id_validation`: valid: `"builtin:standard"`, `"file:My Skin.json"`, `"file:a.JSON"`. Invalid: `""`, `"builtin:nope"`, `"file:../x.json"`, `"file:a/b.json"`, `"file:a\\b.json"`, `"file:x.txt"`, `"file:.json"`, `"file:a:b.json"`, `"other"`, and a 300-byte name.
- [ ] **Step 2: Run** `cargo test config:: skins::skin_id`. Expected: validation and concurrency tests FAIL (the concurrency test may pass by luck; run it 3 times).
- [ ] **Step 3: Implement** the lock, the unique temp file with fsync, `is_valid_skin_id` and the `set_skin` check.
- [ ] **Step 4: Run** `cargo test`, clippy and fmt. Expected: pass.
- [ ] **Step 5: Commit** `fix: serialize settings writes and validate skin ids`

### Task 6: Overflow-safe window geometry (L6)

**Files:**
- Modify: `src-tauri/src/window.rs` `visible_on` (~line 37, tests)

**Interfaces:**
- `visible_on(b: &Bounds, monitors: &[(i32, i32, u32, u32)]) -> bool` keeps its signature. Do the arithmetic in `i64` so `b.x + b.width` and `x + w` cannot overflow.

- [ ] **Step 1: Write the failing test** `visible_on_handles_extreme_values`: `Bounds { x: i32::MAX - 10, y: 0, width: u32::MAX, height: 100 }` against `[(0, 0, 1920, 1080)]` returns without panicking (`false`), and `Bounds { x: -100, y: -100, width: u32::MAX, height: 10 }` against the same monitor returns `true`.
- [ ] **Step 2: Run** `cargo test visible_on`. Expected: panics with overflow in debug.
- [ ] **Step 3: Implement** with `i64`.
- [ ] **Step 4: Run** `cargo test`, clippy and fmt. Expected: pass.
- [ ] **Step 5: Commit** `fix: overflow-safe window visibility check`

---

## PR C: skin file reads (`fix/skin-file-reads`)

### Task 7: One bounded read per skin file, no symlinks, capped count (L4)

**Files:**
- Modify: `src-tauri/src/skins.rs` `list` (~line 170, tests)

**Interfaces:**
- Produces: `pub const MAX_SKIN_FILES: usize = 200;`
- Behaviour:
  - Entries are filtered with `symlink_metadata`: symlinks are skipped with reason `"is a link"`.
  - Each file is read once through `File::open(..).take(MAX_FILE_BYTES + 1).read_to_end`. A read longer than `MAX_FILE_BYTES` gives `"larger than 16 KB"` (same text as today). The separate `fs::metadata` size check goes away.
  - After sorting by file name, only the first `MAX_SKIN_FILES` are parsed. The rest go to `skipped` with reason `"too many skin files"`.

- [ ] **Step 1: Write the failing tests** (using `tempfile::tempdir()`):
  - `oversized_file_skipped_with_single_read`: a 20 KB file is skipped with `"larger than 16 KB"`.
  - `symlinked_skin_is_skipped`: guarded with `#[cfg(unix)]`, a symlink to a valid skin is skipped with `"is a link"`.
  - `skin_file_count_is_capped`: `MAX_SKIN_FILES + 3` valid files give `MAX_SKIN_FILES` loaded and 3 skipped with `"too many skin files"`.
- [ ] **Step 2: Run** `cargo test skins::`. Expected: the new tests FAIL (the symlink test only runs on unix; it is fine that it does not run on Windows).
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test`, clippy and fmt. Expected: pass.
- [ ] **Step 5: Commit** `fix: read skin files once with a bound and skip links`

---

## PR D: setup warning and CI pins (`chore/plain-http-warning-and-ci-pins`)

### Task 8: Warn before sending a key over plain HTTP (L3)

**Files:**
- Modify: `src/views/setup.ts` (URL and key fields, ~lines 15 and 93)
- Modify: `src/styles.css` or the stylesheet the setup form already uses (one small rule, reuse an existing hint/warning class if there is one)

**Interfaces:**
- Produces: `function plainHttpRisk(url: string, key: string): boolean`, exported from `setup.ts`. It is true when `key.trim()` is non-empty, `url` parses (prefix `http://` when no scheme, the way the form already normalises) with protocol `http:`, and the hostname is not `localhost`, `127.0.0.1`, `::1`/`[::1]`, or a name ending in `.localhost`.
- UI: below the key field, a hint with exact text `This key will travel unencrypted over http://. Use https:// if the network isn't trusted.` It is shown only while `plainHttpRisk` is true, and updated on `input` in either field. It is a warning, not a block: Save still works.
- Text goes in with `textContent` (no `innerHTML`).
- Also change the URL field hint example from `http://192.168.1.20:8080` to `http://llama.example.test:8080` (no LAN-looking addresses in UI copy).

- [ ] **Step 1:** There is no frontend test runner, so add no test framework. Write `plainHttpRisk` as a pure function so it is easy to check.
- [ ] **Step 2: Implement** the function, the hint element and the listeners.
- [ ] **Step 3: Run** `npm run build`. Expected: succeeds with no type errors.
- [ ] **Step 4: Commit** `feat: warn before sending the API key over plain http`

### Task 9: Pin GitHub Actions to commit SHAs (I8)

**Files:**
- Modify: `.github/workflows/ci.yml`, `.github/workflows/release.yml`
- Create: `.github/dependabot.yml`

**Interfaces:** Pin each `uses:` to the SHA its current tag points at, keeping the tag as a trailing comment. Do not upgrade major versions in this task. SHAs (looked up 2026-10-27; the implementer must not fetch them):

| Action | Tag | SHA |
|---|---|---|
| actions/checkout | v4 | `11d5960a326750d5838078e36cf38b85af677262` |
| actions/setup-node | v4 | `49933ea5288caeca8642d1e84afbd3f7d6820020` |
| dtolnay/rust-toolchain | stable | `b725eda525362e7cf34af9a7243cac1e3f01a7ec` |
| swatinem/rust-cache | v2 | `d1527c8f67ef9af357290475d4af4b0fab8f12e0` |
| tauri-apps/tauri-action | v0 | `84b9d35b5fc46c1e45415bdb6144030364f7ebc5` |

Form: `- uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4`. For `dtolnay/rust-toolchain@stable`, the pinned commit no longer picks the toolchain from the ref name, so add `with: toolchain: stable` (merge it with any existing `with:` block).

`.github/dependabot.yml` is version 2, with ecosystem `github-actions`, directory `/`, a weekly schedule, and a group `actions` covering `*`. It keeps the pins current.

- [ ] **Step 1: Edit** both workflows and create dependabot.yml.
- [ ] **Step 2: Verify** with `grep -n "uses:" .github/workflows/*.yml`: every line shows a 40-hex SHA and a `# tag` comment. Also confirm both files parse with `python -c "import yaml,sys;[yaml.safe_load(open(f)) for f in sys.argv[1:]]" .github/workflows/ci.yml .github/workflows/release.yml .github/dependabot.yml` (skip this check if PyYAML is missing, and say so).
- [ ] **Step 3: Commit** `ci: pin actions to commit SHAs and let Dependabot bump them`
