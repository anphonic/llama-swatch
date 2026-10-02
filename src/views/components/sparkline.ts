import { svg } from "../../dom";

export function renderSparkline(values: number[], width = 220, height = 32): SVGSVGElement {
  const el = svg("svg", { class: "sparkline", viewBox: `0 0 ${width} ${height}`, width: "100%", height, "aria-hidden": "true", preserveAspectRatio: "none" });
  if (values.length < 2) return el;
  const max = Math.max(...values);
  const min = Math.min(...values);
  const span = max - min || 1;
  const step = width / (values.length - 1);
  const pts = values.map((v, i) => `${(i * step).toFixed(1)},${(height - 2 - ((v - min) / span) * (height - 4)).toFixed(1)}`);
  el.append(
    svg("polygon", { class: "sparkline-fill", points: `0,${height} ${pts.join(" ")} ${width},${height}` }),
    svg("polyline", { class: "sparkline-line", points: pts.join(" ") }),
  );
  return el;
}
