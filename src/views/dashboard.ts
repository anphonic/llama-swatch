import { h } from "../dom";
import { formatAgo, formatCount, formatDuration } from "../format";
import type { Connection, ModelCard, ModelState, Settings, Snapshot } from "../types";
import { renderHistogram } from "./components/histogram";
import { createRing, type Ring } from "./components/ring";
import { renderSparkline } from "./components/sparkline";
import { createStatTile } from "./components/statTile";
import { renderTtlBar } from "./components/ttlBar";

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
      return `Up ${formatDuration(s.uptimeS)}${tok ? ` · last ${tok.toFixed(1)} tok/s` : ""}`;
    case "busy":
      return [
        `${s.requests} request${s.requests === 1 ? "" : "s"}`,
        tok ? `${tok.toFixed(1)} tok/s` : null,
        formatDuration(s.oldestElapsedS),
      ].filter(Boolean).join(" · ");
    case "stalled":
      return s.reason;
    case "unloading":
      return "Stopping…";
  }
}

function isStale(c: Connection): boolean {
  return c.kind !== "connected" && c.kind !== "connecting";
}

class CardView {
  readonly el: HTMLElement;
  private readonly ring: Ring = createRing();
  private readonly title = h("div", { class: "card-title" });
  private readonly pill = h("span", { class: "pill" });
  private readonly detail = h("div", { class: "card-detail" });
  private readonly extras = h("div", { class: "card-extras" });
  private readonly seen = h("div", { class: "card-seen" });

  constructor(private readonly settings: Settings) {
    this.seen.hidden = true;
    this.el = h(
      "article",
      { class: "card" },
      h("div", { class: "card-head" }, this.ring.el, h("div", { class: "card-heading" }, this.title, this.pill)),
      this.detail,
      this.extras,
      this.seen,
    );
  }

  /** lastSeenMs is non-null only while the connection is stale or disconnected. */
  update(card: ModelCard, lastSeenMs: number | null) {
    const s = card.state;
    if (this.el.dataset.state !== s.kind) this.el.dataset.state = s.kind;
    this.title.textContent = card.name;
    this.title.title = card.description ? `${card.id} — ${card.description}` : card.id;
    this.ring.update(s, this.settings.thresholds.loadTimeoutS);
    this.pill.textContent = PILL[s.kind];
    this.detail.textContent = describe(card, this.settings);
    this.seen.hidden = lastSeenMs === null;
    this.seen.textContent = lastSeenMs === null ? "" : `Last seen ${formatAgo(lastSeenMs)}`;
    const extras: Node[] = [];
    if ((s.kind === "busy" || s.kind === "idle") && card.tokSHistory.length >= 2) {
      extras.push(renderSparkline(card.tokSHistory));
    }
    if (s.kind === "idle" && card.ttlS !== null && card.ttlRemainingS !== null) {
      extras.push(renderTtlBar(card.ttlRemainingS, card.ttlS));
    }
    this.extras.replaceChildren(...extras);
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
  const header = h(
    "header",
    { class: "header" },
    dot,
    h("div", { class: "header-main" }, h("div", { class: "header-line" }, statusText, live), h("div", { class: "header-line" }, host, meta)),
    gear,
  );

  const banner = h("div", { class: "banner" });
  banner.hidden = true;
  const openSettings = h("button", { type: "button" }, "Open settings");
  openSettings.addEventListener("click", onOpenSettings);

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
  const element = h("main", { class: "dashboard" }, header, banner, grid, stats);

  const cards = new Map<string, CardView>();
  let last: Snapshot | null = null;

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
        view = new CardView(settings);
        cards.set(m.id, view);
      }
      view.update(m, lastSeenMs);
      ids.add(m.id);
      ordered.push(view.el);
    }
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
    },
  };
}
