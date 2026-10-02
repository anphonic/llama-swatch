import "./styles.css";
import { getSettings, getSnapshot, onSnapshot } from "./api";
import type { SettingsView } from "./types";
import { createDashboard, type Dashboard } from "./views/dashboard";
import { renderSetup } from "./views/setup";

const root = document.getElementById("app")!;
let dashboard: Dashboard | null = null;

function showSetup(view: SettingsView) {
  dashboard?.destroy();
  dashboard = null;
  const cancel = view.configured ? () => void showDashboard(view) : undefined;
  root.replaceChildren(renderSetup(view, (saved) => void showDashboard(saved), cancel));
}

async function showDashboard(view: SettingsView) {
  dashboard?.destroy();
  const current = createDashboard(view.settings, async () => showSetup(await getSettings()));
  dashboard = current;
  root.replaceChildren(current.element);
  // The first snapshot may have been emitted before we subscribed; fetch it.
  const snap = await getSnapshot();
  if (snap && dashboard === current) current.update(snap);
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
