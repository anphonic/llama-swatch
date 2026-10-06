import "./styles.css";
import { checkForUpdate, getSettings, getSnapshot, onSnapshot } from "./api";
import type { SettingsView, UpdateInfo } from "./types";
import { createDashboard, type Dashboard } from "./views/dashboard";
import { renderSetup } from "./views/setup";

const root = document.getElementById("app")!;
let dashboard: Dashboard | null = null;
/** Started once per launch, by the first dashboard shown with the setting on. */
let updateCheck: Promise<UpdateInfo | null> | null = null;

function showSetup(view: SettingsView) {
  dashboard?.destroy();
  dashboard = null;
  const cancel = view.configured ? () => void showDashboard(view) : undefined;
  root.replaceChildren(renderSetup(view, (saved) => void showDashboard(saved), cancel));
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
  if (view.settings.checkForUpdates === true) {
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
  if (view.configured) await showDashboard(view);
  else showSetup(view);
}

boot().catch((e) => {
  root.textContent = `Failed to start: ${e}`;
});
