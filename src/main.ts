import "./styles.css";
import { checkForUpdate, getSettings, getSnapshot, onSnapshot } from "./api";
import type { SettingsView, UpdateInfo } from "./types";
import { createDashboard, type Dashboard } from "./views/dashboard";
import { renderSetup } from "./views/setup";
import { applyTheme, initTheme } from "./theme";

const root = document.getElementById("app")!;
let dashboard: Dashboard | null = null;
/** Started once per launch, by the first dashboard shown with the setting on. */
let updateCheck: Promise<UpdateInfo | null> | null = null;

function showSetup(view: SettingsView) {
  dashboard?.destroy();
  dashboard = null;
  const cancel = view.configured ? () => void showDashboard(view) : undefined;
  const onSaved = (saved: SettingsView) => {
    applyTheme(saved.settings.theme);
    void showDashboard(saved);
  };
  root.replaceChildren(renderSetup(view, onSaved, cancel));
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
  // null = upgraded from a version without the setting: check, and the dashboard says so once.
  if (view.settings.checkForUpdates !== false) {
    updateCheck ??= checkForUpdate().catch((e) => {
      console.warn("update check failed:", String(e));
      return null;
    });
    void updateCheck.then((u) => {
      if (u && dashboard === current) current.showUpdate(u);
    });
  }
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
