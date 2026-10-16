import { isSavedUrl, listSkins, saveSettings, testConnection } from "../api";
import { h } from "../dom";
import type { Settings, SettingsView, SkinList, TestResult, Theme } from "../types";
import { applySkin, resolveSkin, STANDARD_ID } from "../theme";
import { launchCheckAnswer } from "./dashboard";

export function renderSetup(
  view: SettingsView,
  /** null: listing failed, so there is nothing to pick from. */
  skins: SkinList | null,
  onSaved: (saved: SettingsView) => void | Promise<void>,
  onCancel?: () => void,
): HTMLElement {
  const s = view.settings;
  const url = h("input", { id: "url", type: "text", value: s.baseUrl, placeholder: "http://localhost:8080", autocomplete: "off", spellcheck: "false" });
  const key = h("input", {
    id: "key",
    type: "password",
    autocomplete: "off",
    placeholder: view.hasKey ? "Saved — leave blank to keep it" : "Leave blank if llama-swap has no apiKeys",
  });
  const removeKey = h("input", { id: "remove-key", type: "checkbox" });
  const poll = numberInput("poll", s.pollIntervalMs / 1000, 0.5, 60, 0.5);
  const load = numberInput("load", s.thresholds.loadTimeoutS, 5, 3600, 1);
  const stop = numberInput("stop", s.thresholds.stopTimeoutS, 5, 600, 1);
  const firstByte = numberInput("first-byte", s.thresholds.firstByteTimeoutS, 5, 3600, 1);
  const stall = numberInput("stall", s.thresholds.streamStallTimeoutS, 5, 600, 1);
  const checkUpdates = h("input", { id: "check-updates", type: "checkbox" });
  // Never answered means on, unless the notice was answered this launch but the answer couldn't be saved.
  checkUpdates.checked = s.checkForUpdates ?? launchCheckAnswer() ?? true;
  const theme = h(
    "select",
    { id: "theme" },
    ...THEMES.map(([value, label]) => h("option", { value }, label)),
  );
  theme.value = s.theme;
  const miniCycle = h("input", { id: "mini-cycle", type: "checkbox" });
  miniCycle.checked = s.miniCycle;
  const skinSelect = h("select", { id: "skin" });
  const skinInfo = h("div", { class: "hint" });
  const foldersInput = h("input", { id: "skins-folder", type: "text", readonly: "", value: skins?.folder ?? "" });
  const copyBtn = h("button", { type: "button" }, "Copy");
  const rescanBtn = h("button", { type: "button" }, "Rescan");
  let list = skins;
  const skinById = (id: string) => list?.skins.find((k) => k.id === id);

  /** Rebuilds the options from `list`, keeping `selected` when it is still there. */
  const fillSkins = (selected: string) => {
    skinSelect.replaceChildren();
    if (!list) {
      skinSelect.append(h("option", { value: s.skin }, "Standard"));
      skinSelect.disabled = true;
      return;
    }
    let sawBuiltin = false;
    let divided = false;
    for (const k of list.skins) {
      const builtin = k.id.startsWith("builtin:");
      if (builtin) sawBuiltin = true;
      else if (sawBuiltin && !divided) {
        divided = true;
        skinSelect.append(h("option", { disabled: "" }, "──────────"));
      }
      skinSelect.append(h("option", { value: k.id }, k.layout === "mini" ? `${k.name} (mini)` : k.name));
    }
    skinSelect.value = skinById(selected) ? selected : STANDARD_ID;
  };

  /** Skipped files, the folder error and the "not found" note, from the current scan. */
  const fillInfo = (note: string | null) => {
    const lines = list
      ? [
          list.folderError,
          note,
          ...(list.skipped.length
            ? [h("ul", { class: "skins-skipped" }, ...list.skipped.map((k) => h("li", {}, `${k.file}: ${k.reason}`)))]
            : []),
        ]
      : ["Skins couldn't be listed"];
    skinInfo.replaceChildren(...lines.filter((l): l is string | HTMLUListElement => !!l).map((l) => (typeof l === "string" ? h("div", {}, l) : l)));
  };
  fillSkins(s.skin);
  fillInfo(list ? resolveSkin(list, s.skin).note : null);

  const status = h("p", { class: "form-status", role: "status" });
  const testBtn = h("button", { type: "button" }, "Test connection");
  const saveBtn = h("button", { type: "submit", class: "primary" }, "Save");
  const cancelBtn = onCancel ? h("button", { type: "button" }, "Cancel") : null;

  const form = h(
    "form",
    {},
    field("url", "llama-swap URL", url, "Where llama-swap listens, e.g. http://192.168.1.20:8080. A trailing /v1 or /ui is removed."),
    field("key", "API key", key, "Only needed if llama-swap's config sets apiKeys. Stored in your OS keychain, never in a file."),
    view.hasKey ? h("label", { class: "check" }, removeKey, "Remove the saved key") : null,
    field("poll", "Refresh every (seconds)", poll),
    h("label", { class: "check" }, checkUpdates, "Check GitHub for a new release at startup"),
    field("theme", "Theme", theme),
    field("skin", "Skin", skinSelect),
    skinInfo,
    field("skins-folder", "Skins folder", h("div", { class: "folder-row" }, foldersInput, copyBtn, rescanBtn), "Put skin .json files here, then Rescan."),
    h("label", { class: "check" }, miniCycle, "Cycle through loaded models in Mini"),
    h(
      "details",
      {},
      h("summary", {}, "Stall detection"),
      h(
        "div",
        { class: "thresholds" },
        field("load", "Slow-load warning after (s)", load),
        field("stop", "Unloading timeout (s)", stop),
        field("first-byte", "No first byte after (s)", firstByte, "Stalled if a started reply sends nothing this long; with no reply yet, only a \"reply slow\" hint"),
        field("stall", "Output stopped for (s)", stall),
      ),
    ),
    status,
    h("div", { class: "actions" }, cancelBtn, testBtn, saveBtn),
  );

  const apiKeyValue = (): string | null => {
    if (removeKey.checked) return "";
    const v = key.value.trim();
    return v ? v : null;
  };

  const collect = (): Settings => ({
    baseUrl: url.value,
    pollIntervalMs: Math.round(readNumber(poll, 2) * 1000),
    thresholds: {
      loadTimeoutS: readSeconds(load, 120),
      stopTimeoutS: readSeconds(stop, 30),
      firstByteTimeoutS: readSeconds(firstByte, 90),
      streamStallTimeoutS: readSeconds(stall, 30),
    },
    alwaysOnTop: s.alwaysOnTop, // changed from the dashboard's pin button, not this form
    checkForUpdates: checkUpdates.checked,
    theme: theme.value as Theme,
    // Without a skin list the saved choice is passed through, never replaced by a default.
    skin: list ? skinSelect.value : s.skin,
    lastFullSkin: skinById(skinSelect.value)?.layout === "full" ? skinSelect.value : s.lastFullSkin,
    miniCycle: miniCycle.checked,
    fullBounds: s.fullBounds, // written by the backend when entering Mini
  });

  const controls: Array<HTMLInputElement | HTMLSelectElement | HTMLButtonElement> = [
    url, key, removeKey, poll, checkUpdates, theme, load, stop, firstByte, stall, testBtn, saveBtn,
    skinSelect, miniCycle, copyBtn, rescanBtn,
  ];
  if (cancelBtn) controls.push(cancelBtn);
  const setBusy = (busy: boolean) => {
    for (const c of controls) c.disabled = busy;
  };

  const showStatus = (text: string, kind: "ok" | "error" | "pending") => {
    status.textContent = text;
    status.dataset.kind = kind;
  };

  /** Everything the user entered, frozen at the moment they asked. */
  interface Entered {
    baseUrl: string;
    apiKey: string | null;
    settings: Settings;
  }
  const capture = (): Entered => {
    const settings = collect();
    return { baseUrl: settings.baseUrl, apiKey: apiKeyValue(), settings };
  };

  // The form is detached when the app swaps views; a continuation that wakes up
  // after that must not touch the UI or save anything.
  const alive = () => form.isConnected;

  /** Tests exactly what was captured. Returns null if the view went away meanwhile. */
  const runTest = async (entered: Entered): Promise<boolean | null> => {
    showStatus("Testing…", "pending");
    try {
      const result = await testConnection(entered.baseUrl, entered.apiKey);
      const keyBlank = entered.apiKey === null;
      // null = the backend could not answer; then claim neither "sent" nor "not sent".
      const sameUrl = view.hasKey && keyBlank ? await isSavedUrl(entered.baseUrl).catch((): null => null) : false;
      if (!alive()) return null;
      const { ok, text } = describeResult(result, {
        savedKeySent: view.hasKey && keyBlank && sameUrl === true,
        savedKeyNotSent: view.hasKey && keyBlank && sameUrl === false,
      });
      showStatus(text, ok ? "ok" : "error");
      return ok;
    } catch (e) {
      if (!alive()) return null;
      showStatus(String(e), "error");
      return false;
    }
  };

  testBtn.addEventListener("click", async () => {
    const entered = capture();
    setBusy(true);
    try {
      await runTest(entered);
    } finally {
      if (alive()) setBusy(false);
    }
  });
  skinSelect.addEventListener("change", () => {
    const picked = skinById(skinSelect.value);
    if (picked) applySkin(picked);
  });
  copyBtn.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(foldersInput.value);
    } catch {
      foldersInput.select();
    }
  });
  rescanBtn.addEventListener("click", async () => {
    setBusy(true);
    try {
      const next = await listSkins();
      if (!alive()) return;
      list = next;
      foldersInput.value = next.folder;
      fillSkins(skinSelect.value);
      const picked = skinById(skinSelect.value);
      if (picked) applySkin(picked);
      fillInfo(resolveSkin(next, s.skin).note);
    } catch (err) {
      if (alive()) showStatus(String(err), "error");
    } finally {
      if (alive()) setBusy(false);
    }
  });
  cancelBtn?.addEventListener("click", () => {
    // A preview must not outlive the form.
    if (list) applySkin(resolveSkin(list, s.skin).skin);
    onCancel?.();
  });
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const entered = capture();
    setBusy(true);
    try {
      if ((await runTest(entered)) !== true) return;
      const saved = await saveSettings(entered.settings, entered.apiKey);
      if (alive()) await onSaved(saved);
    } catch (err) {
      if (alive()) showStatus(String(err), "error");
    } finally {
      if (alive()) setBusy(false);
    }
  });

  return h(
    "section",
    { class: "setup" },
    h("h1", {}, "Connect to llama-swap"),
    h("p", { class: "muted" }, "Point the monitor at your llama-swap instance. Save checks the connection first."),
    form,
  );
}

