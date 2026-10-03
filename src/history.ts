import type { ActivityRow } from "./types";

export interface MarkedRow {
  row: ActivityRow;
  swap: boolean;
}

/**
 * Rows are newest first. A row is a swap when its model differs from the one before it in time
 * (the next row down) and it reused no cache; the oldest row has no predecessor, so never a swap.
 */
export function markSwaps(rows: readonly ActivityRow[]): MarkedRow[] {
  return rows.map((row, i) => {
    const prev = rows[i + 1];
    return { row, swap: prev !== undefined && prev.model !== row.model && row.cacheTokens === 0 };
  });
}

export function isStream(row: ActivityRow): boolean {
  return row.contentType.toLowerCase().startsWith("text/event-stream");
}
