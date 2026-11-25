import { describe, expect, it } from 'vitest';
import { coerceClampedInt, sanitizeUiSettings } from './settingsUtils';
import { APP_VERSION } from './appVersion';
import pkg from '../../package.json';
import type { UiSettingsPayload } from './types';

function makeBaseSettings(): UiSettingsPayload {
  return {
    viewer_open_in_separate_window: false,
    external_pdf_command: null,
    external_djvu_command: null,
    linux_reveal_mode: 'dbus_with_fallback',
    max_worker_threads: 6,
    cache_limit_mb: 500,
    polite_email: null,
    network_lookups_enabled: true,
    mrlookup_enabled_single: true,
    mrlookup_enabled_bulk: true,
    zbmath_enabled: true,
    crossref_enabled: true,
    arxiv_enabled: true,
    harvard_enabled: true,
    loc_enabled: true,
    googlebooks_enabled: true,
    openlibrary_enabled: true,
    dblp_enabled: true,
    openalex_enabled: true,
    datacite_enabled: true,
    inspirehep_enabled: true,
    europepmc_enabled: false,
    semanticscholar_enabled: true,
    metadata_source_priority: ['mrlookup', 'zbmath'],
    ml_profile: 'quality',
    reranker_enabled: true,
    model_idle_unload_secs: 180,
    hybrid_top_n: 50,
    rerank_top_m_light: 25,
    rerank_top_m_quality: 12,
    search_top_k_docs: 30,
    search_min_score: 0,
    doc_score_aggregation: 'max',
    ui_language: 'it',
    ui_theme: 'system',
    first_run_completed: true,
    fs_watcher_enabled: true,
    tesseract_bin: null,
    ocr_languages: null,
    ocr_dpi: 250,
    llm_provider: 'disabled',
    llm_model: null,
    llm_ollama_url: 'http://127.0.0.1:11434',
    llm_max_passages: 8,
    llm_max_input_tokens: 3500,
    llm_max_output_tokens: 1024,
    llm_consent_anthropic: false,
    llm_consent_openai: false,
    llm_consent_gemini: false,
    backup_path: null,
    auto_backup_enabled: true,
    last_backup_at: null,
    last_backup_bytes: null,
  };
}

describe('settingsUtils & appVersion', () => {
  it('exposes APP_VERSION directly from package.json', () => {
    expect(APP_VERSION).toBe(pkg.version);
    expect(APP_VERSION.length).toBeGreaterThan(0);
  });

  it('coerces null, empty string, NaN, and out-of-range numbers safely', () => {
    expect(coerceClampedInt(null, 1, 32, 6)).toBe(6);
    expect(coerceClampedInt('', 1, 32, 6)).toBe(6);
    expect(coerceClampedInt(undefined, 150, 350, 250)).toBe(250);
    expect(coerceClampedInt(NaN, 150, 350, 250)).toBe(250);
    expect(coerceClampedInt(9999, 150, 350, 250)).toBe(350);
    expect(coerceClampedInt(10, 150, 350, 250)).toBe(150);
    expect(coerceClampedInt('275.4', 150, 350, 250)).toBe(275);
    expect(coerceClampedInt(0, 0, 200, 30)).toBe(0);
  });

  it('sanitizes cleared Svelte number inputs (null) back to valid integers before Rust IPC', () => {
    const base = makeBaseSettings();
    const brokenDraft = {
      ...base,
      max_worker_threads: null,
      cache_limit_mb: '',
      model_idle_unload_secs: undefined,
      search_top_k_docs: null,
      ocr_dpi: 999,
      llm_max_passages: 0,
      llm_max_input_tokens: null,
      llm_max_output_tokens: Number.NaN,
    } as unknown as UiSettingsPayload;

    const clean = sanitizeUiSettings(brokenDraft, base);
    expect(clean.max_worker_threads).toBe(6);
    expect(clean.cache_limit_mb).toBe(500);
    expect(clean.model_idle_unload_secs).toBe(180);
    expect(clean.search_top_k_docs).toBe(30);
    expect(clean.ocr_dpi).toBe(350);
    expect(clean.llm_max_passages).toBe(1);
    expect(clean.llm_max_input_tokens).toBe(3500);
    expect(clean.llm_max_output_tokens).toBe(1024);
  });
});
