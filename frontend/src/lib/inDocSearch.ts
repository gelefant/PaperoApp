import type { ExtractedSpan, NormalizedRect } from './types';

export interface PositionedTextItem {
  text: string;
  rect: NormalizedRect;
  hasEOL?: boolean;
}

export interface InDocSearchMatch {
  pageOrd: number;
  rects: NormalizedRect[];
  snippet: string;
}

const SPECIAL_MAP: Record<string, string> = {
  '\u0142': 'l', // ł
  '\u0141': 'l', // Ł
  '\u00f8': 'o', // ø
  '\u00d8': 'o', // Ø
  '\u00df': 'ss', // ß
  '\u00e6': 'ae', // æ
  '\u00c6': 'ae', // Æ
  '\u0153': 'oe', // œ
  '\u0152': 'oe', // Œ
  '\u0111': 'd', // đ
  '\u0110': 'd', // Đ
  '\u00f0': 'd', // ð
  '\u00d0': 'd', // Ð
  '\u00fe': 'th', // þ
  '\u00de': 'th', // Þ
  '\ufb00': 'ff', // ﬀ
  '\ufb01': 'fi', // ﬁ
  '\ufb02': 'fl', // ﬂ
  '\ufb03': 'ffi', // ﬃ
  '\ufb04': 'ffl', // ﬄ
  '\u2010': '-',
  '\u2011': '-',
  '\u2012': '-',
  '\u2013': '-',
  '\u2014': '-',
};

/**
 * Folds mathematical & European diacritics (`Poincaré` -> `poincare`, `Erdős` -> `erdos`,
 * `Čech` -> `cech`, `Łukasiewicz` -> `lukasiewicz`, `ﬁ` -> `fi`) for accent-insensitive search.
 */
export function foldMathDiacritics(raw: string): string {
  if (!raw) return '';
  let mapped = '';
  for (const ch of raw) {
    if (ch === '\u00ad') continue; // strip soft hyphen
    mapped += SPECIAL_MAP[ch] ?? ch;
  }
  return mapped
    .normalize('NFKD')
    .replace(/[\u0300-\u036f]/g, '')
    .toLowerCase();
}

export function normalizeSearchQuery(raw: string): string {
  return foldMathDiacritics(raw).replace(/\s+/g, ' ').trim();
}

interface CharOrigin {
  itemIdx: number;
  startFrac: number;
  endFrac: number;
}

function endsWithWordHyphen(s: string): boolean {
  const trimmed = s.replace(/\s+$/, '');
  if (trimmed.length < 2) return false;
  const last = trimmed[trimmed.length - 1];
  if (last !== '-' && last !== '\u2010' && last !== '\u00ad') return false;
  const before = trimmed[trimmed.length - 2];
  return /[A-Za-z\u00C0-\u024F]/.test(before);
}

/**
 * Merges rects belonging to the same visual line into compact highlight boxes.
 */
function mergeLineRects(rects: NormalizedRect[]): NormalizedRect[] {
  if (rects.length <= 1) return rects;
  const out: NormalizedRect[] = [{ ...rects[0] }];
  for (let i = 1; i < rects.length; i++) {
    const cur = rects[i];
    const prev = out[out.length - 1];
    const sameLine =
      Math.abs(cur.y - prev.y) <= Math.max(prev.h, cur.h) * 0.55 &&
      cur.x <= prev.x + prev.w + 0.04;
    if (sameLine) {
      const x1 = Math.min(prev.x, cur.x);
      const y1 = Math.min(prev.y, cur.y);
      const x2 = Math.max(prev.x + prev.w, cur.x + cur.w);
      const y2 = Math.max(prev.y + prev.h, cur.y + cur.h);
      out[out.length - 1] = { x: x1, y: y1, w: x2 - x1, h: y2 - y1 };
    } else {
      out.push({ ...cur });
    }
  }
  return out;
}

/**
 * Finds every occurrence of `rawQuery` across a page's positioned text items,
 * folding diacritics and joining hyphenated line breaks (`coho-\nmology` -> `cohomology`),
 * and computes exact normalized bounding boxes for visual highlighting.
 */
