import type { KatexModule } from './katexLoader';

export interface RenderMathMarkdownOptions {
  /** Whether to turn `[Doc #id, p. X]` and `[p. X]` into clickable `<button data-cite-doc="..." data-cite-page="...">` pills. */
  clickableCitations?: boolean;
  /** Default documentId for single-doc `[p. X]` markers when `clickableCitations` is true. */
  defaultDocId?: number | null;
}

export function escapeHtml(raw: string): string {
  return raw
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

interface PlaceholderStore {
  items: string[];
  add(html: string): string;
}

function createStore(): PlaceholderStore {
  const items: string[] = [];
  return {
    items,
    add(html: string) {
      const idx = items.length;
      items.push(html);
      return `\u0000P${idx}\u0000`;
    },
  };
}

function renderLatexExpr(expr: string, displayMode: boolean, katex: KatexModule | null): string {
  const trimmed = expr.trim();
  if (!trimmed) return '';
  if (!katex) {
    const safe = escapeHtml(trimmed);
    return displayMode
      ? `<div class="mm-math-fallback mm-math-display"><code>${safe}</code></div>`
      : `<code class="mm-math-fallback">${safe}</code>`;
  }
  try {
    return katex.renderToString(trimmed, {
      displayMode,
      throwOnError: false,
      strict: 'ignore',
      trust: false,
      maxSize: 500,
      maxExpand: 300,
    });
  } catch {
    return `<code class="mm-math-fallback">${escapeHtml(trimmed)}</code>`;
  }
}

/**
 * Extracts math spans (`$$...$$`, `\[...\]`, `\(...\)`, `$...$`) before Markdown formatting
 * so underscores, asterisks, and backslashes inside LaTeX are never mangled.
 */
function extractMathSpans(input: string, katex: KatexModule | null, store: PlaceholderStore): string {
  // 1. Display math: $$...$$ and \[...\]
  let out = input.replace(/\$\$([\s\S]+?)\$\$/g, (_m, inner: string) =>
    store.add(`<div class="mm-math-block">${renderLatexExpr(inner, true, katex)}</div>`)
  );
  out = out.replace(/\\\[([\s\S]+?)\\\]/g, (_m, inner: string) =>
    store.add(`<div class="mm-math-block">${renderLatexExpr(inner, true, katex)}</div>`)
  );
  // 2. Inline math: \(...\)
  out = out.replace(/\\\((.+?)\\\)/gs, (_m, inner: string) =>
    store.add(`<span class="mm-math-inline">${renderLatexExpr(inner, false, katex)}</span>`)
  );
  // 3. Inline math: $...$ (avoid escaped \$, currency like $100 followed by space, and empty $$)
  out = out.replace(/(^|[^\\$])\$(?!\$|\s)([^$\n]+?)(?<!\\|\s)\$(?!\$)/g, (match, prefix: string, inner: string) => {
    // Reject obvious currency ranges like "$5 to $10" (starts with digit and has spaces without math ops)
    if (/^\d+(?:[.,]\d+)?\s/.test(inner) && !/[=+\-^_\\<>]/.test(inner)) {
      return match;
    }
    return `${prefix}${store.add(`<span class="mm-math-inline">${renderLatexExpr(inner, false, katex)}</span>`)}`;
  });
  return out;
}

function formatInlineMarkdown(
  escapedText: string,
  opts: RenderMathMarkdownOptions,
  store: PlaceholderStore
): string {
  let s = escapedText;

  // Inline code spans (protect before bold/italic/citations)
  s = s.replace(/`([^`\n]+)`/g, (_m, code: string) =>
    store.add(`<code class="mm-code-inline">${code}</code>`)
  );

  // Clickable citations `[Doc #42, p. 17]` and single-doc `[p. 17]`
  if (opts.clickableCitations) {
    s = s.replace(/\[Doc\s*#?(\d+)\s*,\s*p{1,2}\.?\s*(\d+)[^\]]*\]/g, (full, docId: string, page: string) =>
      store.add(
        `<button type="button" class="mm-cite-pill" data-cite-doc="${docId}" data-cite-page="${page}">${full}</button>`
      )
    );
    if (opts.defaultDocId != null) {
      const defDoc = String(opts.defaultDocId);
      s = s.replace(/\[p{1,2}\.?\s*(\d+)[^\]]*\]/g, (full, page: string) =>
        store.add(
          `<button type="button" class="mm-cite-pill" data-cite-doc="${defDoc}" data-cite-page="${page}">${full}</button>`
        )
      );
    }
  }

  // Bold **...** and italic *...*
  s = s.replace(/\*\*([^*\n]+?)\*\*/g, '<strong>$1</strong>');
  s = s.replace(/(^|[^*])\*([^*\n]+?)\*(?!\*)/g, '$1<em>$2</em>');

  return s;
}

/**
 * Safe Markdown + LaTeX (KaTeX) renderer. Strictly escapes all user/LLM HTML first,
 * never executes scripts or arbitrary URLs, and supports headings, lists, blockquotes,
 * fenced code blocks, inline formatting, clickable citations, and inline/display KaTeX.
 */
export function renderMathMarkdown(
  source: string,
  katex: KatexModule | null,
  opts: RenderMathMarkdownOptions = {}
): string {
  if (!source || !source.trim()) return '';

  const store = createStore();
  let working = source.replace(/\r\n?/g, '\n');

  // 1. Protect fenced code blocks first so `$` inside code isn't treated as LaTeX
  working = working.replace(/```([a-zA-Z0-9_-]*)\n([\s\S]*?)```/g, (_m, _lang: string, code: string) =>
    store.add(`<pre class="mm-pre"><code>${escapeHtml(code.replace(/\n$/, ''))}</code></pre>`)
  );

  // 2. Extract and render math spans before HTML escaping & Markdown inline rules
  working = extractMathSpans(working, katex, store);

  // 3. Escape remaining raw HTML strictly
  const escaped = escapeHtml(working);

  // 4. Parse lightweight block structure line by line
  const lines = escaped.split('\n');
  const htmlBlocks: string[] = [];
  let paraBuf: string[] = [];
  let listItems: string[] = [];
  let listTag: 'ul' | 'ol' | null = null;

  const flushPara = () => {
    if (paraBuf.length === 0) return;
    const joined = paraBuf.join('<br />');
    htmlBlocks.push(`<p>${formatInlineMarkdown(joined, opts, store)}</p>`);
    paraBuf = [];
  };

  const flushList = () => {
    if (!listTag || listItems.length === 0) return;
    htmlBlocks.push(`<${listTag}>${listItems.join('')}</${listTag}>`);
    listItems = [];
    listTag = null;
  };

  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed) {
      flushPara();
      flushList();
      continue;
    }

    // Standalone placeholder block (fenced code or display math)
    if (/^\u0000P\d+\u0000$/.test(trimmed)) {
      flushPara();
      flushList();
      htmlBlocks.push(trimmed);
      continue;
    }

    // Headings (# .. ####)
    const headingMatch = trimmed.match(/^(#{1,4})\s+(.+)$/);
    if (headingMatch) {
      flushPara();
      flushList();
      const level = Math.min(4, headingMatch[1].length + 2); // render as h3..h6 for compact UI
      htmlBlocks.push(
        `<h${level} class="mm-heading">${formatInlineMarkdown(headingMatch[2], opts, store)}</h${level}>`
      );
      continue;
    }

    // Blockquote
    if (trimmed.startsWith('&gt; ')) {
      flushPara();
      flushList();
      htmlBlocks.push(
        `<blockquote class="mm-quote">${formatInlineMarkdown(trimmed.slice(5), opts, store)}</blockquote>`
      );
      continue;
    }

    // Unordered list
    const ulMatch = trimmed.match(/^[-*]\s+(.+)$/);
    if (ulMatch) {
      flushPara();
      if (listTag && listTag !== 'ul') flushList();
      listTag = 'ul';
      listItems.push(`<li>${formatInlineMarkdown(ulMatch[1], opts, store)}</li>`);
      continue;
    }

    // Ordered list
    const olMatch = trimmed.match(/^\d+\.\s+(.+)$/);
    if (olMatch) {
      flushPara();
      if (listTag && listTag !== 'ol') flushList();
      listTag = 'ol';
      listItems.push(`<li>${formatInlineMarkdown(olMatch[1], opts, store)}</li>`);
      continue;
    }

    flushList();
    paraBuf.push(trimmed);
  }
  flushPara();
  flushList();

  // 5. Restore placeholders (multi-pass in case nested)
  let output = htmlBlocks.join('\n');
  for (let pass = 0; pass < 3 && output.includes('\u0000P'); pass++) {
    output = output.replace(/\u0000P(\d+)\u0000/g, (_m, idxStr: string) => store.items[Number(idxStr)] ?? '');
  }
  return output;
}
