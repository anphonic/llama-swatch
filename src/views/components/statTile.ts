import { h } from "../../dom";

export interface StatTile {
  el: HTMLElement;
  update(value: string, sub?: string): void;
}

export function createStatTile(label: string): StatTile {
  const value = h("div", { class: "tile-value" }, "—");
  const sub = h("div", { class: "tile-sub" });
  const el = h("div", { class: "tile" }, h("div", { class: "tile-label" }, label), value, sub);
  return {
    el,
    update(v, s = "") {
      value.textContent = v;
      sub.textContent = s;
    },
  };
}
