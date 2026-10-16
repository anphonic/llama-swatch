//! Startup handling for the main window. It is created hidden (tauri.conf.json) so the
//! window-state restore, the one-time first-launch fit and the pin are all applied before the
//! user sees it. After startup the app resizes the window only when the selected skin's layout
//! changes between the full dashboard and the frameless Mini strip (`enter_mini`/`leave_mini`).

use std::path::Path;

use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, PhysicalSize, Size};
use tauri_plugin_window_state::{StateFlags, WindowExt};

use crate::config::Bounds;

const MIN_INNER_HEIGHT_LOGICAL: f64 = 400.0;
const MINI_WIDTH: f64 = 300.0;
const MINI_HEIGHT: f64 = 36.0;
const FULL_MIN: LogicalSize<f64> = LogicalSize { width: 420.0, height: MIN_INNER_HEIGHT_LOGICAL };
const FULL_DEFAULT: LogicalSize<f64> = LogicalSize { width: 481.0, height: 770.0 };

/// The window-state flags, shared by the plugin registration and the explicit restore below.
pub const STATE_FLAGS: StateFlags =
    StateFlags::SIZE.union(StateFlags::POSITION).union(StateFlags::MAXIMIZED);

/// Label of the only window (tauri.conf.json).
pub const MAIN_LABEL: &str = "main";

/// First launch only: the inner height that makes the whole window (chrome included) fit in
/// `work_h`, never below `min_inner`. Unchanged when it already fits. All values physical px.
pub fn fit_inner_height(inner_h: u32, outer_h: u32, work_h: u32, min_inner: u32) -> u32 {
    if outer_h <= work_h {
        return inner_h;
    }
    let chrome = outer_h.saturating_sub(inner_h);
    work_h.saturating_sub(chrome).max(min_inner).min(inner_h)
}

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
    // Save the normal (restored) rect, not the maximized one, and remember it was maximized.
    let was_max = w.is_maximized()?;
    if was_max {
        w.unmaximize()?;
    }
    let pos = w.outer_position()?;
    let size = w.inner_size()?;
    let saved = Bounds { x: pos.x, y: pos.y, width: size.width, height: size.height, maximized: was_max };
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
    place_full(w, saved)
}

fn place_full(w: &tauri::WebviewWindow, saved: Option<Bounds>) -> tauri::Result<()> {
    let monitors: Vec<(i32, i32, u32, u32)> = w
        .available_monitors()?
        .iter()
        .map(|m| (m.position().x, m.position().y, m.size().width, m.size().height))
        .collect();
    match saved.filter(|b| visible_on(b, &monitors)) {
        Some(b) => {
            // Position first: moving to another monitor can change the DPI scale, which
            // would rescale a size set before the move.
            w.set_position(PhysicalPosition::new(b.x, b.y))?;
            w.set_size(PhysicalSize::new(b.width, b.height))?;
            if b.maximized {
                let _ = w.maximize();
            }
        }
        None => {
            w.set_size(FULL_DEFAULT)?;
            w.center()?;
        }
    }
    Ok(())
}

