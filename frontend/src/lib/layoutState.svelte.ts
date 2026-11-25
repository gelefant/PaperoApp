const LAYOUT_STORAGE_KEY = 'papero_layout_v1';

export type ResizablePane = 'sidebar' | 'catalog' | 'inspector';

export interface PersistedLayoutPrefs {
  sidebarWidth: number;
  catalogWidth: number;
  inspectorWidth: number;
  sidebarCollapsed: boolean;
  catalogCollapsed: boolean;
  inspectorOpen: boolean;
}

const DEFAULT_LAYOUT: PersistedLayoutPrefs = {
  sidebarWidth: 245,
  catalogWidth: 360,
  inspectorWidth: 340,
  sidebarCollapsed: false,
  catalogCollapsed: false,
  inspectorOpen: true,
};

export function clampPaneWidth(pane: ResizablePane, value: number): number {
  const n = Number.isFinite(value) ? Math.round(value) : DEFAULT_LAYOUT[`${pane}Width`];
  if (pane === 'sidebar') return Math.max(180, Math.min(420, n));
  if (pane === 'catalog') return Math.max(250, Math.min(680, n));
  return Math.max(280, Math.min(560, n));
}

function loadLayoutPrefs(): PersistedLayoutPrefs {
  try {
    const raw = localStorage.getItem(LAYOUT_STORAGE_KEY);
    if (!raw) return { ...DEFAULT_LAYOUT };
    const parsed = JSON.parse(raw) as Partial<PersistedLayoutPrefs>;
    return {
      sidebarWidth: clampPaneWidth('sidebar', parsed.sidebarWidth ?? DEFAULT_LAYOUT.sidebarWidth),
      catalogWidth: clampPaneWidth('catalog', parsed.catalogWidth ?? DEFAULT_LAYOUT.catalogWidth),
      inspectorWidth: clampPaneWidth(
        'inspector',
        parsed.inspectorWidth ?? DEFAULT_LAYOUT.inspectorWidth
      ),
      sidebarCollapsed: Boolean(parsed.sidebarCollapsed ?? DEFAULT_LAYOUT.sidebarCollapsed),
      catalogCollapsed: Boolean(parsed.catalogCollapsed ?? DEFAULT_LAYOUT.catalogCollapsed),
      inspectorOpen:
        parsed.inspectorOpen !== undefined
          ? Boolean(parsed.inspectorOpen)
          : DEFAULT_LAYOUT.inspectorOpen,
    };
  } catch {
    return { ...DEFAULT_LAYOUT };
  }
}

export function createLayoutState() {
  const initial = loadLayoutPrefs();

  let sidebarWidth = $state(initial.sidebarWidth);
  let catalogWidth = $state(initial.catalogWidth);
  let inspectorWidth = $state(initial.inspectorWidth);
  let sidebarCollapsed = $state(initial.sidebarCollapsed);
  let catalogCollapsed = $state(initial.catalogCollapsed);
  let inspectorOpen = $state(initial.inspectorOpen);
  let zenMode = $state(false);
  let resizingPane = $state<ResizablePane | null>(null);

  let zenRestore = {
    sidebarCollapsed: initial.sidebarCollapsed,
    catalogCollapsed: initial.catalogCollapsed,
    inspectorOpen: initial.inspectorOpen,
  };

  function persist() {
    try {
      const payload: PersistedLayoutPrefs = {
        sidebarWidth,
        catalogWidth,
        inspectorWidth,
        sidebarCollapsed,
        catalogCollapsed,
        inspectorOpen,
      };
      localStorage.setItem(LAYOUT_STORAGE_KEY, JSON.stringify(payload));
    } catch {
      // Ignore storage quota or private-mode restrictions
    }
  }

  function setSidebarCollapsed(v: boolean) {
    sidebarCollapsed = v;
    persist();
  }

  function setCatalogCollapsed(v: boolean) {
    catalogCollapsed = v;
    persist();
  }

  function setInspectorOpen(v: boolean) {
    inspectorOpen = v;
    persist();
  }

  function toggleZenMode(hasOpenTabs: boolean): boolean {
    if (!zenMode) {
      if (!hasOpenTabs) return false;
      zenRestore = { sidebarCollapsed, catalogCollapsed, inspectorOpen };
      zenMode = true;
      return true;
    }
    zenMode = false;
    sidebarCollapsed = zenRestore.sidebarCollapsed;
    catalogCollapsed = zenRestore.catalogCollapsed;
    inspectorOpen = zenRestore.inspectorOpen;
    return true;
  }

  function startResize(pane: ResizablePane, e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    const startX = e.clientX;
    const startW =
      pane === 'sidebar' ? sidebarWidth : pane === 'catalog' ? catalogWidth : inspectorWidth;
    resizingPane = pane;

    const target = e.currentTarget as HTMLElement | null;
    const pointerId = e.pointerId;
    try {
      target?.setPointerCapture(pointerId);
    } catch {
      // Ignore synthetic pointer events
    }

    const onMove = (ev: PointerEvent) => {
      const delta = pane === 'inspector' ? startX - ev.clientX : ev.clientX - startX;
      const next = clampPaneWidth(pane, startW + delta);
      if (pane === 'sidebar') sidebarWidth = next;
      else if (pane === 'catalog') catalogWidth = next;
      else inspectorWidth = next;
    };

    const onUp = () => {
      resizingPane = null;
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      window.removeEventListener('pointercancel', onUp);
      try {
        target?.releasePointerCapture(pointerId);
      } catch {
        // Ignore
      }
      persist();
    };

    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
    window.addEventListener('pointercancel', onUp);
  }

  return {
    get sidebarWidth() {
      return sidebarWidth;
    },
    get catalogWidth() {
      return catalogWidth;
    },
    get inspectorWidth() {
      return inspectorWidth;
    },
    get sidebarCollapsed() {
      return sidebarCollapsed;
    },
    set sidebarCollapsed(v: boolean) {
      setSidebarCollapsed(v);
    },
    get catalogCollapsed() {
      return catalogCollapsed;
    },
    set catalogCollapsed(v: boolean) {
      setCatalogCollapsed(v);
    },
    get inspectorOpen() {
      return inspectorOpen;
    },
    set inspectorOpen(v: boolean) {
      setInspectorOpen(v);
    },
    get zenMode() {
      return zenMode;
    },
    get resizingPane() {
      return resizingPane;
    },
    toggleSidebarCollapsed() {
      setSidebarCollapsed(!sidebarCollapsed);
    },
    toggleCatalogCollapsed() {
      setCatalogCollapsed(!catalogCollapsed);
    },
    toggleInspectorOpen() {
      setInspectorOpen(!inspectorOpen);
    },
    toggleZenMode,
    startResize,
  };
}
