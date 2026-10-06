import "./styles.css";
import { getSettings, getSnapshot, onSnapshot } from "./api";
import type { SettingsView } from "./types";
import { createDashboard, type Dashboard } from "./views/dashboard";
import { renderSetup } from "./views/setup";
import { applyTheme, initTheme } from "./theme";

const root = document.getElementById("app")!;
let dashboard: Dashboard | null = null;

function showSetup(view: SettingsView) {
  dashboard?.destroy();
  dashboard = null;
  const cancel = view.configured ? () => void showDashboard(view) : undefined;
  root.replaceChildren(renderSetup(
      view,
      (saved) => {
        applyTheme(saved.settings.theme);
        void showDashboard(saved);
      },
      cancel,
    ),);
}

async function showDashboard(view: SettingsView) {
  dashboard?.destroy();
  const current = createDashboard(view.settings, async () => {
    try {
      showSetup(await getSettings());
    } catch (e) {
      console.error("failed to load settings:", String(e));
    }
  });
  dashboard = current;
  root.replaceChildren(current.element);
  // The first snapshot may have been emitted before we subscribed; fetch it.
  try {
    const snap = await getSnapshot();
    if (snap && dashboard === current) current.update(snap);
  } catch (e) {
    console.error("failed to fetch snapshot:", String(e));
  }
}

async function boot() {
  await onSnapshot((s) => dashboard?.update(s));
  const view = await getSettings();
  await initTheme(view.settings.theme);
  if (view.configured) await showDashboard(view);
  else showSetup(view);
}

boot().catch((e) => {
  root.textContent = `Failed to start: ${e}`;
});