const THEMES: ReadonlyArray<[Theme, string]> = [
  ["system", "Match system"],
  ["light", "Light"],
  ["dark", "Dark"],
];

interface KeyContext {
  /** The saved key was actually sent: key field blank and URL unchanged. */
  savedKeySent: boolean;
  /** A key is saved but the URL was edited, so the backend did not send it. */
  savedKeyNotSent: boolean;
}

function describeResult(r: TestResult, ctx: KeyContext): { ok: boolean; text: string } {
  switch (r.kind) {
    case "ok":
      return { ok: true, text: `Connected — llama-swap ${r.version}` };
    case "unreachable":
      return { ok: false, text: `Can't reach that address: ${r.message}` };
    case "redirect":
      return { ok: false, text: `${r.message}. Edit the URL above and test again.` };
    case "unauthorized":
      return {
        ok: false,
        text: ctx.savedKeySent
          ? "llama-swap rejected the saved API key."
          : "llama-swap needs a valid API key (see apiKeys in its config)." +
            (ctx.savedKeyNotSent ? " Re-enter the API key — the saved key is only sent to the saved URL." : ""),
      };
    case "notLlamaSwap":
      return { ok: false, text: `That server doesn't look like llama-swap: ${r.message}` };
  }
}

function numberInput(id: string, value: number, min: number, max: number, step: number): HTMLInputElement {
  return h("input", { id, type: "number", value: String(value), min: String(min), max: String(max), step: String(step) });
}

function field(id: string, label: string, input: HTMLElement, hint?: string): HTMLElement {
  return h("div", { class: "field" }, h("label", { for: id }, label), input, hint ? h("span", { class: "hint" }, hint) : null);
}

/** Thresholds are whole seconds on the Rust side (u64). */
function readSeconds(input: HTMLInputElement, fallback: number): number {
  return Math.max(1, Math.round(readNumber(input, fallback)));
}

function readNumber(input: HTMLInputElement, fallback: number): number {
  const n = Number(input.value);
  return Number.isFinite(n) && n > 0 ? n : fallback;
}
