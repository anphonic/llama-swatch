//! Startup handling for the main window. It is created hidden (tauri.conf.json) so the
//! window-state restore, the one-time first-launch fit and the pin are all applied before the
//! user sees it. Nothing here runs after startup: the app never resizes or moves the window later.

use std::path::Path;

use tauri::{AppHandle, Manager, PhysicalSize};

const MIN_INNER_HEIGHT_LOGICAL: f64 = 400.0;

/// First launch only: the inner height that makes the whole window (chrome included) fit in
/// `work_h`, never below `min_inner`. Unchanged when it already fits. All values physical px.
pub fn fit_inner_height(inner_h: u32, outer_h: u32, work_h: u32, min_inner: u32) -> u32 {
    if outer_h <= work_h {
        return inner_h;
    }
    let chrome = outer_h.saturating_sub(inner_h);
    work_h.saturating_sub(chrome).max(min_inner).min(inner_h)
}

/// Applies the pin and the first-launch fit, then shows the window. Every step is best effort
/// and the window is always shown, even if a step fails.
pub fn prepare(app: &AppHandle, always_on_top: bool, first_launch: bool) {
    let Some(w) = app.get_webview_window("main") else { return };
    if always_on_top {
        // Unsupported on some Linux/Wayland compositors; stay quiet.
        let _ = w.set_always_on_top(true);
    }
    if first_launch {
        let _ = fit_first_launch(&w);
    }
    let _ = w.show();
}

fn fit_first_launch(w: &tauri::WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = w.current_monitor()?.or(w.primary_monitor()?) else { return Ok(()) };
    let inner = w.inner_size()?;
    let outer = w.outer_size()?;
    let min = (MIN_INNER_HEIGHT_LOGICAL * monitor.scale_factor()).round() as u32;
    let height = fit_inner_height(inner.height, outer.height, monitor.work_area().size.height, min);
    if height != inner.height {
        w.set_size(PhysicalSize::new(inner.width, height))?;
        w.center()?;
    }
    Ok(())
}

/// The window-state plugin's default file, next to settings.json.
pub fn has_saved_state(config_dir: &Path) -> bool {
    config_dir.join(tauri_plugin_window_state::DEFAULT_FILENAME).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_inner_height_cases() {
        assert_eq!(fit_inner_height(770, 802, 1000, 400), 770, "already fits");
        assert_eq!(fit_inner_height(770, 802, 802, 400), 770, "exact fit");
        assert_eq!(fit_inner_height(770, 802, 720, 400), 688, "shrinks by the overflow");
        assert_eq!(fit_inner_height(770, 802, 300, 400), 400, "never below the minimum");
    }
}
