import { svg } from "../../dom";
import type { Histogram } from "../../types";

export function renderHistogram(hist: Histogram, width = 360, height = 120): SVGSVGElement {
  const el = svg("svg", { class: "histogram", viewBox: `0 0 ${width} ${height}`, width: "100%", height, role: "img", "aria-label": `p50 ${hist.p50.toFixed(1)}, p95 ${hist.p95.toFixed(1)}` });
  if (!hist.bins.length) return el;
  const plotH = height - 18;
  const maxCount = Math.max(1, ...hist.bins);
  const barW = width / hist.bins.length;
  hist.bins.forEach((count, i) => {
    const bh = (count / maxCount) * plotH;
    el.append(svg("rect", {
      class: "hist-bar",
      x: (i * barW + 1).toFixed(1),
      y: (plotH - bh).toFixed(1),
      width: Math.max(1, barW - 2).toFixed(1),
      height: bh.toFixed(1),
    }));
  });
  const range = hist.max - hist.min || 1;
  const marks: [string, number][] = [["p50", hist.p50], ["p95", hist.p95]];
  for (const [name, value] of marks) {
    const x = Math.max(0, Math.min(width, ((value - hist.min) / range) * width));
    el.append(
      svg("line", { class: "hist-mark", x1: x.toFixed(1), x2: x.toFixed(1), y1: 0, y2: plotH }),
      svg("text", { class: "hist-label", x: x.toFixed(1), y: height - 4, "text-anchor": "middle" }, `${name} ${value.toFixed(1)}`),
    );
  }
  return el;
}
