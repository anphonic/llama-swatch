import { h } from "../../dom";

export interface StatTile {
  el: HTMLElement;
  /** `hint` becomes the tile's tooltip; empty removes it. */
  update(value: string, sub?: string, hint?: string): void;
}

export function createStatTile(label: string): StatTile {
  const value = h("div", { class: "tile-value" }, "—");
  const sub = h("div", { class: "tile-sub" });
  const el = h("div", { class: "tile" }, h("div", { class: "tile-label" }, label), value, sub);
  return {
    el,
    update(v, s = "", hint = "") {
      value.textContent = v;
      sub.textContent = s;
      if (el.title !== hint) el.title = hint;
    },
  };
}
