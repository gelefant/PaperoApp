<script lang="ts">
  import { onDestroy, onMount, untrack } from 'svelte';
  import { tr, type Lang } from '../i18n';
  import { shouldShowRowVerifyControl } from '../viewerUtils';
  import type {
    DocumentListItem,
    SortDirection,
    SortField,
    VerifiedFilter,
    WorkCategory,
  } from '../types';

  interface Props {
    lang?: Lang;
    documents: DocumentListItem[];
    selectedDocId: number | null;
    workCategory?: WorkCategory;
    verifiedFilter?: VerifiedFilter;
    unverifiedCount?: number;
    sortBy?: SortField;
    sortDir?: SortDirection;
    onChangeWorkCategory?: (cat: WorkCategory) => void;
    onChangeVerifiedFilter?: (vf: VerifiedFilter) => void;
    onChangeSortBy?: (sortBy: SortField) => void;
    onToggleSortDir?: () => void;
    onToggleVerified?: (docId: number, verified: boolean) => void | Promise<void>;
    onSelectDoc: (doc: DocumentListItem) => void;
    onOpenDoc: (doc: DocumentListItem) => void;
    onContextMenu: (e: MouseEvent, doc: DocumentListItem) => void;
    onConfirmMsc?: (docId: number, mscCode: string | null) => Promise<void>;
    onRejectMsc?: (docId: number) => Promise<void>;
  }

  let {
    lang = 'it',
    documents,
    selectedDocId,
    workCategory = 'all',
    verifiedFilter = 'all',
    unverifiedCount,
    sortBy = 'updated',
    sortDir = 'desc',
    onChangeWorkCategory,
    onChangeVerifiedFilter,
    onChangeSortBy,
    onToggleSortDir,
    onToggleVerified,
    onSelectDoc,
    onOpenDoc,
    onContextMenu,
    onConfirmMsc,
    onRejectMsc,
  }: Props = $props();

  const showVerifyControl = $derived(shouldShowRowVerifyControl(verifiedFilter, unverifiedCount));

  const VIRTUALIZE_THRESHOLD = 80;
  const ESTIMATED_ROW_HEIGHT = 50;
  const OVERSCAN_ROWS = 14;

  let rowsEl: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let viewportHeight = $state(640);
  let heightRevision = $state(0);
  const measuredHeights = new Map<number, number>();
  let scrollRafId: number | null = null;
  let measureRafId: number | null = null;
  const pendingMeasureNodes = new Map<number, HTMLElement>();
  let rowsResizeObserver: ResizeObserver | null = null;

  function scheduleFlushMeasurements() {
    if (measureRafId !== null) return;
    measureRafId = requestAnimationFrame(() => {
      measureRafId = null;
      let changed = false;
      for (const [docId, el] of pendingMeasureNodes) {
        if (!el.isConnected) continue;
        const h = el.offsetHeight;
        if (h > 0 && measuredHeights.get(docId) !== h) {
          measuredHeights.set(docId, h);
          changed = true;
        }
      }
      pendingMeasureNodes.clear();
      if (changed) {
        heightRevision++;
      }
    });
  }

  function measureRow(node: HTMLElement, docId: number) {
    let currentId = docId;
    pendingMeasureNodes.set(currentId, node);
    scheduleFlushMeasurements();
    return {
      update(nextId: number) {
        if (nextId !== currentId) {
          pendingMeasureNodes.delete(currentId);
          currentId = nextId;
        }
        pendingMeasureNodes.set(currentId, node);
        scheduleFlushMeasurements();
      },
      destroy() {
        pendingMeasureNodes.delete(currentId);
      },
    };
  }

  const rowLayout = $derived.by(() => {
    const _rev = heightRevision;
    void _rev;
    const len = documents.length;
    const offsets = new Float64Array(len + 1);
    for (let i = 0; i < len; i++) {
      offsets[i + 1] =
        offsets[i] + (measuredHeights.get(documents[i].document_id) ?? ESTIMATED_ROW_HEIGHT);
    }
    return { offsets, totalHeight: offsets[len] };
  });

  function findRowIndexAtOffset(offsets: Float64Array, count: number, y: number): number {
    if (count <= 0) return 0;
    let lo = 0;
    let hi = count - 1;
    let ans = count - 1;
    while (lo <= hi) {
      const mid = (lo + hi) >>> 1;
      if (offsets[mid + 1] >= y) {
        ans = mid;
        hi = mid - 1;
      } else {
        lo = mid + 1;
      }
    }
    return ans;
  }

  const virtualWindow = $derived.by(() => {
    const len = documents.length;
    if (len <= VIRTUALIZE_THRESHOLD) {
      return { items: documents, topPad: 0, bottomPad: 0, startIdx: 0, endIdx: len };
    }
    const { offsets, totalHeight } = rowLayout;
    const firstVisible = findRowIndexAtOffset(offsets, len, Math.max(0, scrollTop));
    const lastVisible = findRowIndexAtOffset(
      offsets,
      len,
      Math.max(0, scrollTop + viewportHeight)
    );
    const startIdx = Math.max(0, firstVisible - OVERSCAN_ROWS);
    const endIdx = Math.min(len, lastVisible + 1 + OVERSCAN_ROWS);
    return {
      items: documents.slice(startIdx, endIdx),
      topPad: offsets[startIdx],
      bottomPad: Math.max(0, totalHeight - offsets[endIdx]),
      startIdx,
      endIdx,
    };
  });

  function handleRowsScroll() {
    if (scrollRafId !== null) return;
    scrollRafId = requestAnimationFrame(() => {
      scrollRafId = null;
      if (rowsEl) {
        scrollTop = rowsEl.scrollTop;
      }
    });
  }

  $effect(() => {
    const el = rowsEl;
    if (!el) return;
    untrack(() => {
      if (el.clientHeight > 0) {
        viewportHeight = el.clientHeight;
      }
      scrollTop = el.scrollTop;
    });
    rowsResizeObserver?.disconnect();
    rowsResizeObserver = new ResizeObserver((entries) => {
      for (const ent of entries) {
        if (ent.contentRect.height > 0) {
          viewportHeight = ent.contentRect.height;
        }
      }
    });
    rowsResizeObserver.observe(el);
    return () => {
      rowsResizeObserver?.disconnect();
      rowsResizeObserver = null;
    };
  });

  // Keep keyboard/workflow-selected row visible when `selectedDocId` moves outside viewport
  let prevScrolledDocId: number | null = null;
  $effect(() => {
    const sel = selectedDocId;
    if (sel === prevScrolledDocId) return;
    prevScrolledDocId = sel;
    if (sel === null || !rowsEl) return;
    untrack(() => {
      if (!rowsEl) return;
      const idx = documents.findIndex((d) => d.document_id === sel);
      if (idx < 0) return;
      const { offsets } = rowLayout;
      const rowTop = offsets[idx];
      const rowBottom = offsets[idx + 1];
      const curTop = rowsEl.scrollTop;
      const curBottom = curTop + viewportHeight;
      if (rowTop < curTop) {
        rowsEl.scrollTop = rowTop;
        scrollTop = rowTop;
      } else if (rowBottom > curBottom) {
        const nextTop = Math.max(0, rowBottom - viewportHeight);
        rowsEl.scrollTop = nextTop;
        scrollTop = nextTop;
      }
    });
  });

  onMount(() => {
    if (rowsEl && rowsEl.clientHeight > 0) {
      viewportHeight = rowsEl.clientHeight;
    }
  });

  onDestroy(() => {
    if (scrollRafId !== null) cancelAnimationFrame(scrollRafId);
    if (measureRafId !== null) cancelAnimationFrame(measureRafId);
    rowsResizeObserver?.disconnect();
    pendingMeasureNodes.clear();
  });
