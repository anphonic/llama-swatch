import { h, svg } from "../dom";
import { formatAgo, formatCount, formatDuration } from "../format";
import { countLoaded, isLoaded } from "../models";
import type { Connection, ModelCard, ModelState, Settings, Snapshot } from "../types";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { loadModel, setAlwaysOnTopSetting, unloadModel } from "../api";
import { renderHistogram } from "./components/histogram";
import { createRing, type Ring } from "./components/ring";
import { renderSparkline } from "./components/sparkline";
import { createStatTile } from "./components/statTile";
import { createTtlBar, ttlLabel } from "./components/ttlBar";
import { createHistory } from "./history";

export interface Dashboard {
  element: HTMLElement;
  update(s: Snapshot): void;
  destroy(): void;
}

const PILL: Record<ModelState["kind"], string> = {
  notLoaded: "Not loaded",
  loading: "Loading",
  idle: "Idle",
  busy: "Busy",
  stalled: "Stalled",
  unloading: "Unloading",
};

const CONN_TEXT: Record<Connection["kind"], string> = {
  connecting: "Connecting…",
  connected: "Connected",
  unauthorized: "Unauthorized",
  unreachable: "Unreachable",
  error: "Error",
};

function lastOf(values: number[]): number | undefined {
  return values.length ? values[values.length - 1] : undefined;
}

function describe(card: ModelCard, settings: Settings): string {
  const s = card.state;
  const tok = lastOf(card.tokSHistory);
  switch (s.kind) {
    case "notLoaded":
      return card.lastRequestAtMs ? `Last used ${formatAgo(card.lastRequestAtMs)}` : "";
    case "loading":
      return `Starting for ${formatDuration(s.elapsedS)} · timeout ${formatDuration(settings.thresholds.loadTimeoutS)}`;
    case "idle":
      return `Up ${formatDuration(s.uptimeS)}${tok !== undefined ? ` · last ${tok.toFixed(1)} tok/s` : ""}`;
    case "busy":
      return [
        `${s.requests} request${s.requests === 1 ? "" : "s"}`,
        tok !== undefined ? `${tok.toFixed(1)} tok/s` : null,
        formatDuration(s.oldestElapsedS),
      ].filter(Boolean).join(" · ");
    case "stalled":
      return s.reason;
    case "unloading":
      return "Stopping…";
  }
}

function pinIcon(): SVGSVGElement {
  return svg(
    "svg",
    { viewBox: "0 0 24 24", width: 16, height: 16, fill: "none", stroke: "currentColor", "stroke-width": 2, "stroke-linecap": "round", "stroke-linejoin": "round", "aria-hidden": "true" },
    svg("path", { d: "M12 17v5" }),
    svg("path", { d: "M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z" }),
  );
}

function isStale(c: Connection): boolean {
  return c.kind !== "connected" && c.kind !== "connecting";
}

/** What a card can ask the dashboard to do; the dashboard owns confirmation and error display. */
interface CardActions {
  load(card: ModelCard): void;
  unload(card: ModelCard, x: number, y: number): void;
}

/** Fixed-height card (see `.card` in styles.css): every state fills the same slots, so a state
 *  change never moves the cards below it. Empty slots keep their space. */
class CardView {
  readonly el: HTMLElement;
  private readonly ring: Ring = createRing();
  private readonly title = h("div", { class: "card-title" });
  private readonly pill = h("span", { class: "pill" });
  private readonly detail = h("div", { class: "card-detail" });
  private readonly sub = h("div", { class: "card-sub" });
  private readonly spark = h("div", { class: "card-spark" });
  private readonly ttl = createTtlBar();

  private card: ModelCard | null = null;

  constructor(private readonly settings: Settings, actions: CardActions) {
    this.el = h(
      "article",
      { class: "card" },
      h("div", { class: "card-head" }, this.ring.el, h("div", { class: "card-heading" }, this.title, this.pill)),
      this.detail,
      this.spark,
      this.ttl.el,
      this.sub,
    );
    this.el.addEventListener("dblclick", () => {
      if (this.card?.state.kind === "notLoaded") actions.load(this.card);
    });
    this.el.addEventListener("contextmenu", (e) => {
      e.preventDefault(); // no browser menu on cards
      const kind = this.card?.state.kind;
      if (this.card && (kind === "idle" || kind === "busy" || kind === "stalled" || kind === "loading")) {
        actions.unload(this.card, e.clientX, e.clientY);
      }
    });
  }

