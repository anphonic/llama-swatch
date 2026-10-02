import { h } from "../dom";
import type { Settings, Snapshot } from "../types";

export interface Dashboard {
  element: HTMLElement;
  update(s: Snapshot): void;
  destroy(): void;
}

export function createDashboard(settings: Settings, onOpenSettings: () => void): Dashboard {
  const pre = h("pre", {});
  const gear = h("button", { type: "button" }, "Settings");
  gear.addEventListener("click", onOpenSettings);
  const element = h("main", { class: "dashboard" }, h("div", {}, settings.baseUrl, " ", gear), pre);
  return {
    element,
    update(s) {
      pre.textContent = JSON.stringify(s, null, 2);
    },
    destroy() {},
  };
}
