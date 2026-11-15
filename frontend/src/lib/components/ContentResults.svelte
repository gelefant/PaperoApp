<script lang="ts">
  import type {
    ContentSearchDocResult,
    ContentSearchPassageHit,
    HybridSearchStats,
    LlmAnswerResult,
    LlmCitation,
    LlmPromptPreview,
    LlmProvider,
    WorkCategory,
  } from '../types';
  import { tr, type Lang } from '../i18n';
  import { buildHighlightedSnippet, splitAnswerWithCitations } from '../viewerUtils';

  interface Props {
    lang?: Lang;
    query: string;
    results: ContentSearchDocResult[];
    searchStats?: HybridSearchStats | null;
    workCategory?: WorkCategory;
    onChangeWorkCategory?: (cat: WorkCategory) => void;
    llmProvider?: LlmProvider;
    llmPreview?: LlmPromptPreview | null;
    llmAnswer?: LlmAnswerResult | null;
    llmLoading?: boolean;
    llmError?: string;
    onGenerateAnswer?: () => void;
    onOpenCitation?: (citation: LlmCitation) => void;
    onOpenDocTopMatch: (doc: ContentSearchDocResult) => void;
    onOpenPassage: (doc: ContentSearchDocResult, passage: ContentSearchPassageHit) => void;
    onContextMenu: (e: MouseEvent, doc: ContentSearchDocResult, pageOrd?: number) => void;
  }

  let {
    lang = 'it',
    query,
    results,
    searchStats = null,
    workCategory = 'all',
    onChangeWorkCategory,
    llmProvider = 'disabled',
    llmPreview = null,
    llmAnswer = null,
    llmLoading = false,
    llmError = '',
    onGenerateAnswer,
    onOpenCitation,
    onOpenDocTopMatch,
    onOpenPassage,
    onContextMenu,
  }: Props = $props();
  let collapsed = $state<Record<number, boolean>>({});
  let answerSegments = $derived(
    llmAnswer ? splitAnswerWithCitations(llmAnswer.answer_markdown, llmAnswer.citations) : []
  );

  function providerLabel(p: LlmProvider): string {
    switch (p) {
      case 'ollama':
        return 'Ollama (Locale)';
      case 'anthropic':
        return 'Anthropic';
      case 'openai':
        return 'OpenAI';
      case 'gemini':
        return 'Google Gemini';
      default:
        return '';
    }
  }

  function toggle(docId: number) {
    collapsed[docId] = !collapsed[docId];
  }

  function modeBadge(mode: string): string {
    switch (mode) {
      case 'bm25_vector_rerank':
        return 'BM25 + Vettoriale + Reranker';
      case 'bm25_vector':
        return 'BM25 + Vettoriale';
      case 'bm25_rerank':
        return 'BM25 + Reranker';
      default:
        return 'Solo BM25';
    }
  }
</script>

