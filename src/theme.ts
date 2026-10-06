import { getCurrentWindow } from "@tauri-apps/api/window";
import { onColorScheme, systemColorScheme } from "./api";
import type { ColorScheme, Theme } from "./types";

// Without a `data-theme` attribute the CSS follows `prefers-color-scheme`. That is right on
// Windows and macOS, but WebKitGTK only reports the GTK theme, so on Linux the backend reads the
// desktop's dark-style switch instead and "system" resolves to it here.
let chosen: Theme = "system";
let desktop: ColorScheme = "unknown";

function render() {
  const effective = chosen === "system" ? desktop : chosen;
  const root = document.documentElement;
  if (effective === "unknown") delete root.dataset.theme;
  else root.dataset.theme = effective;
  // Title bar / window decorations; null hands it back to the OS.
  getCurrentWindow()
    .setTheme(effective === "unknown" ? null : effective)
    .catch((e) => console.error("setTheme failed:", String(e)));
}

/**
 * Call once at startup. Applies the saved choice right away; the desktop query can take a moment
 * on Linux (D-Bus, gsettings), so it is not awaited and recolours the page when it answers.
 */
export async function initTheme(setting: Theme) {
  chosen = setting;
  render();
  let gotEvent = false;
  await onColorScheme((s) => {
    gotEvent = true;
    desktop = s;
    render();
  }).catch((e) => console.error("color-scheme listener failed:", String(e)));
  void systemColorScheme()
    .catch((): ColorScheme => "unknown")
    .then((s) => {
      // A change event that arrived meanwhile is newer than this answer.
      if (gotEvent) return;
      desktop = s;
      render();
    });
}

/** Call after settings are saved. */
export function applyTheme(setting: Theme) {
  chosen = setting;
  render();
}
