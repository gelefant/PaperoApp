<script lang="ts">
  import { onDestroy, onMount, tick, untrack } from 'svelte';
  import type { PDFDocumentProxy, PDFPageProxy, RenderTask } from 'pdfjs-dist';
  import { save as saveFileDialog } from '@tauri-apps/plugin-dialog';
  import { api } from '../api';
  import { tr, type Lang } from '../i18n';
  import { createPdfRangeTransport, loadPdfJs, type PdfJsModule } from '../pdfjsLoader';
  import {
    djvuSpansToPositionedItems,
    findMatchesInPositionedItems,
    pdfTextContentToPositionedItems,
    type InDocSearchMatch,
    type PositionedTextItem,
  } from '../inDocSearch';
  import type {
    CitationLinkItem,
    DocLlmScope,
    ExtractedSpan,
    FileAnnotation,
    FilePageInfo,
    FileViewerInfo,
    LlmProvider,
    NormalizedRect,
    OutlineItem,
    ViewerTargetPassage,
  } from '../types';
  import {
    computeCenteredScrollTop,
    computePassageCenterFraction,
    resolveGoToPage,
  } from '../viewerUtils';
  import {
    cropSourceElementToPngDataUrl,
    extractBibEntryNearViewportY,
    normalizeDragRect,
    type PositionedLineToken,
  } from '../viewerSnipUtils';
  import { attachSelectableTextLayer } from '../textLayerSelection';
  import ViewerAnnotationsPane from './ViewerAnnotationsPane.svelte';
  import ViewerAskPanel from './ViewerAskPanel.svelte';
  import ViewerSnipPopover from './ViewerSnipPopover.svelte';

  const MAX_CANVAS_PIXELS = 16_777_216; // 16 MP cap prevents 80+ MB Retina BackingStore spikes
  const THUMB_ROW_HEIGHT = 176;
  const DJVU_IMG_CACHE_LIMIT = 28;
  const DJVU_SPANS_CACHE_LIMIT = 64;
  const COLOR_MODE_STORAGE_KEY = 'papero_viewer_color_mode';

  type PageColorMode = 'normal' | 'sepia' | 'night';

  function loadSavedColorMode(): PageColorMode {
    try {
      const v = localStorage.getItem(COLOR_MODE_STORAGE_KEY);
      if (v === 'sepia' || v === 'night') return v;
    } catch {
      // Ignore storage restrictions
    }
    return 'normal';
  }

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

  interface PdfInternalLink {
    rect: NormalizedRect;
    dest: unknown;
  }

  interface PdfPageParams {
    pdfjsLib: PdfJsModule;
    doc: PDFDocumentProxy;
    pageOrd: number;
    scale: number;
    rotation: number;
    width: number;
    height: number;
    onInternalLink: (dest: unknown) => void;
    onInternalLinkHover: (dest: unknown, anchorRect: DOMRect) => void;
    onInternalLinkLeave: () => void;
  }

  function pdfPageAction(surfaceEl: HTMLDivElement, initial: PdfPageParams) {
    let current = initial;
    let activeTask: RenderTask | null = null;
    let renderedPage: PDFPageProxy | null = null;
    let detachSelection: (() => void) | null = null;
    let renderToken = 0;
    let lastRenderedKey = '';

    async function draw(params: PdfPageParams) {
      const key = `${params.pageOrd}:${params.scale.toFixed(4)}:${params.rotation}:${params.width}x${params.height}`;
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
      const linkDiv = surfaceEl.querySelector('.linkLayer') as HTMLDivElement | null;
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
        const effectiveRot = (page.rotate + params.rotation) % 360;
        const viewport = page.getViewport({ scale: params.scale, rotation: effectiveRot });
        const rawPixels =
          Math.max(1, viewport.width * baseDpr) * Math.max(1, viewport.height * baseDpr);
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

        // TextLayer
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

        // Lightweight internal-only PDF Link overlay (`\ref`, `\cite`, TOC jumps)
        if (linkDiv) {
          try {
            linkDiv.textContent = '';
            const anns = await page.getAnnotations({ intent: 'display' });
            if (token !== renderToken) return;
            const vpW = Math.max(1, viewport.width);
            const vpH = Math.max(1, viewport.height);
            const links: PdfInternalLink[] = [];
            for (const a of anns) {
              if (a?.subtype !== 'Link') continue;
              // Internal links only (explicit dest or internal GoTo); never trigger external URLs
              if (!a.dest || a.url || a.unsafeUrl) continue;
              if (!Array.isArray(a.rect) || a.rect.length < 4) continue;
              const vr = viewport.convertToViewportRectangle(a.rect);
              const x1 = Math.min(vr[0], vr[2]);
              const x2 = Math.max(vr[0], vr[2]);
              const y1 = Math.min(vr[1], vr[3]);
              const y2 = Math.max(vr[1], vr[3]);
              if (x2 <= x1 || y2 <= y1) continue;
              links.push({
                dest: a.dest,
                rect: {
                  x: Math.max(0, Math.min(1, x1 / vpW)),
                  y: Math.max(0, Math.min(1, y1 / vpH)),
                  w: Math.max(0.004, Math.min(1, (x2 - x1) / vpW)),
                  h: Math.max(0.004, Math.min(1, (y2 - y1) / vpH)),
                },
              });
            }
            for (const lnk of links) {
              const btn = document.createElement('button');
              btn.type = 'button';
              btn.className = 'pdf-link-hotspot';
              btn.title = 'Vai al riferimento interno';
              btn.style.left = `${lnk.rect.x * 100}%`;
              btn.style.top = `${lnk.rect.y * 100}%`;
              btn.style.width = `${lnk.rect.w * 100}%`;
              btn.style.height = `${lnk.rect.h * 100}%`;
              btn.addEventListener('click', (ev) => {
                ev.preventDefault();
                ev.stopPropagation();
                current.onInternalLinkLeave();
                current.onInternalLink(lnk.dest);
              });
              btn.addEventListener('mouseenter', () => {
                current.onInternalLinkHover(lnk.dest, btn.getBoundingClientRect());
              });
              btn.addEventListener('mouseleave', () => {
                current.onInternalLinkLeave();
              });
              linkDiv.appendChild(btn);
            }
          } catch {
            // Ignore malformed PDF link annotations
          }
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
        const linkDiv = surfaceEl.querySelector('.linkLayer');
        if (linkDiv) {
          linkDiv.textContent = '';
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
    lang?: Lang;
    fileId: number;
    targetPage?: number;
    explicitPageJump?: boolean;
    highlightRects?: NormalizedRect[];
    matches?: ViewerTargetPassage[];
    activeMatchIndex?: number;
    isKeyboardActive?: boolean;
    llmProvider?: LlmProvider;
    onSelectMatchIndex?: (idx: number) => void;
    onOpenExternal?: (fileId: number, pageOrd: number) => void;
    onOpenLibraryDoc?: (doc: CitationLinkItem, inSplit: boolean) => void;
    onConsentAccepted?: (provider: LlmProvider) => void;
    onAnnotationsChanged?: () => void;
    onStatusMessage?: (msg: string) => void;
  }

  let {
    lang = 'it',
    fileId,
    targetPage = 1,
    explicitPageJump = false,
    highlightRects = [],
    matches = [],
    activeMatchIndex = 0,
    isKeyboardActive = true,
    llmProvider = 'disabled',
    onSelectMatchIndex,
    onOpenExternal,
    onOpenLibraryDoc,
    onConsentAccepted,
    onAnnotationsChanged,
    onStatusMessage,
  }: Props = $props();

  let info = $state.raw<FileViewerInfo | null>(null);
  let pdfjsMod = $state.raw<PdfJsModule | null>(null);
  let pdfDoc = $state.raw<PDFDocumentProxy | null>(null);
  let loadError = $state<string | null>(null);
  let loading = $state(true);

  let zoomMode = $state<'fit-width' | 'fit-page' | 'custom'>('fit-width');
  let customZoom = $state(1.0);
  let rotationDeg = $state(0);
  let spreadMode = $state(false);
  let colorMode = $state<PageColorMode>(loadSavedColorMode());
  let showViewPopover = $state(false);

  let sidebarMode = $state<'none' | 'thumbnails' | 'outline' | 'annotations'>('none');
  let goToInput = $state('1');
  let currentPage = $state(1);

  // Per-tab jump history (Alt+Left / Alt+Right)
  let jumpBackStack = $state<number[]>([]);
  let jumpForwardStack = $state<number[]>([]);

  // Find in document (Cmd/Ctrl+F) with visual highlight boxes
  let findBarOpen = $state(false);
  let docSearchQuery = $state('');
  let docSearchMatches = $state.raw<InDocSearchMatch[]>([]);
  let docSearchIdx = $state(0);
  let searchingDoc = $state(false);
  let findInputEl: HTMLInputElement | undefined = $state();

  // Annotations (non-destructive highlights, bookmarks, notes)
  let annotations = $state.raw<FileAnnotation[]>([]);
  let selectionPopover = $state<{
    pageOrd: number;
    text: string;
    rects: NormalizedRect[];
    x: number;
    y: number;
    addingNote: boolean;
    noteDraft: string;
    noteColor: 'yellow' | 'green' | 'blue' | 'pink';
  } | null>(null);

  // Area Snip tool (⇧⌘S): visual formula/diagram/region capture + optional Vision LaTeX
  let snipMode = $state(false);
  let snipDrag = $state<{
    pageOrd: number;
    startX: number;
    startY: number;
    curX: number;
    curY: number;
    slotW: number;
    slotH: number;
  } | null>(null);
  let snipPopover = $state<{
    pageOrd: number;
    rect: NormalizedRect;
    imageDataUrl: string;
    x: number;
    y: number;
  } | null>(null);

  // Hover micro-preview for internal PDF links (\ref, \cite, equations, TOC) + inter-document library bridge
  let linkPreview = $state<{
    pageOrd: number;
    printedLabel: string | null;
    dest: unknown;
    imageUrl: string | null;
    matchedLibraryDoc: CitationLinkItem | null;
    loading: boolean;
    x: number;
    y: number;
    width: number;
    height: number;
  } | null>(null);
  let linkHoverShowTimer: ReturnType<typeof setTimeout> | null = null;
  let linkHoverHideTimer: ReturnType<typeof setTimeout> | null = null;
  let linkPreviewSeq = 0;
  const linkPreviewCache = new Map<
    string,
    { url: string; w: number; h: number; matchedDoc: CitationLinkItem | null }
  >();

  // Ask this document slide-over (visible ONLY when llmProvider !== 'disabled')
  let askOpen = $state(false);
  let askInitialScope = $state<DocLlmScope>('document');
  let askSelectionText = $state('');

  let shellEl: HTMLDivElement | undefined = $state();
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
  const pdfSearchItemsCache = new Map<string, PositionedTextItem[]>();

  let destroyed = false;
  let loadSeq = 0;
  let docSearchSeq = 0;
  let scrollRafId: number | null = null;
  let readingStateSaveTimer: ReturnType<typeof setTimeout> | null = null;
  let readingStateInitialized = false;
  let activeLoadingTask: { destroy?: () => Promise<void> | void } | null = null;

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

  const isCurrentPageBookmarked = $derived(
    annotations.some((a) => a.kind === 'bookmark' && a.page_ord === currentPage)
  );

  const annotationsByPage = $derived.by(() => {
    const map = new Map<number, FileAnnotation[]>();
    for (const a of annotations) {
      if (a.rects.length === 0) continue;
      let list = map.get(a.page_ord);
      if (!list) {
        list = [];
        map.set(a.page_ord, list);
      }
      list.push(a);
    }
    return map;
  });

  const searchMatchesByPage = $derived.by(() => {
    const map = new Map<number, Array<{ rect: NormalizedRect; active: boolean }>>();
    for (let i = 0; i < docSearchMatches.length; i++) {
      const m = docSearchMatches[i];
      const active = i === docSearchIdx;
      let list = map.get(m.pageOrd);
      if (!list) {
        list = [];
        map.set(m.pageOrd, list);
      }
      for (const r of m.rects) {
        list.push({ rect: r, active });
      }
    }
    return map;
  });

  function setColorMode(mode: PageColorMode) {
    colorMode = mode;
    try {
      localStorage.setItem(COLOR_MODE_STORAGE_KEY, mode);
    } catch {
      // Ignore storage error
    }
  }

  function revokeAllDjvuUrls() {
    for (const u of Object.values(djvuImgUrls)) {
      URL.revokeObjectURL(u);
    }
    djvuImgUrls = {};
    djvuImgOrder.length = 0;
    djvuInFlight.clear();
  }

  function rotatedDims(dims: { w: number; h: number }, rot: number): { w: number; h: number } {
    return rot % 180 !== 0 ? { w: dims.h, h: dims.w } : dims;
  }

  function pageBaseDims(pageOrd: number): { w: number; h: number } {
    return rotatedDims(pageDimsByOrd.get(pageOrd) ?? pdfDefaultSize, rotationDeg);
  }

  function effectiveScaleForPage(pageOrd: number): number {
    const { w, h } = pageBaseDims(pageOrd);
    const spreadDiv = spreadMode ? 2 : 1;
    const availW = Math.max(180, (containerWidth - (spreadMode ? 64 : 48)) / spreadDiv);
    const availH = Math.max(240, containerHeight - 36);
    if (zoomMode === 'fit-width') return availW / w;
    if (zoomMode === 'fit-page') return Math.min(availW / w, availH / h);
    return customZoom;
  }

  interface PageSlot {
    pageOrd: number;
    top: number;
    leftPx: number;
    width: number;
    height: number;
    scale: number;
  }

  const pageSlots = $derived.by(() => {
    const count = pageCount;
    const dimsMap = pageDimsByOrd;
    const defSize = pdfDefaultSize;
    const rot = rotationDeg;
    const twoUp = spreadMode;
    const stageW = Math.max(320, containerWidth);
    const gapX = 14;
    const availW = twoUp
      ? Math.max(180, (stageW - 48 - gapX) / 2)
      : Math.max(240, stageW - 48);
    const availH = Math.max(240, containerHeight - 36);
    const mode = zoomMode;
    const cZoom = customZoom;

    const slots: PageSlot[] = new Array(count);
    let y = 12;

    if (!twoUp) {
      for (let p = 1; p <= count; p++) {
        const dims = rotatedDims(dimsMap.get(p) ?? defSize, rot);
        const scale =
          mode === 'fit-width'
            ? availW / dims.w
            : mode === 'fit-page'
              ? Math.min(availW / dims.w, availH / dims.h)
              : cZoom;
        const pxW = Math.round(dims.w * scale);
        const pxH = Math.round(dims.h * scale);
        const leftPx = Math.max(16, Math.round((stageW - pxW) / 2));
        slots[p - 1] = { pageOrd: p, top: y, leftPx, width: pxW, height: pxH, scale };
        y += pxH + 16;
      }
    } else {
      for (let p = 1; p <= count; p += 2) {
        const d1 = rotatedDims(dimsMap.get(p) ?? defSize, rot);
        const s1 =
          mode === 'fit-width'
            ? availW / d1.w
            : mode === 'fit-page'
              ? Math.min(availW / d1.w, availH / d1.h)
              : cZoom;
        const w1 = Math.round(d1.w * s1);
        const h1 = Math.round(d1.h * s1);

        if (p + 1 <= count) {
          const d2 = rotatedDims(dimsMap.get(p + 1) ?? defSize, rot);
          const s2 =
            mode === 'fit-width'
              ? availW / d2.w
              : mode === 'fit-page'
                ? Math.min(availW / d2.w, availH / d2.h)
                : cZoom;
          const w2 = Math.round(d2.w * s2);
          const h2 = Math.round(d2.h * s2);
          const rowW = w1 + gapX + w2;
          const startX = Math.max(16, Math.round((stageW - rowW) / 2));
          slots[p - 1] = { pageOrd: p, top: y, leftPx: startX, width: w1, height: h1, scale: s1 };
          slots[p] = {
            pageOrd: p + 1,
            top: y,
            leftPx: startX + w1 + gapX,
            width: w2,
            height: h2,
            scale: s2,
          };
          y += Math.max(h1, h2) + 16;
        } else {
          const leftPx = Math.max(16, Math.round((stageW - w1) / 2));
          slots[p - 1] = { pageOrd: p, top: y, leftPx, width: w1, height: h1, scale: s1 };
          y += h1 + 16;
        }
      }
    }
    return { slots, totalHeight: y };
  });

  function findFirstSlotAtOrBelow(slots: PageSlot[], targetY: number): number {
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
    const pad = spreadMode ? 4 : 2;
    return {
      start: Math.max(1, first - pad),
      end: Math.min(slots.length, last + pad),
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

  function scheduleReadingStateSave(touchOpened = false) {
    if (!readingStateInitialized || destroyed || !info) return;
    if (readingStateSaveTimer) clearTimeout(readingStateSaveTimer);
    const fid = fileId;
    const patch = {
      last_page_ord: currentPage,
      zoom_mode: zoomMode,
      custom_zoom: customZoom,
      rotation_deg: rotationDeg,
      touch_opened: touchOpened,
    };
    readingStateSaveTimer = setTimeout(() => {
      readingStateSaveTimer = null;
      if (!destroyed) {
        void api.upsertReadingState(fid, patch).catch(() => {});
      }
    }, 650);
  }

  function flushReadingStateNow() {
    if (!readingStateInitialized || !info) return;
    if (readingStateSaveTimer) {
      clearTimeout(readingStateSaveTimer);
      readingStateSaveTimer = null;
    }
    void api
      .upsertReadingState(fileId, {
        last_page_ord: currentPage,
        zoom_mode: zoomMode,
        custom_zoom: customZoom,
        rotation_deg: rotationDeg,
        touch_opened: false,
      })
      .catch(() => {});
  }

  function handleViewportScroll() {
    selectionPopover = null;
    snipPopover = null;
    clearLinkPreviewTimers();
    linkPreview = null;
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
        scheduleReadingStateSave(false);
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
    const clamped = Math.max(1, Math.min(pageCount || 1, pageOrd));
    const slot = pageSlots.slots[clamped - 1];
    if (!slot) return;
    const centerFrac = rects.length > 0 ? computePassageCenterFraction(rects) : 0.08;
    const targetTop =
      rects.length > 0
        ? computeCenteredScrollTop(slot.top, slot.height, centerFrac, containerHeight)
        : Math.max(0, slot.top - 12);
    scrollEl.scrollTo({ top: targetTop, behavior: 'auto' });
    scrollTop = targetTop;
    currentPage = clamped;
    goToInput = String(clamped);
    scheduleReadingStateSave(false);
  }

  function jumpToPage(destPage: number, rects: NormalizedRect[] = [], recordHistory = true) {
    const clamped = Math.max(1, Math.min(pageCount || 1, destPage));
    if (recordHistory && currentPage !== clamped) {
      jumpBackStack = [...jumpBackStack.slice(-49), currentPage];
      jumpForwardStack = [];
    }
    void scrollToPage(clamped, rects);
  }

  function navigateJumpBack() {
    if (jumpBackStack.length === 0) return;
    const target = jumpBackStack[jumpBackStack.length - 1];
    jumpBackStack = jumpBackStack.slice(0, -1);
    jumpForwardStack = [...jumpForwardStack.slice(-49), currentPage];
    void scrollToPage(target);
  }

  function navigateJumpForward() {
    if (jumpForwardStack.length === 0) return;
    const target = jumpForwardStack[jumpForwardStack.length - 1];
    jumpForwardStack = jumpForwardStack.slice(0, -1);
    jumpBackStack = [...jumpBackStack.slice(-49), currentPage];
    void scrollToPage(target);
  }

  function clearLinkPreviewTimers() {
    if (linkHoverShowTimer) {
      clearTimeout(linkHoverShowTimer);
      linkHoverShowTimer = null;
    }
    if (linkHoverHideTimer) {
      clearTimeout(linkHoverHideTimer);
      linkHoverHideTimer = null;
    }
  }

  async function resolvePdfDestination(
    dest: unknown
  ): Promise<{ pageOrd: number; pdfTopY: number | null } | null> {
    if (!pdfDoc) return null;
    try {
      const resolved = typeof dest === 'string' ? await pdfDoc.getDestination(dest) : dest;
      if (!Array.isArray(resolved) || resolved.length === 0) return null;
      const ref = resolved[0];
      let pageIndex: number | null = null;
      if (typeof ref === 'number') {
        pageIndex = ref;
      } else if (ref && typeof ref === 'object') {
        pageIndex = await pdfDoc.getPageIndex(ref as Parameters<PDFDocumentProxy['getPageIndex']>[0]);
      }
      if (pageIndex === null || pageIndex < 0) return null;
      const fitObj = resolved[1] as { name?: string } | string | undefined;
      const fitName = typeof fitObj === 'string' ? fitObj : fitObj?.name ?? '';
      let pdfTopY: number | null = null;
      if (fitName === 'XYZ' && typeof resolved[3] === 'number' && Number.isFinite(resolved[3])) {
        pdfTopY = resolved[3];
      } else if (
        (fitName === 'FitH' || fitName === 'FitBH') &&
        typeof resolved[2] === 'number' &&
        Number.isFinite(resolved[2])
      ) {
        pdfTopY = resolved[2];
      } else if (
        fitName === 'FitR' &&
        typeof resolved[5] === 'number' &&
        Number.isFinite(resolved[5])
      ) {
        pdfTopY = resolved[5];
      }
      return { pageOrd: pageIndex + 1, pdfTopY };
    } catch {
      return null;
    }
  }

  async function handleInternalPdfLink(dest: unknown) {
    const resolved = await resolvePdfDestination(dest);
    if (!resolved) return;
    let targetRects: NormalizedRect[] = [];
    if (resolved.pdfTopY !== null && pdfDoc) {
      try {
        const page = await pdfDoc.getPage(resolved.pageOrd);
        const vp = page.getViewport({ scale: 1, rotation: (page.rotate + rotationDeg) % 360 });
        const [, vy] = vp.convertToViewportPoint(0, resolved.pdfTopY);
        const fracY = Math.max(0, Math.min(0.95, vy / Math.max(1, vp.height)));
        targetRects = [{ x: 0.05, y: fracY, w: 0.9, h: 0.05 }];
      } catch {
        // Fallback to top of destination page
      }
    }
    jumpToPage(resolved.pageOrd, targetRects, true);
  }

  function handleInternalLinkLeave() {
    if (linkHoverShowTimer) {
      clearTimeout(linkHoverShowTimer);
      linkHoverShowTimer = null;
    }
    if (linkHoverHideTimer) clearTimeout(linkHoverHideTimer);
    linkHoverHideTimer = setTimeout(() => {
      linkHoverHideTimer = null;
      linkPreview = null;
      linkPreviewSeq++;
    }, 180);
  }

  function cancelLinkPreviewHide() {
    if (linkHoverHideTimer) {
      clearTimeout(linkHoverHideTimer);
      linkHoverHideTimer = null;
    }
  }

  function handleInternalLinkHover(dest: unknown, anchorRect: DOMRect) {
    if (!pdfDoc || !shellEl || destroyed) return;
    cancelLinkPreviewHide();
    if (linkHoverShowTimer) clearTimeout(linkHoverShowTimer);
    const activeDoc = pdfDoc;
    const activeFid = fileId;
    const activeRot = rotationDeg;
    linkHoverShowTimer = setTimeout(() => {
      linkHoverShowTimer = null;
      void showInternalLinkPreview(activeDoc, activeFid, activeRot, dest, anchorRect);
    }, 220);
  }

  async function showInternalLinkPreview(
    doc: PDFDocumentProxy,
    activeFid: number,
    activeRot: number,
    dest: unknown,
    anchorRect: DOMRect
  ) {
    const seq = ++linkPreviewSeq;
    const resolved = await resolvePdfDestination(dest);
    if (!resolved || destroyed || seq !== linkPreviewSeq || pdfDoc !== doc || !shellEl) return;

    const previewW = 460;
    const previewH = 205;
    const shellRect = shellEl.getBoundingClientRect();
    const x = Math.max(
      12,
      Math.min(shellRect.width - previewW - 14, anchorRect.left - shellRect.left - 24)
    );
    const spaceBelow = shellRect.bottom - anchorRect.bottom;
    const y =
      spaceBelow >= previewH + 46
        ? anchorRect.bottom - shellRect.top + 8
        : Math.max(44, anchorRect.top - shellRect.top - previewH - 36);

    const printedLabel =
      info?.pages.find((p) => p.page_ord === resolved.pageOrd)?.printed_label ?? null;
    const yBucket = resolved.pdfTopY !== null ? Math.round(resolved.pdfTopY / 4) * 4 : 'top';
    const cacheKey = `${activeFid}:${resolved.pageOrd}:${yBucket}:${activeRot}`;
    const cached = linkPreviewCache.get(cacheKey);
    if (cached) {
      linkPreview = {
        pageOrd: resolved.pageOrd,
        printedLabel,
        dest,
        imageUrl: cached.url,
        matchedLibraryDoc: cached.matchedDoc,
        loading: false,
        x,
        y,
        width: cached.w,
        height: cached.h,
      };
      return;
    }

    linkPreview = {
      pageOrd: resolved.pageOrd,
      printedLabel,
      dest,
      imageUrl: null,
      matchedLibraryDoc: null,
      loading: true,
      x,
      y,
      width: previewW,
      height: previewH,
    };

    try {
      const page = await doc.getPage(resolved.pageOrd);
      if (destroyed || seq !== linkPreviewSeq || pdfDoc !== doc) return;
      const effRot = (page.rotate + activeRot) % 360;
      const baseVp = page.getViewport({ scale: 1, rotation: effRot });
      const targetScale = Math.max(0.7, Math.min(1.6, previewW / Math.max(1, baseVp.width)));
      const vp = page.getViewport({ scale: targetScale, rotation: effRot });

      let destVy: number | null = null;
      let cropTopCss = 0;
      if (resolved.pdfTopY !== null) {
        const [, vy] = vp.convertToViewportPoint(0, resolved.pdfTopY);
        destVy = vy;
        cropTopCss = Math.max(0, Math.min(Math.max(0, vp.height - previewH), vy - 24));
      }
      const cssW = Math.min(previewW, Math.round(vp.width));
      const cssH = Math.min(previewH, Math.max(80, Math.round(vp.height - cropTopCss)));
      const dpr = Math.min(2, window.devicePixelRatio || 1);

      const off = document.createElement('canvas');
      off.width = Math.max(1, Math.floor(cssW * dpr));
      off.height = Math.max(1, Math.floor(cssH * dpr));
      const ctx = off.getContext('2d');
      if (!ctx) return;
      ctx.fillStyle = '#ffffff';
      ctx.fillRect(0, 0, off.width, off.height);

      const renderPromise = page.render({
        canvasContext: ctx,
        viewport: vp,
        transform: [dpr, 0, 0, dpr, 0, -cropTopCss * dpr],
      }).promise;

      const citeMatchPromise = (async (): Promise<CitationLinkItem | null> => {
        if (!onOpenLibraryDoc) return null;
        try {
          const tc = await page.getTextContent();
          const tokens: PositionedLineToken[] = [];
          for (const raw of tc.items) {
            if (!raw || typeof (raw as { str?: unknown }).str !== 'string') continue;
            const it = raw as { str: string; transform?: number[] };
            if (!it.str.trim() || !Array.isArray(it.transform) || it.transform.length < 6) continue;
            const [vx, vy] = vp.convertToViewportPoint(it.transform[4], it.transform[5]);
            tokens.push({ str: it.str, x: vx, y: vy });
          }
          const snippet = extractBibEntryNearViewportY(tokens, destVy, vp.height);
          if (snippet.length < 12) return null;
          return await api.matchCitationInLibrary(snippet, info?.document_id ?? null);
        } catch {
          return null;
        }
      })();

      const [, matchedDoc] = await Promise.all([renderPromise, citeMatchPromise]);
      if (destroyed || seq !== linkPreviewSeq || pdfDoc !== doc) return;

      const url = off.toDataURL('image/png');
      off.width = 0;
      off.height = 0;
      if (linkPreviewCache.size >= 12) {
        const oldest = linkPreviewCache.keys().next().value;
        if (oldest) linkPreviewCache.delete(oldest);
      }
      linkPreviewCache.set(cacheKey, { url, w: cssW, h: cssH, matchedDoc });
      linkPreview = {
        pageOrd: resolved.pageOrd,
        printedLabel,
        dest,
        imageUrl: url,
        matchedLibraryDoc: matchedDoc,
        loading: false,
        x,
        y,
        width: cssW,
        height: cssH,
      };
    } catch {
      if (!destroyed && seq === linkPreviewSeq) {
        linkPreview = null;
      }
    }
  }

  function rotateBy(delta: number) {
    rotationDeg = (((rotationDeg + delta) % 360) + 360) % 360;
    pdfSearchItemsCache.clear();
    scheduleReadingStateSave(false);
  }

  async function refreshAnnotations(activeFid = fileId) {
    try {
      const list = await api.listFileAnnotations(activeFid);
      if (!destroyed && activeFid === fileId) {
        annotations = list;
      }
    } catch {
      // Ignore transient load errors
    }
  }

  async function handleToggleBookmark() {
    if (!info) return;
    try {
      await api.togglePageBookmark(fileId, currentPage, null);
      await refreshAnnotations(fileId);
      onAnnotationsChanged?.();
    } catch (e) {
      onStatusMessage?.(`Errore segnalibro: ${String(e)}`);
    }
  }

  function handleViewportMouseUp() {
    if (snipMode || !scrollEl || !info) return;
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || sel.rangeCount === 0) {
      selectionPopover = null;
      return;
    }
    const text = sel.toString().trim();
    if (text.length < 2) {
      selectionPopover = null;
      return;
    }
    const range = sel.getRangeAt(0);
    const anchorEl =
      range.commonAncestorContainer instanceof HTMLElement
        ? range.commonAncestorContainer
        : range.commonAncestorContainer.parentElement;
    const slotEl = anchorEl?.closest<HTMLElement>('.page-slot');
    if (!slotEl || !scrollEl.contains(slotEl)) {
      selectionPopover = null;
      return;
    }
    const pageOrd = Number(slotEl.dataset.pageOrd ?? '0');
    if (!pageOrd) return;

    const slotRect = slotEl.getBoundingClientRect();
    if (slotRect.width <= 0 || slotRect.height <= 0) return;

    const clientRects = Array.from(range.getClientRects()).filter(
      (r) => r.width > 1 && r.height > 1
    );
    if (clientRects.length === 0) return;

    const rects: NormalizedRect[] = clientRects.map((r) => ({
      x: Math.max(0, Math.min(1, (r.left - slotRect.left) / slotRect.width)),
      y: Math.max(0, Math.min(1, (r.top - slotRect.top) / slotRect.height)),
      w: Math.max(0.004, Math.min(1, r.width / slotRect.width)),
      h: Math.max(0.006, Math.min(1, r.height / slotRect.height)),
    }));

    const firstRect = clientRects[0];
    const shellRect = shellEl?.getBoundingClientRect();
    const x = shellRect ? Math.max(12, Math.min(shellRect.width - 230, firstRect.left - shellRect.left)) : 24;
    const y = shellRect ? Math.max(46, firstRect.top - shellRect.top - 42) : 64;

    selectionPopover = {
      pageOrd,
      text,
      rects,
      x,
      y,
      addingNote: false,
      noteDraft: '',
      noteColor: 'yellow',
    };
  }

  async function saveSelectionAnnotation(
    kind: 'highlight' | 'note',
    color: 'yellow' | 'green' | 'blue' | 'pink',
    noteMarkdown = ''
  ) {
    const pop = selectionPopover;
    if (!pop) return;
    try {
      await api.createFileAnnotation(fileId, {
        page_ord: pop.pageOrd,
        kind,
        color,
        rects: pop.rects,
        quoted_text: pop.text,
        note_markdown: noteMarkdown,
      });
      selectionPopover = null;
      window.getSelection()?.removeAllRanges();
      await refreshAnnotations(fileId);
      onAnnotationsChanged?.();
      if (kind === 'note') {
        sidebarMode = 'annotations';
      }
    } catch (e) {
      onStatusMessage?.(`Errore salvataggio annotazione: ${String(e)}`);
    }
  }

  function openAskWithSelection() {
    const pop = selectionPopover;
    if (!pop) return;
    askSelectionText = pop.text;
    askInitialScope = 'selection';
    askOpen = true;
    selectionPopover = null;
  }

  function handleSnipPointerDown(pageOrd: number, e: PointerEvent) {
    if (!snipMode || e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const overlay = e.currentTarget as HTMLElement;
    const rect = overlay.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return;
    try {
      overlay.setPointerCapture(e.pointerId);
    } catch {
      // Ignore synthetic pointer
    }
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;
    snipDrag = {
      pageOrd,
      startX: sx,
      startY: sy,
      curX: sx,
      curY: sy,
      slotW: rect.width,
      slotH: rect.height,
    };
  }

  function handleSnipPointerMove(pageOrd: number, e: PointerEvent) {
    if (!snipDrag || snipDrag.pageOrd !== pageOrd) return;
    const overlay = e.currentTarget as HTMLElement;
    const rect = overlay.getBoundingClientRect();
    snipDrag = {
      ...snipDrag,
      curX: Math.max(0, Math.min(rect.width, e.clientX - rect.left)),
      curY: Math.max(0, Math.min(rect.height, e.clientY - rect.top)),
    };
  }

  function handleSnipPointerUp(pageOrd: number, e: PointerEvent) {
    if (!snipDrag || snipDrag.pageOrd !== pageOrd) return;
    const drag = snipDrag;
    snipDrag = null;
    const overlay = e.currentTarget as HTMLElement;
    try {
      overlay.releasePointerCapture(e.pointerId);
    } catch {
      // Ignore
    }
    const normRect = normalizeDragRect(
      drag.startX,
      drag.startY,
      drag.curX,
      drag.curY,
      drag.slotW,
      drag.slotH,
      10
    );
    if (!normRect) return;

    const slotEl = overlay.closest<HTMLElement>('.page-slot');
    const mediaEl = slotEl?.querySelector<HTMLCanvasElement | HTMLImageElement>('.page-media');
    if (!slotEl || !mediaEl) return;

    const dataUrl = cropSourceElementToPngDataUrl(mediaEl, normRect);
    if (!dataUrl) return;

    snipMode = false;
    const slotBounds = slotEl.getBoundingClientRect();
    const shellRect = shellEl?.getBoundingClientRect();
    const anchorRight = slotBounds.left + (normRect.x + normRect.w) * slotBounds.width;
    const anchorTop = slotBounds.top + normRect.y * slotBounds.height;
    const popX = shellRect
      ? Math.max(12, Math.min(shellRect.width - 344, anchorRight - shellRect.left - 180))
      : 24;
    const popY = shellRect
      ? Math.max(46, Math.min(shellRect.height - 340, anchorTop - shellRect.top + 12))
      : 64;

    snipPopover = {
      pageOrd,
      rect: normRect,
      imageDataUrl: dataUrl,
      x: popX,
      y: popY,
    };
  }

  async function handleSaveSnipAnnotation(opts: {
    pageOrd: number;
    rect: NormalizedRect;
    imageDataUrl: string;
    noteMarkdown: string;
    color: 'yellow' | 'green' | 'blue' | 'pink';
  }) {
    await api.createFileAnnotation(fileId, {
      page_ord: opts.pageOrd,
      kind: 'snip',
      color: opts.color,
      rects: [opts.rect],
      quoted_text: '',
      note_markdown: opts.noteMarkdown,
      image_data_url: opts.imageDataUrl,
    });
    await refreshAnnotations(fileId);
    onAnnotationsChanged?.();
    sidebarMode = 'annotations';
  }

  async function handleUpdateAnnotation(id: number, noteMarkdown: string, color?: string) {
    await api.updateFileAnnotation(id, {
      note_markdown: noteMarkdown,
      color: color ?? null,
    });
    await refreshAnnotations(fileId);
  }

  async function handleDeleteAnnotation(id: number) {
    await api.deleteFileAnnotation(id);
    await refreshAnnotations(fileId);
    onAnnotationsChanged?.();
  }

  async function handleExportAnnotationsMarkdown() {
    try {
      const safeTitle = (info?.title || 'annotazioni')
        .replace(/[^a-zA-Z0-9_-]+/g, '_')
        .slice(0, 48);
      const outPath = await saveFileDialog({
        defaultPath: `${safeTitle}_note.md`,
        filters: [{ name: 'Markdown', extensions: ['md'] }],
      });
      if (!outPath) return;
      await api.exportAnnotationsMarkdown(fileId, outPath);
      onStatusMessage?.(`Annotazioni esportate in ${outPath}`);
    } catch (e) {
      onStatusMessage?.(`Errore esportazione note: ${String(e)}`);
    }
  }

  const INLINE_PDF_MAX_BYTES = 2 * 1024 * 1024;

  async function loadDocument(activeFid: number) {
    const seq = ++loadSeq;
    docSearchSeq++;
    searchingDoc = false;
    docSearchMatches = [];
    docSearchIdx = 0;
    jumpBackStack = [];
    jumpForwardStack = [];
    selectionPopover = null;
    snipMode = false;
    snipDrag = null;
    snipPopover = null;
    clearLinkPreviewTimers();
    linkPreview = null;
    linkPreviewCache.clear();
    readingStateInitialized = false;
    loading = true;
    loadError = null;
    djvuSpansCache = {};
    djvuSpansOrder.length = 0;
    pdfSearchItemsCache.clear();
    revokeAllDjvuUrls();

    if (activeLoadingTask) {
      try {
        await activeLoadingTask.destroy?.();
      } catch {
        // Ignore cancellation
      }
      activeLoadingTask = null;
    }
    if (pdfDoc) {
      const prevDoc = pdfDoc;
      pdfDoc = null;
      try {
        await prevDoc.destroy();
      } catch {
        // Ignore cleanup
      }
    }

    try {
      const [docInfo, anns] = await Promise.all([
        api.getViewerDoc(activeFid),
        api.listFileAnnotations(activeFid).catch(() => [] as FileAnnotation[]),
      ]);
      if (destroyed || seq !== loadSeq || fileId !== activeFid) return;
      info = docInfo;
      annotations = anns;

      // Restore saved zoom & rotation per file
      if (
        docInfo.zoom_mode === 'fit-width' ||
        docInfo.zoom_mode === 'fit-page' ||
        docInfo.zoom_mode === 'custom'
      ) {
        zoomMode = docInfo.zoom_mode;
      }
      if (docInfo.custom_zoom && docInfo.custom_zoom >= 0.35 && docInfo.custom_zoom <= 4.0) {
        customZoom = docInfo.custom_zoom;
      }
      rotationDeg = (((docInfo.rotation_deg ?? 0) % 360) + 360) % 360;

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

      // Resume last saved reading position unless caller requested an explicit search/citation jump
      const resumePage =
        !explicitPageJump &&
        highlightRects.length === 0 &&
        targetPage <= 1 &&
        docInfo.last_page_ord &&
        docInfo.last_page_ord > 1
          ? Math.min(docInfo.last_page_ord, pageCount || docInfo.last_page_ord)
          : targetPage || 1;

      await scrollToPage(resumePage, highlightRects);
      readingStateInitialized = true;
      void api
        .upsertReadingState(activeFid, {
          last_page_ord: resumePage,
          zoom_mode: zoomMode,
          custom_zoom: customZoom,
          rotation_deg: rotationDeg,
          touch_opened: true,
        })
        .catch(() => {});
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
    const ready = !loading && info !== null && readingStateInitialized;
    if (ready && p >= 1 && (explicitPageJump || rects.length > 0)) {
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
        // Ignore transient thumbnail errors
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
            // Ignore cleanup
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
      jumpToPage(target, [], true);
    }
  }

  async function openFindBar() {
    findBarOpen = true;
    await tick();
    findInputEl?.focus();
    findInputEl?.select();
  }

  function closeFindBar() {
    findBarOpen = false;
    docSearchSeq++;
    searchingDoc = false;
    docSearchMatches = [];
    docSearchIdx = 0;
  }

  async function runInDocSearch(e?: Event) {
    e?.preventDefault();
    const q = docSearchQuery.trim();
    if (!q || !info) {
      docSearchMatches = [];
      docSearchIdx = 0;
      return;
    }
    const seq = ++docSearchSeq;
    const activeLoad = loadSeq;
    const rot = rotationDeg;
    searchingDoc = true;
    const allMatches: InDocSearchMatch[] = [];
    try {
      if (info.format === 'pdf' && pdfDoc && pdfjsMod) {
        const doc = pdfDoc;
        const pdfjsLib = pdfjsMod;
        for (let p = 1; p <= doc.numPages; p++) {
          if (destroyed || seq !== docSearchSeq || activeLoad !== loadSeq) return;
          const cacheKey = `${p}:${rot}`;
          let items = pdfSearchItemsCache.get(cacheKey);
          if (!items) {
            const page = await doc.getPage(p);
            try {
              const vp = page.getViewport({ scale: 1, rotation: (page.rotate + rot) % 360 });
              const tc = await page.getTextContent();
              items = pdfTextContentToPositionedItems(tc.items, vp, pdfjsLib.Util.transform);
              if (pdfSearchItemsCache.size > 400) {
                const firstKey = pdfSearchItemsCache.keys().next().value;
                if (firstKey) pdfSearchItemsCache.delete(firstKey);
              }
              pdfSearchItemsCache.set(cacheKey, items);
            } finally {
              page.cleanup();
            }
          }
          const found = findMatchesInPositionedItems(items, q, p);
          for (const m of found) allMatches.push(m);
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
          const items = djvuSpansToPositionedItems(spans);
          const found = findMatchesInPositionedItems(items, q, p);
          for (const m of found) allMatches.push(m);
          if (p % 15 === 0) {
            await new Promise((r) => setTimeout(r, 0));
          }
        }
      }
      if (destroyed || seq !== docSearchSeq || activeLoad !== loadSeq) return;
      docSearchMatches = allMatches;
      // Start at the first match on or after currentPage for natural reading flow
      const firstFromCurrent = allMatches.findIndex((m) => m.pageOrd >= currentPage);
      docSearchIdx = firstFromCurrent >= 0 ? firstFromCurrent : 0;
      if (allMatches.length > 0) {
        const active = allMatches[docSearchIdx];
        jumpToPage(active.pageOrd, active.rects, true);
      }
    } finally {
      if (!destroyed && seq === docSearchSeq) {
        searchingDoc = false;
      }
    }
  }

  function stepDocSearch(delta: number) {
    if (!docSearchMatches.length) return;
    docSearchIdx = (docSearchIdx + delta + docSearchMatches.length) % docSearchMatches.length;
    const m = docSearchMatches[docSearchIdx];
    void scrollToPage(m.pageOrd, m.rects);
  }

  function handleWindowKeydown(e: KeyboardEvent) {
    if (destroyed || !isKeyboardActive) return;
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.shiftKey && e.key.toLowerCase() === 's') {
      e.preventDefault();
      snipMode = !snipMode;
      snipDrag = null;
      snipPopover = null;
      selectionPopover = null;
      return;
    }
    if (mod && !e.shiftKey && e.key.toLowerCase() === 'f') {
      e.preventDefault();
      void openFindBar();
      return;
    }
    if (e.altKey && !mod && e.key === 'ArrowLeft') {
      if (jumpBackStack.length > 0) {
        e.preventDefault();
        navigateJumpBack();
      }
      return;
    }
    if (e.altKey && !mod && e.key === 'ArrowRight') {
      if (jumpForwardStack.length > 0) {
        e.preventDefault();
        navigateJumpForward();
      }
      return;
    }
    if (e.key === 'Escape') {
      if (snipPopover) {
        snipPopover = null;
      } else if (snipMode) {
        snipMode = false;
        snipDrag = null;
      } else if (selectionPopover) {
        selectionPopover = null;
      } else if (showViewPopover) {
        showViewPopover = false;
      } else if (findBarOpen) {
        closeFindBar();
      }
    }
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
    flushReadingStateNow();
    destroyed = true;
    loadSeq++;
    docSearchSeq++;
    if (scrollRafId !== null) {
      cancelAnimationFrame(scrollRafId);
      scrollRafId = null;
    }
    if (readingStateSaveTimer) {
      clearTimeout(readingStateSaveTimer);
      readingStateSaveTimer = null;
    }
    clearLinkPreviewTimers();
    linkPreviewCache.clear();
    resizeObserver?.disconnect();
    pdfSearchItemsCache.clear();
    revokeAllDjvuUrls();
    if (activeLoadingTask) {
      try {
        void activeLoadingTask.destroy?.();
      } catch {
        // Ignore cleanup
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

<svelte:window onkeydown={handleWindowKeydown} />

<div class="viewer-shell" bind:this={shellEl}>
  <div class="viewer-toolbar">
    <!-- Left sidebar toggles: Thumbnails / Outline / Annotations -->
    <div class="group">
      <button
        type="button"
        class="v-icon-btn"
        class:active={sidebarMode === 'thumbnails'}
        title={tr(lang, 'thumbnailsTip')}
        aria-label={tr(lang, 'thumbnailsTip')}
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
        title={tr(lang, 'outlineTip')}
        aria-label={tr(lang, 'outlineTip')}
        onclick={() => (sidebarMode = sidebarMode === 'outline' ? 'none' : 'outline')}
      >
        <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
          <path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01" />
        </svg>
      </button>
      <button
        type="button"
        class="v-icon-btn"
        class:active={sidebarMode === 'annotations'}
        title={`${tr(lang, 'annotationsTip')} (${annotations.length})`}
        aria-label={tr(lang, 'annotationsTip')}
        onclick={() => (sidebarMode = sidebarMode === 'annotations' ? 'none' : 'annotations')}
      >
        <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M12 20h9" />
          <path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z" />
        </svg>
        {#if annotations.length > 0}
          <span class="mini-count">{annotations.length}</span>
        {/if}
      </button>
    </div>

    <!-- Jump history Back/Forward (progressive disclosure: shown only when history non-empty) -->
    {#if jumpBackStack.length > 0 || jumpForwardStack.length > 0}
      <div class="group">
        <button
          type="button"
          class="v-icon-btn"
          disabled={jumpBackStack.length === 0}
          title="Salto precedente (Alt+←)"
          aria-label="Indietro nella cronologia di salto"
          onclick={navigateJumpBack}
        >
          ←
        </button>
        <button
          type="button"
          class="v-icon-btn"
          disabled={jumpForwardStack.length === 0}
          title="Salto successivo (Alt+→)"
          aria-label="Avanti nella cronologia di salto"
          onclick={navigateJumpForward}
        >
          →
        </button>
      </div>
    {/if}

    <!-- Page indicator & Quick bookmark -->
    <form class="group" onsubmit={handleGoToSubmit}>
      <input
        class="page-input"
        bind:this={pageInputEl}
        bind:value={goToInput}
        title="Numero di pagina (Invio per andare)"
        aria-label="Pagina corrente"
      />
      <span class="pg-total">/ {pageCount}</span>
      <button
        type="button"
        class="v-icon-btn"
        class:active={isCurrentPageBookmarked}
        title={isCurrentPageBookmarked ? 'Rimuovi segnalibro pagina' : 'Aggiungi segnalibro a questa pagina'}
        aria-label="Segnalibro pagina"
        onclick={handleToggleBookmark}
      >
        <svg
          viewBox="0 0 24 24"
          width="14"
          height="14"
          fill={isCurrentPageBookmarked ? 'currentColor' : 'none'}
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z" />
        </svg>
      </button>
    </form>

    <!-- Zoom controls -->
    <div class="group">
      <button
        type="button"
        class="v-icon-btn"
        class:active={zoomMode === 'fit-width'}
        title="Adatta alla larghezza"
        aria-label="Adatta alla larghezza"
        onclick={() => {
          zoomMode = 'fit-width';
          scheduleReadingStateSave(false);
        }}
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
        onclick={() => {
          zoomMode = 'fit-page';
          scheduleReadingStateSave(false);
        }}
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
          scheduleReadingStateSave(false);
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
          scheduleReadingStateSave(false);
        }}
      >
        +
      </button>
    </div>

    <!-- Search match stepper when opened from library content search -->
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

    <!-- Right-aligned discreet tools: Area Snip (⇧⌘S), Find (⌘F), View Options (Color/2-up/Rotate), Ask Doc (if LLM active), External -->
    <div class="group right-tools">
      <button
        type="button"
        class="v-icon-btn"
        class:active={snipMode}
        title={tr(lang, 'areaSnipTip')}
        aria-label={tr(lang, 'areaSnipTip')}
        onclick={() => {
          snipMode = !snipMode;
          snipDrag = null;
          snipPopover = null;
          selectionPopover = null;
        }}
      >
        <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M6 2v14a2 2 0 0 0 2 2h14" />
          <path d="M18 22V8a2 2 0 0 0-2-2H2" />
        </svg>
      </button>

      <button
        type="button"
        class="v-icon-btn"
        class:active={findBarOpen}
        title={tr(lang, 'findInDocTip')}
        aria-label={tr(lang, 'findInDocTip')}
        onclick={() => (findBarOpen ? closeFindBar() : openFindBar())}
      >
        <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" aria-hidden="true">
          <circle cx="11" cy="11" r="7" />
          <path d="m20 20-3.5-3.5" />
        </svg>
      </button>

      <div class="view-popover-anchor">
        <button
          type="button"
          class="v-icon-btn"
          class:active={showViewPopover || colorMode !== 'normal' || spreadMode || rotationDeg !== 0}
          title={tr(lang, 'viewOptionsTip')}
          aria-label={tr(lang, 'viewOptionsTip')}
          onclick={() => (showViewPopover = !showViewPopover)}
        >
          <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="12" cy="12" r="4" />
            <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" />
          </svg>
        </button>

        {#if showViewPopover}
          <div class="view-popover" role="dialog" aria-label={tr(lang, 'viewOptionsTip')}>
            <div class="vp-row">
              <span class="vp-label">{tr(lang, 'pageColorLabel')}</span>
              <div class="vp-seg">
                <button
                  type="button"
                  class:active={colorMode === 'normal'}
                  onclick={() => setColorMode('normal')}
                >
                  {tr(lang, 'colorNormal')}
                </button>
                <button
                  type="button"
                  class:active={colorMode === 'sepia'}
                  onclick={() => setColorMode('sepia')}
                >
                  {tr(lang, 'colorSepia')}
                </button>
                <button
                  type="button"
                  class:active={colorMode === 'night'}
                  onclick={() => setColorMode('night')}
                >
                  {tr(lang, 'colorNight')}
                </button>
              </div>
            </div>

            <div class="vp-row">
              <span class="vp-label">{tr(lang, 'layoutLabel')}</span>
              <div class="vp-seg">
                <button
                  type="button"
                  class:active={!spreadMode}
                  onclick={() => (spreadMode = false)}
                >
                  {tr(lang, 'onePage')}
                </button>
                <button
                  type="button"
                  class:active={spreadMode}
                  onclick={() => (spreadMode = true)}
                >
                  {tr(lang, 'twoPages')}
                </button>
              </div>
            </div>

            <div class="vp-row">
              <span class="vp-label">{tr(lang, 'rotationLabel')} {rotationDeg ? `(${rotationDeg}°)` : ''}</span>
              <div class="vp-seg">
                <button type="button" onclick={() => rotateBy(-90)}>↺ −90°</button>
                <button type="button" onclick={() => rotateBy(90)}>↻ +90°</button>
                {#if rotationDeg !== 0}
                  <button type="button" onclick={() => rotateBy(-rotationDeg)}>0°</button>
                {/if}
              </div>
            </div>
          </div>
        {/if}
      </div>

      {#if llmProvider !== 'disabled' && info}
        <button
          type="button"
          class="v-icon-btn"
          class:active={askOpen}
          title="Chiedi a questo documento (LLM)"
          aria-label="Chiedi a questo documento"
          onclick={() => {
            if (!askOpen) {
              askInitialScope = 'document';
            }
            askOpen = !askOpen;
          }}
        >
          <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="m12 3 1.8 4.2L18 9l-4.2 1.8L12 15l-1.8-4.2L6 9l4.2-1.8z" />
            <path d="M5 17h14M5 21h9" />
          </svg>
        </button>
      {/if}

      {#if onOpenExternal}
        <button
          type="button"
          class="v-icon-btn"
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
  </div>

  {#if snipMode}
    <div class="snip-mode-banner" role="status">
      <span>✂ {tr(lang, 'areaSnipBanner')}</span>
      <button
        type="button"
        class="snip-banner-cancel"
        onclick={() => {
          snipMode = false;
          snipDrag = null;
        }}
      >
        {tr(lang, 'cancel')}
      </button>
    </div>
  {/if}

  {#if snipPopover && info}
    <ViewerSnipPopover
      {lang}
      documentId={info.document_id}
      pageOrd={snipPopover.pageOrd}
      rect={snipPopover.rect}
      imageDataUrl={snipPopover.imageDataUrl}
      x={snipPopover.x}
      y={snipPopover.y}
      {llmProvider}
      onSaveSnipAnnotation={handleSaveSnipAnnotation}
      {onConsentAccepted}
      {onStatusMessage}
      onClose={() => (snipPopover = null)}
    />
  {/if}

  <!-- Floating Find bar (progressive disclosure: appears only on ⌘F / Find click) -->
  {#if findBarOpen}
    <form class="floating-find-bar" onsubmit={runInDocSearch}>
      <input
        type="search"
        class="find-input"
        bind:this={findInputEl}
        bind:value={docSearchQuery}
        placeholder={tr(lang, 'findPlaceholder')}
        onkeydown={(e) => {
          if (e.key === 'Enter' && docSearchMatches.length > 0 && !searchingDoc) {
            e.preventDefault();
            stepDocSearch(e.shiftKey ? -1 : 1);
          } else if (e.key === 'Escape') {
            e.preventDefault();
            closeFindBar();
          }
        }}
      />
      <button type="submit" class="v-icon-btn" disabled={searchingDoc} title={tr(lang, 'btnSearch')}>
        {searchingDoc ? '…' : '↵'}
      </button>
      {#if docSearchMatches.length > 0}
        <span class="match-count">{docSearchIdx + 1}/{docSearchMatches.length}</span>
        <button type="button" class="v-icon-btn" title="Shift+Enter" onclick={() => stepDocSearch(-1)}>◀</button>
        <button type="button" class="v-icon-btn" title="Enter" onclick={() => stepDocSearch(1)}>▶</button>
      {:else if docSearchQuery.trim() && !searchingDoc}
        <span class="match-count zero">0</span>
      {/if}
      <button type="button" class="v-icon-btn" title={tr(lang, 'close')} onclick={closeFindBar}>×</button>
    </form>
  {/if}

  <!-- Floating hover preview popover for internal PDF links (\ref, \cite, equations, TOC) -->
  {#if linkPreview}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="link-preview-popover mode-{colorMode}"
      style="left: {linkPreview.x}px; top: {linkPreview.y}px; width: {linkPreview.width + 12}px;"
      onmouseenter={cancelLinkPreviewHide}
      onmouseleave={handleInternalLinkLeave}
    >
      <div class="lp-head">
        <span>
          {tr(lang, 'linkPreviewLabel')} {linkPreview.printedLabel ?? linkPreview.pageOrd}
        </span>
        <button
          type="button"
          class="lp-jump-btn"
          onclick={() => {
            const d = linkPreview?.dest;
            clearLinkPreviewTimers();
            linkPreview = null;
            if (d !== undefined) void handleInternalPdfLink(d);
          }}
        >
          {tr(lang, 'goToPageBtn')}
        </button>
      </div>
      <div class="lp-body" style="height: {linkPreview.height}px;">
        {#if linkPreview.imageUrl}
          <button
            type="button"
            class="lp-img-btn"
            onclick={() => {
              const d = linkPreview?.dest;
              clearLinkPreviewTimers();
              linkPreview = null;
              if (d !== undefined) void handleInternalPdfLink(d);
            }}
          >
            <img
              src={linkPreview.imageUrl}
              alt={`Preview p. ${linkPreview.pageOrd}`}
              class="lp-img"
              style="width: {linkPreview.width}px; height: {linkPreview.height}px;"
            />
          </button>
        {:else}
          <div class="lp-loading">{lang === 'en' ? 'Loading preview…' : 'Caricamento anteprima…'}</div>
        {/if}
      </div>
      {#if linkPreview.matchedLibraryDoc && onOpenLibraryDoc}
        {@const matchedDoc = linkPreview.matchedLibraryDoc}
        <div class="lp-library-bridge">
          <div class="lp-lib-info" title={matchedDoc.title}>
            <span class="lp-lib-badge">✓ {tr(lang, 'ownedInLibraryBadge')}</span>
            <span class="lp-lib-title">{matchedDoc.title}</span>
          </div>
          <div class="lp-lib-actions">
            <button
              type="button"
              class="lp-lib-btn primary"
              onclick={() => {
                clearLinkPreviewTimers();
                linkPreview = null;
                onOpenLibraryDoc(matchedDoc, false);
              }}
            >
              {tr(lang, 'openInPaperoBtn')}
            </button>
            <button
              type="button"
              class="lp-lib-btn"
              onclick={() => {
                clearLinkPreviewTimers();
                linkPreview = null;
                onOpenLibraryDoc(matchedDoc, true);
              }}
            >
              {tr(lang, 'openSideBySideBtn')}
            </button>
          </div>
        </div>
      {/if}
    </div>
  {/if}

  <!-- Floating disappearing Selection Popover for non-destructive highlights, notes & Ask -->
  {#if selectionPopover}
    <div
      class="selection-popover"
      style="left: {selectionPopover.x}px; top: {selectionPopover.y}px;"
      role="toolbar"
      aria-label="Azioni selezione testo"
    >
      {#if !selectionPopover.addingNote}
        <button
          type="button"
          class="sel-dot dot-yellow"
          title="Evidenzia giallo"
          onclick={() => saveSelectionAnnotation('highlight', 'yellow')}
        ></button>
        <button
          type="button"
          class="sel-dot dot-green"
          title="Evidenzia verde"
          onclick={() => saveSelectionAnnotation('highlight', 'green')}
        ></button>
        <button
          type="button"
          class="sel-dot dot-blue"
          title="Evidenzia blu"
          onclick={() => saveSelectionAnnotation('highlight', 'blue')}
        ></button>
        <button
          type="button"
          class="sel-dot dot-pink"
          title="Evidenzia rosa"
          onclick={() => saveSelectionAnnotation('highlight', 'pink')}
        ></button>
        <span class="sel-sep"></span>
        <button
          type="button"
          class="sel-action"
          onclick={() => {
            if (selectionPopover) selectionPopover = { ...selectionPopover, addingNote: true };
          }}
        >
          Nota
        </button>
        {#if llmProvider !== 'disabled'}
          <button type="button" class="sel-action accent" onclick={openAskWithSelection}>
            Chiedi
          </button>
        {/if}
      {:else}
        <div class="sel-note-box">
          <textarea
            rows="2"
            placeholder="Nota (Markdown / LaTeX $...$)…"
            bind:value={selectionPopover.noteDraft}
          ></textarea>
          <div class="sel-note-actions">
            <button
              type="button"
              class="sel-action"
              onclick={() => (selectionPopover = null)}
            >
              Annulla
            </button>
            <button
              type="button"
              class="sel-action accent"
              onclick={() =>
                saveSelectionAnnotation(
                  'note',
                  selectionPopover?.noteColor ?? 'yellow',
                  selectionPopover?.noteDraft ?? ''
                )}
            >
              Salva nota
            </button>
          </div>
        </div>
      {/if}
    </div>
  {/if}

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
                onclick={() => jumpToPage(p, [], true)}
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
              <div class="empty-side">{tr(lang, 'noOutlineMsg')}</div>
            {:else}
              {#snippet renderOutline(items: OutlineItem[], depth: number)}
                {#each items as item}
                  <button
                    type="button"
                    class="outline-item"
                    style="padding-left: {8 + depth * 12}px"
                    disabled={!item.page_ord}
                    onclick={() => item.page_ord && jumpToPage(item.page_ord, [], true)}
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
        {:else if sidebarMode === 'annotations'}
          <ViewerAnnotationsPane
            {lang}
            {annotations}
            onJump={(p, rects) => jumpToPage(p, rects, true)}
            onUpdateNote={handleUpdateAnnotation}
            onDelete={handleDeleteAnnotation}
            onExportMarkdown={handleExportAnnotationsMarkdown}
          />
        {/if}
      </aside>
    {/if}

    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="scroll-viewport mode-{colorMode}"
      bind:this={scrollEl}
      onscroll={handleViewportScroll}
      onmouseup={handleViewportMouseUp}
      onclick={() => {
        if (showViewPopover) showViewPopover = false;
      }}
    >
      {#if loading}
        <div class="center-msg">{tr(lang, 'loadingDocumentMsg')}</div>
      {:else if loadError}
        <div class="center-msg error">{loadError}</div>
      {:else if info}
        <div class="pages-canvas-stage" style="height: {pageSlots.totalHeight}px;">
          {#each visibleSlots as slot (slot.pageOrd)}
            {@const pageAnns = annotationsByPage.get(slot.pageOrd) ?? []}
            {@const pageFindHits = searchMatchesByPage.get(slot.pageOrd) ?? []}
            <div
              id={`page-wrap-${fileId}-${slot.pageOrd}`}
              data-page-ord={slot.pageOrd}
              class="page-slot"
              style="top: {slot.top}px; left: {slot.leftPx}px; width: {slot.width}px; height: {slot.height}px;"
            >
              {#if info.format === 'pdf' && pdfDoc && pdfjsMod}
                <div
                  class="pdf-page-surface"
                  use:pdfPageAction={{
                    pdfjsLib: pdfjsMod,
                    doc: pdfDoc,
                    pageOrd: slot.pageOrd,
                    scale: slot.scale,
                    rotation: rotationDeg,
                    width: slot.width,
                    height: slot.height,
                    onInternalLink: handleInternalPdfLink,
                    onInternalLinkHover: handleInternalLinkHover,
                    onInternalLinkLeave: handleInternalLinkLeave,
                  }}
                >
                  <canvas class="pdf-canvas page-media"></canvas>
                  <div class="textLayer"></div>
                  <div class="linkLayer"></div>
                </div>
              {:else}
                {@const pageDpi = slot.scale > 1.3 ? 200 : 150}
                {@const pageImgUrl = djvuImgUrls[`${fileId}:${slot.pageOrd}:${pageDpi}`]}
                {@const swappedDjvu = rotationDeg % 180 !== 0}
                <div
                  class="djvu-rot-wrap"
                  style={swappedDjvu
                    ? `position:absolute;left:50%;top:50%;width:${slot.height}px;height:${slot.width}px;transform:translate(-50%,-50%) rotate(${rotationDeg}deg);`
                    : rotationDeg === 180
                      ? 'transform:rotate(180deg);'
                      : ''}
                >
                  {#if pageImgUrl}
                    <img
                      class="djvu-page-img page-media"
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
                </div>
              {/if}

              <!-- Saved non-destructive highlight, note & snip overlays -->
              {#if pageAnns.length > 0}
                <div class="annotation-layer">
                  {#each pageAnns as ann (ann.id)}
                    {#each ann.rects as r}
                      <div
                        class="ann-rect ann-{ann.color}"
                        class:ann-snip={ann.kind === 'snip'}
                        style="left: {r.x * 100}%; top: {r.y * 100}%; width: {r.w * 100}%; height: {r.h * 100}%;"
                      ></div>
                    {/each}
                  {/each}
                </div>
              {/if}

              {#if snipMode}
                <div
                  class="snip-capture-overlay"
                  onpointerdown={(e) => handleSnipPointerDown(slot.pageOrd, e)}
                  onpointermove={(e) => handleSnipPointerMove(slot.pageOrd, e)}
                  onpointerup={(e) => handleSnipPointerUp(slot.pageOrd, e)}
                >
                  {#if snipDrag && snipDrag.pageOrd === slot.pageOrd}
                    {@const dx = Math.min(snipDrag.startX, snipDrag.curX)}
                    {@const dy = Math.min(snipDrag.startY, snipDrag.curY)}
                    {@const dw = Math.abs(snipDrag.curX - snipDrag.startX)}
                    {@const dh = Math.abs(snipDrag.curY - snipDrag.startY)}
                    <div
                      class="snip-drag-box"
                      style="left: {dx}px; top: {dy}px; width: {dw}px; height: {dh}px;"
                    ></div>
                  {/if}
                </div>
              {/if}

              <!-- In-document Find (⌘F) visual highlights -->
              {#if pageFindHits.length > 0}
                <div class="find-highlight-layer">
                  {#each pageFindHits as fh}
                    <div
                      class="find-box"
                      class:active={fh.active}
                      style="left: {fh.rect.x * 100}%; top: {fh.rect.y * 100}%; width: {fh.rect.w * 100}%; height: {fh.rect.h * 100}%;"
                    ></div>
                  {/each}
                </div>
              {/if}

              <!-- Library search passage target highlights -->
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

    {#if askOpen && llmProvider !== 'disabled' && info}
      <ViewerAskPanel
        {lang}
        documentId={info.document_id}
        {fileId}
        {currentPage}
        {llmProvider}
        initialScope={askInitialScope}
        selectionText={askSelectionText}
        onJumpToPage={(pg) => jumpToPage(pg, [], true)}
        onClose={() => (askOpen = false)}
        {onConsentAccepted}
      />
    {/if}
  </div>
</div>

<style>
  .viewer-shell {
    position: relative;
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
  .right-tools {
    margin-left: auto;
  }
  .v-icon-btn {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 28px;
    height: 28px;
    padding: 0 6px;
    border-radius: 6px;
    font-weight: 600;
  }
  .mini-count {
    font-size: 9.5px;
    margin-left: 3px;
    opacity: 0.85;
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
  .zoom-pct,
  .match-count {
    font-size: 11.5px;
    min-width: 36px;
    text-align: center;
    color: var(--text-muted);
  }
  .match-count.zero {
    color: var(--danger);
  }
  .view-popover-anchor {
    position: relative;
  }
  .view-popover {
    position: absolute;
    right: 0;
    top: 34px;
    z-index: 30;
    width: 240px;
    padding: 10px;
    border-radius: 8px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    box-shadow: 0 10px 28px rgba(0, 0, 0, 0.24);
    display: flex;
    flex-direction: column;
    gap: 9px;
  }
  .vp-row {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .vp-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-muted);
  }
  .vp-seg {
    display: flex;
    gap: 4px;
  }
  .vp-seg button {
    flex: 1;
    font-size: 11px;
    padding: 3px 6px;
    border-radius: 5px;
  }
  .floating-find-bar {
    position: absolute;
    top: 44px;
    right: 16px;
    z-index: 25;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 5px 7px;
    border-radius: 8px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.22);
  }
  .find-input {
    width: 210px;
    height: 26px;
    padding: 2px 7px;
    font-size: 12px;
  }
  .selection-popover {
    position: absolute;
    z-index: 28;
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 5px 8px;
    border-radius: 8px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.28);
  }
  .sel-dot {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: 1.5px solid rgba(0, 0, 0, 0.15);
    padding: 0;
    cursor: pointer;
  }
  .dot-yellow {
    background: #facc15;
  }
  .dot-green {
    background: #4ade80;
  }
  .dot-blue {
    background: #60a5fa;
  }
  .dot-pink {
    background: #f472b6;
  }
  .sel-sep {
    width: 1px;
    height: 16px;
    background: var(--border);
  }
  .sel-action {
    font-size: 11.5px;
    padding: 2px 7px;
    border-radius: 5px;
    font-weight: 600;
  }
  .sel-action.accent {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--accent);
  }
  .sel-note-box {
    display: flex;
    flex-direction: column;
    gap: 5px;
    width: 230px;
  }
  .sel-note-box textarea {
    width: 100%;
    font-size: 11.5px;
    padding: 5px;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
    box-sizing: border-box;
    font-family: inherit;
  }
  .sel-note-actions {
    display: flex;
    justify-content: flex-end;
    gap: 5px;
  }
  .viewer-body {
    flex: 1;
    min-height: 0;
    display: flex;
    overflow: hidden;
  }
  .viewer-sidebar {
    width: 224px;
    flex-shrink: 0;
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
  .scroll-viewport.mode-sepia {
    background: #e5dcc8;
  }
  .scroll-viewport.mode-night {
    background: #14171d;
  }
  .scroll-viewport.mode-sepia :global(.page-media) {
    filter: sepia(0.32) contrast(0.95) brightness(0.97);
  }
  .scroll-viewport.mode-night :global(.page-media) {
    filter: invert(0.88) hue-rotate(180deg) contrast(0.94);
  }
  .pages-canvas-stage {
    position: relative;
    width: 100%;
  }
  .page-slot {
    position: absolute;
    background: #fff;
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.16);
    overflow: hidden;
    contain: layout paint;
  }
  .scroll-viewport.mode-night .page-slot {
    background: #1d2128;
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
  .linkLayer {
    position: absolute;
    inset: 0;
    pointer-events: none;
    z-index: 4;
  }
  .linkLayer :global(.pdf-link-hotspot) {
    position: absolute;
    pointer-events: auto;
    border: none;
    background: transparent;
    padding: 0;
    cursor: pointer;
    border-radius: 2px;
    transition: background 0.12s;
  }
  .linkLayer :global(.pdf-link-hotspot:hover) {
    background: rgba(59, 130, 246, 0.16);
    outline: 1px solid rgba(59, 130, 246, 0.45);
  }
  .djvu-rot-wrap {
    position: relative;
    width: 100%;
    height: 100%;
  }
  .djvu-page-img {
    width: 100%;
    height: 100%;
    display: block;
    pointer-events: none;
    -webkit-user-select: none;
    user-select: none;
  }
  .annotation-layer,
  .find-highlight-layer,
  .highlight-layer {
    position: absolute;
    inset: 0;
    pointer-events: none;
    z-index: 3;
  }
  .ann-rect {
    position: absolute;
    border-radius: 2px;
    mix-blend-mode: multiply;
  }
  .scroll-viewport.mode-night .ann-rect {
    mix-blend-mode: screen;
  }
  .ann-yellow {
    background: rgba(250, 204, 21, 0.34);
  }
  .ann-green {
    background: rgba(74, 222, 128, 0.32);
  }
  .ann-blue {
    background: rgba(96, 165, 250, 0.32);
  }
  .ann-pink {
    background: rgba(244, 114, 182, 0.32);
  }
  .ann-rect.ann-snip {
    background: rgba(59, 130, 246, 0.08);
    border: 1.5px dashed rgba(37, 99, 235, 0.7);
    mix-blend-mode: normal;
  }
  .snip-mode-banner {
    position: absolute;
    top: 44px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 29;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 11px;
    border-radius: 999px;
    background: var(--bg-panel);
    border: 1px solid var(--accent);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.24);
    font-size: 11.5px;
    font-weight: 600;
    color: var(--accent);
    max-width: calc(100% - 28px);
  }
  .snip-banner-cancel {
    font-size: 11px;
    padding: 1px 7px;
    border-radius: 999px;
  }
  .snip-capture-overlay {
    position: absolute;
    inset: 0;
    z-index: 8;
    cursor: crosshair;
    touch-action: none;
  }
  .snip-drag-box {
    position: absolute;
    border: 2px dashed var(--accent);
    background: rgba(59, 130, 246, 0.16);
    border-radius: 3px;
    pointer-events: none;
  }
  .find-box {
    position: absolute;
    background: rgba(250, 204, 21, 0.36);
    border: 1px solid rgba(202, 138, 4, 0.55);
    border-radius: 2px;
  }
  .find-box.active {
    background: rgba(249, 115, 22, 0.48);
    border: 1.5px solid #ea580c;
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
  .link-preview-popover {
    position: absolute;
    z-index: 36;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.28);
    overflow: hidden;
    padding: 5px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    pointer-events: auto;
  }
  .lp-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-muted);
    padding: 0 3px;
  }
  .lp-jump-btn {
    border: none;
    background: transparent;
    color: var(--accent);
    font-size: 11px;
    font-weight: 600;
    padding: 1px 4px;
    cursor: pointer;
  }
  .lp-jump-btn:hover {
    text-decoration: underline;
  }
  .lp-body {
    overflow: hidden;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: #ffffff;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .lp-img-btn {
    border: none;
    background: transparent;
    padding: 0;
    margin: 0;
    cursor: pointer;
    display: block;
    line-height: 0;
  }
  .lp-img {
    display: block;
    object-fit: cover;
    object-position: top left;
  }
  .link-preview-popover.mode-sepia .lp-img {
    filter: sepia(0.32) contrast(0.96) brightness(0.96);
  }
  .link-preview-popover.mode-night .lp-img {
    filter: invert(0.9) hue-rotate(180deg) contrast(0.94);
  }
  .lp-loading {
    font-size: 11px;
    color: #64748b;
  }
  .lp-library-bridge {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 4px 5px 2px;
    border-top: 1px solid var(--border);
    background: var(--bg-panel);
  }
  .lp-lib-info {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    flex: 1;
  }
  .lp-lib-badge {
    font-size: 10px;
    font-weight: 700;
    padding: 1px 6px;
    border-radius: 999px;
    background: rgba(16, 185, 129, 0.14);
    color: #047857;
    border: 1px solid rgba(16, 185, 129, 0.34);
    flex-shrink: 0;
  }
  .lp-lib-title {
    font-size: 11px;
    font-weight: 600;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .lp-lib-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }
  .lp-lib-btn {
    font-size: 11px;
    font-weight: 600;
    padding: 2px 8px;
    border-radius: 5px;
    cursor: pointer;
  }
</style>
