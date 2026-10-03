import { getActivity } from "../api";
import { h } from "../dom";
import { formatMs } from "../format";
import { isStream, markSwaps } from "../history";
import type { ActivityRow } from "../types";

export interface History {
  element: HTMLElement;
  /** Fetches now and every 5 s until `hide`. */
  show(): void;
  hide(): void;
  destroy(): void;
}

const LIMIT = 200;
const REFRESH_MS = 5000;
const ALL = "";

// Widths in px; Model (width 0) takes the rest. Fixed so a refresh never re-flows the columns.
const COLUMNS: [string, boolean, number][] = [
  ["Time", false, 84], ["Model", false, 0], ["Mode", false, 68], ["Status", false, 64],
  ["Input", true, 76], ["Cached", true, 76], ["Output", true, 76], ["tok/s", true, 68], ["Duration", true, 84],
];

const pad = (n: number) => String(n).padStart(2, "0");

function formatTime(ms: number | null): string {
  if (ms === null) return "—";
  const d = new Date(ms);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

const num = (n: number) => n.toLocaleString("en-US");

function cell(text: string | Node, numeric: boolean): HTMLElement {
  return h("td", numeric ? { class: "num" } : {}, text);
}

export function createHistory(): History {
  let rows: ActivityRow[] = [];
  let filter = ALL;
  let swapsOnly = false;
  let error: string | null = null;
  let timer: number | null = null;
  let gen = 0; // bumped on hide/destroy so a late response is dropped

  const select = h("select", { "aria-label": "Filter by model" });
  const swapsBox = h("input", { type: "checkbox", id: "swaps-only" });
  const noteText = `${LIMIT} most recent · refreshes every ${REFRESH_MS / 1000} s`;
  const note = h("span", { class: "history-note muted" }, noteText);
  const toolbar = h(
    "div",
    { class: "history-toolbar" },
    select,
    h("label", { class: "toggle", for: "swaps-only" }, swapsBox, h("span", {}, "Swaps only")),
    note,
  );
  const tbody = h("tbody");
  const cols = h("colgroup", {}, ...COLUMNS.map(([, , w]) => { const col = h("col"); if (w) col.style.width = `${w}px`; return col; }));
  const table = h("table", { class: "history-table" }, cols, h("thead", {}, h("tr", {}, ...COLUMNS.map(([name, numeric]) => h("th", numeric ? { class: "num" } : {}, name)))), tbody);
  const element = h("section", { class: "history" }, toolbar, h("div", { class: "table-scroll" }, table));

  // Rebuilding an open <select> closes its popup, so only touch it when the model set changed.
  let optionKey = "\u0000";
  function renderSelect() {
    const models = [...new Set(rows.map((r) => r.model))].sort();
    if (filter !== ALL && !models.includes(filter)) models.push(filter);
    const key = models.join("\n");
    if (key !== optionKey) {
      optionKey = key;
      select.replaceChildren(h("option", { value: ALL }, "All models"), ...models.map((m) => h("option", { value: m }, m)));
    }
    select.value = filter;
  }

  function render() {
    renderSelect();
    // Swaps are judged on the full list, then filtered, so a filter never changes what counts as a swap.
    const shown = markSwaps(rows).filter((m) => (filter === ALL || m.row.model === filter) && (!swapsOnly || m.swap));
    // Messages live in the table body (and errors in the toolbar note) rather than in extra
    // elements, so nothing above the table appears or disappears.
    const text = error && !rows.length ? `Could not load history: ${error}`
      : !rows.length ? "No requests recorded yet."
      : !shown.length ? "No rows match the filter."
      : null;
    note.textContent = error && rows.length ? `Could not refresh: ${error}` : noteText;
    note.title = note.textContent;
    note.classList.toggle("history-error", !!error && rows.length > 0);
    if (text !== null) {
      tbody.replaceChildren(h("tr", {}, h("td", { colspan: String(COLUMNS.length), class: "muted history-empty" }, text)));
      return;
    }
    tbody.replaceChildren(
      ...shown.map(({ row, swap }) => {
        const tr = h("tr", { title: `${row.reqPath} · id ${row.id}` });
        if (swap) tr.classList.add("swap");
        const tps = row.tokensPerSecond > 0 ? row.tokensPerSecond.toFixed(1) : "—";
        tr.append(
          cell(formatTime(row.timestampMs), false),
          cell(swap ? h("span", {}, h("span", { class: "swap-tag" }, "⇄ swap"), row.model) : row.model, false),
          cell(isStream(row) ? "stream" : "json", false),
          cell(String(row.status), false),
          cell(num(row.inputTokens), true),
          cell(num(row.cacheTokens), true),
          cell(num(row.outputTokens), true),
          cell(tps, true),
          cell(formatMs(row.durationMs), true),
        );
        return tr;
      }),
    );
  }

  async function refresh() {
    const mine = gen;
    try {
      const fetched = await getActivity(LIMIT);
      if (mine !== gen) return;
      // Newest first by id, whatever order the server used (monitor.rs sorts by id the same way).
      rows = [...fetched].sort((a, b) => b.id - a.id);
      error = null;
    } catch (e) {
      if (mine !== gen) return;
      error = String(e);
    }
    render();
  }

  select.addEventListener("change", () => {
    filter = select.value;
    render();
  });
  swapsBox.addEventListener("change", () => {
    swapsOnly = swapsBox.checked;
    render();
  });

  function stop() {
    gen++;
    if (timer !== null) window.clearInterval(timer);
    timer = null;
  }

  return {
    element,
    show() {
      stop();
      void refresh();
      timer = window.setInterval(() => void refresh(), REFRESH_MS);
    },
    hide: stop,
    destroy: stop,
  };
}
