import type katexType from 'katex';

export type KatexModule = typeof katexType;

let cachedKatexPromise: Promise<KatexModule> | null = null;
let loadedKatex: KatexModule | null = null;

/** Fast pre-check so plain prose never loads the KaTeX chunk or fonts. */
export function hasMathDelimiters(text: string | null | undefined): boolean {
  if (!text) return false;
  return (
    text.includes('$') ||
    text.includes('\\(') ||
    text.includes('\\[') ||
    text.includes('\\begin{')
  );
}

/** Returns synchronously cached KaTeX module if already loaded. */
export function getLoadedKatex(): KatexModule | null {
  return loadedKatex;
}

/**
 * Lazily loads `katex` and its `.woff2`-only stylesheet on first mathematical formula encounter.
 */
export function loadKatex(): Promise<KatexModule> {
  if (loadedKatex) return Promise.resolve(loadedKatex);
  if (!cachedKatexPromise) {
    cachedKatexPromise = Promise.all([
      import('katex'),
      import('katex/dist/katex.min.css'),
    ])
      .then(([mod]) => {
        const k = (mod as unknown as { default?: KatexModule }).default ?? (mod as unknown as KatexModule);
        loadedKatex = k;
        return k;
      })
      .catch((err) => {
        cachedKatexPromise = null;
        throw err;
      });
  }
  return cachedKatexPromise;
}
