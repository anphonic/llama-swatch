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
    /// Keep the window above other windows. Applied at startup and by the header pin button.
    pub always_on_top: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { base_url: "http://localhost:8080".into(), poll_interval_ms: 2000, thresholds: Thresholds::default(), always_on_top: false }
    }
}

impl Settings {
    pub fn sanitized(mut self) -> Result<Self, ConfigError> {
        self.base_url = normalize_base_url(&self.base_url)?;
        self.poll_interval_ms = self.poll_interval_ms.clamp(MIN_POLL_MS, MAX_POLL_MS);
        let t = &mut self.thresholds;
        for v in [&mut t.load_timeout_s, &mut t.stop_timeout_s, &mut t.first_byte_timeout_s, &mut t.stream_stall_timeout_s] {
            *v = (*v).max(1);
        }
        Ok(self)
    }
}

/// Sub-pages of llama-swap's web UI that may follow `/ui` in a pasted browser URL.
const UI_PAGES: &[&str] = &["models", "activity", "logs", "playground", "config", "settings", "stats"];

/// Accepts what people paste: missing scheme, trailing slashes, an OpenAI-style `/v1` suffix,
/// llama-swap's `/ui` web path (as the last segment or before one known UI page), stray query
/// strings. Keeps any reverse-proxy path prefix. Returns `scheme://host[:port][/prefix]` with no
/// trailing slash.
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
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ConfigError::InvalidUrl(
            "URLs must not contain embedded credentials; enter the key in the API key field".into(),
        ));
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_string();
    // llama-swap's web UI lives at /ui (people paste it from the browser). Strip it only when it is
    // the final segment, or the segment before a single known UI page (`/ui/models`). Anything else
    // (`/ui/llama`, `/apps/ui/proxy`) is treated as a reverse-proxy prefix and left alone.
    // Only the leading empty segment (from the leading `/`) is dropped; interior empty segments
    // (`/tenant//llama`) are significant to routing and are preserved exactly.
    let mut segs: Vec<&str> = path.strip_prefix('/').unwrap_or("").split('/').collect();
    if segs == [""] {
        segs.clear();
    }
    match segs.as_slice() {
        [.., "ui"] => {
            segs.pop();
        }
        [.., "ui", page] if UI_PAGES.contains(page) => {
            segs.truncate(segs.len() - 2);
        }
        _ => {}
    }
    if segs.last() == Some(&"v1") {
        segs.pop();
    }
    // Stripping a suffix can expose empty segments (`/a//v1`); they are trailing slashes now.
    while segs.last() == Some(&"") {
        segs.pop();
    }
    let path = if segs.is_empty() { String::new() } else { format!("/{}", segs.join("/")) };
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

/// Persists only the pin flag. Does nothing (returns `Ok(false)`) while unconfigured, so toggling
/// the pin can never create a settings file with a made-up server URL.
pub fn save_always_on_top(path: &Path, on: bool) -> Result<bool, ConfigError> {
    match load_settings(path) {
        Some(s) if s.always_on_top == on => Ok(true),
        Some(s) => save_settings(path, &Settings { always_on_top: on, ..s }).map(|()| true),
        None => Ok(false),
    }
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
            ("https://llama.example.test/ui", "https://llama.example.test"),
            ("https://llama.example.test/ui/", "https://llama.example.test"),
            ("https://llama.example.test/ui/models", "https://llama.example.test"),
            ("https://h/llama/ui", "https://h/llama"),
            ("https://h/llama/ui/models/", "https://h/llama"),
            ("https://h/uikit", "https://h/uikit"),
            ("https://h/menu/v1", "https://h/menu"),
            ("https://h/ui/llama", "https://h/ui/llama"),
            ("https://h/ui/llama/", "https://h/ui/llama"),
            ("https://h/ui/llama/ui", "https://h/ui/llama"),
            ("https://h/ui/llama/ui/models", "https://h/ui/llama"),
            ("https://h/apps/ui/llama-swap", "https://h/apps/ui/llama-swap"),
            ("https://h/v1/ui", "https://h"),
            ("https://h/ui/v1", "https://h/ui"),
            ("https://h/ui/unknownpage", "https://h/ui/unknownpage"),
            ("http://[::1]:8080/ui", "http://[::1]:8080"),
            ("http://box:8080/ui/", "http://box:8080"),
            ("https://h/tenant//llama", "https://h/tenant//llama"),
            ("https://h/tenant//llama/ui", "https://h/tenant//llama"),
            ("https://h/a//v1", "https://h/a"),
        ];
        for (input, expected) in ok {
            assert_eq!(normalize_base_url(input).unwrap(), expected, "input: {input:?}");
        }
        for bad in ["", "   ", "ftp://box", "http://", "http://exa mple.com"] {
            assert!(normalize_base_url(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn credentials_in_url_are_rejected_without_echo() {
        for bad in ["http://user:secret@host:8080", "http://user@host", "http://:secret@host"] {
            let err = normalize_base_url(bad).unwrap_err();
            let msg = err.to_string();
            assert!(matches!(err, ConfigError::InvalidUrl(_)), "{bad:?}");
            assert!(!msg.contains("secret") && !msg.contains("user"), "leaked: {msg}");
            assert!(msg.contains("API key"), "{msg}");
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
    fn sanitized_clamps_thresholds_to_at_least_one() {
        let thresholds = Thresholds { load_timeout_s: 0, stop_timeout_s: 0, first_byte_timeout_s: 0, stream_stall_timeout_s: 0 };
        let s = Settings { thresholds, ..Default::default() }.sanitized().unwrap();
        assert_eq!(
            s.thresholds,
            Thresholds { load_timeout_s: 1, stop_timeout_s: 1, first_byte_timeout_s: 1, stream_stall_timeout_s: 1 }
        );
        let s = Settings::default().sanitized().unwrap();
        assert_eq!(s.thresholds, Thresholds::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.json");
        let s = Settings { base_url: "http://box:8080".into(), poll_interval_ms: 3000, thresholds: Thresholds::default(), always_on_top: true };
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
        assert!(!s.always_on_top, "old settings files default to not pinned");
    }

    #[test]
    fn always_on_top_is_read_and_written_camel_case() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"baseUrl":"box:9000","alwaysOnTop":true}"#).unwrap();
        assert!(load_settings(&path).unwrap().always_on_top);
        let s = Settings { always_on_top: true, ..Default::default() };
        save_settings(&path, &s).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("\"alwaysOnTop\": true"));
    }

    #[test]
    fn save_always_on_top_updates_only_that_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(save_always_on_top(&path, true).unwrap(), false, "unconfigured: nothing written");
        assert!(!path.exists());
        std::fs::write(&path, r#"{"baseUrl":"box:9000","pollIntervalMs":3000}"#).unwrap();
        assert!(save_always_on_top(&path, true).unwrap());
        let s = load_settings(&path).unwrap();
        assert!(s.always_on_top);
        assert_eq!((s.base_url.as_str(), s.poll_interval_ms), ("http://box:9000", 3000));
        assert!(save_always_on_top(&path, false).unwrap());
        assert!(!load_settings(&path).unwrap().always_on_top);
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
