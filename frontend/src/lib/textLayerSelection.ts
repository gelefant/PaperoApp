const activeLayers = new Map<HTMLElement, HTMLDivElement>();
let selectionListenerController: AbortController | null = null;
let prevRange: Range | null = null;
let isPointerDown = false;

function resetEndDiv(endDiv: HTMLDivElement, layerEl: HTMLElement): void {
  if (endDiv.parentElement !== layerEl || layerEl.lastElementChild !== endDiv) {
    layerEl.append(endDiv);
  }
  endDiv.style.width = '';
  endDiv.style.height = '';
  layerEl.classList.remove('selecting');
}

function resetAllLayers(): void {
  for (const [layerEl, endDiv] of activeLayers) {
    resetEndDiv(endDiv, layerEl);
  }
}

function ensureGlobalSelectionListeners(): void {
  if (selectionListenerController) return;
  selectionListenerController = new AbortController();
  const { signal } = selectionListenerController;

  document.addEventListener(
    'pointerdown',
    () => {
      isPointerDown = true;
    },
    { signal }
  );

  document.addEventListener(
    'pointerup',
    () => {
      isPointerDown = false;
      resetAllLayers();
    },
    { signal }
  );

  window.addEventListener(
    'blur',
    () => {
      isPointerDown = false;
      resetAllLayers();
    },
    { signal }
  );

  document.addEventListener(
    'keyup',
    () => {
      if (!isPointerDown) {
        resetAllLayers();
      }
    },
    { signal }
  );

  document.addEventListener(
    'selectionchange',
    () => {
      const selection = document.getSelection();
      if (!selection || selection.rangeCount === 0) {
        prevRange = null;
        resetAllLayers();
        return;
      }

      const active = new Set<HTMLElement>();
      for (let i = 0; i < selection.rangeCount; i++) {
        const range = selection.getRangeAt(i);
        for (const layerEl of activeLayers.keys()) {
          if (!active.has(layerEl) && range.intersectsNode(layerEl)) {
            active.add(layerEl);
          }
        }
      }

      for (const [layerEl, endDiv] of activeLayers) {
        if (active.has(layerEl)) {
          layerEl.classList.add('selecting');
        } else {
          resetEndDiv(endDiv, layerEl);
        }
      }

      const range = selection.getRangeAt(0);
      const modifyStart =
        prevRange !== null &&
        (range.compareBoundaryPoints(Range.END_TO_END, prevRange) === 0 ||
          range.compareBoundaryPoints(Range.START_TO_END, prevRange) === 0);

      let anchor: Node | null = modifyStart ? range.startContainer : range.endContainer;
      if (anchor?.nodeType === Node.TEXT_NODE) {
        anchor = anchor.parentNode;
      }
      const anchorEl = anchor instanceof HTMLElement ? anchor : null;
      const parentLayer = anchorEl?.closest('.textLayer, .djvu-text-layer') as HTMLElement | null;
      const endDiv = parentLayer ? activeLayers.get(parentLayer) : undefined;
      if (
        endDiv &&
        parentLayer &&
        anchorEl &&
        anchorEl !== parentLayer &&
        anchorEl !== endDiv &&
        anchorEl.parentElement &&
        parentLayer.contains(anchorEl.parentElement)
      ) {
        endDiv.style.width = parentLayer.style.width || `${parentLayer.clientWidth}px`;
        endDiv.style.height = parentLayer.style.height || `${parentLayer.clientHeight}px`;
        anchorEl.parentElement.insertBefore(endDiv, modifyStart ? anchorEl : anchorEl.nextSibling);
      }
      prevRange = range.cloneRange();
    },
    { signal }
  );
}

function maybeTeardownGlobalListeners(): void {
  if (activeLayers.size === 0 && selectionListenerController) {
    selectionListenerController.abort();
    selectionListenerController = null;
    prevRange = null;
    isPointerDown = false;
  }
}

/**
 * Enhances a PDF.js `.textLayer` or DjVu `.djvu-text-layer` so drag-selecting across positioned spans
 * stays continuous and visually highlighted in WebKit/WKWebView and Chromium.
 */
export function attachSelectableTextLayer(layerEl: HTMLElement): () => void {
  let endDiv = Array.from(layerEl.children).find((c) =>
    c.classList.contains('endOfContent')
  ) as HTMLDivElement | undefined;
  if (!endDiv) {
    endDiv = document.createElement('div');
    endDiv.className = 'endOfContent';
    layerEl.append(endDiv);
  }

  const onMouseDown = () => {
    layerEl.classList.add('selecting');
  };
  const onCopy = (event: ClipboardEvent) => {
    const sel = document.getSelection()?.toString();
    if (sel && event.clipboardData) {
      event.clipboardData.setData('text/plain', sel.replace(/\0/g, ''));
      event.preventDefault();
    }
  };

  layerEl.addEventListener('mousedown', onMouseDown);
  layerEl.addEventListener('copy', onCopy);
  activeLayers.set(layerEl, endDiv);
  ensureGlobalSelectionListeners();

  return () => {
    layerEl.removeEventListener('mousedown', onMouseDown);
    layerEl.removeEventListener('copy', onCopy);
    activeLayers.delete(layerEl);
    endDiv?.remove();
    maybeTeardownGlobalListeners();
  };
}