/// Restores the saved size/position/maximized state, applies the pin and the first-launch fit,
/// then shows the window. Every step is best effort and the window is always shown, even if a
/// step fails.
///
/// The restore is done here rather than by the plugin (registered with `skip_initial_state`) so
/// that the restore -> first-launch fit -> show order is explicit in this function instead of
/// depending on when Tauri runs the plugin's window-ready hook, and the restore happens exactly
/// once. On first launch there is no saved state, so the restore only records the current state.
///
/// `mini` is the selected skin's layout. Both paths restore with the minimum size cleared, so
/// tauri.conf.json's 420x400 minimum can't stretch a saved strip into a window-sized one. In
/// Mini the window is also made frameless and forced to the strip size after the restore.
/// Otherwise a saved strip (a Mini session whose skin was changed by hand) is replaced by
/// `full_bounds`, the minimum is set, and only then is the maximized state restored (setting a
/// minimum size on Windows resizes the window and would drop it).
pub fn prepare(app: &AppHandle, always_on_top: bool, first_launch: bool, mini: bool, full_bounds: Option<Bounds>) {
    let Some(w) = app.get_webview_window(MAIN_LABEL) else { return };
    if mini {
        let _ = w.set_min_size(None::<Size>);
        let _ = w.set_decorations(false);
        let _ = w.set_resizable(false);
    } else {
        // So the restore applies the saved size unclamped: the 400 px minimum would otherwise
        // stretch a saved strip to exactly the minimum, which looks like a real window there.
        let _ = w.set_min_size(None::<Size>);
    }
    if mini {
        let _ = w.restore_state(STATE_FLAGS);
        let _ = w.set_size(LogicalSize::new(MINI_WIDTH, MINI_HEIGHT));
    } else {
        let _ = w.restore_state(StateFlags::SIZE | StateFlags::POSITION);
        if is_strip(&w) {
            let _ = place_full(&w, full_bounds);
        }
        let _ = w.set_min_size(Some(FULL_MIN));
        let _ = w.restore_state(StateFlags::MAXIMIZED);
    }
    if always_on_top {
        // Unsupported on some Linux/Wayland compositors; stay quiet.
        let _ = w.set_always_on_top(true);
    }
    if first_launch && !mini {
        let _ = fit_first_launch(&w);
    }
    let _ = w.show();
}

/// Whether a restored inner height (physical px) is below the full layout's minimum: a saved
/// Mini strip. A window sitting exactly at the minimum is a real full window.
fn is_strip_height(inner_h: u32, scale: f64) -> bool {
    // Compare against the rounded physical minimum: at 4/3 the 400 logical minimum is 533 px,
    // and a real minimum-height window must not read as a strip.
    f64::from(inner_h) < (MIN_INNER_HEIGHT_LOGICAL * scale).round()
}

/// The restored window is a leftover Mini strip (see `prepare`, which restores unclamped).
fn is_strip(w: &tauri::WebviewWindow) -> bool {
    let (Ok(inner), Ok(scale)) = (w.inner_size(), w.scale_factor()) else { return false };
    is_strip_height(inner.height, scale)
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

    #[test]
    fn strip_height_cases() {
        assert!(is_strip_height(36, 1.0), "strip at 100%");
        assert!(is_strip_height(54, 1.5), "strip at 150%");
        assert!(!is_strip_height(400, 1.0), "exactly the minimum");
        assert!(!is_strip_height(600, 1.5), "exactly the minimum at 150%");
        assert!(!is_strip_height(1100, 1.25), "normal window");
        assert!(!is_strip_height(533, 4.0 / 3.0), "minimum at 4/3");
        assert!(!is_strip_height(534, 4.0 / 3.0), "just above the minimum at 4/3");
        assert!(!is_strip_height(500, 1.25), "minimum at 125%");
        assert!(!is_strip_height(600, 1.5), "minimum at 150%");
        assert!(is_strip_height(48, 4.0 / 3.0), "strip at 4/3");
        assert!(is_strip_height(399, 1.0), "one pixel short of the minimum at 100%");
    }

    #[test]
    fn bounds_must_touch_a_monitor() {
        let monitors = [(0, 0, 1920, 1080), (1920, 0, 2560, 1440)];
        let b = |x, y| Bounds { x, y, width: 500, height: 700, maximized: false };
        assert!(visible_on(&b(100, 100), &monitors));
        assert!(visible_on(&b(2000, 50), &monitors), "second monitor");
        assert!(!visible_on(&b(5000, 100), &monitors), "unplugged monitor to the right");
        assert!(!visible_on(&b(100, -900), &monitors), "title strip above every screen");
        assert!(!visible_on(&b(100, 100), &[]));
    }
}
