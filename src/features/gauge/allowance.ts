import type { ModelInfo } from "../../lib/ipc";

/**
 * OpenRouter's limits page describes the free-model counter as "recorded so far
 * in the current UTC day", so the next reset is the coming 00:00 UTC. The page
 * does not state the reset time outright; this is inferred.
 */
export function nextReset(now: Date): Date {
  return new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate() + 1));
}

/** Hours and minutes in the user's locale (and time zone, unless given). */
export function formatResetTime(at: Date, locale?: string, timeZone?: string): string {
  return at.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit", timeZone });
}

export function resetTimeText(now: Date = new Date()): string {
  return formatResetTime(nextReset(now));
}

export function formatMoney(n: number): string {
  return n > 0 && n < 0.01 ? "under $0.01" : `$${n.toFixed(2)}`;
}

/** The three cheapest paid tool-capable models, cheapest first (ties: id). */
export function cheapestPaidTools(models: ModelInfo[]): ModelInfo[] {
  const price = (m: ModelInfo) => m.prompt_usd_per_mtok + m.completion_usd_per_mtok;
  return models
    .filter((m) => !m.is_free && m.supports_tools)
    .sort((a, b) => price(a) - price(b) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
    .slice(0, 3);
}