  /** lastSeenMs is non-null only while the connection is stale or disconnected. */
  update(card: ModelCard, lastSeenMs: number | null) {
    const s = card.state;
    this.card = card;
    const hint = s.kind === "notLoaded" ? "Double-click to load" : s.kind === "unloading" ? "" : "Right-click to unload";
    if (this.el.title !== hint) this.el.title = hint;
    if (this.el.dataset.state !== s.kind) this.el.dataset.state = s.kind;
    this.title.textContent = card.name;
    this.title.title = card.description ? `${card.id} — ${card.description}` : card.id;
    this.ring.update(s, this.settings.thresholds.loadTimeoutS);
    this.pill.textContent = PILL[s.kind];
    const detail = describe(card, this.settings);
    this.detail.textContent = detail;
    this.detail.title = detail; // full text when ellipsized
    // One reserved line: "last seen" while disconnected, else the unload estimate while idle.
    const ttlOn = s.kind === "idle" && card.ttlS !== null && card.ttlRemainingS !== null;
    this.sub.textContent =
      lastSeenMs !== null ? `Last seen ${formatAgo(lastSeenMs)}` : ttlOn ? ttlLabel(card.ttlRemainingS as number) : "";
    this.ttl.update(ttlOn ? (card.ttlRemainingS as number) : null, card.ttlS ?? 0);
    this.spark.replaceChildren(
      ...((s.kind === "busy" || s.kind === "idle") && card.tokSHistory.length >= 2 ? [renderSparkline(card.tokSHistory)] : []),
    );
  }
}

const LOADED_ONLY_KEY = "llama-swap-monitor.loadedOnly";

function readLoadedOnly(): boolean {
  try {
    return localStorage.getItem(LOADED_ONLY_KEY) === "1";
  } catch {
    return false;
  }
}

function writeLoadedOnly(on: boolean) {
  try {
    localStorage.setItem(LOADED_ONLY_KEY, on ? "1" : "0");
  } catch {
    // Storage unavailable: the choice just won't persist.
  }
}

type View = "models" | "history";
const VIEW_KEY = "llama-swap-monitor.view";

function readView(): View {
  try {
    return localStorage.getItem(VIEW_KEY) === "history" ? "history" : "models";
  } catch {
    return "models";
  }
}

function writeView(v: View) {
  try {
    localStorage.setItem(VIEW_KEY, v);
  } catch {
    // Storage unavailable: the choice just won't persist.
  }
}

