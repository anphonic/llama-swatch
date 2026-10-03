import { h } from "../../dom";
import { formatDuration } from "../../format";

export function renderTtlBar(remainingS: number, totalS: number): HTMLElement {
  const pct = totalS > 0 ? Math.max(0, Math.min(100, (remainingS / totalS) * 100)) : 0;
  const fill = h("div", { class: "ttl-fill" });
  fill.style.width = `${pct}%`;
  return h(
    "div",
    { class: "ttl" },
    h("div", { class: "ttl-track" }, fill),
    h("span", { class: "ttl-text" }, `Unloads in ~${formatDuration(remainingS)} (estimate)`),
  );
}
