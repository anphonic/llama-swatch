import { saveSettings, testConnection } from "../api";
import { h } from "../dom";
import type { Settings, SettingsView, TestResult } from "../types";

export function renderSetup(
  view: SettingsView,
  onSaved: (saved: SettingsView) => void,
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
  const status = h("p", { class: "form-status", role: "status" });
  const testBtn = h("button", { type: "button" }, "Test connection");
  const saveBtn = h("button", { type: "submit", class: "primary" }, "Save");
  const cancelBtn = onCancel ? h("button", { type: "button" }, "Cancel") : null;

  const form = h(
    "form",
    {},
    field("url", "llama-swap URL", url, "Where llama-swap listens, e.g. http://192.168.1.20:8080. A trailing /v1 is removed."),
    field("key", "API key", key, "Only needed if llama-swap's config sets apiKeys. Stored in your OS keychain, never in a file."),
    view.hasKey ? h("label", { class: "check" }, removeKey, "Remove the saved key") : null,
    field("poll", "Refresh every (seconds)", poll),
    h(
      "details",
      {},
      h("summary", {}, "Stall detection"),
      h(
        "div",
        { class: "thresholds" },
        field("load", "Loading timeout (s)", load),
        field("stop", "Unloading timeout (s)", stop),
        field("first-byte", "No first token after (s)", firstByte),
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
  });

  const setBusy = (busy: boolean) => {
    testBtn.disabled = busy;
    saveBtn.disabled = busy;
  };

  const showStatus = (text: string, kind: "ok" | "error" | "pending") => {
    status.textContent = text;
    status.dataset.kind = kind;
  };

  const runTest = async (): Promise<boolean> => {
    setBusy(true);
    showStatus("Testing…", "pending");
    try {
      const result = await testConnection(url.value, apiKeyValue());
      const keyBlank = apiKeyValue() === null;
      const sameUrl = normalizeUrl(url.value) === normalizeUrl(s.baseUrl);
      const { ok, text } = describeResult(result, {
        savedKeySent: view.hasKey && keyBlank && sameUrl,
        savedKeyNotSent: view.hasKey && keyBlank && !sameUrl,
      });
      showStatus(text, ok ? "ok" : "error");
      return ok;
    } catch (e) {
      showStatus(String(e), "error");
      return false;
    } finally {
      setBusy(false);
    }
  };

  testBtn.addEventListener("click", () => void runTest());
  cancelBtn?.addEventListener("click", () => onCancel?.());
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    if (!(await runTest())) return;
    setBusy(true);
    try {
      onSaved(await saveSettings(collect(), apiKeyValue()));
    } catch (err) {
      showStatus(String(err), "error");
    } finally {
      setBusy(false);
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

function normalizeUrl(u: string): string {
  return u.trim().replace(/\/+$/, "");
}

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
