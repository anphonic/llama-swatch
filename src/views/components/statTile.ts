import { h } from "../../dom";

export interface StatTile {
  el: HTMLElement;
  /**
   * `hint` is extra detail: a tooltip on hover, and for the keyboard the tile becomes focusable
   * and shows it over the tile while focused (screen readers get it as the description).
   * Empty removes it.
   */
  update(value: string, sub?: string, hint?: string): void;
}

let nextId = 0;

export function createStatTile(label: string): StatTile {
  const value = h("div", { class: "tile-value" }, "—");
  const sub = h("div", { class: "tile-sub" });
  const hintEl = h("div", { class: "tile-hint", id: `tile-hint-${nextId++}` });
  const el = h("div", { class: "tile" }, h("div", { class: "tile-label" }, label), value, sub, hintEl);
  return {
    el,
    update(v, s = "", hint = "") {
      value.textContent = v;
      sub.textContent = s;
      if (el.title === hint) return;
      el.title = hint;
      hintEl.textContent = hint;
      if (hint) {
        el.tabIndex = 0;
        el.setAttribute("aria-describedby", hintEl.id);
      } else {
        el.removeAttribute("tabindex");
        el.removeAttribute("aria-describedby");
      }
    },
  };
}
