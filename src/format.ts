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

/** Byte counts in powers of 1024: `512 B`, `14.2 KB`, `3.1 MB`. */
export function formatBytes(n: number): string {
  const units: [string, number][] = [["GB", 1024 ** 3], ["MB", 1024 ** 2], ["KB", 1024]];
  for (const [unit, size] of units) {
    // Compare after rounding so 1,048,575 B reads "1.0 MB", not "1024.0 KB".
    if (Math.round((n / size) * 10) >= 10) return `${(n / size).toFixed(1)} ${unit}`;
  }
  return `${n} B`;
}

/** Share of a prompt served from cache, rounded down so it reads 100% only when nothing was new. */
export function cachedPercent(cached: number, fresh: number): number {
  const total = cached + fresh;
  return total > 0 ? Math.floor((cached / total) * 100) : 0;
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
