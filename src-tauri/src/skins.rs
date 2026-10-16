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
    ("border", "#52525b"), ("track", "#a1a1aa"), ("grey", "#52525b"), ("green", "#047353"),
    ("amber", "#a34b08"), ("amberText", "#92400e"), ("blue", "#1d4ed8"), ("red", "#b91c1c"),
];
const CONTRAST_DARK: Table = [
    ("bg", "#000000"), ("panel", "#000000"), ("text", "#ffffff"), ("muted", "#d4d4d8"),
    ("border", "#a1a1aa"), ("track", "#52525b"), ("grey", "#a1a1aa"), ("green", "#4ade80"),
    ("amber", "#fbbf24"), ("amberText", "#fbbf24"), ("blue", "#5583ba"), ("red", "#fca5a5"),
];
const MUTED_LIGHT: Table = [
    ("bg", "#eceff4"), ("panel", "#f8f9fb"), ("text", "#2e3440"), ("muted", "#4c566a"),
    ("border", "#d8dee9"), ("track", "#d8dee9"), ("grey", "#5a6271"), ("green", "#446c35"),
    ("amber", "#815a1d"), ("amberText", "#8f6420"), ("blue", "#496486"), ("red", "#a1424c"),
];
const MUTED_DARK: Table = [
    ("bg", "#2e3440"), ("panel", "#3b4252"), ("text", "#eceff4"), ("muted", "#b4bccc"),
    ("border", "#434c5e"), ("track", "#434c5e"), ("grey", "#ccd1da"), ("green", "#c5d6b7"),
    ("amber", "#ebcb8b"), ("amberText", "#ebcb8b"), ("blue", "#c2d5da"), ("red", "#f2c6ca"),
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
        for bad in [r##""red""##, r##""#12""##, r##""url(http://x)""##, r##""var(--bg)""##, "12", r##""#1234567""##] {
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
        assert_eq!(parse(r##"{"name":"X","light":"#fff"}"##).unwrap_err(), "\"light\" must be an object");
        assert_eq!(parse(r#"{"name":"X","layout":null}"#).unwrap().layout, Layout::Full);
    }

    #[test]
    fn name_is_trimmed_and_limited_and_unknown_keys_ignored() {
        let long = "n".repeat(60);
        let s = parse(&format!(r##"{{"name":"  {long}  ","future":1,"light":{{"glow":"#fff"}}}}"##)).unwrap();
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

    fn lum(hex: &str) -> f64 {
        let h = hex.trim_start_matches('#');
        let c = |i: usize| {
            let v = f64::from(u8::from_str_radix(&h[i..i + 2], 16).unwrap()) / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * c(0) + 0.7152 * c(2) + 0.0722 * c(4)
    }

    fn ratio(a: &str, b: &str) -> f64 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    fn pill_background(fg: &str, panel: &str) -> String {
        let channel =
            |hex: &str, i: usize| u16::from_str_radix(&hex[1 + i * 2..3 + i * 2], 16).unwrap();
        let mixed = (0..3).map(|i| (channel(fg, i) * 15 + channel(panel, i) * 85 + 50) / 100);
        format!("#{}", mixed.map(|v| format!("{v:02x}")).collect::<String>())
    }

    /// Text colours must stay readable on the panel; `blue` is also a fill under contrasting text
    /// (`button.primary`, pressed pin and segmented tab in styles.css). Standard and Minimal keep
    /// today's colours and are not checked.
    #[test]
    fn built_in_skins_are_legible() {
        // (skin id, text on panel/bg, muted, status text)
        let rules = [
            ("builtin:high-contrast", 7.0, 4.5, 4.5),
            ("builtin:muted", 7.0, 4.5, 4.5),
        ];
        let mut fails = Vec::new();
        for (id, text_min, muted_min, status_min) in rules {
            let skin = builtins().into_iter().find(|s| s.id == id).unwrap();
            for (mode, p) in [("light", &skin.light), ("dark", &skin.dark)] {
                let mut check = |what: &str, fg: &str, bg: &str, min: f64| {
                    let r = ratio(fg, bg);
                    if r < min {
                        fails.push(format!("{id} {mode} {what} {fg} on {bg} = {r:.2} (< {min})"));
                    }
                };
                for bg in ["panel", "bg"] {
                    check(&format!("text/{bg}"), &p["text"], &p[bg], text_min);
                    check(&format!("muted/{bg}"), &p["muted"], &p[bg], muted_min);
                }
                for k in ["green", "amber", "amberText", "red"] {
                    check(k, &p[k], &p["panel"], status_min);
                }
                // Muted's blue is also used for text, and grey colours the idle-state pill.
                let soft_min = if id == "builtin:muted" { 3.0 } else { status_min };
                check("blue", &p["blue"], &p["panel"], soft_min);
                check("grey", &p["grey"], &p["panel"], soft_min);
                for k in ["green", "amber", "blue", "red", "grey"] {
                    let pill_bg = pill_background(&p[k], &p["panel"]);
                    check(&format!("pill/{k}"), &p[k], &pill_bg, 4.5);
                }
                let button_text = if ratio("#ffffff", &p["blue"]) >= ratio("#000000", &p["blue"]) {
                    "#ffffff"
                } else {
                    "#000000"
                };
                check("button label on blue", button_text, &p["blue"], 4.5);
            }
        }
        assert!(fails.is_empty(), "illegible palette pairs:
{}", fails.join("
"));
    }
}
