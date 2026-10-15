# Skins and Mini Layout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add built-in and file-based skins (palette + layout) and a frameless Mini strip layout, without changing anything for users who keep the default skin.

**Architecture:** A new Rust module `skins.rs` owns the built-in palettes, parses and validates skin files, and lists them through a `list_skins` command; the webview only ever receives validated hex colours. `theme.ts` applies the chosen palette as inline CSS variables (Standard clears them, so today's CSS is untouched). The Mini layout is a second view (`views/mini.ts`); window changes (frame, size, position) happen in Rust through a `set_layout` command so the webview needs only drag/minimize/close permissions. The snapshot gains `miniOrder`, the loaded models ranked busiest first, computed and tested in Rust.

**Tech Stack:** Tauri 2, Rust (serde, serde_json), TypeScript (no framework; `h()` DOM helper), Vite.

**Spec:** `docs/superpowers/specs/2026-10-15-skins-and-mini-design.md`

## Global Constraints

- Default skin id is `builtin:standard`; with it selected the app must look and behave exactly as before (no inline CSS variables set, no window changes).
- Skin files: `<app config dir>/skins/*.json`, max 16 KB (16384 bytes), colour values only `#rgb` or `#rrggbb`, `layout` only `full` or `mini`, `name` required, trimmed, max 40 characters.
- The 12 palette keys, in this order: `bg`, `panel`, `text`, `muted`, `border`, `track`, `grey`, `green`, `amber`, `amberText`, `blue`, `red`. CSS variable = `--` + kebab-case key (`amberText` → `--amber-text`).
- No `opener` permission in capabilities. No new network access. No raw CSS from skin files ever reaches the webview.
- Never write the real LAN hostname or IPs anywhere; examples use `llama.example.test`.
- Run `npm run build` (PowerShell) and `cargo test` in `src-tauri`. Do not run `cargo fmt`.
- Commits: email `5176615+anphonic@users.noreply.github.com` via `git -c user.email=...`; message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`; write the message to a temp file and `git commit -F`.
- Do not launch the GUI from agents; GUI checks are listed for the user.
- Two PRs: PR 1 = Tasks 1–3 (branch `feat/skins`), PR 2 = Tasks 4–6 (branch `feat/mini-layout`, based on `feat/skins`).

## Rulings (deviations from the spec, decided while planning)

- **Window permissions:** the spec listed six webview permissions. Frame, size and position are changed from Rust (`set_layout` command), which needs no capability, so only `core:window:allow-start-dragging`, `core:window:allow-minimize` and `core:window:allow-close` are added. Fewer permissions; same behaviour.
- **Snapshot field name:** the spec says a `mini` field; it is `miniOrder: string[]` (loaded model ids, busiest first). The strip derives the shown model, the `+N` count and the cycle order from it.
- **Live preview:** previewing in Settings changes colours only. The layout (Full ↔ Mini) switches on Save, because shrinking the window while the Settings form is open would hide the form.
- **Saving a skin:** goes through the existing Save button, which tests the connection first. The Mini menu's "Full dashboard" uses a dedicated `set_skin` command (like `set_always_on_top`) so it works while llama-swap is down.

## Review Focus

1. A settings file from v0.3.0 (no skin fields), or one with a hand-edited non-string `skin` / malformed `fullBounds`, must still load; the whole file must never become unreadable.
2. A selected skin file deleted or broken between launches must fall back to Standard with a note, never a blank or unstyled window.
3. Restarting while in Mini must reopen as the strip (frameless, no 420×400 minimum clamping it); restarting after Mini → Full must reopen full size.
4. `fullBounds` pointing at an unplugged monitor must restore to the default centred size, not off-screen.
5. "System" theme on Windows/macOS with a non-Standard skin must follow the OS light/dark switch live (the backend reports `unknown` there; use `matchMedia`).

---

## PR 1 — Skins

### Task 1: `skins.rs` — palettes, parsing, folder scan

**Files:**
- Create: `src-tauri/src/skins.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod skins;`)
- Test: inline `#[cfg(test)] mod tests` in `skins.rs`

**Interfaces:**
- Produces:
  - `pub const KEYS: [&str; 12]`
  - `pub type Palette = BTreeMap<String, String>`
  - `pub enum Layout { Full, Mini }` (serde lowercase, `Default = Full`)
  - `pub struct Skin { id: String, name: String, layout: Layout, light: Palette, dark: Palette }` (Serialize camelCase)
  - `pub struct Skipped { file: String, reason: String }` (Serialize camelCase)
  - `pub struct SkinList { skins: Vec<Skin>, skipped: Vec<Skipped>, folder: String, folder_error: Option<String> }` (Serialize camelCase)
  - `pub fn builtins() -> Vec<Skin>`
  - `pub fn parse_skin(file_name: &str, bytes: &[u8]) -> Result<Skin, String>`
  - `pub fn list(dir: &Path) -> SkinList`
  - `pub const STANDARD_ID: &str = "builtin:standard"`

- [ ] **Step 1: Write the tests (below) and the public items with `todo!()` bodies; the full implementation follows for Step 3**

```rust
//! Skins: a colour palette (light + dark) and a layout. Built-ins are compiled in; skin files are
//! JSON in `<config dir>/skins`. Everything is validated here, so the webview only ever receives
//! hex colours and a known layout name, never CSS.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value};

/// Palette keys, matching the CSS variables in `src/styles.css` (`amberText` = `--amber-text`).
pub const KEYS: [&str; 12] =
    ["bg", "panel", "text", "muted", "border", "track", "grey", "green", "amber", "amberText", "blue", "red"];
pub const STANDARD_ID: &str = "builtin:standard";
pub const MAX_FILE_BYTES: u64 = 16 * 1024;
pub const MAX_NAME_CHARS: usize = 40;

pub type Palette = BTreeMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    #[default]
    Full,
    Mini,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skin {
    pub id: String,
    pub name: String,
    pub layout: Layout,
    pub light: Palette,
    pub dark: Palette,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub file: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkinList {
    /// Built-ins first (Standard first of all), then valid files sorted by name.
    pub skins: Vec<Skin>,
    pub skipped: Vec<Skipped>,
    /// Shown in Settings so people know where to put files.
    pub folder: String,
    pub folder_error: Option<String>,
}

type Table = [(&'static str, &'static str); 12];

const STANDARD_LIGHT: Table = [
    ("bg", "#f6f7f9"), ("panel", "#ffffff"), ("text", "#1b1f24"), ("muted", "#6b7280"),
    ("border", "#e3e6ea"), ("track", "#e5e7eb"), ("grey", "#9ca3af"), ("green", "#16a34a"),
    ("amber", "#d97706"), ("amberText", "#b45309"), ("blue", "#2563eb"), ("red", "#dc2626"),
];
const STANDARD_DARK: Table = [
    ("bg", "#0f1115"), ("panel", "#171a21"), ("text", "#e6e8eb"), ("muted", "#9aa3ad"),
    ("border", "#262b35"), ("track", "#262b35"), ("grey", "#6b7280"), ("green", "#22c55e"),
    ("amber", "#f59e0b"), ("amberText", "#f59e0b"), ("blue", "#60a5fa"), ("red", "#f87171"),
];
const CONTRAST_LIGHT: Table = [
    ("bg", "#ffffff"), ("panel", "#ffffff"), ("text", "#000000"), ("muted", "#3f3f46"),
    ("border", "#52525b"), ("track", "#a1a1aa"), ("grey", "#52525b"), ("green", "#047857"),
    ("amber", "#b45309"), ("amberText", "#92400e"), ("blue", "#1d4ed8"), ("red", "#b91c1c"),
];
const CONTRAST_DARK: Table = [
    ("bg", "#000000"), ("panel", "#0a0a0a"), ("text", "#ffffff"), ("muted", "#d4d4d8"),
    ("border", "#a1a1aa"), ("track", "#52525b"), ("grey", "#a1a1aa"), ("green", "#4ade80"),
    ("amber", "#fbbf24"), ("amberText", "#fbbf24"), ("blue", "#93c5fd"), ("red", "#fca5a5"),
];
const MUTED_LIGHT: Table = [
    ("bg", "#eceff4"), ("panel", "#f8f9fb"), ("text", "#2e3440"), ("muted", "#4c566a"),
    ("border", "#d8dee9"), ("track", "#d8dee9"), ("grey", "#9aa5b8"), ("green", "#5b8a4a"),
    ("amber", "#c08a2e"), ("amberText", "#8f6420"), ("blue", "#5e81ac"), ("red", "#bf616a"),
];
const MUTED_DARK: Table = [
    ("bg", "#2e3440"), ("panel", "#3b4252"), ("text", "#eceff4"), ("muted", "#a3acbf"),
    ("border", "#434c5e"), ("track", "#434c5e"), ("grey", "#6b7487"), ("green", "#a3be8c"),
    ("amber", "#ebcb8b"), ("amberText", "#ebcb8b"), ("blue", "#88c0d0"), ("red", "#bf616a"),
];

fn palette(t: &Table) -> Palette {
    t.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn builtin(id: &str, name: &str, layout: Layout, light: &Table, dark: &Table) -> Skin {
    Skin { id: format!("builtin:{id}"), name: name.into(), layout, light: palette(light), dark: palette(dark) }
}

pub fn builtins() -> Vec<Skin> {
    vec![
        builtin("standard", "Standard", Layout::Full, &STANDARD_LIGHT, &STANDARD_DARK),
        builtin("high-contrast", "High contrast", Layout::Full, &CONTRAST_LIGHT, &CONTRAST_DARK),
        builtin("muted", "Muted", Layout::Full, &MUTED_LIGHT, &MUTED_DARK),
        builtin("minimal", "Minimal", Layout::Mini, &STANDARD_LIGHT, &STANDARD_DARK),
    ]
}

/// `#rgb` or `#rrggbb`. Rejects everything else, including `url(...)`, `var(...)` and names.
fn is_hex(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('#') else { return false };
    (rest.len() == 3 || rest.len() == 6) && rest.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Colours in `v` over `base`. Missing keys keep `base`; unknown keys are ignored.
fn overlay(block: &str, v: &Value, base: &Palette) -> Result<Palette, String> {
    let obj = v.as_object().ok_or_else(|| format!("\"{block}\" must be an object"))?;
    let mut out = base.clone();
    for key in KEYS {
        if let Some(c) = obj.get(key) {
            match c.as_str() {
                Some(s) if is_hex(s.trim()) => {
                    out.insert(key.to_string(), s.trim().to_ascii_lowercase());
                }
                _ => return Err(format!("colour `{key}` isn't a hex value")),
            }
        }
    }
    Ok(out)
}

fn present<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    obj.get(key).filter(|v| !v.is_null())
}

pub fn parse_skin(file_name: &str, bytes: &[u8]) -> Result<Skin, String> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("larger than 16 KB".into());
    }
    let v: Value = serde_json::from_slice(bytes).map_err(|_| "not valid JSON".to_string())?;
    let obj = v.as_object().ok_or("not a JSON object")?;
    let name: String = obj
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("missing \"name\"")?
        .chars()
        .take(MAX_NAME_CHARS)
        .collect();
    let layout = match present(obj, "layout") {
        None => Layout::Full,
        Some(Value::String(s)) if s == "full" => Layout::Full,
        Some(Value::String(s)) if s == "mini" => Layout::Mini,
        Some(_) => return Err("\"layout\" must be \"full\" or \"mini\"".into()),
    };
    let light = match present(obj, "light") {
        Some(l) => overlay("light", l, &palette(&STANDARD_LIGHT))?,
        None => palette(&STANDARD_LIGHT),
    };
    let dark = match present(obj, "dark") {
        Some(d) => overlay("dark", d, &palette(&STANDARD_DARK))?,
        // No dark variant: the skin looks the same in both.
        None => light.clone(),
    };
    Ok(Skin { id: format!("file:{file_name}"), name, layout, light, dark })
}