</script>

<div class="doc-list">
  <div class="list-header">
    <div class="list-header-top">
      <strong>{tr(lang, 'catalogHeader')} ({documents.length})</strong>
      <span class="hint">{tr(lang, 'catalogHint')}</span>
    </div>
    <div class="list-controls">
      <div class="seg-group" role="group" aria-label={lang === 'en' ? 'Filter document type' : 'Filtra tipo documento'}>
        <button
          type="button"
          class="seg-btn"
          class:active={workCategory === 'all'}
          onclick={() => onChangeWorkCategory?.('all')}
        >
          {tr(lang, 'filterAll')}
        </button>
        <button
          type="button"
          class="seg-btn"
          class:active={workCategory === 'paper'}
          onclick={() => onChangeWorkCategory?.('paper')}
        >
          {tr(lang, 'filterPapers')}
        </button>
        <button
          type="button"
          class="seg-btn"
          class:active={workCategory === 'book'}
          onclick={() => onChangeWorkCategory?.('book')}
        >
          {tr(lang, 'filterBooks')}
        </button>
      </div>

      <div
        class="seg-group check-filter-group"
        role="group"
        aria-label={lang === 'en' ? 'Filter by metadata check status' : 'Filtra per stato controllo metadati'}
      >
        <button
          type="button"
          class="seg-btn"
          class:active={verifiedFilter === 'all'}
          title={lang === 'en' ? 'Show checked and unchecked' : 'Mostra sia controllati sia da controllare'}
          onclick={() => onChangeVerifiedFilter?.('all')}
        >
          {tr(lang, 'filterAll')}
        </button>
        <button
          type="button"
          class="seg-btn"
          class:active={verifiedFilter === 'unverified'}
          title={lang === 'en'
            ? 'Show only documents whose metadata have not been checked yet'
            : 'Mostra solo i documenti con metadati non ancora checkati'}
          onclick={() =>
            onChangeVerifiedFilter?.(verifiedFilter === 'unverified' ? 'all' : 'unverified')}
        >
          {tr(lang, 'filterUnverified')}{#if unverifiedCount !== undefined && unverifiedCount > 0} ({unverifiedCount}){/if}
        </button>
        <button
          type="button"
          class="seg-btn verified-seg"
          class:active={verifiedFilter === 'verified'}
          title={lang === 'en'
            ? 'Show only checked & verified documents'
            : 'Mostra solo i documenti già controllati (✓)'}
          onclick={() =>
            onChangeVerifiedFilter?.(verifiedFilter === 'verified' ? 'all' : 'verified')}
        >
          ✓ {tr(lang, 'filterVerified')}
        </button>
      </div>

      <div class="sort-controls">
        <select
          class="sort-select"
          aria-label={lang === 'en' ? 'Sort by' : 'Ordina per'}
          value={sortBy}
          onchange={(e) => onChangeSortBy?.((e.currentTarget as HTMLSelectElement).value as SortField)}
        >
          <option value="updated">{tr(lang, 'sortUpdated')}</option>
          <option value="title">{tr(lang, 'sortTitle')}</option>
          <option value="author">{tr(lang, 'sortAuthor')}</option>
          <option value="year">{tr(lang, 'sortYear')}</option>
        </select>
        <button
          type="button"
          class="sort-dir-btn"
          title={sortDir === 'asc' ? tr(lang, 'sortOrderAsc') : tr(lang, 'sortOrderDesc')}
          aria-label={sortDir === 'asc' ? tr(lang, 'sortOrderAsc') : tr(lang, 'sortOrderDesc')}
          onclick={() => onToggleSortDir?.()}
        >
          {#if sortDir === 'asc'}
            <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M8 13V3" />
              <path d="m4.5 6.5 3.5-3.5 3.5 3.5" />
            </svg>
          {:else}
            <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M8 3v10" />
              <path d="m4.5 9.5 3.5 3.5 3.5-3.5" />
            </svg>
          {/if}
        </button>
      </div>
    </div>
  </div>
  {#if documents.length === 0}
    <div class="empty">
      {tr(lang, 'emptyCatalog')}
    </div>
  {:else}
    <div class="rows" bind:this={rowsEl} onscroll={handleRowsScroll}>
      <div style="padding-top: {virtualWindow.topPad}px; padding-bottom: {virtualWindow.bottomPad}px;">
      {#each virtualWindow.items as doc (`${doc.document_id}-${doc.file_id}`)}
        <div
          use:measureRow={doc.document_id}
          class="doc-row"
          class:selected={doc.document_id === selectedDocId}
          class:unavailable={!doc.is_available}
          role="button"
          tabindex="0"
          onclick={() => onSelectDoc(doc)}
          ondblclick={() => onOpenDoc(doc)}
          onkeydown={(e) => e.key === 'Enter' && onOpenDoc(doc)}
          oncontextmenu={(e) => {
            e.preventDefault();
            onSelectDoc(doc);
            onContextMenu(e, doc);
          }}
        >
          <div class="doc-title">
            {#if showVerifyControl}
              {#if onToggleVerified}
                <button
                  type="button"
                  class="verify-btn"
                  class:checked={Boolean(doc.metadata_verified)}
                  title={doc.metadata_verified
                    ? tr(lang, 'unmarkVerifiedTip')
                    : tr(lang, 'markVerifiedTip')}
                  aria-label={doc.metadata_verified
                    ? tr(lang, 'unmarkVerifiedTip')
                    : tr(lang, 'markVerifiedTip')}
                  onclick={(e) => {
                    e.stopPropagation();
                    onToggleVerified(doc.document_id, !doc.metadata_verified);
                  }}
                >
                  ✓
                </button>
              {:else if doc.metadata_verified}
                <span class="verified-dot" title={tr(lang, 'unmarkVerifiedTip')}>✓</span>
              {/if}
            {/if}
            <span class="fmt-badge">{doc.format.toUpperCase()}</span>
            {#if doc.work_type === 'book'}
              <span class="book-badge">{tr(lang, 'bookBadge')}</span>
            {/if}
            <span class="title-text">{doc.title}</span>
            {#if doc.msc_primary}
              {#if doc.msc_primary_status === 'proposed'}
                <span
                  class="msc-badge proposed"
                  title="Classificazione MSC2020 proposta tramite k-NN (da confermare)"
                >
                  MSC {doc.msc_primary}?
                </span>
                {#if onConfirmMsc}
                  <button
                    class="msc-act ok"
                    title="Conferma MSC {doc.msc_primary}"
                    onclick={(e) => {
                      e.stopPropagation();
                      onConfirmMsc(doc.document_id, doc.msc_primary);
                    }}
                  >
                    ✓
                  </button>
                {/if}
                {#if onRejectMsc}
                  <button
                    class="msc-act no"
                    title="Rifiuta proposta MSC"
                    onclick={(e) => {
                      e.stopPropagation();
                      onRejectMsc(doc.document_id);
                    }}
                  >
                    ✕
                  </button>
                {/if}
              {:else}
                <span class="msc-badge" title="MSC2020 primaria">{doc.msc_primary}</span>
              {/if}
            {/if}
            {#if !doc.is_available}
              <span class="offline-badge">Non disponibile</span>
            {:else if doc.index_state === 'encrypted' || doc.index_state === 'corrupt'}
              <span class="err-badge">{doc.index_state}</span>
            {:else if !doc.has_text_layer && (doc.index_state === 'bm25_done' || doc.index_state === 'ready')}
              <span class="scan-badge" title="File scansionato senza livello di testo (tasto destro → Esegui OCR)">
                Scansione
              </span>
            {:else if doc.ocr_status === 'done'}
              <span class="ocr-badge" title="Testo indicizzato via Tesseract OCR">OCR</span>
            {/if}
            {#if doc.is_available}
              <button
                type="button"
                class="open-row-btn"
                title={lang === 'en' ? 'Open in internal viewer' : 'Apri nel visualizzatore interno'}
                onclick={(e) => {
                  e.stopPropagation();
                  onOpenDoc(doc);
                }}
              >
                {lang === 'en' ? 'Open' : 'Apri'}
              </button>
            {/if}
          </div>
          {#if doc.authors_summary || doc.year || doc.page_count}
            <div class="doc-bib">
              {#if doc.authors_summary || doc.year}
                <span>{doc.authors_summary || 'Autore sconosciuto'}</span>
                {#if doc.year}
                  <span> · {doc.year}</span>
                {/if}
              {/if}
              {#if doc.page_count}
                <span class="doc-pages">{#if doc.authors_summary || doc.year} · {/if}{doc.page_count} pp.</span>
              {/if}
            </div>
          {/if}
        </div>
      {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .doc-list {
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
    gap: 6px;
  }
  .list-header-top {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
  }
  .list-controls {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    flex-wrap: wrap;
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
  .seg-btn.verified-seg.active {
    background: #16a34a;
    color: #fff;
  }
  .verify-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 17px;
    height: 17px;
    padding: 0;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: transparent;
    font-size: 11px;
    font-weight: 800;
    line-height: 1;
    cursor: pointer;
    flex-shrink: 0;
    transition: border-color 0.12s, background 0.12s, color 0.12s;
  }
  .doc-row:hover .verify-btn:not(.checked) {
    color: var(--text-muted);
    border-color: #86efac;
  }
  .verify-btn:hover:not(.checked) {
    color: #15803d !important;
    border-color: #16a34a !important;
    background: rgba(22, 163, 74, 0.1);
  }
  .verify-btn.checked {
    background: #16a34a;
    border-color: #15803d;
    color: #fff;
  }
  .verify-btn.checked:hover {
    background: #15803d;
  }
  .verified-dot {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    border-radius: 4px;
    background: #16a34a;
    color: #fff;
    font-size: 10px;
    font-weight: 800;
    flex-shrink: 0;
  }
  .sort-controls {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-left: auto;
  }
  .sort-select {
    font-size: 11px;
    padding: 2px 6px;
    height: 24px;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
    cursor: pointer;
  }
  .sort-dir-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
    cursor: pointer;
    flex-shrink: 0;
  }
  .sort-dir-btn:hover {
    border-color: var(--accent);
    background: var(--accent-soft);
    color: var(--accent);
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
  .doc-row {
    padding: 6px 10px;
    border-bottom: 1px solid var(--border);
    cursor: pointer;
  }
  .doc-row:hover {
    background: var(--bg-muted);
  }
  .doc-row.selected {
    background: var(--accent-soft);
  }
  .doc-bib {
    margin-top: 2px;
    font-size: 11px;
    color: var(--text);
  }
  .doc-pages {
    color: var(--text-muted);
  }
  .doc-row.unavailable {
    opacity: 0.65;
  }
  .doc-title {
    display: flex;
    align-items: center;
    gap: 6px;
    font-weight: 500;
  }
  .title-text {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .open-row-btn {
    margin-left: auto;
    font-size: 11px;
    padding: 1px 7px;
    border-radius: 4px;
    flex-shrink: 0;
    opacity: 0.78;
  }
  .doc-row:hover .open-row-btn,
  .doc-row.selected .open-row-btn {
    opacity: 1;
    border-color: var(--accent);
  }
  .fmt-badge {
    font-size: 10px;
    font-weight: 700;
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .msc-badge {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 10px;
    font-weight: 700;
    padding: 1px 5px;
    border-radius: 4px;
    background: #eef2ff;
    color: #3730a3;
    border: 1px solid #c7d2fe;
    flex-shrink: 0;
  }
  .msc-badge.proposed {
    background: #fef3c7;
    color: #92400e;
    border: 1px dashed #d97706;
  }
  .msc-act {
    border: 1px solid #d1d5db;
    background: #fff;
    border-radius: 4px;
    padding: 0 4px;
    font-size: 10px;
    line-height: 1.4;
    cursor: pointer;
    flex-shrink: 0;
  }
  .msc-act.ok {
    color: #15803d;
    border-color: #86efac;
  }
  .msc-act.no {
    color: #b91c1c;
    border-color: #fca5a5;
  }
  .offline-badge {
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 4px;
    background: rgba(217, 119, 6, 0.16);
    color: var(--warn);
  }
  .err-badge {
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 4px;
    background: rgba(220, 38, 38, 0.16);
    color: var(--danger);
  }
  .scan-badge {
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 4px;
    background: rgba(217, 119, 6, 0.14);
    color: var(--warn);
  }
  .ocr-badge {
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 4px;
    background: rgba(22, 163, 74, 0.14);
    color: #15803d;
  }
  .empty {
    padding: 20px;
    color: var(--text-muted);
  }
</style>
