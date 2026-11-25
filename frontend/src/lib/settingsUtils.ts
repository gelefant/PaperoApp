import type { UiSettingsPayload } from './types';

/**
 * Safely coerces a numeric input value (which Svelte 5 `<input type="number" bind:value>`
 * turns into `null` when temporarily cleared) into a clamped integer with a guaranteed fallback.
 */
export function coerceClampedInt(
  raw: unknown,
  min: number,
  max: number,
  fallback: number
): number {
  const safeFallback = Math.min(max, Math.max(min, Math.round(Number.isFinite(fallback) ? fallback : min)));
  if (raw === null || raw === undefined) return safeFallback;
  if (typeof raw === 'string' && raw.trim() === '') return safeFallback;
  const parsed = Number(raw);
  if (!Number.isFinite(parsed)) return safeFallback;
  return Math.min(max, Math.max(min, Math.round(parsed)));
}

export function coerceClampedFloat(
  raw: unknown,
  min: number,
  max: number,
  fallback: number
): number {
  const safeFallback = Math.min(max, Math.max(min, Number.isFinite(fallback) ? fallback : min));
  if (raw === null || raw === undefined) return safeFallback;
  if (typeof raw === 'string' && raw.trim() === '') return safeFallback;
  const parsed = Number(raw);
  if (!Number.isFinite(parsed)) return safeFallback;
  return Math.min(max, Math.max(min, parsed));
}

export function sanitizeUiSettings(
  draft: UiSettingsPayload,
  fallback: UiSettingsPayload
): UiSettingsPayload {
  return {
    ...draft,
    max_worker_threads: coerceClampedInt(
      draft.max_worker_threads,
      1,
      32,
      fallback.max_worker_threads || 4
    ),
    cache_limit_mb: coerceClampedInt(draft.cache_limit_mb, 50, 4096, fallback.cache_limit_mb || 500),
    model_idle_unload_secs: coerceClampedInt(
      draft.model_idle_unload_secs,
      30,
      3600,
      fallback.model_idle_unload_secs || 180
    ),
    hybrid_top_n: coerceClampedInt(draft.hybrid_top_n, 10, 200, fallback.hybrid_top_n || 50),
    rerank_top_m_light: coerceClampedInt(
      draft.rerank_top_m_light,
      5,
      100,
      fallback.rerank_top_m_light || 25
    ),
    rerank_top_m_quality: coerceClampedInt(
      draft.rerank_top_m_quality,
      4,
      60,
      fallback.rerank_top_m_quality || 12
    ),
    search_top_k_docs: coerceClampedInt(
      draft.search_top_k_docs,
      0,
      200,
      fallback.search_top_k_docs ?? 30
    ),
    search_min_score: coerceClampedFloat(
      draft.search_min_score,
      0,
      1,
      fallback.search_min_score ?? 0
    ),
    ocr_dpi: coerceClampedInt(draft.ocr_dpi, 150, 350, fallback.ocr_dpi || 250),
    ocr_front_pages_only: Boolean(draft.ocr_front_pages_only),
    ocr_front_pages_count: coerceClampedInt(
      draft.ocr_front_pages_count,
      1,
      50,
      fallback.ocr_front_pages_count ?? 8
    ),
    check_updates_on_startup: draft.check_updates_on_startup ?? fallback.check_updates_on_startup ?? true,
    llm_max_passages: coerceClampedInt(
      draft.llm_max_passages,
      1,
      40,
      fallback.llm_max_passages || 8
    ),
    llm_max_input_tokens: coerceClampedInt(
      draft.llm_max_input_tokens,
      256,
      64000,
      fallback.llm_max_input_tokens || 3500
    ),
    llm_max_output_tokens: coerceClampedInt(
      draft.llm_max_output_tokens,
      128,
      8192,
      fallback.llm_max_output_tokens || 1024
    ),
  };
}
