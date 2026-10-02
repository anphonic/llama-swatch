import { svg } from "../../dom";
import type { ModelState } from "../../types";

const R = 26;
const C = 2 * Math.PI * R;

export interface Ring {
  el: SVGSVGElement;
  update(state: ModelState, loadTimeoutS: number): void;
}

export function createRing(): Ring {
  const track = svg("circle", { class: "ring-track", cx: 32, cy: 32, r: R });
  const arc = svg("circle", { class: "ring-arc", cx: 32, cy: 32, r: R, "stroke-dasharray": `${C} ${C}` });
  const label = svg("text", { class: "ring-label", x: 32, y: 32, "text-anchor": "middle", "dominant-baseline": "central" });
  const el = svg("svg", { class: "ring", viewBox: "0 0 64 64", width: 64, height: 64, role: "img" }, track, arc, label);

  return {
    el,
    update(state, loadTimeoutS) {
      let fraction = 1;
      let text = "";
      switch (state.kind) {
        case "loading":
          fraction = Math.min(1, state.elapsedS / Math.max(1, loadTimeoutS));
          text = `${state.elapsedS}s`;
          break;
        case "busy":
          fraction = 0.3;
          text = String(state.requests);
          break;
        case "stalled":
          text = "!";
          break;
        case "notLoaded":
        case "idle":
        case "unloading":
          break;
      }
      // Only touch attributes that changed, so running CSS animations are not disturbed.
      if (el.dataset.state !== state.kind) el.dataset.state = state.kind;
      arc.setAttribute("stroke-dasharray", `${(C * fraction).toFixed(1)} ${C.toFixed(1)}`);
      if (label.textContent !== text) label.textContent = text;
      el.setAttribute("aria-label", state.kind);
    },
  };
}
