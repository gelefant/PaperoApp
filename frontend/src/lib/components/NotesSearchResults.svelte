<script lang="ts">
  import type { AnnotationSearchHit } from '../types';
  import { tr, type Lang } from '../i18n';
  import MathMarkdown from './MathMarkdown.svelte';

  interface Props {
    lang?: Lang;
    query: string;
    hits: AnnotationSearchHit[];
    loading?: boolean;
    onOpenHit: (hit: AnnotationSearchHit) => void;
    onExitNotesScope: () => void;
  }

  let {
    lang = 'it',
    query,
    hits,
    loading = false,
    onOpenHit,
    onExitNotesScope,
  }: Props = $props();

  function kindLabel(kind: string): string {
    if (kind === 'bookmark') return tr(lang, 'kindBookmark');
    if (kind === 'note') return tr(lang, 'kindNote');
    if (kind === 'snip') return tr(lang, 'kindSnip');
    return tr(lang, 'kindHighlight');
  }
</script>

<div class="notes-results-pane">
  <div class="nr-head">
    <div class="nr-head-top">
      <strong>
        {lang === 'en' ? `Notes & Highlights (${hits.length})` : `Note ed Evidenziazioni (${hits.length})`}
      </strong>
      <button type="button" class="nr-back" onclick={onExitNotesScope}>
        {tr(lang, 'filterAll')}
      </button>
    </div>
    <span class="nr-hint">
      {query.trim()
        ? lang === 'en'
          ? `Matching “${query.trim()}” · click to jump to page`
          : `Corrispondenze per «${query.trim()}» · clic per aprire alla pagina`
        : lang === 'en'
          ? 'Recent personal notes, highlights & bookmarks · type above to filter'
          : 'Note, evidenziazioni e segnalibri recenti · scrivi in alto per filtrare'}
    </span>
  </div>

  <div class="nr-list">
    {#if loading}
      <div class="nr-empty">{lang === 'en' ? 'Searching notes…' : 'Ricerca nelle note…'}</div>
    {:else if hits.length === 0}
      <div class="nr-empty">
        {lang === 'en'
          ? 'No matching notes or highlights.'
          : 'Nessuna nota o evidenziazione corrispondente.'}
      </div>
    {:else}
      {#each hits as hit (hit.annotation.id)}
        <button type="button" class="nr-card" onclick={() => onOpenHit(hit)}>
          <div class="nr-card-top">
            <span class="nr-doc">{hit.doc_title || `Doc #${hit.document_id ?? '?'}`}</span>
            <span class="nr-page">p. {hit.annotation.page_ord}</span>
          </div>
          <div class="nr-badges">
            <span class="nr-dot dot-{hit.annotation.color}"></span>
            <span class="nr-kind">{kindLabel(hit.annotation.kind)}</span>
          </div>
          {#if hit.annotation.image_data_url}
            <div class="nr-snip-wrap">
              <img
                src={hit.annotation.image_data_url}
                alt={`Snip p. ${hit.annotation.page_ord}`}
                class="nr-snip-img"
              />
            </div>
          {/if}
          {#if hit.annotation.quoted_text.trim()}
            <blockquote class="nr-quote">{hit.annotation.quoted_text.trim()}</blockquote>
          {/if}
          {#if hit.annotation.note_markdown.trim()}
            <div class="nr-note-body">
              <MathMarkdown text={hit.annotation.note_markdown} />
            </div>
          {/if}
        </button>
      {/each}
    {/if}
  </div>
</div>

<style>
  .notes-results-pane {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
    background: var(--bg-panel);
  }
  .nr-head {
    padding: 9px 12px;
    border-bottom: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .nr-head-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    font-size: 13px;
  }
  .nr-back {
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text-muted);
    border-radius: 5px;
    padding: 2px 8px;
    font-size: 11px;
    cursor: pointer;
  }
  .nr-back:hover {
    color: var(--text);
    border-color: var(--accent);
  }
  .nr-hint {
    font-size: 11px;
    color: var(--text-muted);
  }
  .nr-list {
    flex: 1;
    overflow-y: auto;
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 7px;
  }
  .nr-empty {
    padding: 28px 14px;
    text-align: center;
    color: var(--text-muted);
    font-size: 12.5px;
  }
  .nr-card {
    text-align: left;
    border: 1px solid var(--border);
    background: var(--bg);
    border-radius: 8px;
    padding: 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 5px;
    cursor: pointer;
    color: var(--text);
  }
  .nr-card:hover {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .nr-card-top {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 8px;
  }
  .nr-doc {
    font-size: 12px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .nr-page {
    font-size: 11px;
    color: var(--accent);
    font-weight: 600;
    flex-shrink: 0;
  }
  .nr-badges {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 10.5px;
    color: var(--text-muted);
  }
  .nr-dot {
    width: 7px;
    height: 7px;
    border-radius: 999px;
    background: #eab308;
  }
  .nr-dot.dot-green {
    background: #22c55e;
  }
  .nr-dot.dot-blue {
    background: #3b82f6;
  }
  .nr-dot.dot-pink {
    background: #ec4899;
  }
  .nr-quote {
    margin: 0;
    padding: 3px 7px;
    border-left: 2px solid var(--accent);
    background: var(--bg-muted);
    font-size: 11.5px;
    color: var(--text-muted);
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .nr-note-body {
    font-size: 12px;
    line-height: 1.45;
  }
  .nr-snip-wrap {
    max-height: 110px;
    overflow: hidden;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: #ffffff;
    padding: 3px;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .nr-snip-img {
    max-width: 100%;
    max-height: 102px;
    object-fit: contain;
    display: block;
  }
</style>
