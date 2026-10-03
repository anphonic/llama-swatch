//! Tauri commands and the glue between settings, keychain and the monitor.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::api::ActivityRow;
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

/// Key for a connection test. A saved key is only reused when the tested URL
/// is the saved URL, so a typo'd or new host never receives it.
pub fn key_for_test(
    secrets: &dyn SecretStore,
    saved: Option<&Settings>,
    base_url_normalized: &str,
    api_key: Option<String>,
) -> Result<Option<String>, ConfigError> {
    let same_url = saved.is_some_and(|c| c.base_url == base_url_normalized);
    resolve_key(secrets, saved.filter(|_| same_url), api_key)
}

/// Validates, stores the key in the keychain, writes the settings file, then
/// cleans up stale keychain entries. Returns what was saved.
///
/// `api_key`: `Some(k)` stores `k`, `Some("")` removes the key, `None` keeps
/// the saved key only if the URL is unchanged (a saved key is never handed to
/// a different host). Keychain trouble is fatal only when the user supplied a
/// key that cannot be stored; otherwise it is logged and treated as "no key".
pub fn apply_settings(
    path: &Path,
    secrets: &dyn SecretStore,
    settings: Settings,
    api_key: Option<String>,
) -> Result<(Settings, Option<String>), ConfigError> {
    let current = load_settings(path);
    let settings = settings.sanitized()?;
    let old_url = current.as_ref().map(|c| c.base_url.clone()).filter(|u| *u != settings.base_url);

    let key = match api_key.map(|k| k.trim().to_string()) {
        Some(k) if !k.is_empty() => {
            secrets.set(&settings.base_url, &k)?;
            Some(k)
        }
        Some(_) => None,
        None if old_url.is_none() && current.is_some() => match secrets.get(&settings.base_url) {
            Ok(k) => k,
            Err(e) => {
                eprintln!("keychain unavailable, continuing without a saved key: {e}");
                None
            }
        },
        None => None,
    };

    config::save_settings(path, &settings)?;

    // Best effort: a keychain failure here must not fail an already-saved config.
    if key.is_none() {
        if let Err(e) = secrets.delete(&settings.base_url) {
            eprintln!("could not remove keychain entry: {e}");
        }
    }
    if let Some(old) = old_url {
        if let Err(e) = secrets.delete(&old) {
            eprintln!("could not remove old keychain entry: {e}");
        }
    }
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
        publish_if_current(&self.generation, self.mine, &self.latest, snapshot, |s| {
            let _ = self.app.emit("snapshot", s);
        });
    }
}

/// Starts a new monitor generation. Bumping and clearing happen under the
/// `latest` lock, the same lock `publish_if_current` holds, so a publisher
/// either finishes before the restart or sees the new generation.
pub fn begin_generation(generation: &AtomicU64, latest: &Mutex<Option<Snapshot>>) -> u64 {
    let mut guard = latest.lock().expect("latest poisoned");
    let next = generation.fetch_add(1, Ordering::SeqCst) + 1;
    *guard = None;
    next
}

/// Generation check, `latest` write and emit as one critical section, so a stale
/// task can never write or emit after a restart. `emit` must not block or call
/// back into anything that takes the `latest` lock (Tauri's `emit` only queues).
pub fn publish_if_current(
    generation: &AtomicU64,
    mine: u64,
    latest: &Mutex<Option<Snapshot>>,
    snapshot: &Snapshot,
    emit: impl FnOnce(&Snapshot),
) {
    let mut guard = latest.lock().expect("latest poisoned");
    if generation.load(Ordering::SeqCst) != mine {
        return;
    }
    *guard = Some(snapshot.clone());
    emit(snapshot);
}

