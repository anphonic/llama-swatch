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
    let key = key_for_test(state.secrets.as_ref(), current.as_ref(), &url, api_key).map_err(|e| e.to_string())?;
    let client = LlamaSwapClient::new(&url, key).map_err(|e| e.to_string())?;
    Ok(client::test_connection(&client).await)
}

#[tauri::command]
pub fn get_snapshot(state: State<'_, AppState>) -> Option<Snapshot> {
    state.latest.lock().expect("latest poisoned").clone()
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
}
