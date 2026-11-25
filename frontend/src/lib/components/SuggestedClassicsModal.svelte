<script lang="ts">
  import { api } from '../api';
  import { tr, type Lang } from '../i18n';
  import type { MissingClassicItem } from '../types';

  interface Props {
    open: boolean;
    lang?: Lang;
    onSelectLibraryDoc: (documentId: number) => void;
    onStatusMessage?: (msg: string) => void;
    onClose: () => void;
  }

  let { open, lang = 'it', onSelectLibraryDoc, onStatusMessage, onClose }: Props = $props();

  let category = $state<'all' | 'paper' | 'book'>('all');
  let items = $state.raw<MissingClassicItem[]>([]);
  let loading = $state(false);
  let errorMsg = $state('');

  async function loadClassics(cat = category) {
    loading = true;
    errorMsg = '';
    try {
      items = await api.getMissingClassics(cat, 60);
    } catch (e) {
      errorMsg = String(e);
      items = [];
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (open) {
      void loadClassics(category);
    }
  });

  function switchCategory(next: 'all' | 'paper' | 'book') {
    if (category === next) return;
    category = next;
    void loadClassics(next);
  }

  function lookupUrlFor(item: MissingClassicItem): string {
    if (item.doi) return `https://doi.org/${encodeURIComponent(item.doi)}`;
    if (item.arxiv_id) return `https://arxiv.org/abs/${encodeURIComponent(item.arxiv_id)}`;
    const q = [item.title, item.authors, item.year ? String(item.year) : '']
      .filter(Boolean)
      .join(' ');
    return `https://scholar.google.com/scholar?q=${encodeURIComponent(q)}`;
  }

  async function copyRef(item: MissingClassicItem) {
    const text = [
      item.authors,
      item.title,
      item.venue,
      item.year ? `(${item.year})` : '',
      item.doi ? `DOI: ${item.doi}` : item.isbn ? `ISBN: ${item.isbn}` : '',
    ]
      .filter(Boolean)
      .join(' — ');
    try {
      await navigator.clipboard.writeText(text);
      onStatusMessage?.(lang === 'en' ? 'Reference copied.' : 'Riferimento copiato negli appunti.');
    } catch {
      // Ignore clipboard error
    }
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="cl-backdrop" onclick={onClose}>
    <div
      class="cl-dialog"
      role="dialog"
      aria-modal="true"
      aria-label={tr(lang, 'suggestedClassicsTitle')}
      tabindex="-1"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.key === 'Escape' && onClose()}
    >
      <div class="cl-head">
        <div>
          <h3>{tr(lang, 'suggestedClassicsTitle')}</h3>
          <p class="cl-sub">
            {lang === 'en'
              ? 'Most-cited works across your library’s cached Crossref / Semantic Scholar references that you do not own yet (zero extra network calls).'
              : 'Opere più citate dai documenti della tua libreria (da cache Crossref / Semantic Scholar, zero chiamate di rete) che non possiedi ancora.'}
          </p>
        </div>
        <button type="button" class="cl-close" onclick={onClose} aria-label={tr(lang, 'close')}>
          ✕
        </button>
      </div>

      <div class="cl-bar">
        <div class="seg-group" role="group" aria-label="Tipo di opere consigliate">
          <button
            type="button"
            class="seg-btn"
            class:active={category === 'all'}
            onclick={() => switchCategory('all')}
          >
            {tr(lang, 'filterAll')}
          </button>
          <button
            type="button"
            class="seg-btn"
            class:active={category === 'paper'}
            onclick={() => switchCategory('paper')}
          >
            {tr(lang, 'filterPapers')}
          </button>
          <button
            type="button"
            class="seg-btn"
            class:active={category === 'book'}
            onclick={() => switchCategory('book')}
          >
            {tr(lang, 'filterBooks')}
          </button>
        </div>
        <span class="cl-count">
          {items.length} {lang === 'en' ? 'suggestions' : 'suggerimenti'}
        </span>
      </div>

      <div class="cl-body">
        {#if loading}
          <div class="cl-empty">
            {lang === 'en' ? 'Analyzing cached citations…' : 'Analisi delle citazioni locali in corso…'}
          </div>
        {:else if errorMsg}
          <div class="cl-empty err">{errorMsg}</div>
        {:else if items.length === 0}
          <div class="cl-empty">
            {lang === 'en'
              ? 'No missing cited works found yet for this filter. Enriching documents via Crossref or Semantic Scholar automatically populates local citation data.'
              : 'Nessun riferimento mancante trovato per questo filtro. I riferimenti si popolano automaticamente quando aggiorni i metadati tramite Crossref o Semantic Scholar.'}
          </div>
        {:else}
          <ul class="cl-list">
            {#each items as item, idx (`${item.doi ?? item.isbn ?? item.title}:${idx}`)}
              <li class="cl-item">
                <div class="cl-item-top">
                  <div class="cl-title-wrap">
                    <span class="cl-kind" class:book={item.work_category === 'book'}>
                      {item.work_category === 'book' ? tr(lang, 'bookBadge') : 'PAPER'}
                    </span>
                    <strong class="cl-title">{item.title}</strong>
                  </div>
                  <span class="cl-cite-count" title="Documenti della tua libreria che citano quest'opera">
                    {lang === 'en'
                      ? `Cited by ${item.cited_by_count}`
                      : `Citato da ${item.cited_by_count}`}
                  </span>
                </div>
                <div class="cl-meta">
                  {#if item.authors}<span>{item.authors}</span>{/if}
                  {#if item.year}<span>({item.year})</span>{/if}
                  {#if item.venue}<span class="cl-venue">· {item.venue}</span>{/if}
                  {#if item.doi}<code class="cl-id">DOI {item.doi}</code>{/if}
                  {#if item.isbn}<code class="cl-id">ISBN {item.isbn}</code>{/if}
                </div>
                <div class="cl-item-foot">
                  <div class="cl-citers">
                    {#each item.citing_docs.slice(0, 4) as c (c.document_id)}
                      <button
                        type="button"
                        class="cl-citer-chip"
                        title={c.title}
                        onclick={() => {
                          onSelectLibraryDoc(c.document_id);
                          onClose();
                        }}
                      >
                        ↳ {c.title}
                      </button>
                    {/each}
                    {#if item.citing_docs.length > 4}
                      <span class="cl-more">+{item.citing_docs.length - 4}</span>
                    {/if}
                  </div>
                  <div class="cl-acts">
                    <button type="button" class="mini-btn" onclick={() => copyRef(item)}>
                      {lang === 'en' ? 'Copy' : 'Copia'}
                    </button>
                    <button
                      type="button"
                      class="mini-btn accent"
                      onclick={() => void api.openExternalUrl(lookupUrlFor(item))}
                    >
                      {lang === 'en' ? 'Open ↗' : 'Apri ↗'}
                    </button>
                  </div>
                </div>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .cl-backdrop {
    position: fixed;
    inset: 0;
    z-index: 1150;
    background: rgba(15, 23, 42, 0.52);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 20px;
  }
  .cl-dialog {
    width: min(760px, 94vw);
    max-height: 82vh;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 24px 60px rgba(0, 0, 0, 0.36);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .cl-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
    padding: 14px 18px 10px;
    border-bottom: 1px solid var(--border);
  }
  .cl-head h3 {
    margin: 0;
    font-size: 15px;
  }
  .cl-sub {
    margin: 4px 0 0;
    font-size: 11.5px;
    color: var(--text-muted);
    line-height: 1.4;
  }
  .cl-close {
    border: none;
    background: transparent;
    color: var(--text-muted);
    font-size: 15px;
    cursor: pointer;
    padding: 2px 6px;
  }
  .cl-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 18px;
    border-bottom: 1px solid var(--border);
    background: var(--bg);
  }
  .seg-group {
    display: inline-flex;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
    background: var(--bg-muted);
  }
  .seg-btn {
    border: none;
    background: transparent;
    color: var(--text-muted);
    padding: 4px 11px;
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
  }
  .seg-btn.active {
    background: var(--accent);
    color: #fff;
  }
  .cl-count {
    font-size: 11.5px;
    color: var(--text-muted);
  }
  .cl-body {
    flex: 1;
    overflow-y: auto;
    padding: 12px 16px;
  }
  .cl-empty {
    padding: 32px 18px;
    text-align: center;
    color: var(--text-muted);
    font-size: 13px;
    line-height: 1.5;
  }
  .cl-empty.err {
    color: var(--danger);
  }
  .cl-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .cl-item {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 10px 12px;
    background: var(--bg);
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .cl-item-top {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 10px;
  }
  .cl-title-wrap {
    display: flex;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
  }
  .cl-kind {
    font-size: 9.5px;
    font-weight: 700;
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--accent-soft);
    color: var(--accent);
    flex-shrink: 0;
  }
  .cl-kind.book {
    background: rgba(217, 119, 6, 0.14);
    color: #b45309;
  }
  .cl-title {
    font-size: 13px;
    line-height: 1.35;
  }
  .cl-cite-count {
    font-size: 11px;
    font-weight: 700;
    padding: 2px 8px;
    border-radius: 999px;
    background: var(--accent-soft);
    color: var(--accent);
    white-space: nowrap;
    flex-shrink: 0;
  }
  .cl-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    font-size: 11.5px;
    color: var(--text-muted);
  }
  .cl-venue {
    font-style: italic;
  }
  .cl-id {
    font-size: 10.5px;
    background: var(--bg-muted);
    padding: 0 4px;
    border-radius: 4px;
  }
  .cl-item-foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-top: 3px;
  }
  .cl-citers {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    min-width: 0;
  }
  .cl-citer-chip {
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--text-muted);
    border-radius: 5px;
    padding: 1px 6px;
    font-size: 10.5px;
    max-width: 200px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: pointer;
  }
  .cl-citer-chip:hover {
    border-color: var(--accent);
    color: var(--text);
  }
  .cl-more {
    font-size: 10.5px;
    color: var(--text-muted);
    align-self: center;
  }
  .cl-acts {
    display: flex;
    gap: 5px;
    flex-shrink: 0;
  }
  .mini-btn {
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--text);
    border-radius: 5px;
    padding: 3px 8px;
    font-size: 11px;
    cursor: pointer;
  }
  .mini-btn.accent {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
