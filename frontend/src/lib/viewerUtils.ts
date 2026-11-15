import type { FilePageInfo, LlmCitation, NormalizedRect } from './types';

/**
 * Builds a platform-aware custom protocol URL for `papero-doc://`.
 */
export function paperoDocUrl(
  path: string,
  isWindows = typeof navigator !== 'undefined' && navigator.userAgent.includes('Windows')
): string {
  const clean = path.startsWith('/') ? path : `/${path}`;
  return isWindows
    ? `http://papero-doc.localhost${clean}`
    : `papero-doc://localhost${clean}`;
}

/**
 * Resolves a user "go to page" query against either a 1-based ordinal number
 * or a printed page label (e.g. "xiv", "142"), returning the 1-based ordinal page.
 */
export function resolveGoToPage(
  input: string,
  pageCount: number,
  pages: FilePageInfo[]
): number | null {
  const trimmed = input.trim();
  if (!trimmed || pageCount <= 0) return null;

  // Prefer exact printed label match first if non-numeric or if explicitly matching a label
  const lower = trimmed.toLowerCase();
  const exactLabel = pages.find(
    (p) => p.printed_label && p.printed_label.trim().toLowerCase() === lower
  );
  if (exactLabel && !/^\d+$/.test(trimmed)) {
    return exactLabel.page_ord;
  }

  if (/^\d+$/.test(trimmed)) {
    const ord = Number.parseInt(trimmed, 10);
    if (ord >= 1 && ord <= pageCount) {
      return ord;
    }
  }

  if (exactLabel) {
    return exactLabel.page_ord;
  }
  return null;
}

/**
 * Computes the vertical center fraction `[0.0, 1.0]` of a set of normalized top-left
 * passage rectangles so the viewer can scroll and center the highlighted passage.
 */
export function computePassageCenterFraction(rects: NormalizedRect[]): number {
  if (!rects.length) return 0.25;
  let minY = 1.0;
  let maxY = 0.0;
  for (const r of rects) {
    if (r.y < minY) minY = r.y;
    if (r.y + r.h > maxY) maxY = r.y + r.h;
  }
  return Math.max(0, Math.min(1, (minY + maxY) / 2));
}

/**
 * Computes the scrollTop offset inside the scroll container to center `centerFraction`
 * of a page slot at `(pageTopPx, pageHeightPx)`.
 */
export function computeCenteredScrollTop(
  pageTopPx: number,
  pageHeightPx: number,
  centerFraction: number,
  viewportHeightPx: number
): number {
  const targetY = pageTopPx + pageHeightPx * centerFraction;
  return Math.max(0, targetY - viewportHeightPx / 2);
}

/**
 * Builds a clean highlight snippet around the matched query terms.
 */
export function buildHighlightedSnippet(
  text: string,
  query: string,
  maxLen = 280
): string {
  const clean = text.replace(/\s+/g, ' ').trim();
  if (clean.length <= maxLen) return clean;
  const terms = query
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter((t) => t.length >= 2);
  let pos = 0;
  const lower = clean.toLowerCase();
  for (const t of terms) {
    const idx = lower.indexOf(t);
    if (idx >= 0) {
      pos = Math.max(0, idx - 80);
      break;
    }
  }
  const slice = clean.slice(pos, pos + maxLen);
  return (pos > 0 ? '…' : '') + slice + (pos + maxLen < clean.length ? '…' : '');
}

export type AnswerSegment =
  | { kind: 'text'; text: string }
  | {
      kind: 'citation';
      label: string;
      documentId: number;
      pageOrd: number;
      citation: LlmCitation | null;
    };

/**
 * Splits an LLM answer into plain text and clickable `[Doc #id, p. X]` inline citation segments,
 * binding each marker to the resolved [`LlmCitation`] entry returned by Rust.
 */
export function splitAnswerWithCitations(
  answerText: string,
  citations: LlmCitation[]
): AnswerSegment[] {
  const byKey = new Map<string, LlmCitation>();
  for (const c of citations) {
    byKey.set(`${c.document_id}:${c.page_ord}`, c);
  }

  const re = /\[\s*Doc\s*#(\d+)\s*,\s*(?:p\.?|pag\.?)\s*(\d+)\s*\]/gi;
  const out: AnswerSegment[] = [];
  let cursor = 0;

  for (const match of answerText.matchAll(re)) {
    const idx = match.index ?? 0;
    if (idx > cursor) {
      out.push({ kind: 'text', text: answerText.slice(cursor, idx) });
    }
    const documentId = Number.parseInt(match[1], 10);
    const pageOrd = Number.parseInt(match[2], 10);
    const citation =
      byKey.get(`${documentId}:${pageOrd}`) ??
      citations.find((c) => c.marker === match[0]) ??
      null;
    out.push({
      kind: 'citation',
      label: `[Doc #${documentId}, p. ${pageOrd}]`,
      documentId,
      pageOrd,
      citation,
    });
    cursor = idx + match[0].length;
  }

  if (cursor < answerText.length) {
    out.push({ kind: 'text', text: answerText.slice(cursor) });
  }
  return out;
}
