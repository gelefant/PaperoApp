import type { NormalizedRect } from './types';

export interface PositionedLineToken {
  str: string;
  x: number;
  y: number;
}

/**
 * Converts a pointer drag box `(x0, y0) -> (x1, y1)` inside a page container of `(width, height)`
 * into a clamped `[0, 1]` `NormalizedRect`, returning `null` if smaller than `minPx`.
 */
export function normalizeDragRect(
  x0: number,
  y0: number,
  x1: number,
  y1: number,
  width: number,
  height: number,
  minPx = 10
): NormalizedRect | null {
  if (width <= 0 || height <= 0) return null;
  const left = Math.max(0, Math.min(width, Math.min(x0, x1)));
  const right = Math.max(0, Math.min(width, Math.max(x0, x1)));
  const top = Math.max(0, Math.min(height, Math.min(y0, y1)));
  const bottom = Math.max(0, Math.min(height, Math.max(y0, y1)));
  const pxW = right - left;
  const pxH = bottom - top;
  if (pxW < minPx || pxH < minPx) return null;
  return {
    x: left / width,
    y: top / height,
    w: pxW / width,
    h: pxH / height,
  };
}

/**
 * Crops a normalized rectangle from a rendered `<canvas>` or `<img>` into a crisp PNG data URL.
 */
export function cropSourceElementToPngDataUrl(
  source: HTMLCanvasElement | HTMLImageElement,
  rect: NormalizedRect,
  maxSidePx = 1800
): string | null {
  const natW =
    source instanceof HTMLCanvasElement ? source.width : source.naturalWidth || source.width;
  const natH =
    source instanceof HTMLCanvasElement ? source.height : source.naturalHeight || source.height;
  if (natW <= 0 || natH <= 0) return null;

  const sx = Math.max(0, Math.min(natW - 1, Math.round(rect.x * natW)));
  const sy = Math.max(0, Math.min(natH - 1, Math.round(rect.y * natH)));
  const sw = Math.max(1, Math.min(natW - sx, Math.round(rect.w * natW)));
  const sh = Math.max(1, Math.min(natH - sy, Math.round(rect.h * natH)));

  const scale = Math.min(1, maxSidePx / Math.max(sw, sh));
  const outW = Math.max(1, Math.round(sw * scale));
  const outH = Math.max(1, Math.round(sh * scale));

  const off = document.createElement('canvas');
  off.width = outW;
  off.height = outH;
  const ctx = off.getContext('2d');
  if (!ctx) return null;
  ctx.fillStyle = '#ffffff';
  ctx.fillRect(0, 0, outW, outH);
  ctx.drawImage(source, sx, sy, sw, sh, 0, 0, outW, outH);
  const url = off.toDataURL('image/png');
  off.width = 0;
  off.height = 0;
  return url;
}

export function dataUrlToPngBlob(dataUrl: string): Blob | null {
  const prefix = 'data:image/png;base64,';
  if (!dataUrl.startsWith(prefix)) return null;
  try {
    const bin = atob(dataUrl.slice(prefix.length));
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) {
      bytes[i] = bin.charCodeAt(i);
    }
    return new Blob([bytes], { type: 'image/png' });
  } catch {
    return null;
  }
}

const BIB_ITEM_START_RE = /^\s*(?:\[[A-Za-z0-9+\-–/.]{1,14}\]|\(\d{1,3}\)|\d{1,3}\.)\s+\S/;

/**
 * Extracts the single bibliography reference entry near `targetViewportY` on a PDF destination page
 * without bleeding into subsequent `[Ref]` / `12.` entries below it.
 */
export function extractBibEntryNearViewportY(
  tokens: PositionedLineToken[],
  targetViewportY: number | null,
  viewportHeight: number
): string {
  if (!tokens.length || viewportHeight <= 0) return '';

  const nonEmpty = tokens.filter((t) => t.str.trim().length > 0);
  if (!nonEmpty.length) return '';

  const sorted = [...nonEmpty].sort((a, b) => a.y - b.y || a.x - b.x);
  const lines: Array<{ y: number; text: string }> = [];

  for (const tok of sorted) {
    const last = lines[lines.length - 1];
    if (last && Math.abs(tok.y - last.y) <= 4.5) {
      const needsSpace =
        !last.text.endsWith('-') && !last.text.endsWith(' ') && !tok.str.startsWith(' ');
      last.text += (needsSpace ? ' ' : '') + tok.str.trim();
    } else {
      lines.push({ y: tok.y, text: tok.str.trim() });
    }
  }
  if (!lines.length) return '';

  let startIdx = 0;
  if (targetViewportY !== null && Number.isFinite(targetViewportY)) {
    let bestDist = Infinity;
    for (let i = 0; i < lines.length; i++) {
      const d = Math.abs(lines[i].y - targetViewportY);
      if (d < bestDist) {
        bestDist = d;
        startIdx = i;
      }
    }
    if (
      startIdx > 0 &&
      !BIB_ITEM_START_RE.test(lines[startIdx].text) &&
      BIB_ITEM_START_RE.test(lines[startIdx - 1].text) &&
      targetViewportY - lines[startIdx - 1].y <= 20
    ) {
      startIdx -= 1;
    }
  }

  const collected: string[] = [];
  const baseY = lines[startIdx].y;
  for (let i = startIdx; i < lines.length && collected.length < 6; i++) {
    const ln = lines[i];
    if (i > startIdx) {
      if (ln.y - baseY > 115) break;
      if (BIB_ITEM_START_RE.test(ln.text)) break;
    }
    collected.push(ln.text);
  }

  return collected.join(' ').replace(/\s+/g, ' ').trim();
}
