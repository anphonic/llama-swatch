import { getCurrentWindow } from "@tauri-apps/api/window";
import { setAlwaysOnTopSetting } from "../api";
import { h } from "../dom";
import type { ModelCard, Settings, Snapshot } from "../types";
import { bannerText, CONN_TEXT, PILL, pinIcon, type Dashboard } from "./dashboard";

const CYCLE_MS = 4000;
/** The strip has no visible menu, so its tooltip says how to reach one. */
const HINT = "Drag to move. Double-click or right-click for Full dashboard and Settings.";

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
  const pin = h("button", { type: "button", class: "mini-btn mini-pin" });
  pin.append(pinIcon());
  // Own titles, so the buttons and menu don't inherit the strip's hint.
  const min = h("button", { type: "button", class: "mini-btn", "aria-label": "Minimize", title: "Minimize" }, "–");
  const close = h("button", { type: "button", class: "mini-btn", "aria-label": "Close", title: "Close" }, "×");
  const menu = h(
    "div",
    { class: "mini-menu", role: "menu", hidden: "", title: "" },
    h("button", { type: "button", role: "menuitem", "data-act": "full" }, "Full dashboard"),
    h("button", { type: "button", role: "menuitem", "data-act": "settings" }, "Settings…"),
  );
  const strip = h(
    "div",
    { class: "mini", role: "group", tabindex: "0", "aria-label": "Llama Swatch Mini", "data-tauri-drag-region": "" },
    icon,
    name,
    dot,
    word,
    more,
    h("span", { class: "mini-gap", "data-tauri-drag-region": "" }),
    pin,
    min,
    close,
    menu,
  );

  // Shares the alwaysOnTop setting with the full dashboard's pin button.
  let onTop = settings.alwaysOnTop;
  function renderPin() {
    pin.setAttribute("aria-pressed", String(onTop));
    const label = onTop ? "Stop keeping on top" : "Keep on top";
    pin.title = label;
    pin.setAttribute("aria-label", label);
  }
  renderPin();
  pin.addEventListener("click", () => {
    onTop = !onTop;
    renderPin();
    // Some platforms (e.g. certain Wayland compositors) ignore or reject this; stay quiet.
    win.setAlwaysOnTop(onTop).catch((e) => console.warn("setAlwaysOnTop:", String(e)));
    setAlwaysOnTopSetting(onTop).catch((e) => console.warn("could not save pin state:", String(e)));
  });
  strip.title = HINT;
  min.addEventListener("click", () => void win.minimize());
  close.addEventListener("click", () => void win.close());
  /** What had focus when the menu opened, so Escape can give it back. */
  let returnFocus: HTMLElement = strip;
  const openMenu = () => {
    if (!menu.hidden) return;
    const active = document.activeElement;
    returnFocus = active instanceof HTMLElement && active !== document.body && !menu.contains(active) ? active : strip;
    menu.hidden = false;
    (menu.firstElementChild as HTMLElement).focus();
  };
  strip.addEventListener("dblclick", (e) => {
    if ((e.target as HTMLElement).closest(".mini-btn, .mini-menu")) return;
    e.preventDefault();
    openMenu();
  });
  // Always ours, never the WebView's default menu; the context-menu key on a focused button too.
  strip.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    if ((e.target as HTMLElement).closest(".mini-menu")) return;
    openMenu();
  });
  strip.addEventListener("keydown", (e) => {
    if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
      e.preventDefault();
      openMenu();
    }
  });
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
  // Clicking another app sends no mousedown here.
  const hideOnBlur = () => {
    menu.hidden = true;
  };
  window.addEventListener("blur", hideOnBlur);
  const onKey = (e: KeyboardEvent) => {
    if (e.key !== "Escape" || menu.hidden) return;
    menu.hidden = true;
    returnFocus.focus();
  };
  document.addEventListener("keydown", onKey);

  let last: Snapshot | null = null;
  let shownId: string | null = null;
  let timer: number | null = null;

  function show() {
    const s = last;
    if (!s) return;
    strip.dataset.conn = s.connection.kind;
    const tip = bannerText(s.connection, s.lastOkMs);
    strip.title = tip === null ? HINT : `${tip}

${HINT}`;
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
      name.textContent = s.connection.kind === "connecting" ? CONN_TEXT.connecting : "No model loaded";
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
      window.removeEventListener("blur", hideOnBlur);
      document.removeEventListener("keydown", onKey);
    },
  };
}
