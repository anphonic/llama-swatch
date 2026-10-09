import type { ModelCard } from "./types";

/** A model counts as loaded in every state except "notLoaded" (loading and unloading included). */
export function isLoaded(card: ModelCard): boolean {
  return card.state.kind !== "notLoaded";
}

export function countLoaded(models: readonly ModelCard[]): number {
  return models.filter(isLoaded).length;
}

/**
 * The model to offer for reloading: the most recently used one, but only while nothing at all is
 * loaded. "Used" comes from llama-swap's own request history, so it survives our restarts and is
 * forgotten when llama-swap restarts.
 */
export function reloadCandidate(models: readonly ModelCard[]): ModelCard | null {
  if (models.some(isLoaded)) return null;
  let best: ModelCard | null = null;
  for (const m of models) {
    if (m.lastRequestAtMs !== null && (best === null || m.lastRequestAtMs > (best.lastRequestAtMs ?? 0))) best = m;
  }
  return best;
}
