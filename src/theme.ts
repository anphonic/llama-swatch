import { getCurrentWindow } from "@tauri-apps/api/window";
import { onColorScheme, systemColorScheme } from "./api";
import type { ColorScheme, Skin, SkinList, Theme } from "./types";

// Without a `data-theme` attribute the CSS follows `prefers-color-scheme`. That is right on
// Windows and macOS, but WebKitGTK only reports the GTK theme, so on Linux the backend reads the
// desktop's dark-style switch instead and "system" resolves to it here.
let chosen: Theme = "system";
let desktop: ColorScheme = "unknown";
let skin: Skin | null = null;

export const STANDARD_ID = "builtin:standard";
const media = window.matchMedia("(prefers-color-scheme: dark)");
media.addEventListener("change", () => render());

/** Same order as skins.rs `KEYS`. */
const KEYS = ["bg", "panel", "text", "muted", "border", "track", "grey", "green", "amber", "amberText", "blue", "red"];
const cssVar = (key: string) => "--" + key.replace(/[A-Z]/g, (c) => "-" + c.toLowerCase());

function paint(effective: ColorScheme) {
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

function render() {
  const effective = chosen === "system" ? desktop : chosen;
  const root = document.documentElement;
  if (effective === "unknown") delete root.dataset.theme;
  else root.dataset.theme = effective;
  paint(effective);
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
