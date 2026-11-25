import { describe, expect, it } from 'vitest';
import { clampPaneWidth } from './layoutState.svelte';
import { createCloneTab } from './viewerTabsState.svelte';
import type { ViewerTab } from './types';

describe('layoutState & viewerTabsState helpers', () => {
  it('clamps resizable pane widths within ergonomic bounds', () => {
    expect(clampPaneWidth('sidebar', 90)).toBe(180);
    expect(clampPaneWidth('sidebar', 600)).toBe(420);
    expect(clampPaneWidth('catalog', 200)).toBe(250);
    expect(clampPaneWidth('catalog', 380)).toBe(380);
    expect(clampPaneWidth('inspector', 999)).toBe(560);
  });

  it('creates an independent clone tab for same-document side-by-side split view', () => {
    const base: ViewerTab = {
      tabId: 'tab-42',
      fileId: 42,
      documentId: 7,
      title: 'Algebraic Geometry',
      format: 'pdf',
      targetPage: 19,
      explicitPageJump: false,
      highlightRects: [{ x: 0.1, y: 0.2, w: 0.5, h: 0.04 }],
      matches: [],
      activeMatchIndex: 0,
    };
    const clone = createCloneTab(base, 99);
    expect(clone.tabId).toBe('tab-42-split-99');
    expect(clone.fileId).toBe(42);
    expect(clone.title).toBe('Algebraic Geometry (2)');
    expect(clone.targetPage).toBe(19);
    expect(clone.explicitPageJump).toBe(true);
    expect(clone.highlightRects).toEqual([]);
  });
});
