export function formatDuration(totalSeconds: number): string {
  const s = Math.max(0, Math.round(totalSeconds));
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return s % 60 ? `${m} m ${s % 60} s` : `${m} m`;
  const hr = Math.floor(m / 60);
  if (hr < 24) return m % 60 ? `${hr} h ${m % 60} m` : `${hr} h`;
  const d = Math.floor(hr / 24);
  return hr % 24 ? `${d} d ${hr % 24} h` : `${d} d`;
}

export function formatCount(n: number): string {
  const units: [string, number][] = [["B", 1e9], ["M", 1e6], ["k", 1e3]];
  for (const [unit, size] of units) {
    if (n >= size) {
      const x = n / size;
      return `${x >= 100 ? x.toFixed(0) : x.toFixed(1)}${unit}`;
    }
  }
  return String(n);
}

export function formatAgo(ms: number, now: number = Date.now()): string {
  return `${formatDuration((now - ms) / 1000)} ago`;
}

/** Request durations: milliseconds under a second, one decimal under ten, then `formatDuration`. */
export function formatMs(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 10_000) return `${(ms / 1000).toFixed(1)} s`;
  return formatDuration(ms / 1000);
}
