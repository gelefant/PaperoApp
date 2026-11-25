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
  import { hasMathDelimiters } from '../katexLoader';
  import { buildHighlightedSnippet } from '../viewerUtils';
  import MathMarkdown from './MathMarkdown.svelte';

  interface Props {
    lang?: Lang;
    query: string;
    results: ContentSearchDocResult[];
    searchStats?: HybridSearchStats | null;
    workCategory?: WorkCategory;
    statementsOnly?: boolean;
    statementKindGroup?: string;
    activeFilterChips?: string[];
    authorFilter?: string;
    yearFromFilter?: number | null;
    yearToFilter?: number | null;
    onChangeWorkCategory?: (cat: WorkCategory) => void;
    onToggleStatementsOnly?: (active: boolean) => void;
    onChangeStatementKindGroup?: (group: string) => void;
    onChangeContentMetaFilters?: (patch: {
      author?: string | null;
      year_from?: number | null;
      year_to?: number | null;
    }) => void;
    onClearContentFilters?: () => void;
    llmProvider?: LlmProvider;
    llmPreview?: LlmPromptPreview | null;
    llmAnswer?: LlmAnswerResult | null;
    llmStreamText?: string;
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
    statementsOnly = false,
    statementKindGroup = 'all',
    activeFilterChips = [],
    authorFilter = '',
    yearFromFilter = null,
    yearToFilter = null,
    onChangeWorkCategory,
    onToggleStatementsOnly,
    onChangeStatementKindGroup,
    onChangeContentMetaFilters,
    onClearContentFilters,
    llmProvider = 'disabled',
    llmPreview = null,
    llmAnswer = null,
    llmStreamText = '',
    llmLoading = false,
    llmError = '',
    onGenerateAnswer,
    onOpenCitation,
    onOpenDocTopMatch,
    onOpenPassage,
    onContextMenu,
  }: Props = $props();
  let collapsed = $state<Record<number, boolean>>({});
  let showAdvFilters = $state(false);
  let authorDraft = $state('');
  let yearFromDraft = $state('');
  let yearToDraft = $state('');

  $effect(() => {
    authorDraft = authorFilter ?? '';
    yearFromDraft = yearFromFilter !== null && yearFromFilter !== undefined ? String(yearFromFilter) : '';
    yearToDraft = yearToFilter !== null && yearToFilter !== undefined ? String(yearToFilter) : '';
  });

  function applyAdvFilters(e: Event) {
    e.preventDefault();
    onChangeContentMetaFilters?.({
      author: authorDraft.trim() || null,
      year_from: yearFromDraft.trim() ? Number(yearFromDraft) : null,
      year_to: yearToDraft.trim() ? Number(yearToDraft) : null,
    });
  }

  function handleMarkdownCiteClick(docId: number, pageOrd: number) {
    if (!onOpenCitation) return;
    const exact = llmAnswer?.citations.find(
      (c) => c.document_id === docId && c.page_ord === pageOrd
    );
    if (exact) {
      onOpenCitation(exact);
      return;
    }
    const matchingDoc = results.find((r) => r.document_id === docId);
    if (matchingDoc) {
      const pass = matchingDoc.passages.find((p) => p.page_ord === pageOrd);
      onOpenCitation({
        marker: `[Doc #${docId}, p. ${pageOrd}]`,
        document_id: docId,
        file_id: pass?.file_id ?? matchingDoc.file_id,
        page_ord: pageOrd,
        printed_label: pass?.printed_label ?? null,
        doc_title: matchingDoc.title,
        bboxes: pass?.bboxes ?? [],
        snippet: pass?.text_content ?? '',
      });
    }
  }

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
    if (lang === 'en') {
      switch (mode) {
        case 'bm25_vector_rerank':
          return 'BM25 + Vector + Reranker';
        case 'bm25_vector':
          return 'BM25 + Vector';
        case 'bm25_rerank':
          return 'BM25 + Reranker';
        default:
          return 'BM25 only';
      }
    }
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
      {#if onToggleStatementsOnly}
        <button
          type="button"
          class="adv-filter-toggle"
          class:active={statementsOnly}
          title={lang === 'en'
            ? 'Show only mathematical statements (Theorems, Lemmas, Propositions, Definitions, Remarks, Examples)'
            : 'Mostra solo enunciati matematici (Teoremi, Lemmi, Proposizioni, Definizioni, Osservazioni, Esempi)'}
          onclick={() => onToggleStatementsOnly(!statementsOnly)}
        >
          {tr(lang, 'statementsOnlyLabel')}
        </button>
        {#if statementsOnly && onChangeStatementKindGroup}
          <select
            class="stmt-subselect"
            aria-label={tr(lang, 'statementsOnlyLabel')}
            value={statementKindGroup}
            onchange={(e) =>
              onChangeStatementKindGroup((e.currentTarget as HTMLSelectElement).value)}
          >
            <option value="all">{tr(lang, 'stmtAll')}</option>
            <option value="theorem">{tr(lang, 'stmtTheoremGroup')}</option>
            <option value="definition">{tr(lang, 'stmtDefinitions')}</option>
            <option value="remark">{tr(lang, 'stmtRemarks')}</option>
            <option value="example">{tr(lang, 'stmtExamples')}</option>
            <option value="conjecture">{tr(lang, 'stmtConjectures')}</option>
          </select>
        {/if}
      {/if}
      {#if onChangeContentMetaFilters}
        <button
          type="button"
          class="adv-filter-toggle"
          class:active={showAdvFilters || Boolean(authorFilter) || yearFromFilter !== null || yearToFilter !== null}
          title={lang === 'en' ? 'Filter content search by author and year range' : 'Filtra ricerca nel contenuto per autore e intervallo anni'}
          onclick={() => (showAdvFilters = !showAdvFilters)}
        >
          {lang === 'en' ? 'Filters' : 'Filtri'}
        </button>
      {/if}
    </div>
    {#if activeFilterChips.length > 0}
      <div class="active-filter-chips">
        {#each activeFilterChips as chip (chip)}
          <span class="af-chip">{chip}</span>
        {/each}
        {#if onClearContentFilters}
          <button type="button" class="af-clear" onclick={onClearContentFilters}>✕</button>
        {/if}
      </div>
    {/if}
    {#if showAdvFilters && onChangeContentMetaFilters}
      <form class="adv-filter-row" onsubmit={applyAdvFilters}>
        <input
          type="text"
          class="adv-input"
          placeholder={lang === 'en' ? 'Author…' : 'Autore…'}
          bind:value={authorDraft}
        />
        <input
          type="number"
          class="adv-year"
          placeholder={lang === 'en' ? 'From' : 'Da'}
          bind:value={yearFromDraft}
        />
        <input
          type="number"
          class="adv-year"
          placeholder={lang === 'en' ? 'To' : 'A'}
          bind:value={yearToDraft}
        />
        <button type="submit" class="adv-apply">OK</button>
      </form>
    {/if}
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

  {#if llmAnswer || (llmLoading && llmStreamText)}
    <section class="llm-answer" class:refused={llmAnswer?.refused}>
      <header class="llm-ans-head">
        <div class="llm-ans-title">
          <strong>{tr(lang, 'llmAnswerHeading')}</strong>
          {#if llmAnswer?.model || llmPreview?.model}
            <span class="llm-chip">{llmAnswer?.model ?? llmPreview?.model}</span>
          {/if}
          {#if llmAnswer?.refused}
            <span class="llm-warn-chip">
              {lang === 'en' ? 'Insufficient passage support' : 'Informazioni insufficienti nei passaggi'}
            </span>
          {/if}
        </div>
        <span class="llm-usage">
          {#if llmAnswer && (llmAnswer.usage.input_tokens !== null || llmAnswer.usage.output_tokens !== null)}
            Token: in {llmAnswer.usage.input_tokens ?? '?'} · out {llmAnswer.usage.output_tokens ?? '?'}{#if llmAnswer.usage.total_tokens !== null} · tot {llmAnswer.usage.total_tokens}{/if}
          {:else if llmAnswer?.refusal_reason}
            {llmAnswer.refusal_reason}
          {:else if llmLoading}
            streaming…
          {/if}
        </span>
      </header>
      <div class="llm-ans-body">
        <MathMarkdown
          text={llmAnswer ? llmAnswer.answer_markdown : llmStreamText}
          clickableCitations={true}
          onCitationClick={handleMarkdownCiteClick}
        />
        {#if llmLoading}
          <span class="stream-cursor" aria-hidden="true">▍</span>
        {/if}
      </div>
      {#if llmAnswer && llmAnswer.citations.length > 0}
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
    <div class="empty">
      {lang === 'en' ? `No passages found for “${query}”.` : `Nessun passaggio trovato per «${query}».`}
    </div>
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
                    <div class="pass-head-left">
                      <span class="page-chip">
                        p. {pass.page_ord}{pass.printed_label ? ` (${pass.printed_label})` : ''}
                      </span>
                      {#if pass.statement_label}
                        <span class="stmt-badge">{pass.statement_label}</span>
                      {/if}
                    </div>
                    <span class="pass-score">{pass.score.toFixed(2)}</span>
                  </div>
                  <div class="snippet">
                    {#if pass.is_alt_formula || hasMathDelimiters(pass.text_content)}
                      <MathMarkdown text={buildHighlightedSnippet(pass.text_content, query)} compact={true} />
                    {:else}
                      {buildHighlightedSnippet(pass.text_content, query)}
                    {/if}
                  </div>
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
    contain: content;
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
    color: var(--text);
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
  .adv-filter-toggle {
    font-size: 11px;
    padding: 2px 7px;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text-muted);
    cursor: pointer;
  }
  .adv-filter-toggle.active {
    border-color: var(--accent);
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
  }
  .stmt-subselect {
    font-size: 11px;
    height: 24px;
    padding: 1px 6px;
    border-radius: 5px;
    border: 1px solid var(--accent);
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
  }
  .active-filter-chips {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
  }
  .af-chip {
    font-size: 10.5px;
    padding: 1px 7px;
    border-radius: 999px;
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
  }
  .af-clear {
    border: none;
    background: transparent;
    color: var(--text-muted);
    font-size: 11px;
    cursor: pointer;
    padding: 0 4px;
  }
  .adv-filter-row {
    display: flex;
    gap: 4px;
    align-items: center;
  }
  .adv-input {
    flex: 1;
    min-width: 0;
    height: 24px;
    font-size: 11.5px;
    padding: 2px 6px;
  }
  .adv-year {
    width: 60px;
    height: 24px;
    font-size: 11.5px;
    padding: 2px 5px;
  }
  .adv-apply {
    height: 24px;
    font-size: 11px;
    padding: 0 8px;
  }
  .stream-cursor {
    color: var(--accent);
    animation: cr-blink 0.9s steps(2, start) infinite;
  }
  @keyframes cr-blink {
    to {
      opacity: 0;
    }
  }
  .pass-head-left {
    display: flex;
    align-items: center;
    gap: 5px;
    flex-wrap: wrap;
  }
  .stmt-badge {
    font-size: 10px;
    font-weight: 700;
    padding: 0 5px;
    border-radius: 4px;
    background: rgba(16, 185, 129, 0.14);
    color: #047857;
    border: 1px solid rgba(16, 185, 129, 0.3);
  }
</style>
