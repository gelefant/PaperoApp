import type { NormalizedRect, ViewerTab, ViewerTargetPassage } from './types';

export type FocusedViewerPane = 'primary' | 'split';

let cloneSeq = 1;

export interface OpenTabParams {
  fileId: number;
  documentId?: number;
  title: string;
  format: string;
  pageOrd?: number;
  highlightRects?: NormalizedRect[];
  matches?: ViewerTargetPassage[];
  activeMatchIndex?: number;
  explicitPageJump?: boolean;
}

export function createCloneTab(source: ViewerTab, seq = cloneSeq++): ViewerTab {
  const baseTitle = source.title.replace(/\s+\(\d+\)$/, '');
  return {
    ...source,
    tabId: `tab-${source.fileId}-split-${seq}`,
    title: `${baseTitle} (2)`,
    explicitPageJump: true,
    highlightRects: [],
  };
}

export function createViewerTabsState() {
  let tabs = $state<ViewerTab[]>([]);
  let activeTabId = $state<string | null>(null);
  let splitTabId = $state<string | null>(null);
  let focusedPane = $state<FocusedViewerPane>('primary');

  function openInPrimary(params: OpenTabParams) {
    const pageOrd = params.pageOrd ?? 1;
    const rects = params.highlightRects ?? [];
    const matches = params.matches ?? [];
    const activeMatchIndex = params.activeMatchIndex ?? 0;
    const explicit = Boolean(params.explicitPageJump) || rects.length > 0 || pageOrd > 1;

    // Prefer updating an existing primary (non-clone) tab for this fileId
    const existing =
      tabs.find((t) => t.fileId === params.fileId && !t.tabId.includes('-split-')) ??
      tabs.find((t) => t.fileId === params.fileId && t.tabId !== splitTabId);

    if (existing) {
      tabs = tabs.map((t) =>
        t.tabId === existing.tabId
          ? {
              ...t,
              documentId: params.documentId ?? t.documentId,
              targetPage: pageOrd,
              explicitPageJump: explicit,
              highlightRects: rects,
              matches: matches.length ? matches : t.matches,
              activeMatchIndex,
            }
          : t
      );
      if (splitTabId === existing.tabId) {
        splitTabId = null;
      }
      activeTabId = existing.tabId;
      focusedPane = 'primary';
      return;
    }

    const baseId = `tab-${params.fileId}`;
    const tabId = tabs.some((t) => t.tabId === baseId)
      ? `tab-${params.fileId}-${cloneSeq++}`
      : baseId;
    tabs = [
      ...tabs,
      {
        tabId,
        fileId: params.fileId,
        documentId: params.documentId,
        title: params.title,
        format: params.format,
        targetPage: pageOrd,
        explicitPageJump: explicit,
        highlightRects: rects,
        matches,
        activeMatchIndex,
      },
    ];
    activeTabId = tabId;
    focusedPane = 'primary';
  }

  function openInSplit(params: OpenTabParams) {
    const pageOrd = params.pageOrd ?? 1;
    const rects = params.highlightRects ?? [];
    const matches = params.matches ?? [];
    const activeMatchIndex = params.activeMatchIndex ?? 0;
    const explicit = Boolean(params.explicitPageJump) || rects.length > 0 || pageOrd > 1;

    if (!activeTabId || tabs.length === 0) {
      openInPrimary(params);
      return;
    }

    // Reuse an existing non-primary tab for this file if present
    const existingOther = tabs.find(
      (t) => t.fileId === params.fileId && t.tabId !== activeTabId
    );
    if (existingOther) {
      tabs = tabs.map((t) =>
        t.tabId === existingOther.tabId
          ? {
              ...t,
              documentId: params.documentId ?? t.documentId,
              targetPage: pageOrd,
              explicitPageJump: explicit,
              highlightRects: rects,
              matches: matches.length ? matches : t.matches,
              activeMatchIndex,
            }
          : t
      );
      splitTabId = existingOther.tabId;
      focusedPane = 'split';
      return;
    }

    // If same file as active primary tab, create a split clone tab
    const isSameAsPrimary = tabs.find((t) => t.tabId === activeTabId)?.fileId === params.fileId;
    const tabId = isSameAsPrimary
      ? `tab-${params.fileId}-split-${cloneSeq++}`
      : `tab-${params.fileId}`;
    const finalTabId = tabs.some((t) => t.tabId === tabId)
      ? `tab-${params.fileId}-split-${cloneSeq++}`
      : tabId;

    const newTab: ViewerTab = {
      tabId: finalTabId,
      fileId: params.fileId,
      documentId: params.documentId,
      title: isSameAsPrimary ? `${params.title} (2)` : params.title,
      format: params.format,
      targetPage: pageOrd,
      explicitPageJump: explicit,
      highlightRects: rects,
      matches,
      activeMatchIndex,
    };
    tabs = [...tabs, newTab];
    splitTabId = finalTabId;
    focusedPane = 'split';
  }

  function duplicateTabToSplit(sourceTabId?: string | null) {
    const targetId = sourceTabId ?? activeTabId;
    if (!targetId) return;
    const source = tabs.find((t) => t.tabId === targetId);
    if (!source) return;
    const clone = createCloneTab(source);
    tabs = [...tabs, clone];
    if (!activeTabId) {
      activeTabId = source.tabId;
    }
    splitTabId = clone.tabId;
    focusedPane = 'split';
  }

  function selectTab(tabId: string) {
    if (splitTabId === tabId && activeTabId && activeTabId !== tabId) {
      const prevPrimary = activeTabId;
      activeTabId = tabId;
      splitTabId = prevPrimary;
      focusedPane = 'primary';
      return;
    }
    activeTabId = tabId;
    focusedPane = 'primary';
  }

  function setSplitTab(tabId: string | null) {
    splitTabId = tabId && tabId !== activeTabId ? tabId : null;
    if (!splitTabId) {
      focusedPane = 'primary';
    } else {
      focusedPane = 'split';
    }
  }

  function closeTab(tabId: string) {
    tabs = tabs.filter((t) => t.tabId !== tabId);
    if (activeTabId === tabId) {
      const nextPrimary = tabs.find((t) => t.tabId !== splitTabId) ?? tabs[0] ?? null;
      activeTabId = nextPrimary?.tabId ?? null;
      if (activeTabId && splitTabId === activeTabId) {
        splitTabId = null;
      }
    }
    if (splitTabId === tabId || tabs.length < 2) {
      splitTabId = null;
      focusedPane = 'primary';
    }
  }

  function selectMatchIndex(tabId: string, idx: number) {
    tabs = tabs.map((t) => {
      if (t.tabId !== tabId || !t.matches[idx]) return t;
      const m = t.matches[idx];
      return {
        ...t,
        activeMatchIndex: idx,
        targetPage: m.page_ord,
        explicitPageJump: true,
        highlightRects: m.bboxes,
      };
    });
  }

  function pruneRemovedFiles(validFileIds: { has: (fileId: number) => boolean }) {
    if (tabs.length === 0) return;
    const remaining = tabs.filter((t) => validFileIds.has(t.fileId));
    if (remaining.length !== tabs.length) {
      tabs = remaining;
      if (!remaining.some((t) => t.tabId === activeTabId)) {
        activeTabId = remaining[0]?.tabId ?? null;
      }
      if (!remaining.some((t) => t.tabId === splitTabId) || remaining.length < 2) {
        splitTabId = null;
        focusedPane = 'primary';
      }
    }
  }

  return {
    get tabs() {
      return tabs;
    },
    get activeTabId() {
      return activeTabId;
    },
    get splitTabId() {
      return splitTabId;
    },
    get focusedPane() {
      return focusedPane;
    },
    set focusedPane(pane: FocusedViewerPane) {
      focusedPane = pane;
    },
    openInPrimary,
    openInSplit,
    duplicateTabToSplit,
    selectTab,
    setSplitTab,
    closeTab,
    selectMatchIndex,
    pruneRemovedFiles,
  };
}
