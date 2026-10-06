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

/** Call once at startup, before the first render. */
export async function initTheme(setting: Theme) {
  chosen = setting;
  await onColorScheme((s) => {
    desktop = s;
    render();
  }).catch((e) => console.error("color-scheme listener failed:", String(e)));
  desktop = await systemColorScheme().catch((): ColorScheme => "unknown");
  render();
}

/** Call after settings are saved. */
export function applyTheme(setting: Theme) {
  chosen = setting;
  render();
}