export function findMatchesInPositionedItems(
  items: PositionedTextItem[],
  rawQuery: string,
  pageOrd: number
): InDocSearchMatch[] {
  const needle = normalizeSearchQuery(rawQuery);
  if (!needle || items.length === 0) return [];

  const chars: string[] = [];
  const origins: (CharOrigin | null)[] = [];

  for (let i = 0; i < items.length; i++) {
    const it = items[i];
    const next = i + 1 < items.length ? items[i + 1] : null;
    const rawStr = it.text ?? '';
    if (!rawStr) continue;

    // Detect hyphenated line break to next item: strip trailing hyphen & avoid inserting space
    const isLineBreakNext =
      next !== null &&
      (it.hasEOL === true ||
        next.rect.y > it.rect.y + Math.max(it.rect.h, 0.008) * 0.45 ||
        next.rect.x + 0.05 < it.rect.x);
    const joinHyphen =
      isLineBreakNext &&
      endsWithWordHyphen(rawStr) &&
      next !== null &&
      /^[A-Za-z\u00C0-\u024F]/.test(next.text.trimStart());

    const effectiveRaw = joinHyphen ? rawStr.replace(/[-\u2010\u00ad]\s*$/, '') : rawStr;
    const rawChars = Array.from(effectiveRaw);
    const denom = Math.max(1, Array.from(rawStr).length);

    for (let c = 0; c < rawChars.length; c++) {
      const foldedPart = foldMathDiacritics(rawChars[c]);
      for (const fc of foldedPart) {
        if (/\s/.test(fc)) {
          if (chars.length > 0 && chars[chars.length - 1] !== ' ') {
            chars.push(' ');
            origins.push(null);
          }
        } else {
          chars.push(fc);
          origins.push({
            itemIdx: i,
            startFrac: c / denom,
            endFrac: (c + 1) / denom,
          });
        }
      }
    }

    // Insert separating space between items unless hyphen-joined or already whitespace
    if (
      !joinHyphen &&
      next !== null &&
      chars.length > 0 &&
      chars[chars.length - 1] !== ' '
    ) {
      const gapX = next.rect.x - (it.rect.x + it.rect.w);
      const needsSpace =
        isLineBreakNext || gapX > Math.max(0.0025, it.rect.h * 0.14);
      if (needsSpace) {
        chars.push(' ');
        origins.push(null);
      }
    }
  }

  const haystack = chars.join('');
  if (!haystack) return [];

  const matches: InDocSearchMatch[] = [];
  let searchFrom = 0;
  while (searchFrom <= haystack.length - needle.length) {
    const idx = haystack.indexOf(needle, searchFrom);
    if (idx === -1) break;
    const endIdx = idx + needle.length;
    searchFrom = idx + Math.max(1, needle.length);

    // Group hit character spans per itemIdx to slice exact horizontal sub-rects
    const perItem = new Map<number, { minF: number; maxF: number }>();
    for (let k = idx; k < endIdx; k++) {
      const orig = origins[k];
      if (!orig) continue;
      const cur = perItem.get(orig.itemIdx);
      if (!cur) {
        perItem.set(orig.itemIdx, { minF: orig.startFrac, maxF: orig.endFrac });
      } else {
        if (orig.startFrac < cur.minF) cur.minF = orig.startFrac;
        if (orig.endFrac > cur.maxF) cur.maxF = orig.endFrac;
      }
    }

    const rawRects: NormalizedRect[] = [];
    for (const [itemIdx, frac] of perItem.entries()) {
      const base = items[itemIdx].rect;
      const rx = Math.max(0, Math.min(1, base.x + base.w * frac.minF));
      const rw = Math.max(0.004, Math.min(1 - rx, base.w * Math.max(0.05, frac.maxF - frac.minF)));
      rawRects.push({
        x: rx,
        y: Math.max(0, Math.min(1, base.y)),
        w: rw,
        h: Math.max(0.008, Math.min(1 - base.y, base.h)),
      });
    }

    if (rawRects.length > 0) {
      const snipStart = Math.max(0, idx - 24);
      const snipEnd = Math.min(haystack.length, endIdx + 24);
      matches.push({
        pageOrd,
        rects: mergeLineRects(rawRects),
        snippet: haystack.slice(snipStart, snipEnd).trim(),
      });
    }
  }

  return matches;
}

export function djvuSpansToPositionedItems(spans: ExtractedSpan[]): PositionedTextItem[] {
  return spans.map((sp) => ({
    text: sp.text,
    rect: sp.bbox,
    hasEOL: false,
  }));
}

/**
 * Converts a PDF.js `TextContent` item array into normalized top-left `PositionedTextItem[]`
 * using the page's rotation-aware `viewport` (at scale 1).
 */
export function pdfTextContentToPositionedItems(
  items: ReadonlyArray<unknown>,
  viewport: { width: number; height: number; transform: number[] },
  transformFn: (m1: number[], m2: number[]) => number[]
): PositionedTextItem[] {
  const out: PositionedTextItem[] = [];
  const vpW = Math.max(1, viewport.width);
  const vpH = Math.max(1, viewport.height);

  for (const raw of items) {
    if (!raw || typeof raw !== 'object' || !('str' in raw)) continue;
    const item = raw as {
      str?: string;
      transform?: number[];
      width?: number;
      height?: number;
      hasEOL?: boolean;
    };
    const str = item.str ?? '';
    if (!str) continue;
    const tx = Array.isArray(item.transform) && item.transform.length >= 6
      ? transformFn(viewport.transform, item.transform)
      : [1, 0, 0, 1, 0, 0];

    const fontHeight = Math.max(
      6,
      Math.hypot(tx[2], tx[3]) || Math.hypot(tx[0], tx[1]) || (item.height ?? 10)
    );
    const widthPts = Math.max(2, (item.width ?? 0) * Math.hypot(viewport.transform[0], viewport.transform[1]) || fontHeight * str.length * 0.45);
    const xPx = tx[4];
    const yTopPx = tx[5] - fontHeight * 0.85;

    const x = Math.max(0, Math.min(0.995, xPx / vpW));
    const y = Math.max(0, Math.min(0.995, yTopPx / vpH));
    const w = Math.max(0.005, Math.min(1 - x, widthPts / vpW));
    const h = Math.max(0.008, Math.min(1 - y, (fontHeight * 1.15) / vpH));

    out.push({
      text: str,
      rect: { x, y, w, h },
      hasEOL: Boolean(item.hasEOL),
    });
  }
  return out;
}
