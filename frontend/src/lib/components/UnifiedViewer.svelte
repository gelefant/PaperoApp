<script lang="ts">
  import { onDestroy, onMount, tick, untrack } from 'svelte';
  import type { PDFDocumentProxy, PDFPageProxy, RenderTask } from 'pdfjs-dist';
  import { api } from '../api';
  import { createPdfRangeTransport, loadPdfJs, type PdfJsModule } from '../pdfjsLoader';
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
  import { attachSelectableTextLayer } from '../textLayerSelection';

  const MAX_CANVAS_PIXELS = 16_777_216; // 16 MP cap prevents 80+ MB Retina BackingStore spikes
  const THUMB_ROW_HEIGHT = 176;
  const DJVU_IMG_CACHE_LIMIT = 28;
  const DJVU_SPANS_CACHE_LIMIT = 64;

  function toUint8Array(raw: unknown): Uint8Array<ArrayBuffer> {
    if (raw instanceof ArrayBuffer) {
      return new Uint8Array(raw);
    }
    if (raw instanceof Uint8Array) {
      if (
        raw.buffer instanceof ArrayBuffer &&
        raw.byteOffset === 0 &&
        raw.byteLength === raw.buffer.byteLength
      ) {
        return raw as Uint8Array<ArrayBuffer>;
      }
      const copy = new ArrayBuffer(raw.byteLength);
      const out = new Uint8Array(copy);
      out.set(raw);
      return out;
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

  interface PdfPageParams {
    pdfjsLib: PdfJsModule;
    doc: PDFDocumentProxy;
    pageOrd: number;
    scale: number;
    width: number;
    height: number;
  }

  function pdfPageAction(surfaceEl: HTMLDivElement, initial: PdfPageParams) {
    let current = initial;
    let activeTask: RenderTask | null = null;
    let renderedPage: PDFPageProxy | null = null;
    let detachSelection: (() => void) | null = null;
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
        if (token !== renderToken) {
          page.cleanup();
          return;
        }
        if (renderedPage && renderedPage !== page) {
          renderedPage.cleanup();
        }
        renderedPage = page;

        const baseDpr = window.devicePixelRatio || 1;
        const viewport = page.getViewport({ scale: params.scale });
        const rawPixels = Math.max(1, viewport.width * baseDpr) * Math.max(1, viewport.height * baseDpr);
        const renderDpr =
          rawPixels > MAX_CANVAS_PIXELS
            ? baseDpr * Math.sqrt(MAX_CANVAS_PIXELS / rawPixels)
            : baseDpr;

        canvas.width = Math.max(1, Math.floor(viewport.width * renderDpr));
        canvas.height = Math.max(1, Math.floor(viewport.height * renderDpr));
        canvas.style.width = `${Math.floor(viewport.width)}px`;
        canvas.style.height = `${Math.floor(viewport.height)}px`;

        const ctx = canvas.getContext('2d');
        if (!ctx) return;

        const task = page.render({
          canvasContext: ctx,
          viewport,
          transform: renderDpr !== 1 ? [renderDpr, 0, 0, renderDpr, 0, 0] : undefined,
        });
        activeTask = task;
        await task.promise;
        if (token !== renderToken) return;
        activeTask = null;
        lastRenderedKey = key;

        try {
          detachSelection?.();
          detachSelection = null;
          textDiv.textContent = '';
          textDiv.style.width = `${Math.floor(viewport.width)}px`;
          textDiv.style.height = `${Math.floor(viewport.height)}px`;
          textDiv.style.setProperty('--scale-factor', String(params.scale));
          const textContent = await page.getTextContent();
          if (token !== renderToken) return;
          const textLayer = new params.pdfjsLib.TextLayer({
            textContentSource: textContent,
            container: textDiv,
            viewport,
          });
          await textLayer.render();
          if (token !== renderToken) return;
          detachSelection = attachSelectableTextLayer(textDiv);
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
        detachSelection?.();
        detachSelection = null;
        if (activeTask) {
          try {
            activeTask.cancel();
          } catch {
            // Ignore cancellation errors
          }
          activeTask = null;
        }
        const canvas = surfaceEl.querySelector('canvas');
        if (canvas) {
          canvas.width = 0;
          canvas.height = 0;
        }
        const textDiv = surfaceEl.querySelector('.textLayer');
        if (textDiv) {
          textDiv.textContent = '';
        }
        if (renderedPage) {
          try {
            renderedPage.cleanup();
          } catch {
            // Ignore cleanup errors
          }
          renderedPage = null;
        }
      },
    };
  }

  function djvuTextLayerAction(node: HTMLDivElement) {
    const detach = attachSelectableTextLayer(node);
    return {
      destroy() {
        detach();
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

  let info = $state.raw<FileViewerInfo | null>(null);
  let pdfjsMod = $state.raw<PdfJsModule | null>(null);
  let pdfDoc = $state.raw<PDFDocumentProxy | null>(null);
  let loadError = $state<string | null>(null);
  let loading = $state(true);

  let zoomMode = $state<'fit-width' | 'fit-page' | 'custom'>('fit-width');
  let customZoom = $state(1.0);
  let sidebarMode = $state<'none' | 'thumbnails' | 'outline'>('none');
  let goToInput = $state('1');
  let currentPage = $state(1);

  let docSearchQuery = $state('');
  let docSearchHits = $state.raw<number[]>([]);
  let docSearchIdx = $state(0);
  let searchingDoc = $state(false);

  let scrollEl: HTMLDivElement | undefined = $state();
  let pageInputEl: HTMLInputElement | undefined = $state();
  let thumbSidebarEl: HTMLElement | undefined = $state();
  let containerWidth = $state(760);
  let containerHeight = $state(640);
  let scrollTop = $state(0);
  let thumbScrollTop = $state(0);
  let thumbViewportHeight = $state(600);

  let djvuSpansCache = $state.raw<Record<number, ExtractedSpan[]>>({});
  let djvuImgUrls = $state.raw<Record<string, string>>({});
  const djvuImgOrder: string[] = [];
  const djvuSpansOrder: number[] = [];
  const djvuInFlight = new Set<string>();

  let destroyed = false;
  let loadSeq = 0;
  let docSearchSeq = 0;
  let scrollRafId: number | null = null;
  let activeLoadingTask: { destroy?: () => Promise<void> | void } | null = null;

  // Base page sizes (points) fallback when file_pages isn't populated yet
  let pdfDefaultSize = $state<{ w: number; h: number }>({ w: 612, h: 792 });

  const pageCount = $derived(info?.page_count || pdfDoc?.numPages || 0);

  const pageDimsByOrd = $derived.by(() => {
    const map = new Map<number, { w: number; h: number }>();
    if (info?.pages) {
      for (const p of info.pages) {
        if (p.width_pts > 0 && p.height_pts > 0) {
          map.set(p.page_ord, { w: p.width_pts, h: p.height_pts });
        }
      }
    }
    return map;
  });

  function revokeAllDjvuUrls() {
    for (const u of Object.values(djvuImgUrls)) {
      URL.revokeObjectURL(u);
    }
    djvuImgUrls = {};
    djvuImgOrder.length = 0;
    djvuInFlight.clear();
  }

  function pageBaseDims(pageOrd: number): { w: number; h: number } {
    return pageDimsByOrd.get(pageOrd) ?? pdfDefaultSize;
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
    const dimsMap = pageDimsByOrd;
    const defSize = pdfDefaultSize;
    const availW = Math.max(240, containerWidth - 48);
    const availH = Math.max(240, containerHeight - 36);
    const mode = zoomMode;
    const cZoom = customZoom;

    const slots: Array<{ pageOrd: number; top: number; width: number; height: number; scale: number }> =
      new Array(count);
    let y = 12;
    for (let p = 1; p <= count; p++) {
      const dims = dimsMap.get(p) ?? defSize;
      const scale =
        mode === 'fit-width'
          ? availW / dims.w
          : mode === 'fit-page'
            ? Math.min(availW / dims.w, availH / dims.h)
            : cZoom;
      const pxW = Math.round(dims.w * scale);
      const pxH = Math.round(dims.h * scale);
      slots[p - 1] = { pageOrd: p, top: y, width: pxW, height: pxH, scale };
      y += pxH + 16;
    }
    return { slots, totalHeight: y };
  });

  function findFirstSlotAtOrBelow(
    slots: Array<{ pageOrd: number; top: number; height: number }>,
    targetY: number
  ): number {
    let lo = 0;
    let hi = slots.length - 1;
    let ans = slots.length - 1;
    while (lo <= hi) {
      const mid = (lo + hi) >>> 1;
      if (slots[mid].top + slots[mid].height >= targetY) {
        ans = mid;
        hi = mid - 1;
      } else {
        lo = mid + 1;
      }
    }
    return ans;
  }

  const visibleRange = $derived.by(() => {
    const slots = pageSlots.slots;
    if (!slots.length) return { start: 1, end: 0 };
    const viewTop = scrollTop;
    const viewBottom = scrollTop + containerHeight;
    const firstIdx = findFirstSlotAtOrBelow(slots, viewTop);
    const first = slots[firstIdx].pageOrd;
    let last = slots.length;
    for (let i = firstIdx; i < slots.length; i++) {
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

  const visibleSlots = $derived.by(() => {
    const { start, end } = visibleRange;
    if (end < start) return [];
    return pageSlots.slots.slice(start - 1, end);
  });

  const thumbWindow = $derived.by(() => {
    const total = pageCount;
    if (sidebarMode !== 'thumbnails' || total <= 0) {
      return { pages: [] as number[], topPad: 0, bottomPad: 0, start: 1, end: 0 };
    }
    const firstIdx = Math.max(0, Math.floor(thumbScrollTop / THUMB_ROW_HEIGHT) - 6);
    const visibleCount = Math.ceil(thumbViewportHeight / THUMB_ROW_HEIGHT) + 12;
    const endIdx = Math.min(total, firstIdx + visibleCount);
    const pages: number[] = [];
    for (let p = firstIdx + 1; p <= endIdx; p++) {
      pages.push(p);
    }
    return {
      pages,
      topPad: firstIdx * THUMB_ROW_HEIGHT,
      bottomPad: Math.max(0, (total - endIdx) * THUMB_ROW_HEIGHT),
      start: firstIdx + 1,
      end: endIdx,
    };
  });

  function handleViewportScroll() {
    if (scrollRafId !== null) return;
    scrollRafId = requestAnimationFrame(() => {
      scrollRafId = null;
      if (!scrollEl || destroyed) return;
      scrollTop = scrollEl.scrollTop;
      const slots = pageSlots.slots;
      if (!slots.length) return;
      const mid = scrollTop + containerHeight * 0.35;
      const idx = findFirstSlotAtOrBelow(slots, mid);
      const pg = slots[idx]?.pageOrd ?? 1;
      if (currentPage !== pg) {
        currentPage = pg;
      }
      if (document.activeElement !== pageInputEl) {
        const nextStr = String(pg);
        if (goToInput !== nextStr) {
          goToInput = nextStr;
        }
      }
    });
  }

  function handleThumbSidebarScroll() {
    if (!thumbSidebarEl) return;
    thumbScrollTop = thumbSidebarEl.scrollTop;
    if (thumbSidebarEl.clientHeight > 0) {
      thumbViewportHeight = thumbSidebarEl.clientHeight;
    }
  }

  export async function scrollToPage(pageOrd: number, rects: NormalizedRect[] = []) {
    await tick();
    if (!scrollEl || destroyed) return;
    const slot = pageSlots.slots[pageOrd - 1];
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
    const seq = ++loadSeq;
    docSearchSeq++;
    searchingDoc = false;
    docSearchHits = [];
    docSearchIdx = 0;
    loading = true;
    loadError = null;
    djvuSpansCache = {};
    djvuSpansOrder.length = 0;
    revokeAllDjvuUrls();

    if (activeLoadingTask) {
      try {
        await activeLoadingTask.destroy?.();
      } catch {
        // Ignore task cancellation errors
      }
      activeLoadingTask = null;
    }
    if (pdfDoc) {
      const prevDoc = pdfDoc;
      pdfDoc = null;
      try {
        await prevDoc.destroy();
      } catch {
        // Ignore cleanup errors
      }
    }

    try {
      const docInfo = await api.getViewerDoc(activeFid);
      if (destroyed || seq !== loadSeq || fileId !== activeFid) return;
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

        const pdfjsLib = await loadPdfJs();
        if (destroyed || seq !== loadSeq || fileId !== activeFid) return;
        pdfjsMod = pdfjsLib;

        let loadingTask;
        if (totalSize <= INLINE_PDF_MAX_BYTES) {
          const fullBuf = toUint8Array(await api.readPdfRange(activeFid, 0, totalSize));
          if (destroyed || seq !== loadSeq || fileId !== activeFid) return;
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
          if (destroyed || seq !== loadSeq || fileId !== activeFid) return;
          const transport = createPdfRangeTransport(
            pdfjsLib,
            totalSize,
            initialBytes,
            async (begin, len) => toUint8Array(await api.readPdfRange(activeFid, begin, len)),
            () => destroyed || seq !== loadSeq || fileId !== activeFid,
            (err) => {
              if (!destroyed && seq === loadSeq && !loadError) {
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

        activeLoadingTask = loadingTask;
        const loaded = await loadingTask.promise;
        if (activeLoadingTask === loadingTask) {
          activeLoadingTask = null;
        }
        if (destroyed || seq !== loadSeq || fileId !== activeFid) {
          await loaded.destroy();
          return;
        }
        pdfDoc = loaded;
        if (!docInfo.pages.length && loaded.numPages > 0) {
          const firstPage = await loaded.getPage(1);
          const vp = firstPage.getViewport({ scale: 1 });
          pdfDefaultSize = { w: vp.width, h: vp.height };
          firstPage.cleanup();
        }
        if (docInfo.page_count === 0) {
          info = { ...docInfo, page_count: loaded.numPages };
        }
      }

      if (destroyed || seq !== loadSeq) return;
      loading = false;
      await tick();
      if (destroyed || seq !== loadSeq) return;
      if (scrollEl && scrollEl.clientWidth > 0) {
        containerWidth = scrollEl.clientWidth;
        containerHeight = scrollEl.clientHeight;
      }
      await scrollToPage(targetPage || 1, highlightRects);
    } catch (e) {
      if (!destroyed && seq === loadSeq) {
        loadError = `Impossibile aprire il documento: ${String(e)}`;
        loading = false;
      }
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
    const seq = loadSeq;
    untrack(() => {
      for (let p = start; p <= end; p++) {
        const slot = slots[p - 1];
        const dpi = slot && slot.scale > 1.3 ? 200 : 150;
        void ensureDjvuPageImage(p, dpi, seq);
        void ensureDjvuText(p, seq);
      }
    });
  });

  function insertBoundedDjvuUrl(key: string, url: string) {
    const next = { ...djvuImgUrls, [key]: url };
    djvuImgOrder.push(key);
    if (djvuImgOrder.length > DJVU_IMG_CACHE_LIMIT) {
      const { start, end } = visibleRange;
      const tStart = thumbWindow.start;
      const tEnd = thumbWindow.end;
      let i = 0;
      while (djvuImgOrder.length > DJVU_IMG_CACHE_LIMIT && i < djvuImgOrder.length) {
        const candKey = djvuImgOrder[i];
        const parts = candKey.split(':');
        const pg = Number(parts[1] ?? '0');
        const dpi = Number(parts[2] ?? '0');
        const pinMain = dpi !== 60 && pg >= start && pg <= end;
        const pinThumb = dpi === 60 && pg >= tStart && pg <= tEnd;
        if (!pinMain && !pinThumb) {
          djvuImgOrder.splice(i, 1);
          const oldUrl = next[candKey];
          if (oldUrl) {
            URL.revokeObjectURL(oldUrl);
            delete next[candKey];
          }
        } else {
          i++;
        }
      }
    }
    djvuImgUrls = next;
  }

  async function ensureDjvuPageImage(pageOrd: number, dpi: number, seq = loadSeq) {
    const activeFid = fileId;
    const key = `${activeFid}:${pageOrd}:${dpi}`;
    if (djvuImgUrls[key] || djvuInFlight.has(key)) return;
    djvuInFlight.add(key);
    try {
      const ab = await api.renderDjvuPage(activeFid, pageOrd, dpi);
      if (destroyed || seq !== loadSeq || fileId !== activeFid) return;
      const bytes = toUint8Array(ab);
      const url = URL.createObjectURL(new Blob([bytes], { type: 'image/png' }));
      if (destroyed || seq !== loadSeq || fileId !== activeFid) {
        URL.revokeObjectURL(url);
        return;
      }
      insertBoundedDjvuUrl(key, url);
    } catch {
      // Ignore per-page DjVu render error
    } finally {
      djvuInFlight.delete(key);
    }
  }

  function djvuThumbAction(_node: HTMLElement, pageOrd: number) {
    void ensureDjvuPageImage(pageOrd, 60);
    return {};
  }

  async function ensureDjvuText(pageOrd: number, seq = loadSeq) {
    if (djvuSpansCache[pageOrd] !== undefined) return;
    const activeFid = fileId;
    try {
      const spans = await api.getDjvuPageText(activeFid, pageOrd);
      if (destroyed || seq !== loadSeq || fileId !== activeFid) return;
      const next = { ...djvuSpansCache, [pageOrd]: spans };
      djvuSpansOrder.push(pageOrd);
      while (djvuSpansOrder.length > DJVU_SPANS_CACHE_LIMIT) {
        const oldest = djvuSpansOrder.shift();
        if (oldest !== undefined && (oldest < visibleRange.start || oldest > visibleRange.end)) {
          delete next[oldest];
        }
      }
      djvuSpansCache = next;
    } catch {
      if (!destroyed && seq === loadSeq && fileId === activeFid) {
        djvuSpansCache = { ...djvuSpansCache, [pageOrd]: [] };
      }
    }
  }

  function thumbAction(canvas: HTMLCanvasElement, pageOrd: number) {
    let cancelled = false;
    let activeTask: RenderTask | null = null;
    let renderedThumbPage: PDFPageProxy | null = null;

    void (async () => {
      const doc = pdfDoc;
      if (!doc || cancelled || destroyed) return;
      try {
        const page = await doc.getPage(pageOrd);
        if (cancelled || destroyed || pdfDoc !== doc) {
          page.cleanup();
          return;
        }
        renderedThumbPage = page;
        const vp = page.getViewport({ scale: 0.2 });
        canvas.width = Math.max(1, Math.floor(vp.width));
        canvas.height = Math.max(1, Math.floor(vp.height));
        const ctx = canvas.getContext('2d');
        if (!ctx) return;
        const task = page.render({ canvasContext: ctx, viewport: vp });
        activeTask = task;
        await task.promise;
        activeTask = null;
      } catch {
        // Ignore cancelled/transient thumbnail errors
      }
    })();

    return {
      destroy() {
        cancelled = true;
        if (activeTask) {
          try {
            activeTask.cancel();
          } catch {
            // Ignore cancellation
          }
          activeTask = null;
        }
        canvas.width = 0;
        canvas.height = 0;
        if (renderedThumbPage) {
          try {
            renderedThumbPage.cleanup();
          } catch {
            // Ignore cleanup errors
          }
          renderedThumbPage = null;
        }
      },
    };
  }

  function handleGoToSubmit(e: Event) {
    e.preventDefault();
    const pages: FilePageInfo[] = info?.pages ?? [];
    const target = resolveGoToPage(goToInput, pageCount, pages);
    if (target !== null) {
      void scrollToPage(target);
    }
  }

  async function runInDocSearch(e: Event) {
    e.preventDefault();
    const q = docSearchQuery.trim().toLowerCase();
    if (!q || !info) return;
    const seq = ++docSearchSeq;
    const activeLoad = loadSeq;
    searchingDoc = true;
    const hits: number[] = [];
    try {
      if (info.format === 'pdf' && pdfDoc) {
        const doc = pdfDoc;
        for (let p = 1; p <= doc.numPages; p++) {
          if (destroyed || seq !== docSearchSeq || activeLoad !== loadSeq) return;
          const page = await doc.getPage(p);
          try {
            const tc = await page.getTextContent();
            let matched = false;
            let combined = '';
            for (const it of tc.items) {
              if ('str' in it && it.str) {
                combined += (combined ? ' ' : '') + it.str.toLowerCase();
              }
            }
            matched = combined.includes(q);
            if (matched) hits.push(p);
          } finally {
            page.cleanup();
          }
          if (p % 25 === 0) {
            await new Promise((r) => setTimeout(r, 0));
          }
        }
      } else if (info.format === 'djvu') {
        const total = info.page_count;
        const fid = fileId;
        for (let p = 1; p <= total; p++) {
          if (destroyed || seq !== docSearchSeq || activeLoad !== loadSeq) return;
          const spans = djvuSpansCache[p] ?? (await api.getDjvuPageText(fid, p).catch(() => []));
          const full = spans.map((s) => s.text).join(' ').toLowerCase();
          if (full.includes(q)) hits.push(p);
          if (p % 15 === 0) {
            await new Promise((r) => setTimeout(r, 0));
          }
        }
      }
      if (destroyed || seq !== docSearchSeq || activeLoad !== loadSeq) return;
      docSearchHits = hits;
      docSearchIdx = 0;
      if (hits.length > 0) {
        void scrollToPage(hits[0]);
      }
    } finally {
      if (!destroyed && seq === docSearchSeq) {
        searchingDoc = false;
      }
    }
  }

  function stepDocSearch(delta: number) {
    if (!docSearchHits.length) return;
    docSearchIdx = (docSearchIdx + delta + docSearchHits.length) % docSearchHits.length;
    void scrollToPage(docSearchHits[docSearchIdx]);
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
    destroyed = true;
    loadSeq++;
    docSearchSeq++;
    if (scrollRafId !== null) {
      cancelAnimationFrame(scrollRafId);
      scrollRafId = null;
    }
    resizeObserver?.disconnect();
    revokeAllDjvuUrls();
    if (activeLoadingTask) {
      try {
        void activeLoadingTask.destroy?.();
      } catch {
        // Ignore cleanup errors
      }
      activeLoadingTask = null;
    }
    if (pdfDoc) {
      const prev = pdfDoc;
      pdfDoc = null;
      void prev.destroy();
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
        onclick={() => {
          sidebarMode = sidebarMode === 'thumbnails' ? 'none' : 'thumbnails';
          thumbScrollTop = Math.max(0, (currentPage - 2) * THUMB_ROW_HEIGHT);
        }}
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
      <input
        class="page-input"
        bind:this={pageInputEl}
        bind:value={goToInput}
        title="Numero di pagina (Invio per andare)"
        aria-label="Pagina corrente"
      />
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
      <aside class="viewer-sidebar" bind:this={thumbSidebarEl} onscroll={handleThumbSidebarScroll}>
        {#if sidebarMode === 'thumbnails'}
          <div
            class="thumbs-list"
            style="padding-top: {thumbWindow.topPad}px; padding-bottom: {thumbWindow.bottomPad}px;"
          >
            {#each thumbWindow.pages as p (p)}
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

    <div class="scroll-viewport" bind:this={scrollEl} onscroll={handleViewportScroll}>
      {#if loading}
        <div class="center-msg">Caricamento documento…</div>
      {:else if loadError}
        <div class="center-msg error">{loadError}</div>
      {:else if info}
        <div class="pages-canvas-stage" style="height: {pageSlots.totalHeight}px;">
          {#each visibleSlots as slot (slot.pageOrd)}
            <div
              id={`page-wrap-${fileId}-${slot.pageOrd}`}
              class="page-slot"
              style="top: {slot.top}px; width: {slot.width}px; height: {slot.height}px;"
            >
              {#if info.format === 'pdf' && pdfDoc && pdfjsMod}
                <div
                  class="pdf-page-surface"
                  use:pdfPageAction={{
                    pdfjsLib: pdfjsMod,
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
                    draggable="false"
                  />
                {:else}
                  <div class="page-placeholder">Rendering DjVu p. {slot.pageOrd}…</div>
                {/if}
                <div class="djvu-text-layer" use:djvuTextLayerAction>
                  {#each djvuSpansCache[slot.pageOrd] ?? [] as sp}
                    <span
                      class="djvu-text-span"
                      style="left: {sp.bbox.x * 100}%; top: {sp.bbox.y * 100}%; width: {sp.bbox.w * 100}%; height: {sp.bbox.h * 100}%; font-size: {Math.max(8, sp.bbox.h * slot.height * 0.85)}px;"
                    >{sp.text}</span>
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
    contain: content;
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
    justify-content: center;
    height: 168px;
    box-sizing: border-box;
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
    max-height: 132px;
    border: 1px solid var(--border);
    background: #fff;
    display: block;
  }
  .thumb-placeholder {
    width: 110px;
    height: 132px;
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
    contain: strict;
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
    contain: layout paint;
  }
  .pdf-page-surface {
    position: relative;
    width: 100%;
    height: 100%;
  }
  .pdf-canvas {
    display: block;
    pointer-events: none;
    -webkit-user-select: none;
    user-select: none;
  }
  .djvu-page-img {
    width: 100%;
    height: 100%;
    display: block;
    pointer-events: none;
    -webkit-user-select: none;
    user-select: none;
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
