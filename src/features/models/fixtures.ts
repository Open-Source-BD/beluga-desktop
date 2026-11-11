import type { ModelInfo } from "../../lib/ipc";

/** Test fixture builder: a paid, tool-capable model unless overridden. */
export function model(over: Partial<ModelInfo> & { id: string }): ModelInfo {
  return {
    name: over.id,
    prompt_usd_per_mtok: 0.5,
    completion_usd_per_mtok: 1.5,
    is_free: false,
    supports_tools: true,
    accepts_images: false,
    context_length: 8000,
    ...over,
  };
}
