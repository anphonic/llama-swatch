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

const COLUMNS: [string, boolean][] = [
  ["Time", false], ["Model", false], ["Mode", false], ["Status", false],
  ["Input", true], ["Cached", true], ["Output", true], ["tok/s", true], ["Duration", true],
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
  const toolbar = h(
    "div",
    { class: "history-toolbar" },
    select,
    h("label", { class: "toggle", for: "swaps-only" }, swapsBox, h("span", {}, "Swaps only")),
    h("span", { class: "history-note muted" }, `${LIMIT} most recent · refreshes every ${REFRESH_MS / 1000} s`),
  );
  const message = h("p", { class: "muted history-message" });
  message.hidden = true;
  const tbody = h("tbody");
  const table = h("table", { class: "history-table" }, h("thead", {}, h("tr", {}, ...COLUMNS.map(([name, numeric]) => h("th", numeric ? { class: "num" } : {}, name)))), tbody);
  const element = h("section", { class: "history" }, toolbar, message, h("div", { class: "table-scroll" }, table));

  function renderSelect() {
    const models = [...new Set(rows.map((r) => r.model))].sort();
    if (filter !== ALL && !models.includes(filter)) models.push(filter);
    select.replaceChildren(h("option", { value: ALL }, "All models"), ...models.map((m) => h("option", { value: m }, m)));
    select.value = filter;
  }

  function render() {
    renderSelect();
    // Swaps are judged on the full list, then filtered, so a filter never changes what counts as a swap.
    const shown = markSwaps(rows).filter((m) => (filter === ALL || m.row.model === filter) && (!swapsOnly || m.swap));
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
    if (error) message.textContent = `Could not load history: ${error}`;
    else if (!rows.length) message.textContent = "No requests recorded yet.";
    else if (!shown.length) message.textContent = "No rows match the filter.";
    message.hidden = !error && !!shown.length;
  }

  async function refresh() {
    const mine = gen;
    try {
      const fetched = await getActivity(LIMIT);
      if (mine !== gen) return;
      rows = fetched;
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
