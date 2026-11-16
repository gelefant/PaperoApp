import { describe, expect, it } from 'vitest';
import {
  buildHighlightedSnippet,
  computeCenteredScrollTop,
  computePassageCenterFraction,
  paperoDocUrl,
  resolveGoToPage,
  shouldShowRowVerifyControl,
  splitAnswerWithCitations,
} from './viewerUtils';

describe('viewerUtils', () => {
  it('builds platform-specific papero-doc URLs', () => {
    expect(paperoDocUrl('/pdf/42', false)).toBe('papero-doc://localhost/pdf/42');
    expect(paperoDocUrl('djvu-page/7/3?dpi=150', true)).toBe(
      'http://papero-doc.localhost/djvu-page/7/3?dpi=150'
    );
  });

  it('resolves go-to-page by both printed label and ordinal number', () => {
    const pages = [
      { page_ord: 1, printed_label: 'i', width_pts: 600, height_pts: 800 },
      { page_ord: 2, printed_label: 'xiv', width_pts: 600, height_pts: 800 },
      { page_ord: 3, printed_label: '1', width_pts: 600, height_pts: 800 },
    ];
    expect(resolveGoToPage('xiv', 3, pages)).toBe(2);
    expect(resolveGoToPage('3', 3, pages)).toBe(3);
    expect(resolveGoToPage('99', 3, pages)).toBeNull();
  });

  it('computes vertical passage center and viewport scrollTop', () => {
    const center = computePassageCenterFraction([
      { x: 0.1, y: 0.4, w: 0.8, h: 0.04 },
      { x: 0.1, y: 0.46, w: 0.7, h: 0.04 },
    ]);
    expect(center).toBeCloseTo(0.45, 5);

    const scrollTop = computeCenteredScrollTop(2000, 1000, center, 600);
    // 2000 + 450 - 300 = 2150
    expect(scrollTop).toBe(2150);
  });

  it('builds snippets centered around matched query term', () => {
    const longText = `${'lorem ipsum '.repeat(30)}Poincare duality holds on compact manifolds${' dolor sit'.repeat(20)}`;
    const snippet = buildHighlightedSnippet(longText, 'Poincare', 120);
    expect(snippet).toContain('Poincare duality');
  });

  it('splits grounded LLM answers into text and resolved clickable citation segments', () => {
    const segs = splitAnswerWithCitations(
      'By [Doc #7, p. 14] the cohomology is finite, unlike [Doc #99, p. 2].',
      [
        {
          marker: '[Doc #7, p. 14]',
          document_id: 7,
          file_id: 70,
          page_ord: 14,
          printed_label: '12',
          doc_title: 'Hatcher Algebraic Topology',
          bboxes: [{ x: 0.1, y: 0.2, w: 0.7, h: 0.05 }],
          snippet: 'Cohomology groups are finitely generated.',
        },
      ]
    );
    expect(segs).toHaveLength(5);
    expect(segs[0]).toEqual({ kind: 'text', text: 'By ' });
    expect(segs[1]).toMatchObject({
      kind: 'citation',
      label: '[Doc #7, p. 14]',
      documentId: 7,
      pageOrd: 14,
    });
    if (segs[1].kind === 'citation') {
      expect(segs[1].citation?.file_id).toBe(70);
      expect(segs[1].citation?.bboxes).toHaveLength(1);
    }
    expect(segs[3]).toMatchObject({
      kind: 'citation',
      label: '[Doc #99, p. 2]',
      documentId: 99,
      pageOrd: 2,
      citation: null,
    });
  });

  it('hides row verify controls when filtering verified or when unverified list is empty', () => {
    expect(shouldShowRowVerifyControl('all', 5)).toBe(true);
    expect(shouldShowRowVerifyControl('unverified', 5)).toBe(true);
    expect(shouldShowRowVerifyControl('verified', 5)).toBe(false);
    expect(shouldShowRowVerifyControl('all', 0)).toBe(false);
    expect(shouldShowRowVerifyControl('verified', 0)).toBe(false);
    expect(shouldShowRowVerifyControl('all', undefined)).toBe(true);
  });
});
