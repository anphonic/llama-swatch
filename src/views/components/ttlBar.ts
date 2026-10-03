import { h } from "../../dom";
import { formatDuration } from "../../format";

export interface TtlBar {
  el: HTMLElement;
  /** Shows the bar at remaining/total; `null` hides it but keeps its space. */
  update(remainingS: number | null, totalS: number): void;
}

export function createTtlBar(): TtlBar {
  const fill = h("div", { class: "ttl-fill" });
  const el = h("div", { class: "ttl-track" }, fill);
  el.style.visibility = "hidden";
  return {
    el,
    update(remainingS, totalS) {
      if (remainingS === null) {
        if (el.style.visibility !== "hidden") el.style.visibility = "hidden";
        return;
      }
      const pct = totalS > 0 ? Math.max(0, Math.min(100, (remainingS / totalS) * 100)) : 0;
      fill.style.width = `${pct}%`;
      if (el.style.visibility !== "visible") el.style.visibility = "visible";
    },
  };
}

export function ttlLabel(remainingS: number): string {
  return `Unloads in ~${formatDuration(remainingS)} (estimate)`;
}
