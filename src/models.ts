import type { ModelCard } from "./types";

/** A model counts as loaded in every state except "notLoaded" (loading and unloading included). */
export function isLoaded(card: ModelCard): boolean {
  return card.state.kind !== "notLoaded";
}

export function countLoaded(models: readonly ModelCard[]): number {
  return models.filter(isLoaded).length;
}
