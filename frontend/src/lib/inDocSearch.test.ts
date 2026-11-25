import { describe, expect, it } from 'vitest';
import {
  findMatchesInPositionedItems,
  foldMathDiacritics,
  type PositionedTextItem,
} from './inDocSearch';

describe('inDocSearch (diacritic folding, hyphenation & visual rects)', () => {
  it('folds mathematical and European diacritics + ligatures', () => {
    expect(foldMathDiacritics('Poincaré')).toBe('poincare');
    expect(foldMathDiacritics('Erdős')).toBe('erdos');
    expect(foldMathDiacritics('Łukasiewicz')).toBe('lukasiewicz');
    expect(foldMathDiacritics('Čech Hölder ﬁnite')).toBe('cech holder finite');
  });

  it('finds diacritic matches with tight horizontal sub-rects', () => {
    const items: PositionedTextItem[] = [
      {
        text: 'Theorem (Poincaré duality) for Erdős graphs.',
        rect: { x: 0.1, y: 0.2, w: 0.8, h: 0.03 },
      },
    ];
    const pMatches = findMatchesInPositionedItems(items, 'Poincare', 3);
    expect(pMatches).toHaveLength(1);
    expect(pMatches[0].pageOrd).toBe(3);
    expect(pMatches[0].rects).toHaveLength(1);
    // Highlight should be narrower than the full line
    expect(pMatches[0].rects[0].w).toBeLessThan(0.35);
    expect(pMatches[0].rects[0].x).toBeGreaterThan(0.2);

    const eMatches = findMatchesInPositionedItems(items, 'Erdos', 3);
    expect(eMatches).toHaveLength(1);
    expect(eMatches[0].rects[0].x).toBeGreaterThan(0.5);
  });

  it('joins hyphenated line-break words and produces two highlight rects', () => {
    const items: PositionedTextItem[] = [
      {
        text: 'We compute the coho-',
        rect: { x: 0.1, y: 0.3, w: 0.7, h: 0.025 },
        hasEOL: true,
      },
      {
        text: 'mology groups of X.',
        rect: { x: 0.1, y: 0.34, w: 0.5, h: 0.025 },
      },
    ];
    const matches = findMatchesInPositionedItems(items, 'cohomology', 12);
    expect(matches).toHaveLength(1);
    expect(matches[0].pageOrd).toBe(12);
    expect(matches[0].rects).toHaveLength(2);
    expect(matches[0].rects[0].y).toBeCloseTo(0.3, 3);
    expect(matches[0].rects[1].y).toBeCloseTo(0.34, 3);
  });
});