pub fn restart_monitor(app: &AppHandle, state: &AppState, settings: &Settings, key: Option<String>) -> Result<(), String> {
    let mut guard = state.monitor.lock().expect("monitor poisoned");
    if let Some(old) = guard.take() {
        old.stop();
    }
    let mine = begin_generation(&state.generation, &state.latest);
    let sink = Arc::new(TauriSink { app: app.clone(), latest: state.latest.clone(), generation: state.generation.clone(), mine });
    let handle = runtime::start(
        MonitorConfig {
            base_url: settings.base_url.clone(),
            api_key: key,
            poll_interval: Duration::from_millis(settings.poll_interval_ms),
            version_retry: runtime::MODELS_REFRESH,
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

/// Remembers the header pin button's state. The window itself is changed by the frontend
/// (`setAlwaysOnTop`), so a platform that ignores it never produces an error here.
#[tauri::command]
pub fn set_always_on_top(state: State<'_, AppState>, on: bool) -> Result<(), String> {
    config::save_always_on_top(&state.config_path, on).map(|_| ()).map_err(|e| e.to_string())
}

/// INVARIANT: every command that takes the `latest` lock must be `async`. `publish_if_current`
/// emits under that lock, and with Tauri's `tracing` feature `emit` blocks until the main
/// thread services it; a sync command runs on the main thread and would deadlock waiting for
/// the lock a publisher holds. Async commands run on the async runtime, never the main thread.
#[tauri::command]
pub async fn save_settings(
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
    let key = key_for_test(state.secrets.as_ref(), current.as_ref(), &url, api_key).map_err(|e| e.to_string())?;
    let client = LlamaSwapClient::new(&url, key).map_err(|e| e.to_string())?;
    Ok(client::test_connection(&client).await)
}

/// True only when `base_url` normalizes successfully to the saved URL. Never
/// reads the key.
pub fn url_matches_saved(base_url: &str, saved: Option<&Settings>) -> bool {
    match (normalize_base_url(base_url), saved) {
        (Ok(url), Some(s)) => url == s.base_url,
        _ => false,
    }
}

/// Lets the UI ask whether an edited URL is the saved one (and so would reuse
/// the saved key) without duplicating the normalization rules.
#[tauri::command]
pub fn is_saved_url(state: State<'_, AppState>, base_url: String) -> bool {
    url_matches_saved(&base_url, load_settings(&state.config_path).as_ref())
}

/// Async for the same reason as `save_settings` (see the invariant there).
#[tauri::command]
pub async fn get_snapshot(state: State<'_, AppState>) -> Result<Option<Snapshot>, String> {
    Ok(state.latest.lock().expect("latest poisoned").clone())
}

/// Most rows `get_activity` will return, whatever the webview asks for.
const MAX_ACTIVITY_LIMIT: u32 = 1000;

/// Model ids come from the webview, so only an id in the latest snapshot's model list is used.
pub fn validate_model_id(snapshot: Option<&Snapshot>, id: &str) -> Result<(), String> {
    match snapshot {
        Some(s) if s.models.iter().any(|m| m.id == id) => Ok(()),
        _ => Err("unknown model".into()),
    }
}

/// Validates `id` against the snapshot AND checks that the snapshot came from the server the
/// request will go to. `save_settings` writes the new URL before the monitor restart clears
/// `latest`, so without the host check an id validated against the old server could be sent
/// to the new one.
pub fn validate_for(snapshot: Option<&Snapshot>, id: &str, saved_base_url: &str) -> Result<(), String> {
    validate_model_id(snapshot, id)?;
    match snapshot {
        Some(s) if s.host.trim_end_matches('/') == saved_base_url.trim_end_matches('/') => Ok(()),
        _ => Err("settings changed, try again".into()),
    }
}

/// Client for the given saved settings and their saved key; the key never leaves this function
/// except into the client.
fn client_for(state: &AppState, settings: &Settings) -> Result<LlamaSwapClient, String> {
    let key = state.secrets.get(&settings.base_url).ok().flatten();
    LlamaSwapClient::new(&settings.base_url, key).map_err(|e| e.to_string())
}

/// Client for the saved URL and its saved key.
fn saved_client(state: &AppState) -> Result<LlamaSwapClient, String> {
    let settings = load_settings(&state.config_path).ok_or("not configured")?;
    client_for(state, &settings)
}

/// Client for a model command: the id is checked against the latest snapshot, and that snapshot
/// must come from the saved URL, before the keychain is read or any request is built. The
/// `latest` lock is held only for the check (never across an `.await`).
fn model_client(state: &AppState, id: &str) -> Result<LlamaSwapClient, String> {
    let settings = load_settings(&state.config_path).ok_or("not configured")?;
    validate_for(state.latest.lock().expect("latest poisoned").as_ref(), id, &settings.base_url)?;
    client_for(state, &settings)
}

/// Recent requests, newest first. Async for the same reason as `save_settings`.
#[tauri::command]
pub async fn get_activity(state: State<'_, AppState>, limit: u32) -> Result<Vec<ActivityRow>, String> {
    let client = saved_client(&state)?;
    let rows = client.activity(limit.clamp(1, MAX_ACTIVITY_LIMIT)).await.map_err(|e| e.to_string())?;
    Ok(rows.iter().map(ActivityRow::from).collect())
}

/// Blocks until the model is up (possibly minutes); the webview fires it and watches snapshots.
#[tauri::command]
pub async fn load_model(state: State<'_, AppState>, id: String) -> Result<(), String> {
    model_client(&state, &id)?.load_model(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn unload_model(state: State<'_, AppState>, id: String) -> Result<(), String> {
    model_client(&state, &id)?.unload_model(&id).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MemoryStore;

    fn settings(url: &str) -> Settings {
        Settings { base_url: url.into(), ..Default::default() }
    }

    fn snap(host: &str) -> Snapshot {
        crate::monitor::MonitorState::new(host).snapshot(
            std::time::Instant::now(),
            0,
            &crate::state::Thresholds::default(),
        )
    }

    #[test]
    fn stale_generation_cannot_publish_after_restart() {
        let latest = Mutex::new(None);
        let generation = AtomicU64::new(0);
        let mine = begin_generation(&generation, &latest);
        let mut emitted = 0;
        publish_if_current(&generation, mine, &latest, &snap("a"), |_| emitted += 1);
        assert_eq!((emitted, latest.lock().unwrap().is_some()), (1, true));

        let next = begin_generation(&generation, &latest);
        assert!(latest.lock().unwrap().is_none(), "restart clears the cached snapshot");
        publish_if_current(&generation, mine, &latest, &snap("stale"), |_| emitted += 1);
        assert_eq!(emitted, 1, "old generation must not emit");
        assert!(latest.lock().unwrap().is_none(), "old generation must not write latest");
        publish_if_current(&generation, next, &latest, &snap("b"), |_| emitted += 1);
        assert_eq!(latest.lock().unwrap().as_ref().map(|s| s.host.as_str()), Some("b"));
    }

    #[test]
    fn publish_check_write_and_emit_are_one_critical_section() {
        // While the emit callback runs, a restart on another thread must block on the
        // `latest` lock, so it cannot clear `latest` between the check and the write.
        let latest = Arc::new(Mutex::new(None));
        let generation = Arc::new(AtomicU64::new(0));
        let mine = begin_generation(&generation, &latest);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let restarter = {
            let (latest, generation) = (latest.clone(), generation.clone());
            std::thread::spawn(move || {
                started_rx.recv().unwrap();
                begin_generation(&generation, &latest)
            })
        };
        publish_if_current(&generation, mine, &latest, &snap("a"), |_| {
            started_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(100));
            assert_eq!(generation.load(Ordering::SeqCst), mine, "restart ran inside the critical section");
        });
        assert_eq!(restarter.join().unwrap(), mine + 1);
        assert!(latest.lock().unwrap().is_none(), "restart's clear came after the publish");
    }

    #[test]
    fn commands_that_take_the_latest_lock_are_async() {
        // `emit` runs under the `latest` lock; with Tauri's `tracing` feature it blocks on the
        // main thread. A sync command runs on the main thread, so taking the lock there can
        // deadlock against a publisher. Such commands must be `async`.
        let src = include_str!("commands.rs").replace("\r\n", "\n");
        for name in ["save_settings", "get_snapshot", "load_model", "unload_model"] {
            assert!(
                src.contains(&format!("#[tauri::command]\npub async fn {name}(")),
                "{name} must be an async command"
            );
        }
    }

    #[test]
    fn url_matches_saved_uses_backend_normalization() {
        let saved = settings("http://box:8080");
        for same in ["box:8080/v1", "http://box:8080/", " HTTP://Box:8080/v1/ ", "http://box:8080/?x=1", "box:8080/ui", "http://box:8080/ui/"] {
            assert!(url_matches_saved(same, Some(&saved)), "{same:?}");
        }
        for other in ["http://other:8080", "https://box:8080", "box:9090", "", "ftp://box", "http://u:p@box:8080"] {
            assert!(!url_matches_saved(other, Some(&saved)), "{other:?}");
        }
        assert!(!url_matches_saved("http://box:8080", None), "nothing saved");
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
    fn url_change_with_blank_key_drops_key_and_never_sends_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = MemoryStore::default();
        apply_settings(&path, &store, settings("http://old:8080"), Some("k1".into())).unwrap();
        let (saved, key) = apply_settings(&path, &store, settings("http://new:8080"), None).unwrap();
        assert_eq!(saved.base_url, "http://new:8080");
        assert_eq!(key, None, "old key must not be sent to the new host");
        assert_eq!(store.get("http://new:8080").unwrap(), None);
        assert_eq!(store.get("http://old:8080").unwrap(), None, "old entry is cleaned up");
    }

    /// Keychain whose every call fails like Linux without a Secret Service.
    struct BrokenStore;
    impl SecretStore for BrokenStore {
        fn get(&self, _: &str) -> Result<Option<String>, ConfigError> {
            Err(ConfigError::Keychain("platform failure".into()))
        }
        fn set(&self, _: &str, _: &str) -> Result<(), ConfigError> {
            Err(ConfigError::Keychain("platform failure".into()))
        }
        fn delete(&self, _: &str) -> Result<(), ConfigError> {
            Err(ConfigError::Keychain("platform failure".into()))
        }
    }

    #[test]
    fn broken_keychain_does_not_block_save_without_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let (_, key) = apply_settings(&path, &BrokenStore, settings("http://box:8080"), None).unwrap();
        assert_eq!(key, None);
        // second save: same URL, saved-key lookup fails, still non-fatal
        let (_, key) = apply_settings(&path, &BrokenStore, settings("http://box:8080"), None).unwrap();
        assert_eq!(key, None);
        let (_, key) = apply_settings(&path, &BrokenStore, settings("http://other:8080"), Some(" ".into())).unwrap();
        assert_eq!(key, None);
    }

    #[test]
    fn broken_keychain_fails_save_when_user_supplies_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert!(apply_settings(&path, &BrokenStore, settings("http://box:8080"), Some("k".into())).is_err());
        assert!(!path.exists(), "nothing written when the key could not be stored");
    }

    #[test]
    fn old_key_survives_a_failed_file_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = MemoryStore::default();
        apply_settings(&path, &store, settings("http://old:8080"), Some("k1".into())).unwrap();
        // Make the atomic rename fail by putting a directory where the tmp file goes.
        std::fs::create_dir(path.with_extension("json.tmp")).unwrap();
        assert!(apply_settings(&path, &store, settings("http://new:8080"), Some("k2".into())).is_err());
        assert_eq!(store.get("http://old:8080").unwrap().as_deref(), Some("k1"));
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

    #[test]
    fn key_for_test_only_sends_saved_key_to_saved_url() {
        let store = MemoryStore::default();
        store.set("http://box:8080", "k1").unwrap();
        let saved = settings("http://box:8080");
        assert_eq!(key_for_test(&store, Some(&saved), "http://box:8080", None).unwrap().as_deref(), Some("k1"));
        assert_eq!(key_for_test(&store, Some(&saved), "http://typo:8080", None).unwrap(), None);
        assert_eq!(key_for_test(&store, None, "http://box:8080", None).unwrap(), None);
        assert_eq!(key_for_test(&store, Some(&saved), "http://typo:8080", Some(" new ".into())).unwrap().as_deref(), Some("new"));
        assert_eq!(key_for_test(&store, Some(&saved), "http://box:8080", Some("".into())).unwrap(), None);
    }

    #[test]
    fn model_ids_must_be_in_the_latest_snapshot() {
        let mut s = snap("h");
        s.models.push(model("qwen"));
        assert!(validate_model_id(Some(&s), "qwen").is_ok());
        for bad in ["qwen/../x", "qwe", "", "QWEN", "other"] {
            assert!(validate_model_id(Some(&s), bad).is_err(), "{bad:?}");
        }
        assert!(validate_model_id(None, "qwen").is_err(), "no snapshot yet");
    }

    fn model(id: &str) -> crate::monitor::ModelCard {
        crate::monitor::ModelCard {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            state: crate::state::ModelState::NotLoaded,
            ttl_s: None,
            ttl_remaining_s: None,
            tok_s_history: vec![],
            last_request_at_ms: None,
        }
    }

    #[test]
    fn validate_for_binds_the_id_to_the_snapshot_host() {
        let mut s = snap("http://box:8080");
        s.models.push(model("qwen"));
        assert!(validate_for(Some(&s), "qwen", "http://box:8080").is_ok(), "matching host");
        assert_eq!(
            validate_for(Some(&s), "qwen", "http://other:8080").unwrap_err(),
            "settings changed, try again",
            "id valid but snapshot is from a different host"
        );
        assert!(validate_for(Some(&s), "nope", "http://box:8080").is_err(), "unknown id");
        assert!(validate_for(None, "qwen", "http://box:8080").is_err(), "no snapshot");
    }

    #[test]
    fn rejected_ids_send_no_request() {
        // The commands build their client through model_client, which validates (id and host)
        // before reading the key or building a client, so a rejected id cannot reach HTTP.
        let src = include_str!("commands.rs");
        for name in ["load_model", "unload_model"] {
            let body = src.split(&format!("pub async fn {name}(")).nth(1).unwrap();
            let body = &body[..body.find("
}").unwrap()];
            assert!(body.contains("model_client("), "{name} must use model_client");
            assert!(!body.contains("saved_client("), "{name} must not bypass validation");
        }
        let body = src.split("fn model_client(").nth(1).unwrap();
        let v = body.find("validate_for(").unwrap();
        let c = body.find("client_for(").unwrap();
        assert!(v < c, "model_client must validate before reading the key");
    }
}
