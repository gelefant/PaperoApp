<script lang="ts">
  import { onDestroy, onMount, tick, untrack } from 'svelte';
  import * as pdfjsLib from 'pdfjs-dist';
  import { WorkerMessageHandler } from 'pdfjs-dist/build/pdf.worker.min.mjs';
  import type { PDFDocumentProxy, RenderTask } from 'pdfjs-dist';
  import { api } from '../api';
  import type {
    ExtractedSpan,
    FilePageInfo,
    FileViewerInfo,
    NormalizedRect,
    OutlineItem,
    ViewerTargetPassage,
  } from '../types';
  import {
    computeCenteredScrollTop,
    computePassageCenterFraction,
    resolveGoToPage,
  } from '../viewerUtils';

  // Register PDF.js's built-in LoopbackPort worker handler directly in-module.
  // On macOS WKWebView (`tauri://localhost`), `new URL(location.href).origin` is `"null"`,
  // which causes PDF.js's external `new Worker(..., { type: "module" })` wrapper to hang silently.
  (globalThis as Record<string, unknown>).pdfjsWorker = { WorkerMessageHandler };

  function toUint8Array(raw: unknown): Uint8Array<ArrayBuffer> {
    if (raw instanceof ArrayBuffer) {
      return new Uint8Array(raw);
    }
    if (ArrayBuffer.isView(raw)) {
      const copy = new ArrayBuffer(raw.byteLength);
      const out = new Uint8Array(copy);
      out.set(new Uint8Array(raw.buffer, raw.byteOffset, raw.byteLength));
      return out;
    }
    if (Array.isArray(raw)) {
      const buf = new ArrayBuffer(raw.length);
      const out = new Uint8Array(buf);
      out.set(raw);
      return out;
    }
    throw new Error('Formato binario IPC non valido');
  }

  class TauriPdfRangeTransport extends pdfjsLib.PDFDataRangeTransport {
    private fid: number;
    private onRangeError: (err: unknown) => void;

    constructor(
      length: number,
      initialData: Uint8Array<ArrayBuffer>,
      fid: number,
      onRangeError: (err: unknown) => void
    ) {
      super(length, initialData, length <= initialData.byteLength);
      this.fid = fid;
      this.onRangeError = onRangeError;
    }

    override requestDataRange(begin: number, end: number): void {
      const len = Math.max(0, end - begin);
      if (len === 0) {
        this.onDataRange(begin, new Uint8Array(0));
        return;
      }
      api
        .readPdfRange(this.fid, begin, len)
        .then((ab) => {
          this.onDataRange(begin, toUint8Array(ab));
        })
        .catch((err) => {
          this.onRangeError(err);
        });
    }
  }

  interface PdfPageParams {
    doc: PDFDocumentProxy;
    pageOrd: number;
    scale: number;
    width: number;
    height: number;
  }

  function pdfPageAction(surfaceEl: HTMLDivElement, initial: PdfPageParams) {
    let current = initial;
    let activeTask: RenderTask | null = null;
    let renderToken = 0;
    let lastRenderedKey = '';

    async function draw(params: PdfPageParams) {
      const key = `${params.pageOrd}:${params.scale.toFixed(4)}:${params.width}x${params.height}`;
      if (key === lastRenderedKey) return;

      const token = ++renderToken;
      if (activeTask) {
        const prev = activeTask;
        activeTask = null;
        try {
          prev.cancel();
          await prev.promise;
        } catch {
          // Ignore cancellation errors
        }
      }
      if (token !== renderToken) return;

      const canvas = surfaceEl.querySelector('canvas');
      const textDiv = surfaceEl.querySelector('.textLayer') as HTMLDivElement | null;
      if (!canvas || !textDiv) return;

      try {
        const page = await params.doc.getPage(params.pageOrd);
        if (token !== renderToken) return;

        const dpr = window.devicePixelRatio || 1;
        const viewport = page.getViewport({ scale: params.scale });
        canvas.width = Math.max(1, Math.floor(viewport.width * dpr));
        canvas.height = Math.max(1, Math.floor(viewport.height * dpr));
        canvas.style.width = `${Math.floor(viewport.width)}px`;
        canvas.style.height = `${Math.floor(viewport.height)}px`;

        const ctx = canvas.getContext('2d');
        if (!ctx) return;

        const task = page.render({
          canvasContext: ctx,
          viewport,
          transform: dpr !== 1 ? [dpr, 0, 0, dpr, 0, 0] : undefined,
        });
        activeTask = task;
        await task.promise;
        if (token !== renderToken) return;
        activeTask = null;
        lastRenderedKey = key;

        try {
          textDiv.innerHTML = '';
          textDiv.style.width = `${Math.floor(viewport.width)}px`;
          textDiv.style.height = `${Math.floor(viewport.height)}px`;
          textDiv.style.setProperty('--scale-factor', String(params.scale));
          const textContent = await page.getTextContent();
          if (token !== renderToken) return;
          const textLayer = new pdfjsLib.TextLayer({
            textContentSource: textContent,
            container: textDiv,
            viewport,
          });
          await textLayer.render();
        } catch {
          // Keep rendered canvas even if TextLayer encounters font quirks
        }
      } catch (err) {
        if (token === renderToken) {
          activeTask = null;
          const name = (err as { name?: string })?.name;
          if (name !== 'RenderingCancelledException') {
            console.warn(`PDF page ${params.pageOrd} render error:`, err);
          }
        }
      }
    }

    void draw(current);

    return {
      update(next: PdfPageParams) {
        current = next;
        void draw(current);
      },
      destroy() {
        renderToken++;
        if (activeTask) {
          try {
            activeTask.cancel();
          } catch {
            // Ignore cancellation errors
          }
          activeTask = null;
        }
      },
    };
  }

  interface Props {
    fileId: number;
    targetPage?: number;
    highlightRects?: NormalizedRect[];
    matches?: ViewerTargetPassage[];
    activeMatchIndex?: number;
    onSelectMatchIndex?: (idx: number) => void;
    onOpenExternal?: (fileId: number, pageOrd: number) => void;
  }

  let {
    fileId,
    targetPage = 1,
    highlightRects = [],
    matches = [],
    activeMatchIndex = 0,
    onSelectMatchIndex,
    onOpenExternal,
  }: Props = $props();

  let info = $state<FileViewerInfo | null>(null);
  let pdfDoc = $state<PDFDocumentProxy | null>(null);
  let loadError = $state<string | null>(null);
  let loading = $state(true);

  let zoomMode = $state<'fit-width' | 'fit-page' | 'custom'>('fit-width');
  let customZoom = $state(1.0);
  let sidebarMode = $state<'none' | 'thumbnails' | 'outline'>('none');
  let goToInput = $state('1');
  let currentPage = $state(1);

  let docSearchQuery = $state('');
  let docSearchHits = $state<number[]>([]);
  let docSearchIdx = $state(0);
  let searchingDoc = $state(false);

  let scrollEl: HTMLDivElement | undefined = $state();
  let containerWidth = $state(760);
  let containerHeight = $state(640);
  let scrollTop = $state(0);

  let djvuSpansCache = $state<Record<number, ExtractedSpan[]>>({});
  let djvuImgUrls = $state<Record<string, string>>({});
  let djvuInFlight = new Set<string>();
  let thumbRenderedPages = new Set<number>();

  // Base page sizes (points) fallback when file_pages isn't populated yet
  let pdfDefaultSize = $state<{ w: number; h: number }>({ w: 612, h: 792 });

  const pageCount = $derived(info?.page_count || pdfDoc?.numPages || 0);

  function revokeAllDjvuUrls() {
    for (const u of Object.values(djvuImgUrls)) {
      URL.revokeObjectURL(u);
    }
    djvuImgUrls = {};
    djvuInFlight.clear();
  }

  function pageBaseDims(pageOrd: number): { w: number; h: number } {
    const found = info?.pages.find((p) => p.page_ord === pageOrd);
    if (found && found.width_pts > 0 && found.height_pts > 0) {
      return { w: found.width_pts, h: found.height_pts };
    }
    return pdfDefaultSize;
  }

  function effectiveScaleForPage(pageOrd: number): number {
    const { w, h } = pageBaseDims(pageOrd);
    const availW = Math.max(240, containerWidth - 48);
    const availH = Math.max(240, containerHeight - 36);
    if (zoomMode === 'fit-width') return availW / w;
    if (zoomMode === 'fit-page') return Math.min(availW / w, availH / h);
    return customZoom;
  }

  const pageSlots = $derived.by(() => {
    const count = pageCount;
    const slots: Array<{ pageOrd: number; top: number; width: number; height: number; scale: number }> = [];
    let y = 12;
    for (let p = 1; p <= count; p++) {
      const { w, h } = pageBaseDims(p);
      const scale = effectiveScaleForPage(p);
      const pxW = Math.round(w * scale);
      const pxH = Math.round(h * scale);
      slots.push({ pageOrd: p, top: y, width: pxW, height: pxH, scale });
      y += pxH + 16;
    }
    return { slots, totalHeight: y };
  });

  const visibleRange = $derived.by(() => {
    const slots = pageSlots.slots;
    if (!slots.length) return { start: 1, end: 0 };
    const viewTop = scrollTop;
    const viewBottom = scrollTop + containerHeight;
    let first = 1;
    let last = slots.length;
    for (let i = 0; i < slots.length; i++) {
      if (slots[i].top + slots[i].height >= viewTop) {
        first = slots[i].pageOrd;
        break;
      }
    }
    for (let i = first - 1; i < slots.length; i++) {
      if (slots[i].top > viewBottom) {
        last = Math.max(first, slots[i].pageOrd - 1);
        break;
      }
    }
    return {
      start: Math.max(1, first - 2),
      end: Math.min(slots.length, last + 2),
    };
  });

  function updateCurrentPageFromScroll() {
    if (!scrollEl) return;
    scrollTop = scrollEl.scrollTop;
    const mid = scrollTop + containerHeight * 0.35;
    for (const s of pageSlots.slots) {
      if (s.top + s.height >= mid) {
        currentPage = s.pageOrd;
        goToInput = String(s.pageOrd);
        break;
      }
    }
  }

  export async function scrollToPage(pageOrd: number, rects: NormalizedRect[] = []) {
    await tick();
    if (!scrollEl) return;
    const slot = pageSlots.slots.find((s) => s.pageOrd === pageOrd);
    if (!slot) return;
    const centerFrac = rects.length > 0 ? computePassageCenterFraction(rects) : 0.08;
    const targetTop =
      rects.length > 0
        ? computeCenteredScrollTop(slot.top, slot.height, centerFrac, containerHeight)
        : Math.max(0, slot.top - 12);
    scrollEl.scrollTo({ top: targetTop, behavior: 'auto' });
    scrollTop = targetTop;
    currentPage = pageOrd;
    goToInput = String(pageOrd);
  }

  const INLINE_PDF_MAX_BYTES = 2 * 1024 * 1024; // <= 2 MB: single IPC read; > 2 MB: 64 KB range streaming

  async function loadDocument(activeFid: number) {
    loading = true;
    loadError = null;
    thumbRenderedPages.clear();
    djvuSpansCache = {};
    revokeAllDjvuUrls();
    if (pdfDoc) {
      try {
        await pdfDoc.destroy();
      } catch {
        // Ignore cleanup errors
      }
      pdfDoc = null;
    }

    try {
      const docInfo = await api.getViewerDoc(activeFid);
      if (fileId !== activeFid) return;
      info = docInfo;
      if (!docInfo.is_available) {
        loadError = 'Cartella sorgente non raggiungibile.';
        loading = false;
        return;
      }

      if (docInfo.format === 'pdf') {
        const totalSize = docInfo.byte_size || 0;
        if (totalSize === 0) {
          throw new Error('Il file PDF risulta vuoto (0 byte).');
        }

        let loadingTask;
        if (totalSize <= INLINE_PDF_MAX_BYTES) {
          const fullBuf = toUint8Array(await api.readPdfRange(activeFid, 0, totalSize));
          loadingTask = pdfjsLib.getDocument({
            data: fullBuf,
            isOffscreenCanvasSupported: false,
            isImageDecoderSupported: false,
            useSystemFonts: true,
          });
        } else {
          const chunkSize = 65536;
          const initialBytes = toUint8Array(
            await api.readPdfRange(activeFid, 0, Math.min(chunkSize, totalSize))
          );
          const transport = new TauriPdfRangeTransport(
            totalSize,
            initialBytes,
            activeFid,
            (err) => {
              if (!loadError) {
                loadError = `Errore lettura PDF: ${String(err)}`;
              }
            }
          );
          loadingTask = pdfjsLib.getDocument({
            range: transport,
            length: totalSize,
            rangeChunkSize: chunkSize,
            disableAutoFetch: true,
            disableStream: true,
            isOffscreenCanvasSupported: false,
            isImageDecoderSupported: false,
            useSystemFonts: true,
          });
        }

        const loaded = await loadingTask.promise;
        if (fileId !== activeFid) {
          await loaded.destroy();
          return;
        }
        pdfDoc = loaded;
        if (!docInfo.pages.length && loaded.numPages > 0) {
          const firstPage = await loaded.getPage(1);
          const vp = firstPage.getViewport({ scale: 1 });
          pdfDefaultSize = { w: vp.width, h: vp.height };
        }
        if (docInfo.page_count === 0) {
          info = { ...docInfo, page_count: loaded.numPages };
        }
      }

      loading = false;
      await tick();
      if (scrollEl && scrollEl.clientWidth > 0) {
        containerWidth = scrollEl.clientWidth;
        containerHeight = scrollEl.clientHeight;
      }
      await scrollToPage(targetPage || 1, highlightRects);
    } catch (e) {
      loadError = `Impossibile aprire il documento: ${String(e)}`;
      loading = false;
    }
  }

  $effect(() => {
    const fid = fileId;
    untrack(() => {
      void loadDocument(fid);
    });
  });

  $effect(() => {
    const p = targetPage;
    const rects = highlightRects;
    const ready = !loading && info !== null;
    if (ready && p >= 1) {
      untrack(() => {
        void scrollToPage(p, rects);
      });
    }
  });

  $effect(() => {
    if (!info || loading || info.format !== 'djvu') return;
    const { start, end } = visibleRange;
    const slots = pageSlots.slots;
    untrack(() => {
      for (let p = start; p <= end; p++) {
        const slot = slots[p - 1];
        const dpi = slot && slot.scale > 1.3 ? 200 : 150;
        void ensureDjvuPageImage(p, dpi);
        void ensureDjvuText(p);
      }
    });
  });

  async function ensureDjvuPageImage(pageOrd: number, dpi: number) {
    const key = `${fileId}:${pageOrd}:${dpi}`;
    if (djvuImgUrls[key] || djvuInFlight.has(key)) return;
    djvuInFlight.add(key);
    try {
      const ab = await api.renderDjvuPage(fileId, pageOrd, dpi);
      const bytes = toUint8Array(ab);
      const url = URL.createObjectURL(new Blob([bytes], { type: 'image/png' }));
      djvuImgUrls = { ...djvuImgUrls, [key]: url };
    } catch {
      // Ignore per-page DjVu render error
    } finally {
      djvuInFlight.delete(key);
    }
  }

  function djvuThumbAction(_node: HTMLElement, pageOrd: number) {
    ensureDjvuPageImage(pageOrd, 60);
    return {};
  }

  async function ensureDjvuText(pageOrd: number) {
    if (djvuSpansCache[pageOrd]) return;
    try {
      const spans = await api.getDjvuPageText(fileId, pageOrd);
      djvuSpansCache = { ...djvuSpansCache, [pageOrd]: spans };
    } catch {
      djvuSpansCache = { ...djvuSpansCache, [pageOrd]: [] };
    }
  }

  async function renderThumbnail(canvas: HTMLCanvasElement, pageOrd: number) {
    if (!pdfDoc || thumbRenderedPages.has(pageOrd)) return;
    thumbRenderedPages.add(pageOrd);
    try {
      const page = await pdfDoc.getPage(pageOrd);
      const vp = page.getViewport({ scale: 0.2 });
      canvas.width = Math.max(1, Math.floor(vp.width));
      canvas.height = Math.max(1, Math.floor(vp.height));
      const ctx = canvas.getContext('2d');
      if (ctx) {
        await page.render({ canvasContext: ctx, viewport: vp }).promise;
      }
    } catch {
      thumbRenderedPages.delete(pageOrd);
    }
  }

  function thumbAction(node: HTMLCanvasElement, pageOrd: number) {
    renderThumbnail(node, pageOrd);
    return {};
  }

  function handleGoToSubmit(e: Event) {
    e.preventDefault();
    const pages: FilePageInfo[] = info?.pages ?? [];
    const target = resolveGoToPage(goToInput, pageCount, pages);
    if (target !== null) {
      scrollToPage(target);
    }
  }

  async function runInDocSearch(e: Event) {
    e.preventDefault();
    const q = docSearchQuery.trim().toLowerCase();
    if (!q || !info) return;
    searchingDoc = true;
    const hits: number[] = [];
    try {
      if (info.format === 'pdf' && pdfDoc) {
        for (let p = 1; p <= pdfDoc.numPages; p++) {
          const page = await pdfDoc.getPage(p);
          const tc = await page.getTextContent();
          const full = tc.items
            .map((it) => ('str' in it ? it.str : ''))
            .join(' ')
            .toLowerCase();
          if (full.includes(q)) hits.push(p);
        }
      } else if (info.format === 'djvu') {
        for (let p = 1; p <= info.page_count; p++) {
          await ensureDjvuText(p);
          const spans = djvuSpansCache[p] ?? [];
          const full = spans.map((s) => s.text).join(' ').toLowerCase();
          if (full.includes(q)) hits.push(p);
        }
      }
      docSearchHits = hits;
      docSearchIdx = 0;
      if (hits.length > 0) {
        scrollToPage(hits[0]);
      }
    } finally {
      searchingDoc = false;
    }
  }

  function stepDocSearch(delta: number) {
    if (!docSearchHits.length) return;
    docSearchIdx = (docSearchIdx + delta + docSearchHits.length) % docSearchHits.length;
    scrollToPage(docSearchHits[docSearchIdx]);
  }

  let resizeObserver: ResizeObserver | null = null;
  onMount(() => {
    if (scrollEl) {
      if (scrollEl.clientWidth > 0) {
        containerWidth = scrollEl.clientWidth;
        containerHeight = scrollEl.clientHeight;
      }
      resizeObserver = new ResizeObserver((entries) => {
        for (const ent of entries) {
          if (ent.contentRect.width > 0) {
            containerWidth = ent.contentRect.width;
          }
          if (ent.contentRect.height > 0) {
            containerHeight = ent.contentRect.height;
          }
        }
      });
      resizeObserver.observe(scrollEl);
    }
  });

  onDestroy(() => {
    resizeObserver?.disconnect();
    revokeAllDjvuUrls();
    if (pdfDoc) {
      pdfDoc.destroy();
    }
  });
