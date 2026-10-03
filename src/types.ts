// Mirrors the Rust types serialized in monitor.rs, state.rs, client.rs, config.rs, commands.rs.

export type ModelState =
  | { kind: "notLoaded" }
  | { kind: "loading"; elapsedS: number }
  | { kind: "idle"; uptimeS: number }
  | { kind: "busy"; requests: number; oldestElapsedS: number }
  | { kind: "stalled"; reason: string }
  | { kind: "unloading" };

export type Connection =
  | { kind: "connecting" }
  | { kind: "connected"; latencyMs: number }
  | { kind: "unauthorized" }
  | { kind: "unreachable"; message: string }
  | { kind: "error"; message: string };

export interface ModelCard {
  id: string;
  name: string;
  description: string;
  state: ModelState;
  ttlS: number | null;
  ttlRemainingS: number | null;
  tokSHistory: number[];
  lastRequestAtMs: number | null;
}

export interface Histogram {
  bins: number[];
  min: number;
  max: number;
  binSize: number;
  p50: number;
  p95: number;
  p99: number;
}

export interface StatsSummary {
  totalRequests: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCacheTokens: number;
  gen: Histogram | null;
  prompt: Histogram | null;
}

export interface Snapshot {
  host: string;
  connection: Connection;
  lastOkMs: number | null;
  version: string | null;
  models: ModelCard[];
  stats: StatsSummary | null;
  statsAvailable: boolean;
  liveEvents: boolean;
}

export interface Thresholds {
  loadTimeoutS: number;
  stopTimeoutS: number;
  firstByteTimeoutS: number;
  streamStallTimeoutS: number;
}

export interface Settings {
  baseUrl: string;
  pollIntervalMs: number;
  thresholds: Thresholds;
}

export interface SettingsView {
  configured: boolean;
  settings: Settings;
  hasKey: boolean;
}

export type TestResult =
  | { kind: "ok"; version: string }
  | { kind: "unreachable"; message: string }
  | { kind: "redirect"; message: string }
  | { kind: "unauthorized" }
  | { kind: "notLlamaSwap"; message: string };

/** One History row, serialized from api.rs `ActivityRow`. Newest first as returned by llama-swap. */
export interface ActivityRow {
  id: number;
  timestampMs: number | null;
  model: string;
  reqPath: string;
  contentType: string;
  status: number;
  durationMs: number;
  inputTokens: number;
  cacheTokens: number;
  outputTokens: number;
  tokensPerSecond: number;
}
