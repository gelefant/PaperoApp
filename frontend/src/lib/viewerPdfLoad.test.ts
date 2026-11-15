import { describe, expect, it } from 'vitest';
import * as pdfjsLib from 'pdfjs-dist';
import { WorkerMessageHandler } from 'pdfjs-dist/build/pdf.worker.min.mjs';

(globalThis as Record<string, unknown>).pdfjsWorker = { WorkerMessageHandler };

function buildMinimalValidPdf(): Uint8Array {
  const objects = [
    '1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n',
    '2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n',
    '3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n',
    '4 0 obj\n<< /Length 44 >>\nstream\nBT /F1 18 Tf 72 700 Td (Hello Papero) Tj ET\nendstream\nendobj\n',
    '5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n',
  ];

  let out = '%PDF-1.4\n';
  const offsets: number[] = [0];
  for (const obj of objects) {
    offsets.push(out.length);
    out += obj;
  }
  const xrefOffset = out.length;
  out += `xref\n0 ${objects.length + 1}\n`;
  out += '0000000000 65535 f \n';
  for (let i = 1; i <= objects.length; i++) {
    out += `${String(offsets[i]).padStart(10, '0')} 00000 n \n`;
  }
  out += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xrefOffset}\n%%EOF\n`;
  return new TextEncoder().encode(out);
}

describe('PDF.js in-process WorkerMessageHandler integration', () => {
  it('loads PDF bytes and extracts page viewport and text without spawning an external Worker', async () => {
    const data = buildMinimalValidPdf();
    const doc = await pdfjsLib.getDocument({
      data,
      isOffscreenCanvasSupported: false,
      isImageDecoderSupported: false,
      useSystemFonts: true,
    }).promise;

    expect(doc.numPages).toBe(1);
    const page = await doc.getPage(1);
    const vp = page.getViewport({ scale: 1 });
    expect(vp.width).toBe(612);
    expect(vp.height).toBe(792);

    const tc = await page.getTextContent();
    const text = tc.items.map((it) => ('str' in it ? it.str : '')).join(' ');
    expect(text).toContain('Hello Papero');
    await doc.destroy();
  });
});