</script>

<div class="viewer-shell">
  <div class="viewer-toolbar">
    <div class="group">
      <button
        type="button"
        class="v-icon-btn"
        class:active={sidebarMode === 'thumbnails'}
        title="Miniature pagine"
        aria-label="Miniature pagine"
        onclick={() => (sidebarMode = sidebarMode === 'thumbnails' ? 'none' : 'thumbnails')}
      >
        <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <rect x="3" y="3" width="7" height="7" rx="1" />
          <rect x="14" y="3" width="7" height="7" rx="1" />
          <rect x="3" y="14" width="7" height="7" rx="1" />
          <rect x="14" y="14" width="7" height="7" rx="1" />
        </svg>
      </button>
      <button
        type="button"
        class="v-icon-btn"
        class:active={sidebarMode === 'outline'}
        title="Indice segnalibri"
        aria-label="Indice segnalibri"
        onclick={() => (sidebarMode = sidebarMode === 'outline' ? 'none' : 'outline')}
      >
        <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
          <path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01" />
        </svg>
      </button>
    </div>

    <form class="group" onsubmit={handleGoToSubmit}>
      <input class="page-input" bind:value={goToInput} title="Numero di pagina (Invio per andare)" aria-label="Pagina corrente" />
      <span class="pg-total">/ {pageCount}</span>
    </form>

    <div class="group">
      <button
        type="button"
        class="v-icon-btn"
        class:active={zoomMode === 'fit-width'}
        title="Adatta alla larghezza"
        aria-label="Adatta alla larghezza"
        onclick={() => (zoomMode = 'fit-width')}
      >
        <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M4 8V5a1 1 0 0 1 1-1h14a1 1 0 0 1 1 1v3M4 16v3a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-3" />
          <path d="M7 12h10M7 12l2.5-2.5M7 12l2.5 2.5M17 12l-2.5-2.5M17 12l-2.5 2.5" />
        </svg>
      </button>
      <button
        type="button"
        class="v-icon-btn"
        class:active={zoomMode === 'fit-page'}
        title="Adatta pagina intera"
        aria-label="Adatta pagina intera"
        onclick={() => (zoomMode = 'fit-page')}
      >
        <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <rect x="5" y="3" width="14" height="18" rx="2" />
          <path d="M9 9h6M9 15h6" />
        </svg>
      </button>
      <button
        type="button"
        class="v-icon-btn"
        title="Riduci zoom"
        aria-label="Riduci zoom"
        onclick={() => {
          const curScale = effectiveScaleForPage(currentPage);
          zoomMode = 'custom';
          customZoom = Math.max(0.4, curScale - 0.15);
        }}
      >
        −
      </button>
      <span class="zoom-pct">
        {Math.round(effectiveScaleForPage(currentPage) * 100)}%
      </span>
      <button
        type="button"
        class="v-icon-btn"
        title="Aumenta zoom"
        aria-label="Aumenta zoom"
        onclick={() => {
          const curScale = effectiveScaleForPage(currentPage);
          zoomMode = 'custom';
          customZoom = Math.min(3.5, curScale + 0.15);
        }}
      >
        +
      </button>
    </div>

    <form class="group search-doc-group" onsubmit={runInDocSearch}>
      <input
        type="search"
        class="doc-search-input"
        placeholder="Trova nel doc…"
        bind:value={docSearchQuery}
      />
      <button type="submit" class="v-icon-btn" disabled={searchingDoc} title="Trova nel documento" aria-label="Trova nel documento">
        <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" aria-hidden="true">
          <circle cx="11" cy="11" r="7" />
          <path d="m20 20-3.5-3.5" />
        </svg>
      </button>
      {#if docSearchHits.length > 0}
        <button type="button" class="v-icon-btn" title="Risultato precedente" onclick={() => stepDocSearch(-1)}>◀</button>
        <span class="match-count">{docSearchIdx + 1}/{docSearchHits.length}</span>
        <button type="button" class="v-icon-btn" title="Risultato successivo" onclick={() => stepDocSearch(1)}>▶</button>
      {/if}
    </form>

    {#if matches.length > 1}
      <div class="group">
        <button
          type="button"
          class="v-icon-btn"
          title="Passaggio precedente"
          onclick={() => onSelectMatchIndex?.((activeMatchIndex - 1 + matches.length) % matches.length)}
        >
          ◀
        </button>
        <span class="match-count">{activeMatchIndex + 1}/{matches.length}</span>
        <button
          type="button"
          class="v-icon-btn"
          title="Passaggio successivo"
          onclick={() => onSelectMatchIndex?.((activeMatchIndex + 1) % matches.length)}
        >
          ▶
        </button>
      </div>
    {/if}

    {#if onOpenExternal}
      <button
        type="button"
        class="v-icon-btn"
        style="margin-left: auto;"
        title="Apri nel lettore PDF/DjVu esterno"
        aria-label="Apri nel lettore esterno"
        onclick={() => onOpenExternal?.(fileId, currentPage)}
      >
        <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
          <path d="M15 3h6v6" />
          <path d="M10 14 21 3" />
        </svg>
      </button>
    {/if}
  </div>

  <div class="viewer-body">
    {#if sidebarMode !== 'none' && info}
      <aside class="viewer-sidebar">
        {#if sidebarMode === 'thumbnails'}
          <div class="thumbs-list">
            {#each Array.from({ length: pageCount }, (_, i) => i + 1) as p (p)}
              <button
                type="button"
                class="thumb-item"
                class:current={p === currentPage}
                onclick={() => scrollToPage(p)}
              >
                {#if info.format === 'pdf'}
                  <canvas use:thumbAction={p} class="thumb-canvas"></canvas>
                {:else}
                  {@const thumbUrl = djvuImgUrls[`${fileId}:${p}:60`]}
                  <div use:djvuThumbAction={p}>
                    {#if thumbUrl}
                      <img src={thumbUrl} alt={`Pagina ${p}`} class="thumb-img" />
                    {:else}
                      <div class="thumb-placeholder">p. {p}</div>
                    {/if}
                  </div>
                {/if}
                <span>p. {p}</span>
              </button>
            {/each}
          </div>
        {:else if sidebarMode === 'outline'}
          <div class="outline-tree">
            {#if info.outline.length === 0}
              <div class="empty-side">Nessun indice disponibile nel documento.</div>
            {:else}
              {#snippet renderOutline(items: OutlineItem[], depth: number)}
                {#each items as item}
                  <button
                    type="button"
                    class="outline-item"
                    style="padding-left: {8 + depth * 12}px"
                    disabled={!item.page_ord}
                    onclick={() => item.page_ord && scrollToPage(item.page_ord)}
                  >
                    <span>{item.title}</span>
                    {#if item.page_ord}
                      <span class="out-pg">p. {item.page_ord}</span>
                    {/if}
                  </button>
                  {#if item.children.length}
                    {@render renderOutline(item.children, depth + 1)}
                  {/if}
                {/each}
              {/snippet}
              {@render renderOutline(info.outline, 0)}
            {/if}
          </div>
        {/if}
      </aside>
    {/if}

    <div class="scroll-viewport" bind:this={scrollEl} onscroll={updateCurrentPageFromScroll}>
      {#if loading}
        <div class="center-msg">Caricamento documento…</div>
      {:else if loadError}
        <div class="center-msg error">{loadError}</div>
      {:else if info}
        <div class="pages-canvas-stage" style="height: {pageSlots.totalHeight}px;">
          {#each pageSlots.slots as slot (slot.pageOrd)}
            <div
              id={`page-wrap-${fileId}-${slot.pageOrd}`}
              class="page-slot"
              style="top: {slot.top}px; width: {slot.width}px; height: {slot.height}px;"
            >
              {#if slot.pageOrd >= visibleRange.start && slot.pageOrd <= visibleRange.end}
                {#if info.format === 'pdf' && pdfDoc}
                  <div
                    class="pdf-page-surface"
                    use:pdfPageAction={{
                      doc: pdfDoc,
                      pageOrd: slot.pageOrd,
                      scale: slot.scale,
                      width: slot.width,
                      height: slot.height,
                    }}
                  >
                    <canvas class="pdf-canvas"></canvas>
                    <div class="textLayer"></div>
                  </div>
                {:else}
                  {@const pageDpi = slot.scale > 1.3 ? 200 : 150}
                  {@const pageImgUrl = djvuImgUrls[`${fileId}:${slot.pageOrd}:${pageDpi}`]}
                  {#if pageImgUrl}
                    <img
                      class="djvu-page-img"
                      src={pageImgUrl}
                      alt={`Pagina ${slot.pageOrd}`}
                    />
                  {:else}
                    <div class="page-placeholder">Rendering DjVu p. {slot.pageOrd}…</div>
                  {/if}
                  <div class="djvu-text-layer">
                    {#each djvuSpansCache[slot.pageOrd] ?? [] as sp}
                      <span
                        class="djvu-text-span"
                        style="left: {sp.bbox.x * 100}%; top: {sp.bbox.y * 100}%; width: {sp.bbox.w * 100}%; height: {sp.bbox.h * 100}%; font-size: {Math.max(8, sp.bbox.h * slot.height * 0.85)}px;"
                      >
                        {sp.text}
                      </span>
                    {/each}
                  </div>
                {/if}

                {#if slot.pageOrd === targetPage && highlightRects.length > 0}
                  <div class="highlight-layer">
                    {#each highlightRects as rect}
                      <div
                        class="highlight-box"
                        style="left: {rect.x * 100}%; top: {rect.y * 100}%; width: {rect.w * 100}%; height: {rect.h * 100}%;"
                      ></div>
                    {/each}
                  </div>
                {/if}
              {:else}
                <div class="page-placeholder">Pagina {slot.pageOrd}</div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .viewer-shell {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    overflow: hidden;
  }
  .viewer-toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    padding: 5px 10px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
  }
  .group {
    display: flex;
    align-items: center;
    gap: 3px;
  }
  .v-icon-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 28px;
    height: 28px;
    padding: 0 6px;
    border-radius: 6px;
    font-weight: 600;
  }
  button.active {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--accent);
  }
  .page-input {
    width: 48px;
    height: 28px;
    text-align: center;
    padding: 2px 4px;
  }
  .pg-total {
    font-size: 12px;
    color: var(--text-muted);
    margin-right: 2px;
  }
  .doc-search-input {
    width: 126px;
    height: 28px;
    padding: 2px 7px;
  }
  .zoom-pct,
  .match-count {
    font-size: 11.5px;
    min-width: 38px;
    text-align: center;
    color: var(--text-muted);
  }
  .viewer-body {
    flex: 1;
    min-height: 0;
    display: flex;
    overflow: hidden;
  }
  .viewer-sidebar {
    width: 210px;
    border-right: 1px solid var(--border);
    background: var(--bg-panel);
    overflow-y: auto;
    padding: 6px;
  }
  .thumbs-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .thumb-item {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 6px;
    background: transparent;
  }
  .thumb-item.current {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .thumb-canvas,
  .thumb-img {
    max-width: 150px;
    border: 1px solid var(--border);
    background: #fff;
    display: block;
  }
  .thumb-placeholder {
    width: 110px;
    height: 145px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--border);
    background: #fff;
    color: #94a3b8;
    font-size: 11px;
  }
  .outline-tree {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .outline-item {
    display: flex;
    justify-content: space-between;
    gap: 6px;
    text-align: left;
    border: none;
    background: transparent;
    padding: 4px 6px;
    font-size: 12px;
  }
  .outline-item:hover:not(:disabled) {
    background: var(--accent-soft);
  }
  .out-pg {
    color: var(--text-muted);
    font-size: 11px;
  }
  .empty-side {
    padding: 12px;
    color: var(--text-muted);
    font-size: 12px;
  }
  .scroll-viewport {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: auto;
    position: relative;
    background: var(--bg-muted);
  }
  .pages-canvas-stage {
    position: relative;
    width: 100%;
  }
  .page-slot {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    background: #fff;
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.16);
    overflow: hidden;
  }
  .pdf-page-surface {
    position: relative;
    width: 100%;
    height: 100%;
  }
  .pdf-canvas {
    display: block;
    user-select: none;
  }
  .textLayer {
    position: absolute;
    inset: 0;
    overflow: hidden;
    line-height: 1;
    text-size-adjust: none;
    forced-color-adjust: none;
    transform-origin: 0 0;
    z-index: 2;
  }
  .textLayer :global(span),
  .textLayer :global(br) {
    color: transparent;
    position: absolute;
    white-space: pre;
    cursor: text;
    transform-origin: 0% 0%;
  }
  .textLayer :global(::selection) {
    background: rgba(37, 99, 235, 0.28);
  }
  .djvu-page-img {
    width: 100%;
    height: 100%;
    display: block;
    user-select: none;
  }
  .djvu-text-layer {
    position: absolute;
    inset: 0;
    overflow: hidden;
    z-index: 2;
  }
  .djvu-text-span {
    position: absolute;
    color: transparent;
    white-space: pre;
    cursor: text;
    line-height: 1;
  }
  .djvu-text-span::selection {
    background: rgba(37, 99, 235, 0.28);
  }
  .highlight-layer {
    position: absolute;
    inset: 0;
    pointer-events: none;
    z-index: 3;
  }
  .highlight-box {
    position: absolute;
    background: var(--highlight);
    border: 1px solid var(--highlight-border);
    border-radius: 2px;
  }
  .page-placeholder {
    width: 100%;
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #94a3b8;
  }
  .center-msg {
    padding: 40px;
    text-align: center;
    color: var(--text-muted);
  }
  .center-msg.error {
    color: var(--danger);
  }
</style>