/// Built-ins plus every valid `*.json` in `dir`. A missing folder is not an error.
pub fn list(dir: &Path) -> SkinList {
    let mut skins = builtins();
    let mut skipped = Vec::new();
    let mut folder_error = None;
    match fs::read_dir(dir) {
        Ok(entries) => {
            let mut files: Vec<(String, std::path::PathBuf)> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
                .filter_map(|p| Some((p.file_name()?.to_str()?.to_string(), p)))
                .collect();
            files.sort();
            let mut found = Vec::new();
            for (file, path) in files {
                let parsed = match fs::metadata(&path) {
                    Ok(m) if m.len() > MAX_FILE_BYTES => Err("larger than 16 KB".to_string()),
                    Ok(_) => fs::read(&path).map_err(|_| "couldn't be read".to_string()).and_then(|b| parse_skin(&file, &b)),
                    Err(_) => Err("couldn't be read".to_string()),
                };
                match parsed {
                    Ok(s) => found.push(s),
                    Err(reason) => skipped.push(Skipped { file, reason }),
                }
            }
            found.sort_by_key(|s| s.name.to_lowercase());
            skins.extend(found);
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(_) => folder_error = Some("Can't read the skins folder".into()),
    }
    SkinList { skins, skipped, folder: dir.display().to_string(), folder_error }
}
```

Tests (append to the file):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<Skin, String> {
        parse_skin("t.json", json.as_bytes())
    }

    #[test]
    fn builtins_have_all_keys_and_standard_first() {
        let b = builtins();
        assert_eq!(b[0].id, STANDARD_ID);
        for s in &b {
            for k in KEYS {
                assert!(is_hex(&s.light[k]) && is_hex(&s.dark[k]), "{} {k}", s.id);
            }
        }
        assert_eq!(b.iter().find(|s| s.id == "builtin:minimal").unwrap().layout, Layout::Mini);
    }

    #[test]
    fn valid_file_parses() {
        let s = parse(r##"{"name":"Owl","layout":"mini","light":{"bg":"#FFF"},"dark":{"bg":"#011627"}}"##).unwrap();
        assert_eq!(s.id, "file:t.json");
        assert_eq!(s.layout, Layout::Mini);
        assert_eq!(s.light["bg"], "#fff");
        assert_eq!(s.dark["bg"], "#011627");
    }

    #[test]
    fn partial_colours_fill_from_standard() {
        let s = parse(r##"{"name":"P","light":{"bg":"#000000"},"dark":{"red":"#ff0000"}}"##).unwrap();
        assert_eq!(s.light["text"], "#1b1f24");
        assert_eq!(s.dark["bg"], "#0f1115");
        assert_eq!(s.dark["red"], "#ff0000");
    }

    #[test]
    fn missing_dark_copies_light() {
        let s = parse(r##"{"name":"L","light":{"bg":"#123456"}}"##).unwrap();
        assert_eq!(s.dark, s.light);
    }

    #[test]
    fn bad_colours_are_rejected_with_the_key() {
        for bad in [r#""red""#, r#""#12""#, r#""url(http://x)""#, r#""var(--bg)""#, "12", r#""#1234567""#] {
            let err = parse(&format!(r#"{{"name":"B","light":{{"bg":{bad}}}}}"#)).unwrap_err();
            assert_eq!(err, "colour `bg` isn't a hex value", "{bad}");
        }
    }

    #[test]
    fn layout_name_and_shape_rules() {
        assert_eq!(parse(r#"{"name":"X","layout":"tiny"}"#).unwrap_err(), "\"layout\" must be \"full\" or \"mini\"");
        assert_eq!(parse(r#"{"layout":"mini"}"#).unwrap_err(), "missing \"name\"");
        assert_eq!(parse(r#"{"name":"   "}"#).unwrap_err(), "missing \"name\"");
        assert_eq!(parse("[1]").unwrap_err(), "not a JSON object");
        assert_eq!(parse("{nope").unwrap_err(), "not valid JSON");
        assert_eq!(parse(r#"{"name":"X","light":"#fff"}"#).unwrap_err(), "\"light\" must be an object");
        assert_eq!(parse(r#"{"name":"X","layout":null}"#).unwrap().layout, Layout::Full);
    }

    #[test]
    fn name_is_trimmed_and_limited_and_unknown_keys_ignored() {
        let long = "n".repeat(60);
        let s = parse(&format!(r#"{{"name":"  {long}  ","future":1,"light":{{"glow":"#fff"}}}}"#)).unwrap();
        assert_eq!(s.name.chars().count(), MAX_NAME_CHARS);
        assert!(!s.light.contains_key("glow"));
    }

    #[test]
    fn oversize_is_rejected() {
        let big = format!(r#"{{"name":"B","pad":"{}"}}"#, "x".repeat(MAX_FILE_BYTES as usize));
        assert_eq!(parse(&big).unwrap_err(), "larger than 16 KB");
    }

    #[test]
    fn missing_folder_lists_builtins_only() {
        let l = list(Path::new("this/does/not/exist/skins"));
        assert_eq!(l.skins.len(), builtins().len());
        assert!(l.skipped.is_empty() && l.folder_error.is_none());
    }

    #[test]
    fn folder_mixes_good_and_bad_files() {
        let dir = std::env::temp_dir().join(format!("swatch-skins-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("b.json"), r#"{"name":"Beta"}"#).unwrap();
        fs::write(dir.join("a.JSON"), r#"{"name":"alpha","layout":"mini"}"#).unwrap();
        fs::write(dir.join("bad.json"), r#"{"name":"Bad","dark":{"bg":"blue"}}"#).unwrap();
        fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let l = list(&dir);
        let names: Vec<&str> = l.skins.iter().skip(builtins().len()).map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["alpha", "Beta"]);
        assert_eq!(l.skipped, [Skipped { file: "bad.json".into(), reason: "colour `bg` isn't a hex value".into() }]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn serializes_camel_case_for_the_frontend() {
        let v = serde_json::to_value(list(Path::new("nope"))).unwrap();
        assert_eq!(v["skins"][0]["layout"], "full");
        assert!(v["skins"][0]["light"]["amberText"].is_string());
        assert!(v.get("folderError").is_some());
    }
}
```

- [ ] **Step 2: Run tests, expect failures from `todo!()` stubs** — `cargo test --lib skins` in `src-tauri`.
- [ ] **Step 3: Fill in the implementation above.**
- [ ] **Step 4: Run `cargo test --lib skins`; expect all pass.** Then full `cargo test`.
- [ ] **Step 5: Commit** — `feat: skin palettes, skin file parsing and folder scan`.

### Task 2: Settings fields and skin commands

**Files:**
- Modify: `src-tauri/src/config.rs` (fields, lenient deserializers, `save_skin`, tests)
- Modify: `src-tauri/src/commands.rs` (`list_skins`, `set_skin`; preserve `full_bounds` in `apply_settings`)
- Modify: `src-tauri/src/lib.rs` (register commands)

**Interfaces:**
- Consumes: `skins::list`, `skins::STANDARD_ID`
- Produces:
  - `Settings` fields `skin: String`, `last_full_skin: String`, `mini_cycle: bool`, `full_bounds: Option<Bounds>`
  - `pub struct Bounds { x: i32, y: i32, width: u32, height: u32 }` (Serialize/Deserialize, camelCase, physical px)
  - `pub fn save_skin(path: &Path, skin: &str, full: bool) -> Result<bool, ConfigError>`
  - `pub fn save_full_bounds(path: &Path, b: Option<Bounds>) -> Result<bool, ConfigError>`
  - commands `list_skins() -> SkinList` (async) and `set_skin(skin: String, full: bool) -> Result<(), String>`

- [ ] **Step 1: Tests in `config.rs` tests module**

```rust
#[test]
fn skin_fields_default_for_old_files() {
    let p = tmp("old-file");
    std::fs::write(&p, r#"{"baseUrl":"http://llama.example.test:8080","theme":"dark"}"#).unwrap();
    let s = load_settings(&p).unwrap();
    assert_eq!(s.skin, "builtin:standard");
    assert_eq!(s.last_full_skin, "builtin:standard");
    assert!(!s.mini_cycle);
    assert_eq!(s.full_bounds, None);
}

#[test]
fn bad_skin_values_do_not_break_the_file() {
    let p = tmp("bad-skin");
    std::fs::write(
        &p,
        r#"{"baseUrl":"http://llama.example.test:8080","skin":5,"lastFullSkin":null,"miniCycle":"yes","fullBounds":{"x":"a"}}"#,
    )
    .unwrap();
    let s = load_settings(&p).expect("file must stay readable");
    assert_eq!(s.skin, "builtin:standard");
    assert_eq!(s.last_full_skin, "builtin:standard");
    assert!(!s.mini_cycle);
    assert_eq!(s.full_bounds, None);
}

#[test]
fn save_skin_updates_last_full_only_for_full_skins() {
    let p = tmp("save-skin");
    save_settings(&p, &Settings::default()).unwrap();
    assert!(save_skin(&p, "builtin:minimal", false).unwrap());
    let s = load_settings(&p).unwrap();
    assert_eq!((s.skin.as_str(), s.last_full_skin.as_str()), ("builtin:minimal", "builtin:standard"));
    save_skin(&p, "file:x.json", true).unwrap();
    let s = load_settings(&p).unwrap();
    assert_eq!((s.skin.as_str(), s.last_full_skin.as_str()), ("file:x.json", "file:x.json"));
}

#[test]
fn save_skin_and_bounds_do_nothing_while_unconfigured() {
    let p = tmp("unconfigured-skin");
    let _ = std::fs::remove_file(&p);
    assert!(!save_skin(&p, "builtin:minimal", false).unwrap());
    assert!(!save_full_bounds(&p, Some(Bounds { x: 1, y: 2, width: 3, height: 4 })).unwrap());
    assert!(!p.exists());
}
```

Use the module's existing temp-path helper if there is one (check the existing `save_then_load_round_trips` test); otherwise add `fn tmp(name: &str) -> PathBuf { std::env::temp_dir().join(format!("swatch-cfg-{name}-{}.json", std::process::id())) }`.

- [ ] **Step 2: Run, expect compile failures.**
- [ ] **Step 3: Implement in `config.rs`**

```rust
/// Saved window rect for leaving Mini, physical pixels (outer position, inner size).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Wrong-typed value (hand edit, newer version) reads as the default instead of making the
/// whole settings file unreadable. Same idea as `lenient_theme`.
fn lenient<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    Ok(T::deserialize(serde_json::Value::deserialize(d)?).unwrap_or_default())
}

fn lenient_skin<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) if !s.trim().is_empty() => s,
        _ => crate::skins::STANDARD_ID.to_string(),
    })
}
```

Add to `Settings` (after `theme`), with doc comments:

```rust
    /// Selected skin id (`builtin:<name>` or `file:<file name>`).
    #[serde(deserialize_with = "lenient_skin")]
    pub skin: String,
    /// Last full-layout skin used; the Mini menu's "Full dashboard" goes back to it.
    #[serde(deserialize_with = "lenient_skin")]
    pub last_full_skin: String,
    /// Mini rotates through loaded models instead of showing only the busiest.
    #[serde(deserialize_with = "lenient")]
    pub mini_cycle: bool,
    /// Window rect to restore when leaving Mini. Written only by `set_layout`.
    #[serde(deserialize_with = "lenient")]
    pub full_bounds: Option<Bounds>,
```

Defaults in `impl Default for Settings`: `skin: STANDARD_ID.into(), last_full_skin: STANDARD_ID.into(), mini_cycle: false, full_bounds: None`.

Note: `lenient_skin` handles `null` too, since `deserialize_with` is called for present keys; absent keys use the struct default.

```rust
/// Persists only the skin choice, like `save_always_on_top`. `full`: the skin uses the full
/// layout, so it also becomes the one "Full dashboard" returns to.
pub fn save_skin(path: &Path, skin: &str, full: bool) -> Result<bool, ConfigError> {
    match load_settings(path) {
        Some(s) => {
            let last_full_skin = if full { skin.to_string() } else { s.last_full_skin.clone() };
            save_settings(path, &Settings { skin: skin.to_string(), last_full_skin, ..s }).map(|()| true)
        }
        None => Ok(false),
    }
}

/// Persists only the full-layout window rect.
pub fn save_full_bounds(path: &Path, full_bounds: Option<Bounds>) -> Result<bool, ConfigError> {
    match load_settings(path) {
        Some(s) if s.full_bounds == full_bounds => Ok(true),
        Some(s) => save_settings(path, &Settings { full_bounds, ..s }).map(|()| true),
        None => Ok(false),
    }
}
```

- [ ] **Step 4: Commands in `commands.rs`**

```rust
/// Built-in skins plus valid files from `<config dir>/skins`. File I/O, so async (off the main thread).
#[tauri::command]
pub async fn list_skins(state: State<'_, AppState>) -> Result<crate::skins::SkinList, String> {
    let dir = state.config_path.parent().map(|d| d.join("skins")).ok_or("no config folder")?;
    Ok(crate::skins::list(&dir))
}

/// Saves only the skin choice (the Mini menu's "Full dashboard" works while llama-swap is down).
#[tauri::command]
pub fn set_skin(state: State<'_, AppState>, skin: String, full: bool) -> Result<(), String> {
    match config::save_skin(&state.config_path, &skin, full) {
        Ok(true) => Ok(()),
        Ok(false) => Err("the settings file could not be read".into()),
        Err(e) => Err(e.to_string()),
    }
}
```

In `apply_settings`, right after `let settings = settings.sanitized()?;`, keep the saved bounds (the form never edits them and may hold a stale copy):

```rust
    let settings = Settings { full_bounds: current.as_ref().and_then(|c| c.full_bounds), ..settings };
```

Add a test next to the existing `apply_settings` tests: save settings with `full_bounds: Some(..)`, call `apply_settings` with `full_bounds: None`, and assert the saved file still has the bounds.

Register `commands::list_skins, commands::set_skin` in `lib.rs`.

- [ ] **Step 5: `cargo test`; expect all pass.**
- [ ] **Step 6: Commit** — `feat: skin settings fields and list_skins / set_skin commands`.

### Task 3: Apply skins in the webview and the Settings picker

**Files:**
- Modify: `src/types.ts`, `src/api.ts`, `src/theme.ts`, `src/main.ts`, `src/views/setup.ts`, `src/styles.css`

**Interfaces:**
- Consumes: `list_skins`, `set_skin`, new `Settings` fields
- Produces (used by Task 6):
  - `types.ts`: `Layout`, `Skin`, `SkinList`, `Bounds`; `Settings` gains `skin`, `lastFullSkin`, `miniCycle`, `fullBounds`
  - `api.ts`: `listSkins()`, `setSkin(skin, full)`
  - `theme.ts`: `applySkin(skin: Skin | null)`, `resolveSkin(list: SkinList, id: string): { skin: Skin; note: string | null }`

- [ ] **Step 1: Types and API**

```ts
// types.ts
export type Layout = "full" | "mini";
/** Palette keys mirror skins.rs `KEYS`; values are validated `#rgb`/`#rrggbb`. */
export type Palette = Record<string, string>;
export interface Skin { id: string; name: string; layout: Layout; light: Palette; dark: Palette }
export interface SkinList {
  skins: Skin[];
  skipped: { file: string; reason: string }[];
  folder: string;
  folderError: string | null;
}
export interface Bounds { x: number; y: number; width: number; height: number }
// Settings gains:
  skin: string;
  lastFullSkin: string;
  miniCycle: boolean;
  /** Written by the backend when entering Mini; the form passes it through unchanged. */
  fullBounds: Bounds | null;
```

```ts
// api.ts
export const listSkins = () => invoke<SkinList>("list_skins");
/** Saves only the skin choice; `full` = it uses the full layout. */
export const setSkin = (skin: string, full: boolean) => invoke<void>("set_skin", { skin, full });
```

- [ ] **Step 2: `theme.ts` — palette on top of the existing light/dark logic**

Add module state `let skin: Skin | null = null;` and a `matchMedia("(prefers-color-scheme: dark)")` query. In `render()`, after setting `data-theme`, apply the palette:

```ts
const STANDARD_ID = "builtin:standard";
const media = window.matchMedia("(prefers-color-scheme: dark)");
media.addEventListener("change", () => render());

/** Same order as skins.rs `KEYS`. */
const KEYS = ["bg", "panel", "text", "muted", "border", "track", "grey", "green", "amber", "amberText", "blue", "red"];
const cssVar = (key: string) => "--" + key.replace(/[A-Z]/g, (c) => "-" + c.toLowerCase());

function paint(effective: "light" | "dark" | "unknown") {
  const style = document.documentElement.style;
  // Standard is what styles.css already says; leave it to the stylesheet.
  if (!skin || skin.id === STANDARD_ID) {
    for (const k of KEYS) style.removeProperty(cssVar(k));
    return;
  }
  const dark = effective === "unknown" ? media.matches : effective === "dark";
  const p = dark ? skin.dark : skin.light;
  for (const k of KEYS) if (p[k]) style.setProperty(cssVar(k), p[k]);
}
```

Call `paint(effective)` at the end of `render()`, where `effective` is the forced theme or the desktop scheme `render()` already computes.

Also set the layout attribute in `render()`: `root.dataset.layout = skin?.layout ?? "full";`.

```ts
/** Applies a skin's colours now (Settings preview and after Save). null = Standard. */
export function applySkin(next: Skin | null) {
  skin = next;
  render();
}

/** The skin for `id`, falling back to Standard with a note when it's gone or broken. */
export function resolveSkin(list: SkinList, id: string): { skin: Skin; note: string | null } {
  const standard = list.skins.find((s) => s.id === STANDARD_ID)!;
  const found = list.skins.find((s) => s.id === id);
  if (found) return { skin: found, note: null };
  const file = id.startsWith("file:") ? id.slice(5) : id;
  return { skin: standard, note: id === STANDARD_ID ? null : `Skin "${file}" not found, using Standard` };
}
```

- [ ] **Step 3: `main.ts` — load skins at boot**

Keep a module `let skins: SkinList | null = null;`. In `boot()` after `initTheme`: `skins = await listSkins().catch(() => null);` and if non-null, `applySkin(resolveSkin(skins, view.settings.skin).skin)`. In `showSetup`'s `onSaved`, re-resolve and apply the saved skin. Pass `skins` into `renderSetup` (new parameter `skins: SkinList | null`).

- [ ] **Step 4: `setup.ts` — Skin section**

Below the Theme field add:
- `<select id="skin">` with one `<option>` per skin, built-ins first; insert a disabled `"──────────"` option between the last `builtin:` and first `file:` skin; label `${name} (mini)` when `layout === "mini"`. Initial value = `s.skin` if present in the list, else `builtin:standard`.
- On `change`: `applySkin(selected)` (colours only, see Rulings).
- Cancel: `applySkin(<resolved saved skin>)` before calling `onCancel`, so a preview never sticks.
- A read-only `<input id="skins-folder">` with `skins.folder`, a **Copy** button (`navigator.clipboard.writeText`; on failure select the input text), and a **Rescan** button that calls `listSkins()`, rebuilds the options, keeps the current selection when still present, and re-renders the skipped list.
- Skipped list: `<ul class="skins-skipped">` with `` `${file}`: ${reason} `` per entry (text nodes via `h()`, never innerHTML), plus `folderError` and the resolve note when set.
- Checkbox `<input id="mini-cycle">` "Cycle through loaded models in Mini", bound to `miniCycle`.
- `collect()` adds: `skin: skinSelect.value`, `lastFullSkin: selectedSkin.layout === "full" ? skinSelect.value : s.lastFullSkin`, `miniCycle: miniCycle.checked`, `fullBounds: s.fullBounds`.
- Add the new controls to the `controls` array so `setBusy` disables them.

If `skins` is null (command failed), show only a disabled select with "Standard" and the note "Skins couldn't be listed".

- [ ] **Step 5: CSS** — `.skins-skipped { margin: 4px 0 0; padding-left: 18px; color: var(--amber-text); font-size: 12px; }` and a `.folder-row { display: flex; gap: 6px; } .folder-row input { flex: 1; min-width: 0; }`.

- [ ] **Step 6: `npm run build` (PowerShell); expect no type errors.** `cargo test` still green.
- [ ] **Step 7: Commit** — `feat: skin picker in Settings with live preview`.

Manual checks for the PR body (user runs): pick High contrast → colours change live; Cancel reverts; Save keeps it across restart; put a broken `bad.json` in the skins folder → Rescan lists it as skipped; delete the selected file → restart shows Standard and the note; flip OS dark mode with Muted selected → palette follows.

---

## PR 2 — Mini layout

### Task 4: `miniOrder` on the snapshot

**Files:**
- Modify: `src-tauri/src/monitor.rs` (field, pure ranking fn, tests, JSON shape test)
- Modify: `src/types.ts` (`Snapshot.miniOrder: string[]`)

**Interfaces:**
- Produces: `Snapshot.mini_order: Vec<String>` (JSON `miniOrder`); `pub fn mini_order(models: &[ModelCard]) -> Vec<String>`

- [ ] **Step 1: Tests (monitor.rs tests module)**

Build `ModelCard`s directly with a small helper `fn card(id: &str, state: ModelState, last: Option<i64>) -> ModelCard` filling the other fields with defaults (`name`/`description` empty, `ttl_s: None`, `ttl_remaining_s: None`, `tok_s_history: vec![]`, `last_request_at_ms: last`).

```rust
#[test]
fn mini_order_ranks_busiest_first() {
    let busy = ModelState::Busy { requests: 1, oldest_elapsed_s: 1, streaming: 1, waiting_first_token: 0, awaiting_reply: 0, last_output_s: Some(0), first_token_long: false, awaiting_long: false };
    let models = vec![
        card("idle", ModelState::Idle { uptime_s: 5 }, Some(100)),
        card("off", ModelState::NotLoaded, Some(999)),
        card("load", ModelState::Loading { elapsed_s: 1, slow: false }, None),
        card("busy", busy, Some(1)),
        card("stuck", ModelState::Stalled { reason: "x".into() }, None),
        card("bye", ModelState::Unloading, None),
        card("loaded", ModelState::Loaded, Some(200)),
    ];
    assert_eq!(mini_order(&models), ["stuck", "busy", "load", "bye", "loaded", "idle"]);
}

#[test]
fn mini_order_ties_go_to_most_recent_then_id() {
    let models = vec![
        card("b", ModelState::Idle { uptime_s: 1 }, None),
        card("a", ModelState::Idle { uptime_s: 1 }, None),
        card("c", ModelState::Idle { uptime_s: 1 }, Some(5)),
    ];
    assert_eq!(mini_order(&models), ["c", "a", "b"]);
}

#[test]
fn mini_order_is_empty_when_nothing_is_loaded() {
    assert!(mini_order(&[card("x", ModelState::NotLoaded, Some(1))]).is_empty());
}
```

Add `"miniOrder"` to the key list in `snapshot_json_shape_matches_frontend_types`.

- [ ] **Step 2: Run, expect failures.**
- [ ] **Step 3: Implement**

```rust
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
```

In `snapshot()`, bind `let models: Vec<ModelCard> = ...collect();` then set `mini_order: mini_order(&models)` before moving `models` into the struct. Add the field to `Snapshot` with doc comment `/// Loaded model ids for the Mini strip, busiest first (see \`mini_order\`).`

- [ ] **Step 4: `cargo test`; `npm run build` after adding `miniOrder: string[]` to `Snapshot` in `types.ts`.**
- [ ] **Step 5: Commit** — `feat: rank loaded models for the Mini strip`.

### Task 5: Window handling in Rust

**Files:**
- Modify: `src-tauri/src/window.rs` (`set_mini`, `visible_on`, startup handling, tests)
- Modify: `src-tauri/src/commands.rs` (`set_layout` command)
- Modify: `src-tauri/src/lib.rs` (pass `mini` into `prepare`, register `set_layout`)
- Modify: `src-tauri/capabilities/default.json`

**Interfaces:**
- Consumes: `config::Bounds`, `config::save_full_bounds`, `skins::list`, `skins::Layout`
- Produces: command `set_layout(mini: bool) -> Result<(), String>`; JS `setLayout(mini)` in Task 6.

- [ ] **Step 1: Test the pure placement check**

```rust
#[test]
fn bounds_must_touch_a_monitor() {
    let monitors = [(0, 0, 1920, 1080), (1920, 0, 2560, 1440)];
    let b = |x, y| Bounds { x, y, width: 500, height: 700 };
    assert!(visible_on(&b(100, 100), &monitors));
    assert!(visible_on(&b(2000, 50), &monitors), "second monitor");
    assert!(!visible_on(&b(5000, 100), &monitors), "unplugged monitor to the right");
    assert!(!visible_on(&b(100, -900), &monitors), "title strip above every screen");
    assert!(!visible_on(&b(100, 100), &[]));
}
```

- [ ] **Step 2: Implement in `window.rs`** — update the module doc: the app now also resizes when the skin's layout changes.

```rust
use tauri::{LogicalSize, PhysicalPosition, Size};
use crate::config::Bounds;

pub const MINI_WIDTH: f64 = 300.0;
pub const MINI_HEIGHT: f64 = 36.0;
const FULL_MIN: LogicalSize<f64> = LogicalSize { width: 420.0, height: 400.0 };
const FULL_DEFAULT: LogicalSize<f64> = LogicalSize { width: 481.0, height: 770.0 };

/// Whether the top 40 px of `b` (where the title bar is) overlap a monitor (x, y, w, h).
pub fn visible_on(b: &Bounds, monitors: &[(i32, i32, u32, u32)]) -> bool {
    let (l, t, r, btm) = (b.x, b.y, b.x + b.width as i32, b.y + 40);
    monitors.iter().any(|&(x, y, w, h)| l < x + w as i32 && r > x && t < y + h as i32 && btm > y)
}

/// Frameless strip, anchored at the window's current top-left corner. Returns the bounds to
/// save for leaving Mini, or `None` when the window was already Mini.
pub fn enter_mini(w: &tauri::WebviewWindow) -> tauri::Result<Option<Bounds>> {
    if !w.is_decorated()? {
        return Ok(None);
    }
    let pos = w.outer_position()?;
    let size = w.inner_size()?;
    let saved = Bounds { x: pos.x, y: pos.y, width: size.width, height: size.height };
    w.set_min_size(None::<Size>)?;
    w.set_decorations(false)?;
    w.set_resizable(false)?;
    w.set_size(LogicalSize::new(MINI_WIDTH, MINI_HEIGHT))?;
    w.set_position(PhysicalPosition::new(pos.x, pos.y))?;
    Ok(Some(saved))
}

/// Back to the framed window at `saved`, or the default centred size when `saved` is missing
/// or no longer on any monitor.
pub fn leave_mini(w: &tauri::WebviewWindow, saved: Option<Bounds>) -> tauri::Result<()> {
    if w.is_decorated()? {
        return Ok(());
    }
    w.set_decorations(true)?;
    w.set_resizable(true)?;
    w.set_min_size(Some(FULL_MIN))?;
    let monitors: Vec<(i32, i32, u32, u32)> = w
        .available_monitors()?
        .iter()
        .map(|m| (m.position().x, m.position().y, m.size().width, m.size().height))
        .collect();
    match saved.filter(|b| visible_on(b, &monitors)) {
        Some(b) => {
            w.set_size(tauri::PhysicalSize::new(b.width, b.height))?;
            w.set_position(PhysicalPosition::new(b.x, b.y))?;
        }
        None => {
            w.set_size(FULL_DEFAULT)?;
            w.center()?;
        }
    }
    Ok(())
}
```

Change `prepare` to `prepare(app, always_on_top, first_launch, mini: bool, full_bounds: Option<Bounds>)`:
- If `mini`: before `restore_state`, call `set_min_size(None)`, `set_decorations(false)`, `set_resizable(false)` (so the 420×400 minimum from `tauri.conf.json` can't clamp the restored strip); after the restore, `set_size(LogicalSize::new(MINI_WIDTH, MINI_HEIGHT))` (the saved state may be full size after a crash). Skip `fit_first_launch` in Mini.
- If not `mini` and, after the restore, the inner height is below `MIN_INNER_HEIGHT_LOGICAL` × scale (the saved state is a strip from a Mini session whose skin was changed by hand): restore `full_bounds` the same way `leave_mini` does (factor the size/position part into `fn place_full(w, saved)` and call it from both).
- Every step stays best effort (`let _ =`), and the window is always shown.

- [ ] **Step 3: `lib.rs`** — compute `mini` at startup:

```rust
let mini = settings.as_ref().is_some_and(|s| {
    let list = skins::list(&dir.join("skins"));
    list.skins.iter().find(|k| k.id == s.skin).is_some_and(|k| k.layout == skins::Layout::Mini)
});
```

Pass `mini` and `settings.as_ref().and_then(|s| s.full_bounds)` to `window::prepare`. Register `commands::set_layout`.

- [ ] **Step 4: Command in `commands.rs`**

```rust
/// Switches the main window between the full dashboard and the Mini strip. Done here rather
/// than from the webview, so the webview needs no resize/decoration permissions.
#[tauri::command]
pub fn set_layout(app: AppHandle, state: State<'_, AppState>, mini: bool) -> Result<(), String> {
    let w = app.get_webview_window(crate::window::MAIN_LABEL).ok_or("no main window")?;
    if mini {
        if let Some(b) = crate::window::enter_mini(&w).map_err(|e| e.to_string())? {
            // Not configured yet: nothing to save, and the strip still works this session.
            config::save_full_bounds(&state.config_path, Some(b)).map_err(|e| e.to_string())?;
        }
        Ok(())
    } else {
        let saved = load_settings(&state.config_path).and_then(|s| s.full_bounds);
        crate::window::leave_mini(&w, saved).map_err(|e| e.to_string())
    }
}
```

(`use tauri::Manager;` if not already imported.)

- [ ] **Step 5: Capabilities** — `"permissions": ["core:default", "core:window:allow-set-always-on-top", "core:window:allow-set-theme", "core:window:allow-start-dragging", "core:window:allow-minimize", "core:window:allow-close"]`.

- [ ] **Step 6: `cargo test`; expect pass.**
- [ ] **Step 7: Commit** — `feat: switch the window between full and Mini from Rust`.

### Task 6: Mini view, routing and menu

**Files:**
- Create: `src/views/mini.ts`
- Modify: `src/main.ts`, `src/api.ts`, `src/styles.css`
- Modify: `src/views/dashboard.ts` (export `PILL` so Mini uses the same words)

**Interfaces:**
- Consumes: `Snapshot.miniOrder`, `setLayout`, `setSkin`, `resolveSkin`, `applySkin`, `SkinList`
- Produces: `createMini(settings: Settings, actions: { openSettings(): void; fullDashboard(): void }): Dashboard`

- [ ] **Step 1: API** — `export const setLayout = (mini: boolean) => invoke<void>("set_layout", { mini });`

- [ ] **Step 2: `views/mini.ts`**

```ts
import { getCurrentWindow } from "@tauri-apps/api/window";
import { h } from "../dom";
import type { ModelCard, Settings, Snapshot } from "../types";
import { PILL, type Dashboard } from "./dashboard";

const CYCLE_MS = 4000;

export interface MiniActions {
  openSettings(): void;
  fullDashboard(): void;
}

/** Busy + streaming reads "streaming", as on the cards' detail line. */
function stateWord(c: ModelCard): string {
  return c.state.kind === "busy" && c.state.streaming > 0 ? "Streaming" : PILL[c.state.kind];
}

export function createMini(settings: Settings, actions: MiniActions): Dashboard {
  const win = getCurrentWindow();
  const icon = h("img", { class: "mini-icon", src: "/icon.png", alt: "", "data-tauri-drag-region": "" });
  const name = h("span", { class: "mini-name", "data-tauri-drag-region": "" }, "Connecting…");
  const dot = h("span", { class: "dot mini-dot" });
  const word = h("span", { class: "mini-word", "data-tauri-drag-region": "" });
  const more = h("span", { class: "mini-more" });
  const min = h("button", { type: "button", class: "mini-btn", "aria-label": "Minimize" }, "–");
  const close = h("button", { type: "button", class: "mini-btn", "aria-label": "Close" }, "×");
  const menu = h(
    "div",
    { class: "mini-menu", role: "menu", hidden: "" },
    h("button", { type: "button", role: "menuitem", "data-act": "full" }, "Full dashboard"),
    h("button", { type: "button", role: "menuitem", "data-act": "settings" }, "Settings…"),
  );
  const strip = h("div", { class: "mini", "data-tauri-drag-region": "" }, icon, name, dot, word, more, h("span", { class: "mini-gap", "data-tauri-drag-region": "" }), min, close, menu);

  min.addEventListener("click", () => void win.minimize());
  close.addEventListener("click", () => void win.close());
  const openMenu = (e: MouseEvent) => {
    e.preventDefault();
    menu.hidden = false;
    (menu.firstElementChild as HTMLElement).focus();
  };
  strip.addEventListener("dblclick", openMenu);
  strip.addEventListener("contextmenu", openMenu);
  menu.addEventListener("click", (e) => {
    const act = (e.target as HTMLElement).dataset.act;
    menu.hidden = true;
    if (act === "full") actions.fullDashboard();
    if (act === "settings") actions.openSettings();
  });
  const hideMenu = (e: Event) => {
    if (!menu.contains(e.target as Node)) menu.hidden = true;
  };
  document.addEventListener("mousedown", hideMenu);
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") menu.hidden = true;
  };
  document.addEventListener("keydown", onKey);

  let last: Snapshot | null = null;
  let shownId: string | null = null;
  let timer: number | null = null;

  function show() {
    const s = last;
    if (!s) return;
    strip.dataset.conn = s.connection.kind;
    if (s.connection.kind !== "connected" && s.connection.kind !== "connecting") {
      name.textContent = "Can't reach llama-swap";
      word.textContent = "";
      more.textContent = "";
      strip.removeAttribute("data-state");
      return;
    }
    const order = s.miniOrder;
    if (order.length === 0) {
      name.textContent = "No model loaded";
      word.textContent = more.textContent = "";
      strip.removeAttribute("data-state");
      return;
    }
    const byId = new Map(s.models.map((m) => [m.id, m]));
    // Stalled always wins; otherwise keep the cycled model while it is still loaded.
    const pinned = byId.get(order[0])?.state.kind === "stalled";
    if (!settings.miniCycle || pinned || !shownId || !order.includes(shownId)) shownId = order[0];
    const card = byId.get(shownId)!;
    name.textContent = card.name || card.id;
    word.textContent = stateWord(card);
    strip.dataset.state = card.state.kind;
    const others = order.filter((id) => id !== shownId);
    more.textContent = others.length ? `+${others.length}` : "";
    more.title = others.map((id) => byId.get(id)?.name || id).join("\n");
  }

  if (settings.miniCycle) {
    timer = window.setInterval(() => {
      const order = last?.miniOrder ?? [];
      if (order.length < 2 || !shownId) return;
      shownId = order[(order.indexOf(shownId) + 1) % order.length];
      show();
    }, CYCLE_MS);
  }

  return {
    element: strip,
    update(s) {
      last = s;
      show();
    },
    showUpdate() {
      // No room on the strip; the full dashboard shows it.
    },
    destroy() {
      if (timer !== null) clearInterval(timer);
      document.removeEventListener("mousedown", hideMenu);
      document.removeEventListener("keydown", onKey);
    },
  };
}
```

Check what the app icon path is in `public/` (or `src-tauri/icons`); if no web-served icon exists, copy `src-tauri/icons/32x32.png` to `public/icon.png` and use that path. The CSP allows `img-src 'self' data:`.

- [ ] **Step 3: `main.ts` routing**

Replace the body of `showDashboard(view)` with a layout switch:

```ts
async function showMain(view: SettingsView) {
  const { skin, note } = skins ? resolveSkin(skins, view.settings.skin) : { skin: null, note: null };
  if (note) console.warn(note);
  applySkin(skin);
  const mini = skin?.layout === "mini";
  await setLayout(mini).catch((e) => console.error("set_layout failed:", String(e)));
  if (mini) showMini(view);
  else await showDashboard(view);
}
```

- `showMini(view)`: destroys the current view, creates `createMini(view.settings, { openSettings, fullDashboard })`, mounts it, then fetches `getSnapshot()` like `showDashboard` does.
- `openSettings`: `await setLayout(false)`, then `showSetup(await getSettings())`, after `skins = await listSkins().catch(() => skins)` so Rescan starts fresh.
- `fullDashboard`: `await setSkin(view.settings.lastFullSkin, true)` (on error log it and still continue for this session), then `showMain(await getSettings())`.
- `showSetup` stays full layout. Its Cancel and Save both call `showMain(...)`, so Cancel from a Mini skin goes back to the strip.
- `boot()`: call `showMain(view)` where it called `showDashboard(view)`.
- `onSnapshot` already calls `dashboard?.update(s)`; the Mini view implements the same `Dashboard` interface, so keep a single `dashboard` variable.

- [ ] **Step 4: CSS**

```css
:root[data-layout="mini"] { scrollbar-gutter: auto; overflow: hidden; }
:root[data-layout="mini"] body, :root[data-layout="mini"] #app { margin: 0; padding: 0; }
.mini { display: flex; align-items: center; gap: 6px; height: 36px; padding: 0 4px 0 8px; background: var(--panel); border: 1px solid var(--border); position: relative; user-select: none; white-space: nowrap; }
.mini-icon { width: 18px; height: 18px; flex: none; }
.mini-name { font-weight: 600; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
.mini-word { color: var(--muted); flex: none; }
.mini-more { color: var(--muted); font-size: 12px; flex: none; }
.mini-gap { flex: 1; align-self: stretch; }
.mini-btn { border: 0; background: transparent; width: 28px; height: 28px; padding: 0; flex: none; }
.mini-btn:hover { background: var(--track); }
.mini .dot { width: 10px; height: 10px; }
.mini[data-state="idle"] .dot, .mini[data-state="loaded"] .dot { background: var(--green); }
.mini[data-state="busy"] .dot { background: var(--blue); }
.mini[data-state="loading"] .dot, .mini[data-state="unloading"] .dot { background: var(--amber); }
.mini[data-state="stalled"] .dot { background: var(--red); }
.mini[data-state="stalled"] .mini-word { color: var(--red); }
.mini[data-conn="unreachable"] .mini-name { color: var(--amber-text); }
.mini[data-conn="unauthorized"] .mini-name, .mini[data-conn="error"] .mini-name { color: var(--red); }
/* The window is only 36 px tall, so the menu is one row laid over the strip, not a dropdown. */
.mini-menu { position: absolute; inset: 0; display: flex; gap: 4px; align-items: center; justify-content: center; background: var(--panel); z-index: 10; }
.mini-menu[hidden] { display: none; }
.mini-menu button { height: 28px; padding: 0 10px; }
```

Check the real selector of the app root in `index.html` (`#app` assumed above) and match it.

- [ ] **Step 5: `npm run build`; `cargo test`.**
- [ ] **Step 6: Commit** — `feat: Mini strip view with skin-driven layout switching`.

Manual checks for the PR body (user runs): pick Minimal → Save → window becomes the strip with no frame; drag it; – minimizes, × quits; restart → opens as the strip; double-click → Full dashboard → previous size/position return; with two models loaded → busiest shown with `+1` and tooltip; turn on cycling → name rotates every 4 s, stops on a stalled model; unplug/move to a missing monitor (or edit `fullBounds` to x=99999) → Full returns centred; with llama-swap down → "Can't reach llama-swap", and Full dashboard still works.
