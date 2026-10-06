// Mirrors the Rust types serialized in monitor.rs, state.rs, client.rs, config.rs, commands.rs.

export type ModelState =
  | { kind: "notLoaded" }
  | { kind: "loading"; elapsedS: number; slow: boolean }
  | { kind: "idle"; uptimeS: number }
  /** Ready per /running, but busy or idle is unknown: live events are not being read. */
  | { kind: "loaded" }
  | {
      kind: "busy";
      requests: number;
      oldestElapsedS: number;
      /** Requests that have produced output. */
      streaming: number;
      /** Requests whose reply has started (headers sent) but has no output yet. */
      waitingFirstToken: number;
      /** Requests with no reply yet; a non-streaming or embeddings request looks like this until done. */
      awaitingReply: number;
      /** Seconds since any streaming request last produced output; null while nothing streams. */
      lastOutputS: number | null;
      /** Hint: a request has waited for its first token over 10x the first-byte timeout while others stream. */
      firstTokenLong: boolean;
      /** Hint: a request has had no reply for longer than the first-byte timeout. Never Stalled. */
      awaitingLong: boolean;
    }
  | { kind: "stalled"; reason: string }
  | { kind: "unloading" };

export type Connection =
  | { kind: "connecting" }
  | { kind: "connected"; latencyMs: number }
  | { kind: "unauthorized" }
  | { kind: "unreachable"; message: string }
  | { kind: "error"; message: string };

/** Health of llama-swap's `/api/events` stream (events.rs `EventStream`). */
export type EventStream = "offline" | "connected" | "live" | "unreadable";

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
  /** Set when the reported version is outside the range this monitor was tested with. */
  versionNote: string | null;
  models: ModelCard[];
  stats: StatsSummary | null;
  statsAvailable: boolean;
  eventStream: EventStream;
  /** An event on this connection failed to decode since the last one that decoded. */
  eventReadFailed: boolean;
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
  alwaysOnTop: boolean;
  checkForUpdates: boolean;
}

/** A newer release on GitHub (update.rs `UpdateInfo`). */
export interface UpdateInfo {
  /** e.g. "v0.2.0" */
  tag: string;
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
