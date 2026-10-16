import "./styles.css";
import { checkForUpdate, getSettings, getSnapshot, listSkins, onSnapshot, setLayout, setSkin } from "./api";
import type { SettingsView, SkinList, UpdateInfo } from "./types";
import { createDashboard, type Dashboard } from "./views/dashboard";
import { createMini } from "./views/mini";
import { renderSetup } from "./views/setup";
import { applySkin, applyTheme, initTheme, resolveSkin } from "./theme";

const root = document.getElementById("app")!;
let dashboard: Dashboard | null = null;
/** Started once per launch, by the first dashboard shown with the setting on. */
let updateCheck: Promise<UpdateInfo | null> | null = null;
/** null until listed, or if listing failed; Settings then offers no picker. */
let skins: SkinList | null = null;
/** Bumped by every navigation so a slower, older one can tell it was superseded and stop. */
let navSeq = 0;

/** The window shape is decided here, not by applySkin, so a Settings preview only recolours. */
function setLayoutAttr(mini: boolean) {
  document.documentElement.dataset.layout = mini ? "mini" : "full";
}

async function showSetup(view: SettingsView) {
  const seq = ++navSeq;
  dashboard?.destroy();
  dashboard = null;
  setLayoutAttr(false);
  const cancel = view.configured ? () => void showMain(view) : undefined;
  const onSaved = async (saved: SettingsView) => {
    applyTheme(saved.settings.theme);
    // Settings may have rescanned the folder, so the saved skin can be newer than `skins`.
    skins = await listSkins().catch(() => skins);
    if (skins) applySkin(resolveSkin(skins, saved.settings.skin).skin);
    void showMain(saved);
  };
  // The backend rescans the skins folder whenever Settings opens.
  skins = await listSkins().catch(() => skins);
  if (seq !== navSeq) return;
  root.replaceChildren(renderSetup(view, skins, onSaved, cancel));
}

/** Applies the saved skin, sizes the window for its layout, and shows the matching view. */
async function showMain(view: SettingsView, forceFull = false) {
  const seq = ++navSeq;
  // forceFull: the saved skin is Mini but switching away failed; show the full view for this session.
  const id = forceFull ? view.settings.lastFullSkin : view.settings.skin;
  const { skin, note } = skins ? resolveSkin(skins, id) : { skin: null, note: null };
  if (note) console.warn(note);
  applySkin(skin);
  const mini = !forceFull && skin?.layout === "mini";
  await setLayout(mini).catch((e) => console.error("set_layout failed:", String(e)));
  if (seq !== navSeq) return;
  setLayoutAttr(mini);
  if (mini) await showMini(view);
  else await showDashboard(view);
}

async function openSettings() {
  const seq = ++navSeq;
  try {
    await setLayout(false);
    if (seq !== navSeq) return;
    setLayoutAttr(false);
    const view = await getSettings();
    if (seq !== navSeq) return;
    // showSetup rescans the skins folder, so Settings starts fresh.
    await showSetup(view);
  } catch (e) {
    console.error("failed to load settings:", String(e));
  }
}

async function showMini(view: SettingsView) {
  dashboard?.destroy();
  const current = createMini(view.settings, {
    openSettings: () => void openSettings(),
    fullDashboard: () => {
      void (async () => {
        const seq = navSeq;
        let saved = true;
        try {
          await setSkin(view.settings.lastFullSkin, true);
        } catch (e) {
          saved = false;
          console.error("failed to restore the full skin:", String(e));
        }
        const settings = await getSettings();
        if (seq !== navSeq) return;
        // If saving failed the stored skin is still Mini; show full anyway for this session.
        await showMain(settings, !saved);
      })().catch((e) => console.error("failed to load settings:", String(e)));
    },
  });
  dashboard = current;
  root.replaceChildren(current.element);
  try {
    const snap = await getSnapshot();
    if (snap && dashboard === current) current.update(snap);
  } catch (e) {
    console.error("failed to fetch snapshot:", String(e));
  }
}

async function showDashboard(view: SettingsView) {
  dashboard?.destroy();
  const current = createDashboard(view.settings, openSettings);
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
  skins = await listSkins().catch((e) => {
    console.error("failed to list skins:", String(e));
    return null;
  });
  if (skins) applySkin(resolveSkin(skins, view.settings.skin).skin);
  if (view.configured) await showMain(view);
  else await showSetup(view);
}

boot().catch((e) => {
  root.textContent = `Failed to start: ${e}`;
});
