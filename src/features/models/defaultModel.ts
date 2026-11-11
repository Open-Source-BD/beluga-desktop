import type { ModelInfo } from "../../lib/ipc";

function byContextThenId(a: ModelInfo, b: ModelInfo): number {
  return b.context_length - a.context_length || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
}

/**
 * Default for this plan: the free, tool-capable model with the largest
 * context (ties: lowest id); otherwise the largest-context free model;
 * otherwise null.
 */
export function pickDefaultModel(models: ModelInfo[]): ModelInfo | null {
  const free = models.filter((m) => m.is_free);
  const pool = free.some((m) => m.supports_tools) ? free.filter((m) => m.supports_tools) : free;
  return [...pool].sort(byContextThenId)[0] ?? null;
}

/** The first candidate id present in the list, else the default's id, else null. */
export function chooseModel(models: ModelInfo[], candidates: (string | null | undefined)[]): string | null {
  for (const id of candidates) {
    if (id && models.some((m) => m.id === id)) return id;
  }
  return pickDefaultModel(models)?.id ?? null;
}

function dollars(n: number): string {
  return `$${n > 0 && n < 0.01 ? n.toFixed(3) : n.toFixed(2)}`;
}

export function formatPrice(m: ModelInfo): string {
  if (m.is_free) return "Free";
  return `${dollars(m.prompt_usd_per_mtok)} in, ${dollars(m.completion_usd_per_mtok)} out per million tokens`;
}