export function createDashboard(settings: Settings, onOpenSettings: () => void): Dashboard {
  const dot = h("span", { class: "dot" });
  const statusText = h("span", { class: "status-text" }, "Connecting…");
  const live = h("span", { class: "badge" });
  const host = h("span", { class: "host" }, settings.baseUrl);
  const meta = h("span", { class: "meta" });
  const gear = h("button", { type: "button", class: "icon-button", "aria-label": "Settings", title: "Settings" }, "⚙");
  gear.addEventListener("click", onOpenSettings);
  let pinned = settings.alwaysOnTop;
  const pin = h("button", { type: "button", class: "icon-button pin" });
  pin.append(pinIcon());
  function renderPin() {
    pin.setAttribute("aria-pressed", String(pinned));
    const label = pinned ? "Stop keeping on top" : "Keep on top";
    pin.title = label;
    pin.setAttribute("aria-label", label);
  }
  renderPin();
  pin.addEventListener("click", () => {
    pinned = !pinned;
    renderPin();
    // Some platforms (e.g. certain Wayland compositors) ignore or reject this; stay quiet.
    getCurrentWindow().setAlwaysOnTop(pinned).catch((e) => console.warn("setAlwaysOnTop:", String(e)));
    setAlwaysOnTopSetting(pinned).catch((e) => console.warn("could not save pin state:", String(e)));
  });
  const viewButtons: Record<View, HTMLButtonElement> = {
    models: h("button", { type: "button", class: "seg" }, "Models"),
    history: h("button", { type: "button", class: "seg" }, "History"),
  };
  const switcher = h("div", { class: "segmented", role: "group", "aria-label": "View" }, viewButtons.models, viewButtons.history);
  const header = h(
    "header",
    { class: "header" },
    dot,
    h("div", { class: "header-main" }, h("div", { class: "header-line" }, statusText, live), h("div", { class: "header-line" }, host, meta)),
    switcher,
    pin,
    gear,
  );

  const banner = h("div", { class: "banner" });
  banner.hidden = true;
  const openSettings = h("button", { type: "button" }, "Open settings");
  openSettings.addEventListener("click", onOpenSettings);

  let loadedOnly = readLoadedOnly();
  const loadedOnlyBox = h("input", { type: "checkbox", id: "loaded-only" });
  loadedOnlyBox.checked = loadedOnly;
  const loadedCount = h("span", { class: "muted loaded-count" });
  const toolbar = h(
    "div",
    { class: "toolbar" },
    loadedCount,
    h("label", { class: "toggle", for: "loaded-only" }, loadedOnlyBox, h("span", {}, "Loaded only")),
  );
  loadedOnlyBox.addEventListener("change", () => {
    loadedOnly = loadedOnlyBox.checked;
    writeLoadedOnly(loadedOnly);
    render();
  });

  const toast = h("div", { class: "toast", role: "status" });
  toast.hidden = true;
  const grid = h("section", { class: "grid" });
  const empty = h("p", { class: "empty" }, "No models reported yet.");
  const tiles = {
    requests: createStatTile("Requests"),
    input: createStatTile("Input tokens"),
    output: createStatTile("Output tokens"),
    speed: createStatTile("Generation p50"),
  };
  const histBox = h("div", { class: "hist-box" });
  const stats = h(
    "section",
    { class: "stats" },
    h("div", { class: "tiles" }, tiles.requests.el, tiles.input.el, tiles.output.el, tiles.speed.el),
    histBox,
  );
  const modelsView = h("div", { class: "models-view" }, toolbar, grid, stats);
  const history = createHistory();
  // Banner and toast overlay the bottom of the window instead of sitting in
  // the flow, so showing or hiding them never pushes content down.
  const notices = h("div", { class: "notices" }, banner, toast);
  // Keep the page's bottom padding equal to the overlay's height so the last card / History row
  // can always be scrolled fully clear of it. Also publish the scrollbar width (see styles.css).
  const syncOverlay = () => {
    element.style.setProperty("--notices-h", `${notices.offsetHeight}px`);
    const sbw = Math.max(0, window.innerWidth - document.documentElement.clientWidth);
    document.documentElement.style.setProperty("--sbw", `${sbw}px`);
  };
  const overlayObserver = new ResizeObserver(syncOverlay);
  overlayObserver.observe(notices);
  window.addEventListener("resize", syncOverlay);
  const element = h("main", { class: "dashboard" }, header, modelsView, history.element, notices);

  const cards = new Map<string, CardView>();
  let last: Snapshot | null = null;

  let view: View = readView();
  function applyView() {
    modelsView.hidden = view !== "models";
    history.element.hidden = view !== "history";
    for (const [v, b] of Object.entries(viewButtons)) b.setAttribute("aria-pressed", String(v === view));
    if (view === "history") history.show();
    else history.hide();
  }
  for (const v of ["models", "history"] as const) {
    viewButtons[v].addEventListener("click", () => {
      if (view === v) return;
      view = v;
      writeView(v);
      applyView();
    });
  }

  let toastTimer: number | null = null;
  function showToast(text: string) {
    toast.textContent = text;
    toast.hidden = false;
    if (toastTimer !== null) window.clearTimeout(toastTimer);
    toastTimer = window.setTimeout(() => (toast.hidden = true), 8000);
  }

  function busyOther(id: string): ModelCard | undefined {
    return last?.models.find((m) => m.id !== id && m.state.kind === "busy");
  }

  const loading = new Set<string>();

  // Fire and forget: the poller shows Loading, then Idle. Only a failure is reported.
  const actions: CardActions = {
    load(card) {
      if (loading.has(card.id)) return; // a load for this model is already pending
      const busy = busyOther(card.id);
      if (busy && !confirm(`${busy.name} is busy. Loading ${card.name} may swap it out. Load anyway?`)) return;
      loading.add(card.id);
      loadModel(card.id)
        .catch((e) => showToast(`Could not load ${card.name}: ${String(e)}`))
        .finally(() => loading.delete(card.id));
    },
    unload(card, x, y) {
      openMenu(x, y, () => {
        if (card.state.kind === "busy" && !confirm(`${card.name} is busy. Unload anyway?`)) return;
        unloadModel(card.id).catch((e) => showToast(`Could not unload ${card.name}: ${String(e)}`));
      });
    },
  };

  let menu: HTMLElement | null = null;
  function closeMenu() {
    menu?.remove();
    menu = null;
  }
  function openMenu(x: number, y: number, onUnload: () => void) {
    closeMenu();
    const item = h("button", { type: "button", class: "ctx-item", role: "menuitem" }, "Unload");
    item.addEventListener("click", () => {
      closeMenu();
      onUnload();
    });
    menu = h("div", { class: "ctx-menu", role: "menu" }, item);
    menu.style.left = `${Math.min(x, window.innerWidth - 120)}px`;
    menu.style.top = `${Math.min(y, window.innerHeight - 50)}px`;
    element.append(menu);
    item.focus();
  }
  const onDocClick = (e: Event) => {
    if (menu && !menu.contains(e.target as Node)) closeMenu();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") closeMenu();
  };
  document.addEventListener("mousedown", onDocClick);
  document.addEventListener("keydown", onKey);
  window.addEventListener("blur", closeMenu);
  window.addEventListener("scroll", closeMenu, true);

  function showBanner(text: string, withSettingsButton: boolean) {
    banner.replaceChildren(h("span", {}, text), ...(withSettingsButton ? [openSettings] : []));
    banner.hidden = false;
  }

  function renderHeader(s: Snapshot) {
    const c = s.connection;
    element.dataset.conn = c.kind;
    element.classList.toggle("stale", isStale(c));
    statusText.textContent = CONN_TEXT[c.kind];
    meta.textContent = [s.version ? `llama-swap ${s.version}` : null, c.kind === "connected" ? `${c.latencyMs} ms` : null]
      .filter(Boolean)
      .join(" · ");
    live.textContent = s.liveEvents ? "live" : "polling";
    live.title = s.liveEvents
      ? "Receiving live request events"
      : "Live events unavailable: Busy and stalled-request detection are off";
    live.classList.toggle("badge-live", s.liveEvents);

    const seen = s.lastOkMs ? ` · last seen ${formatAgo(s.lastOkMs)}` : "";
    if (c.kind === "unauthorized") showBanner("llama-swap rejected the API key.", true);
    else if (c.kind === "unreachable" || c.kind === "error") showBanner(`${c.message}${seen}`, false);
    else banner.hidden = true;
  }

  function renderCards(s: Snapshot) {
    const lastSeenMs = isStale(s.connection) ? s.lastOkMs : null;
    const ordered: HTMLElement[] = [];
    const ids = new Set<string>();
    for (const m of s.models) {
      let view = cards.get(m.id);
      if (!view) {
        view = new CardView(settings, actions);
        cards.set(m.id, view);
      }
      view.update(m, lastSeenMs);
      ids.add(m.id);
      if (!loadedOnly || isLoaded(m)) ordered.push(view.el);
    }
    loadedCount.textContent = `${countLoaded(s.models)} of ${s.models.length} loaded`;
    empty.textContent = s.models.length ? "No models loaded." : "No models reported yet.";
    for (const id of [...cards.keys()]) if (!ids.has(id)) cards.delete(id);
    const wanted = ordered.length ? ordered : [empty];
    const same = wanted.length === grid.children.length && wanted.every((el, i) => grid.children[i] === el);
    if (!same) grid.replaceChildren(...wanted);
  }

  function renderStats(s: Snapshot) {
    const st = s.stats;
    if (!st) {
      for (const t of Object.values(tiles)) t.update("—");
      histBox.replaceChildren(
        h("p", { class: "muted" }, s.statsAvailable ? "Waiting for metrics…" : "This llama-swap version does not report metrics."),
      );
      return;
    }
    tiles.requests.update(formatCount(st.totalRequests));
    tiles.input.update(formatCount(st.totalInputTokens), st.totalCacheTokens ? `${formatCount(st.totalCacheTokens)} cached` : "");
    tiles.output.update(formatCount(st.totalOutputTokens));
    if (st.gen && st.gen.bins.length) {
      tiles.speed.update(`${st.gen.p50.toFixed(1)} tok/s`, `p95 ${st.gen.p95.toFixed(1)} · p99 ${st.gen.p99.toFixed(1)}`);
      histBox.replaceChildren(h("div", { class: "hist-title" }, "Generation speed (tok/s)"), renderHistogram(st.gen));
    } else {
      tiles.speed.update("—");
      histBox.replaceChildren(h("p", { class: "muted" }, "No generation requests yet."));
    }
  }

  function render() {
    if (!last) return;
    renderHeader(last);
    renderCards(last);
    renderStats(last);
  }

  applyView();

  // Re-render every second so "last seen … ago" stays current between snapshots.
  const timer = window.setInterval(render, 1000);

  return {
    element,
    update(s) {
      last = s;
      render();
    },
    destroy() {
      window.clearInterval(timer);
      history.destroy();
      overlayObserver.disconnect();
      window.removeEventListener("resize", syncOverlay);
      closeMenu();
      if (toastTimer !== null) window.clearTimeout(toastTimer);
      document.removeEventListener("mousedown", onDocClick);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("blur", closeMenu);
      window.removeEventListener("scroll", closeMenu, true);
    },
  };
}
