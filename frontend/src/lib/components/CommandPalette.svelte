<script lang="ts">
  import { tick } from 'svelte';
  import { tr, type Lang } from '../i18n';
  import type { CollectionView, DocumentListItem, SavedSearchView } from '../types';

  export interface PaletteAction {
    id: string;
    label: string;
    hint?: string;
    run: () => void;
  }

  interface Props {
    open: boolean;
    lang?: Lang;
    documents: DocumentListItem[];
    collections: CollectionView[];
    savedSearches: SavedSearchView[];
    actions: PaletteAction[];
    onOpenDocument: (doc: DocumentListItem) => void;
    onSelectCollection: (collectionId: number) => void;
    onApplySavedSearch: (sv: SavedSearchView) => void;
    onClose: () => void;
  }

  let {
    open,
    lang = 'it',
    documents,
    collections,
    savedSearches,
    actions,
    onOpenDocument,
    onSelectCollection,
    onApplySavedSearch,
    onClose,
  }: Props = $props();

  let query = $state('');
  let activeIdx = $state(0);
  let inputEl = $state<HTMLInputElement | undefined>(undefined);

  function fold(raw: string): string {
    return raw
      .normalize('NFD')
      .replace(/[\u0300-\u036f]/g, '')
      .toLowerCase();
  }

  interface PaletteRow {
    key: string;
    group: string;
    label: string;
    sub?: string;
    badge?: string;
    exec: () => void;
  }

  const rows = $derived.by((): PaletteRow[] => {
    const tokens = fold(query)
      .split(/\s+/)
      .filter((t) => t.length > 0);
    const matches = (haystack: string) => {
      if (tokens.length === 0) return true;
      const f = fold(haystack);
      return tokens.every((t) => f.includes(t));
    };

    const out: PaletteRow[] = [];

    // 1. Quick actions
    for (const act of actions) {
      if (matches(`${act.label} ${act.hint ?? ''}`)) {
        out.push({
          key: `act:${act.id}`,
          group: lang === 'en' ? 'Actions' : 'Azioni',
          label: act.label,
          sub: act.hint,
          badge: '⌘',
          exec: () => {
            onClose();
            act.run();
          },
        });
      }
    }

    // 2. Collections
    for (const col of collections) {
      if (matches(`${col.name} collezione collection bib`)) {
        out.push({
          key: `col:${col.id}`,
          group: tr(lang, 'collectionsSection'),
          label: col.name,
          sub: col.bib_sync_enabled && col.bib_sync_path ? `.bib · ${col.bib_sync_path}` : undefined,
          badge: `${col.document_count}`,
          exec: () => {
            onClose();
            onSelectCollection(col.id);
          },
        });
      }
    }

    // 3. Saved searches
    for (const sv of savedSearches) {
      if (matches(`${sv.name} ${sv.query_text}`)) {
        out.push({
          key: `sv:${sv.id}`,
          group: tr(lang, 'savedViewsSection'),
          label: sv.name,
          sub: sv.query_text || undefined,
          badge: sv.search_mode === 'content' ? '⌕' : '⛃',
          exec: () => {
            onClose();
            onApplySavedSearch(sv);
          },
        });
      }
    }

    // 4. Documents (cap to keep instantaneous on 10k+ libraries)
    let docAdded = 0;
    const docCap = tokens.length > 0 ? 18 : 8;
    for (const doc of documents) {
      if (docAdded >= docCap) break;
      const hay = `${doc.title} ${doc.authors_summary} ${doc.year ?? ''} ${doc.msc_primary ?? ''}`;
      if (matches(hay)) {
        docAdded++;
        out.push({
          key: `doc:${doc.document_id}`,
          group: lang === 'en' ? 'Documents' : 'Documenti',
          label: doc.title,
          sub: [doc.authors_summary, doc.year ? String(doc.year) : ''].filter(Boolean).join(' · '),
          badge: doc.work_type === 'book' ? tr(lang, 'bookBadge') : doc.format.toUpperCase(),
          exec: () => {
            onClose();
            onOpenDocument(doc);
          },
        });
      }
    }

    return out;
  });

  $effect(() => {
    if (open) {
      query = '';
      activeIdx = 0;
      void tick().then(() => {
        inputEl?.focus();
        inputEl?.select();
      });
    }
  });

  $effect(() => {
    const len = rows.length;
    if (activeIdx >= len) {
      activeIdx = Math.max(0, len - 1);
    }
  });

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (rows.length > 0) activeIdx = (activeIdx + 1) % rows.length;
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (rows.length > 0) activeIdx = (activeIdx - 1 + rows.length) % rows.length;
    } else if (e.key === 'Enter') {
      e.preventDefault();
      rows[activeIdx]?.exec();
    }
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="cp-backdrop" onclick={onClose}>
    <div
      class="cp-dialog"
      role="dialog"
      aria-modal="true"
      aria-label="Command Palette"
      tabindex="-1"
      onclick={(e) => e.stopPropagation()}
      onkeydown={handleKeydown}
    >
      <div class="cp-input-row">
        <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <circle cx="11" cy="11" r="7" />
          <path d="m20 20-3.5-3.5" />
        </svg>
        <input
          type="text"
          class="cp-input"
          bind:this={inputEl}
          bind:value={query}
          placeholder={tr(lang, 'commandPalettePlaceholder')}
        />
        <kbd class="cp-kbd">Esc</kbd>
      </div>

      <div class="cp-list" role="listbox">
        {#if rows.length === 0}
          <div class="cp-empty">
            {lang === 'en' ? 'No matching items.' : 'Nessun elemento corrispondente.'}
          </div>
        {:else}
          {#each rows as row, idx (row.key)}
            {@const showHeader = idx === 0 || rows[idx - 1].group !== row.group}
            {#if showHeader}
              <div class="cp-group-hdr">{row.group}</div>
            {/if}
            <button
              type="button"
              class="cp-row"
              class:active={idx === activeIdx}
              role="option"
              aria-selected={idx === activeIdx}
              onmouseenter={() => (activeIdx = idx)}
              onclick={() => row.exec()}
            >
              <div class="cp-row-main">
                <span class="cp-row-label">{row.label}</span>
                {#if row.sub}
                  <span class="cp-row-sub">{row.sub}</span>
                {/if}
              </div>
              {#if row.badge}
                <span class="cp-row-badge">{row.badge}</span>
              {/if}
            </button>
          {/each}
        {/if}
      </div>

      <div class="cp-foot">
        <span>↑↓ {lang === 'en' ? 'navigate' : 'naviga'}</span>
        <span>↵ {lang === 'en' ? 'open / run' : 'apri / esegui'}</span>
        <span>Esc {lang === 'en' ? 'close' : 'chiudi'}</span>
      </div>
    </div>
  </div>
{/if}

<style>
  .cp-backdrop {
    position: fixed;
    inset: 0;
    z-index: 1200;
    background: rgba(15, 23, 42, 0.48);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 11vh;
  }
  .cp-dialog {
    width: min(600px, 92vw);
    max-height: 70vh;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 24px 60px rgba(0, 0, 0, 0.35);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .cp-input-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 11px 14px;
    border-bottom: 1px solid var(--border);
    color: var(--text-muted);
  }
  .cp-input {
    flex: 1;
    border: none;
    background: transparent;
    color: var(--text);
    font-size: 14px;
    outline: none;
  }
  .cp-kbd {
    font-size: 10px;
    padding: 2px 5px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--bg-muted);
    color: var(--text-muted);
  }
  .cp-list {
    flex: 1;
    overflow-y: auto;
    padding: 6px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .cp-group-hdr {
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-muted);
    padding: 6px 8px 3px;
  }
  .cp-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    text-align: left;
    border: none;
    background: transparent;
    border-radius: 7px;
    padding: 7px 10px;
    cursor: pointer;
    color: var(--text);
  }
  .cp-row.active {
    background: var(--accent-soft);
  }
  .cp-row-main {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .cp-row-label {
    font-size: 13px;
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cp-row-sub {
    font-size: 11px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cp-row-badge {
    font-size: 10px;
    font-weight: 600;
    padding: 2px 6px;
    border-radius: 999px;
    background: var(--bg-muted);
    color: var(--text-muted);
    flex-shrink: 0;
  }
  .cp-empty {
    padding: 26px;
    text-align: center;
    color: var(--text-muted);
    font-size: 13px;
  }
  .cp-foot {
    display: flex;
    gap: 14px;
    padding: 7px 14px;
    border-top: 1px solid var(--border);
    background: var(--bg);
    font-size: 11px;
    color: var(--text-muted);
  }
</style>
