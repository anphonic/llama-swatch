//! The desktop's light/dark preference on Linux.
//!
//! WebKitGTK derives `prefers-color-scheme` from the GTK theme only, so on GNOME/KDE with the
//! desktop's "dark style" switch on, the page still sees light. The XDG desktop portal's
//! `org.freedesktop.appearance color-scheme` setting is what that switch actually sets; it is read
//! here and pushed to the page, which then sets `data-theme` itself. On Windows and macOS the
//! webview already follows the OS, so this always answers `Unknown` there and the CSS media query
//! decides.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    Light,
    Dark,
    /// No preference, or nothing could be read: the page falls back to its media query.
    Unknown,
}

/// Event emitted when the desktop's preference changes. Payload: a `Scheme`.
pub const CHANGED_EVENT: &str = "color-scheme";

/// Portal values: 0 = no preference, 1 = prefer dark, 2 = prefer light.
pub fn from_portal(v: u32) -> Scheme {
    match v {
        1 => Scheme::Dark,
        2 => Scheme::Light,
        _ => Scheme::Unknown,
    }
}

/// `gsettings` output, e.g. `'prefer-dark'` for color-scheme or `'Adwaita-dark'` for gtk-theme.
pub fn from_gsettings(color_scheme: Option<&str>, gtk_theme: Option<&str>) -> Scheme {
    let unquote = |s: &str| s.trim().trim_matches('\'').to_ascii_lowercase();
    match color_scheme.map(unquote).as_deref() {
        Some("prefer-dark") => return Scheme::Dark,
        Some("prefer-light") => return Scheme::Light,
        _ => {}
    }
    match gtk_theme.map(unquote) {
        Some(t) if t.ends_with("-dark") || t.ends_with(":dark") => Scheme::Dark,
        _ => Scheme::Unknown,
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::time::Duration;

    use dbus::arg::{RefArg, Variant};
    use dbus::blocking::Connection;
    use dbus::message::MatchRule;
    use tauri::{AppHandle, Emitter};

    use super::{from_gsettings, from_portal, Scheme, CHANGED_EVENT};

    const DEST: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    const IFACE: &str = "org.freedesktop.portal.Settings";
    const NS: &str = "org.freedesktop.appearance";
    const KEY: &str = "color-scheme";
    const TIMEOUT: Duration = Duration::from_millis(500);

    /// Old portals (`Read`) wrap the value in an extra variant; unwrap until a number appears.
    fn as_u32(a: &dyn RefArg) -> Option<u32> {
        if let Some(n) = a.as_u64() {
            return u32::try_from(n).ok();
        }
        a.as_iter()?.next().and_then(as_u32)
    }

    fn read_portal() -> Option<u32> {
        let conn = Connection::new_session().ok()?;
        let proxy = conn.with_proxy(DEST, PATH, TIMEOUT);
        let (v,): (Variant<Box<dyn RefArg>>,) = proxy
            .method_call(IFACE, "ReadOne", (NS, KEY))
            .or_else(|_| proxy.method_call(IFACE, "Read", (NS, KEY)))
            .ok()?;
        as_u32(&v.0)
    }

    fn gsettings(key: &str) -> Option<String> {
        let out = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", key])
            .output()
            .ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn current() -> Scheme {
        match read_portal().map(from_portal) {
            Some(s @ (Scheme::Dark | Scheme::Light)) => s,
            _ => from_gsettings(gsettings("color-scheme").as_deref(), gsettings("gtk-theme").as_deref()),
        }
    }

    /// Follows the portal's `SettingChanged` signal for the life of the app. Silently does nothing
    /// when there is no session bus or no portal; the setting then only updates on restart.
    pub fn watch(app: &AppHandle) {
        let app = app.clone();
        let spawned = std::thread::Builder::new().name("color-scheme".into()).spawn(move || {
            let Ok(conn) = Connection::new_session() else { return };
            let rule = MatchRule::new_signal(IFACE, "SettingChanged").with_path(PATH);
            let added = conn.add_match(rule, move |(ns, key, v): (String, String, Variant<Box<dyn RefArg>>), _, _| {
                if ns == NS && key == KEY {
                    let scheme = as_u32(&v.0).map_or(Scheme::Unknown, from_portal);
                    let _ = app.emit(CHANGED_EVENT, scheme);
                }
                true
            });
            if added.is_err() {
                return;
            }
            while conn.process(Duration::from_secs(60)).is_ok() {}
        });
        if let Err(e) = spawned {
            eprintln!("color-scheme watcher not started: {e}");
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{current, watch};

/// The desktop's current preference: always `Unknown` outside Linux.
#[cfg(not(target_os = "linux"))]
pub fn current() -> Scheme {
    Scheme::Unknown
}

/// Follows preference changes: a no-op outside Linux.
#[cfg(not(target_os = "linux"))]
pub fn watch(_app: &tauri::AppHandle) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_values() {
        assert_eq!(from_portal(0), Scheme::Unknown);
        assert_eq!(from_portal(1), Scheme::Dark);
        assert_eq!(from_portal(2), Scheme::Light);
        assert_eq!(from_portal(7), Scheme::Unknown);
    }

    #[test]
    fn gsettings_values() {
        assert_eq!(from_gsettings(Some("'prefer-dark'\n"), None), Scheme::Dark);
        assert_eq!(from_gsettings(Some("'prefer-light'"), Some("'Adwaita-dark'")), Scheme::Light);
        assert_eq!(from_gsettings(Some("'default'"), Some("'Adwaita-dark'\n")), Scheme::Dark);
        assert_eq!(from_gsettings(Some("'default'"), Some("'Adwaita'")), Scheme::Unknown);
        assert_eq!(from_gsettings(None, Some("'Breeze-Dark'")), Scheme::Dark);
        assert_eq!(from_gsettings(None, Some("'Darkly'")), Scheme::Unknown);
        assert_eq!(from_gsettings(None, None), Scheme::Unknown);
    }

    #[test]
    fn scheme_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Scheme::Dark).unwrap(), "\"dark\"");
        assert_eq!(serde_json::to_string(&Scheme::Unknown).unwrap(), "\"unknown\"");
    }
}
