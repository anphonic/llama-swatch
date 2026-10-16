import { getCurrentWindow } from "@tauri-apps/api/window";
import { h } from "../dom";
import type { ModelCard, Settings, Snapshot } from "../types";
import { CONN_TEXT, PILL, type Dashboard } from "./dashboard";

const CYCLE_MS = 4000;

export interface MiniActions {
  openSettings(): void;
  fullDashboard(): void;
}

/** Busy + streaming reads "Streaming", as on the cards' detail line. */
function stateWord(c: ModelCard): string {
  return c.state.kind === "busy" && c.state.streaming > 0 ? "Streaming" : PILL[c.state.kind];
}

export function createMini(settings: Settings, actions: MiniActions): Dashboard {
  const win = getCurrentWindow();
  const icon = h("img", { class: "mini-icon", src: "/icon.png", alt: "", "data-tauri-drag-region": "" });
  const name = h("span", { class: "mini-name", "data-tauri-drag-region": "" }, "Connecting…");
  const dot = h("span", { class: "dot mini-dot", "data-tauri-drag-region": "" });
  const word = h("span", { class: "mini-word", "data-tauri-drag-region": "" });
  const more = h("span", { class: "mini-more", "data-tauri-drag-region": "" });
  const min = h("button", { type: "button", class: "mini-btn", "aria-label": "Minimize" }, "–");
  const close = h("button", { type: "button", class: "mini-btn", "aria-label": "Close" }, "×");
  const menu = h(
    "div",
    { class: "mini-menu", role: "menu", hidden: "" },
    h("button", { type: "button", role: "menuitem", "data-act": "full" }, "Full dashboard"),
    h("button", { type: "button", role: "menuitem", "data-act": "settings" }, "Settings…"),
  );
  const strip = h(
    "div",
    { class: "mini", "data-tauri-drag-region": "" },
    icon,
    name,
    dot,
    word,
    more,
    h("span", { class: "mini-gap", "data-tauri-drag-region": "" }),
    min,
    close,
    menu,
  );

  min.addEventListener("click", () => void win.minimize());
  close.addEventListener("click", () => void win.close());
  const openMenu = (e: MouseEvent) => {
    if ((e.target as HTMLElement).closest(".mini-btn, .mini-menu")) return;
    e.preventDefault();
    menu.hidden = false;
    (menu.firstElementChild as HTMLElement).focus();
  };
  strip.addEventListener("dblclick", openMenu);
  strip.addEventListener("contextmenu", openMenu);
  menu.addEventListener("click", (e) => {
    const act = (e.target as HTMLElement).dataset.act;
    menu.hidden = true;
    if (act === "full") actions.fullDashboard();
    if (act === "settings") actions.openSettings();
  });
  const hideMenu = (e: Event) => {
    if (!menu.contains(e.target as Node)) menu.hidden = true;
  };
  document.addEventListener("mousedown", hideMenu);
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") menu.hidden = true;
  };
  document.addEventListener("keydown", onKey);

  let last: Snapshot | null = null;
  let shownId: string | null = null;
  let timer: number | null = null;

  function show() {
    const s = last;
    if (!s) return;
    strip.dataset.conn = s.connection.kind;
    if (s.connection.kind !== "connected" && s.connection.kind !== "connecting") {
      name.textContent = CONN_TEXT[s.connection.kind];
      word.textContent = "";
      more.textContent = "";
      more.removeAttribute("title");
      strip.removeAttribute("data-state");
      return;
    }
    const order = s.miniOrder;
    if (order.length === 0) {
      name.textContent = "No model loaded";
      word.textContent = more.textContent = "";
      more.removeAttribute("title");
      strip.removeAttribute("data-state");
      return;
    }
    const byId = new Map(s.models.map((m) => [m.id, m]));
    // Stalled always wins; otherwise keep the cycled model while it is still loaded.
    const pinned = byId.get(order[0])?.state.kind === "stalled";
    if (!settings.miniCycle || pinned || !shownId || !order.includes(shownId)) shownId = order[0];
    const card = byId.get(shownId);
    if (!card) return;
    name.textContent = card.name || card.id;
    word.textContent = stateWord(card);
    strip.dataset.state = card.state.kind;
    const others = order.filter((id) => id !== shownId);
    more.textContent = others.length ? `+${others.length}` : "";
    more.title = others.map((id) => byId.get(id)?.name || id).join("\n");
  }

  if (settings.miniCycle) {
    timer = window.setInterval(() => {
      const order = last?.miniOrder ?? [];
      if (order.length < 2 || !shownId) return;
      shownId = order[(order.indexOf(shownId) + 1) % order.length];
      show();
    }, CYCLE_MS);
  }

  return {
    element: strip,
    update(s) {
      last = s;
      show();
    },
    showUpdate() {
      // No room on the strip; the full dashboard shows it.
    },
    destroy() {
      if (timer !== null) clearInterval(timer);
      document.removeEventListener("mousedown", hideMenu);
      document.removeEventListener("keydown", onKey);
    },
  };
}
