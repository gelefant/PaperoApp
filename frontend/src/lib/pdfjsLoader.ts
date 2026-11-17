import type * as PdfJsNamespace from 'pdfjs-dist';
import type { PDFDataRangeTransport } from 'pdfjs-dist';

export type PdfJsModule = typeof PdfJsNamespace;

let cachedPdfjsPromise: Promise<PdfJsModule> | null = null;

type RangeReaderFn = (begin: number, length: number) => Promise<Uint8Array<ArrayBuffer>>;

type RangeTransportCtor = new (
  length: number,
  initialData: Uint8Array<ArrayBuffer>,
  readRange: RangeReaderFn,
  isAborted: () => boolean,
  onRangeError: (err: unknown) => void
) => PDFDataRangeTransport;

let cachedTransportCtor: RangeTransportCtor | null = null;

/**
 * Lazily loads `pdfjs-dist` and registers its LoopbackPort `WorkerMessageHandler` in-module
 * on first PDF open so cold startup never parses or allocates the ~1.65 MB PDF.js bundle.
 */
export function loadPdfJs(): Promise<PdfJsModule> {
  if (!cachedPdfjsPromise) {
    cachedPdfjsPromise = Promise.all([
      import('pdfjs-dist'),
      import('pdfjs-dist/build/pdf.worker.min.mjs'),
    ])
      .then(([pdfjsLib, workerMod]) => {
        (globalThis as Record<string, unknown>).pdfjsWorker = {
          WorkerMessageHandler: workerMod.WorkerMessageHandler,
        };
        return pdfjsLib;
      })
      .catch((err) => {
        cachedPdfjsPromise = null;
        throw err;
      });
  }
  return cachedPdfjsPromise;
}

export function createPdfRangeTransport(
  pdfjsLib: PdfJsModule,
  length: number,
  initialData: Uint8Array<ArrayBuffer>,
  readRange: RangeReaderFn,
  isAborted: () => boolean,
  onRangeError: (err: unknown) => void
): PDFDataRangeTransport {
  if (!cachedTransportCtor) {
    class TauriPdfRangeTransport extends pdfjsLib.PDFDataRangeTransport {
      private readRange: RangeReaderFn;
      private isAborted: () => boolean;
      private onRangeError: (err: unknown) => void;

      constructor(
        totalLen: number,
        initBytes: Uint8Array<ArrayBuffer>,
        reader: RangeReaderFn,
        abortedCheck: () => boolean,
        errorCb: (err: unknown) => void
      ) {
        super(totalLen, initBytes, totalLen <= initBytes.byteLength);
        this.readRange = reader;
        this.isAborted = abortedCheck;
        this.onRangeError = errorCb;
      }

      override requestDataRange(begin: number, end: number): void {
        if (this.isAborted()) return;
        const len = Math.max(0, end - begin);
        if (len === 0) {
          this.onDataRange(begin, new Uint8Array(0));
          return;
        }
        this.readRange(begin, len)
          .then((bytes) => {
            if (!this.isAborted()) {
              this.onDataRange(begin, bytes);
            }
          })
          .catch((err) => {
            if (!this.isAborted()) {
              this.onRangeError(err);
            }
          });
      }
    }
    cachedTransportCtor = TauriPdfRangeTransport;
  }
  return new cachedTransportCtor(length, initialData, readRange, isAborted, onRangeError);
}
