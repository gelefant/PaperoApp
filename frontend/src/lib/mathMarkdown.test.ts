import { describe, expect, it } from 'vitest';
import katex from 'katex';
import { hasMathDelimiters } from './katexLoader';
import { renderMathMarkdown } from './mathMarkdown';

describe('mathMarkdown & lazy KaTeX', () => {
  it('detects math delimiters only when present', () => {
    expect(hasMathDelimiters('Plain mathematical text without delimiters.')).toBe(false);
    expect(hasMathDelimiters('Let $H^1(X, \\mathcal{O}_X) = 0$.')).toBe(true);
    expect(hasMathDelimiters('Display \\[\\int_0^1 x^2 dx\\]')).toBe(true);
  });

  it('escapes raw HTML strictly and prevents XSS injection', () => {
    const html = renderMathMarkdown('Hello <script>alert(1)</script> **bold**', katex);
    expect(html).not.toContain('<script>');
    expect(html).toContain('&lt;script&gt;');
    expect(html).toContain('<strong>bold</strong>');
  });

  it('renders inline and display KaTeX without mangling underscores', () => {
    const html = renderMathMarkdown(
      'For $a_n + b_n = c_n$ we have:\n\n$$\\sum_{k=1}^n k = \\frac{n(n+1)}{2}$$',
      katex
    );
    expect(html).toContain('katex');
    expect(html).toContain('mm-math-inline');
    expect(html).toContain('mm-math-block');
  });

  it('turns [Doc #id, p. X] and single-doc [p. X] into clickable citation buttons', () => {
    const multi = renderMathMarkdown(
      'By Hahn-Banach [Doc #42, p. 17] and also [p. 9].',
      null,
      { clickableCitations: true, defaultDocId: 42 }
    );
    expect(multi).toContain('data-cite-doc="42" data-cite-page="17"');
    expect(multi).toContain('data-cite-doc="42" data-cite-page="9"');
  });
});
