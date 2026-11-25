import { describe, expect, it } from 'vitest';
import {
  extractBibEntryNearViewportY,
  normalizeDragRect,
  type PositionedLineToken,
} from './viewerSnipUtils';

describe('viewerSnipUtils', () => {
  it('normalizes reverse-dragged rectangles and rejects accidental tiny clicks', () => {
    expect(normalizeDragRect(100, 100, 104, 105, 600, 800)).toBeNull();

    const r = normalizeDragRect(300, 400, 150, 200, 600, 800);
    expect(r).toEqual({
      x: 0.25,
      y: 0.25,
      w: 0.25,
      h: 0.25,
    });
  });

  it('extracts only the targeted bibliography entry without bleeding into the next reference', () => {
    const tokens: PositionedLineToken[] = [
      { str: '[CKN82]', x: 50, y: 120 },
      {
        str: 'L. Caffarelli, R. Kohn, L. Nirenberg, Partial regularity of suitable weak solutions,',
        x: 95,
        y: 120,
      },
      { str: 'Comm. Pure Appl. Math. 35 (1982), 771–831.', x: 95, y: 134 },
      { str: '[Har77]', x: 50, y: 156 },
      { str: 'R. Hartshorne, Algebraic Geometry, Springer-Verlag,', x: 95, y: 156 },
      { str: 'Graduate Texts in Mathematics 52, New York, 1977.', x: 95, y: 170 },
      { str: '[Rud91]', x: 50, y: 192 },
      { str: 'W. Rudin, Functional Analysis, 2nd ed., 1991.', x: 95, y: 192 },
    ];

    const entry = extractBibEntryNearViewportY(tokens, 154, 800);
    expect(entry).toContain('[Har77] R. Hartshorne, Algebraic Geometry');
    expect(entry).toContain('1977.');
    expect(entry).not.toContain('Caffarelli');
    expect(entry).not.toContain('Rudin');
  });
});
