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
  const id = `tile-${nextId++}`;
  const value = h("div", { class: "tile-value", id: `${id}-value` }, "—");
  const sub = h("div", { class: "tile-sub", id: `${id}-sub` });
  const hintEl = h("div", { class: "tile-hint", id: `${id}-hint` });
  const labelEl = h("div", { class: "tile-label", id: `${id}-label` }, label);
  // Named by its own text, so a focused tile reads "Last input, 812, 0 cached" rather than the tooltip.
  const el = h(
    "div",
    { class: "tile", role: "group", "aria-labelledby": `${labelEl.id} ${value.id} ${sub.id}` },
    labelEl,
    value,
    sub,
    hintEl,
  );
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