<div class="results-pane">
  <div class="list-header">
    <div class="list-header-top">
      <div class="header-left">
        <strong>{lang === 'en' ? `Results (${results.length} docs)` : `Risultati (${results.length} documenti)`}</strong>
        {#if searchStats}
          <span class="mode-badge" title={`BM25: ${searchStats.bm25_hits}, Vec: ${searchStats.vector_hits}, Rerank: ${searchStats.reranked_count}`}>
            {modeBadge(searchStats.mode)} · {Math.round(searchStats.total_ms)} ms
          </span>
        {/if}
      </div>
      {#if onChangeWorkCategory}
        <div class="seg-group" role="group" aria-label={lang === 'en' ? 'Filter document type' : 'Filtra tipo documento'}>
          <button
            type="button"
            class="seg-btn"
            class:active={workCategory === 'all'}
            onclick={() => onChangeWorkCategory('all')}
          >
            {tr(lang, 'filterAll')}
          </button>
          <button
            type="button"
            class="seg-btn"
            class:active={workCategory === 'paper'}
            onclick={() => onChangeWorkCategory('paper')}
          >
            {tr(lang, 'filterPapers')}
          </button>
          <button
            type="button"
            class="seg-btn"
            class:active={workCategory === 'book'}
            onclick={() => onChangeWorkCategory('book')}
          >
            {tr(lang, 'filterBooks')}
          </button>
        </div>
      {/if}
    </div>
    <span class="hint">{lang === 'en' ? 'Double-click passage or document to open' : 'Doppio clic sul passaggio o sul documento'}</span>
  </div>
  {#if llmProvider !== 'disabled' && results.length > 0}
    <div class="llm-bar">
      <div class="llm-meta">
        <span class="llm-prov">{providerLabel(llmProvider)}</span>
        {#if llmPreview}
          {#if llmPreview.model}
            <span class="llm-chip">{llmPreview.model}</span>
          {:else}
            <span class="llm-warn-chip">
              {lang === 'en'
                ? 'Select a model in Settings'
                : 'Seleziona un modello nelle Impostazioni'}
            </span>
          {/if}
          <span class="llm-est">
            {lang === 'en' ? 'Est.' : 'Stima'}: ~{llmPreview.estimated_input_tokens}/{llmPreview.max_input_tokens} tok · {llmPreview.passages_included}/{llmPreview.passages_total} pass.
          </span>
          {#if llmPreview.passages_excluded_cloud > 0}
            <span
              class="llm-excl-chip"
              title={llmPreview.excluded_doc_titles.join(', ')}
            >
              {llmPreview.passages_excluded_cloud} {lang === 'en' ? 'excluded from cloud' : 'esclusi da cloud'}
            </span>
          {/if}
        {/if}
      </div>
      <button
        type="button"
        class="gen-btn"
        disabled={llmLoading || !llmPreview?.model}
        onclick={() => onGenerateAnswer?.()}
      >
        {llmLoading ? tr(lang, 'llmGeneratingBtn') : tr(lang, 'llmGenerateBtn')}
      </button>
    </div>
  {/if}

  {#if llmError}
    <div class="llm-error">{llmError}</div>
  {/if}

  {#if llmAnswer}
    <section class="llm-answer" class:refused={llmAnswer.refused}>
      <header class="llm-ans-head">
        <div class="llm-ans-title">
          <strong>{tr(lang, 'llmAnswerHeading')}</strong>
          <span class="llm-chip">{llmAnswer.model}</span>
          {#if llmAnswer.refused}
            <span class="llm-warn-chip">
              {lang === 'en' ? 'Insufficient passage support' : 'Informazioni insufficienti nei passaggi'}
            </span>
          {/if}
        </div>
        <span class="llm-usage">
          {#if llmAnswer.usage.input_tokens !== null || llmAnswer.usage.output_tokens !== null}
            Token: in {llmAnswer.usage.input_tokens ?? '?'} · out {llmAnswer.usage.output_tokens ?? '?'}{#if llmAnswer.usage.total_tokens !== null} · tot {llmAnswer.usage.total_tokens}{/if}
          {:else if llmAnswer.refusal_reason}
            {llmAnswer.refusal_reason}
          {/if}
        </span>
      </header>
      <div class="llm-ans-body">
        {#each answerSegments as seg, idx (idx)}
          {#if seg.kind === 'text'}
            {seg.text}
          {:else if seg.citation}
            <button
              type="button"
              class="cite-pill"
              title={`${seg.citation.doc_title} — p. ${seg.citation.page_ord}`}
              onclick={() => onOpenCitation?.(seg.citation!)}
            >
              {seg.label}
            </button>
          {:else}
            <span class="cite-unresolved">{seg.label}</span>
          {/if}
        {/each}
      </div>
      {#if llmAnswer.citations.length > 0}
        <div class="llm-cites-foot">
          <span class="cites-label">{lang === 'en' ? 'Sources:' : 'Fonti:'}</span>
          {#each llmAnswer.citations as c (c.marker + '-' + c.file_id + '-' + c.page_ord)}
            <button
              type="button"
              class="cite-foot-btn"
              title={c.snippet}
              onclick={() => onOpenCitation?.(c)}
            >
              {c.marker} · {c.doc_title}
            </button>
          {/each}
        </div>
      {/if}
    </section>
  {/if}

  {#if results.length === 0}
    <div class="empty">Nessun passaggio trovato per «{query}».</div>
  {:else}
    <div class="rows">
      {#each results as doc (doc.document_id)}
        <div class="doc-group">
          <div
            class="doc-header"
            class:unavailable={!doc.is_available}
            role="button"
            tabindex="0"
            ondblclick={() => onOpenDocTopMatch(doc)}
            oncontextmenu={(e) => {
              e.preventDefault();
              onContextMenu(e, doc, doc.passages[0]?.page_ord);
            }}
          >
            <button type="button" class="toggle-btn" onclick={() => toggle(doc.document_id)}>
              {collapsed[doc.document_id] ? '▸' : '▾'}
            </button>
            <span class="fmt-badge">{doc.format.toUpperCase()}</span>
            {#if doc.work_type === 'book'}
              <span class="book-badge">{tr(lang, 'bookBadge')}</span>
            {/if}
            <span class="doc-title">{doc.title}</span>
            <span class="score">{doc.passages.length} pass. · {doc.max_score.toFixed(2)}</span>
          </div>

          {#if !collapsed[doc.document_id]}
            <div class="passages">
              {#each doc.passages as pass (pass.passage_id)}
                <div
                  class="passage-row"
                  role="button"
                  tabindex="0"
                  ondblclick={() => onOpenPassage(doc, pass)}
                  onkeydown={(e) => e.key === 'Enter' && onOpenPassage(doc, pass)}
                  oncontextmenu={(e) => {
                    e.preventDefault();
                    onContextMenu(e, doc, pass.page_ord);
                  }}
                >
                  <div class="passage-head">
                    <span class="page-chip">
                      p. {pass.page_ord}{pass.printed_label ? ` (${pass.printed_label})` : ''}
                    </span>
                    <span class="pass-score">{pass.score.toFixed(2)}</span>
                  </div>
                  <div class="snippet">{buildHighlightedSnippet(pass.text_content, query)}</div>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .results-pane {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
  }
  .list-header {
    padding: 7px 10px;
    border-bottom: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .list-header-top {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  .header-left {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 4px;
  }
  .seg-group {
    display: inline-flex;
    align-items: center;
    background: var(--bg-muted);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 1px;
    gap: 1px;
  }
  .seg-btn {
    border: none;
    background: transparent;
    color: var(--text-muted);
    font-size: 11px;
    font-weight: 600;
    padding: 2px 8px;
    border-radius: 5px;
    cursor: pointer;
    line-height: 1.35;
  }
  .seg-btn:hover {
    color: var(--text);
  }
  .seg-btn.active {
    background: var(--accent);
    color: #fff;
  }
  .book-badge {
    font-size: 9.5px;
    font-weight: 700;
    padding: 1px 5px;
    border-radius: 4px;
    background: rgba(16, 185, 129, 0.14);
    color: #047857;
    border: 1px solid rgba(16, 185, 129, 0.32);
    flex-shrink: 0;
  }
  .hint {
    font-size: 11px;
    color: var(--text-muted);
  }
  .rows {
    overflow-y: auto;
    flex: 1;
  }
  .doc-group {
    border-bottom: 1px solid var(--border);
  }
  .doc-header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 7px 10px;
    background: var(--bg-muted);
    cursor: pointer;
  }
  .doc-header.unavailable {
    opacity: 0.6;
  }
  .toggle-btn {
    padding: 1px 6px;
    font-size: 11px;
  }
  .fmt-badge {
    font-size: 10px;
    font-weight: 700;
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .doc-title {
    flex: 1;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .score {
    font-size: 11px;
    color: var(--text-muted);
  }
  .passages {
    display: flex;
    flex-direction: column;
  }
  .passage-row {
    padding: 7px 12px 7px 26px;
    border-top: 1px solid var(--border);
    cursor: pointer;
  }
  .passage-row:hover {
    background: var(--accent-soft);
  }
  .passage-head {
    display: flex;
    justify-content: space-between;
    margin-bottom: 2px;
  }
  .page-chip {
    font-size: 11px;
    font-weight: 600;
    color: var(--accent);
  }
  .pass-score {
    font-size: 11px;
    color: var(--text-muted);
  }
  .snippet {
    font-size: 12px;
    line-height: 1.4;
    color: var(--text);
  }
  .empty {
    padding: 20px;
    color: var(--text-muted);
  }
  .mode-badge {
    margin-left: 8px;
    font-size: 11px;
    padding: 2px 7px;
    border-radius: 999px;
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 500;
  }
  .llm-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 7px 10px;
    background: var(--bg-muted);
    border-bottom: 1px solid var(--border);
    flex-wrap: wrap;
  }
  .llm-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    font-size: 11px;
  }
  .llm-prov {
    font-weight: 700;
    color: var(--text);
  }
  .llm-chip {
    padding: 1px 6px;
    border-radius: 999px;
    background: var(--accent-soft);
    color: var(--accent);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 10px;
  }
  .llm-est {
    color: var(--text-muted);
  }
  .llm-excl-chip,
  .llm-warn-chip {
    padding: 1px 6px;
    border-radius: 999px;
    background: rgba(245, 158, 11, 0.16);
    border: 1px solid rgba(245, 158, 11, 0.4);
    font-size: 10px;
    font-weight: 600;
  }
  .gen-btn {
    padding: 4px 10px;
    font-size: 12px;
    font-weight: 600;
    border-radius: 6px;
    background: var(--accent);
    color: #fff;
    border: none;
    cursor: pointer;
  }
  .gen-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .llm-error {
    padding: 8px 10px;
    background: rgba(220, 38, 38, 0.1);
    border-bottom: 1px solid rgba(220, 38, 38, 0.35);
    color: var(--danger);
    font-size: 12px;
  }
  .llm-answer {
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-panel);
    display: flex;
    flex-direction: column;
    gap: 7px;
    max-height: 44%;
    overflow-y: auto;
  }
  .llm-answer.refused {
    background: var(--bg-muted);
  }
  .llm-ans-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    font-size: 12px;
  }
  .llm-ans-title {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  .llm-usage {
    font-size: 11px;
    color: var(--text-muted);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }
  .llm-ans-body {
    font-size: 13px;
    line-height: 1.55;
    white-space: pre-wrap;
    color: var(--text);
  }
  .cite-pill {
    display: inline-block;
    padding: 0 5px;
    margin: 0 1px;
    border-radius: 4px;
    border: 1px solid var(--accent);
    background: var(--accent-soft);
    color: var(--accent);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
  }
  .cite-pill:hover {
    background: var(--accent);
    color: #fff;
  }
  .cite-unresolved {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
    color: var(--text-muted);
  }
  .llm-cites-foot {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 5px;
    padding-top: 4px;
    border-top: 1px dashed var(--border);
  }
  .cites-label {
    font-size: 11px;
    color: var(--text-muted);
    font-weight: 600;
  }
  .cite-foot-btn {
    padding: 2px 6px;
    font-size: 11px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--bg-muted);
    max-width: 260px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cite-foot-btn:hover {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
