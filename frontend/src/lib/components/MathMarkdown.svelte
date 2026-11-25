<script lang="ts">
  import { getLoadedKatex, hasMathDelimiters, loadKatex, type KatexModule } from '../katexLoader';
  import { renderMathMarkdown } from '../mathMarkdown';

  interface Props {
    text: string;
    clickableCitations?: boolean;
    defaultDocId?: number | null;
    onCitationClick?: (docId: number, pageOrd: number) => void;
    compact?: boolean;
  }

  let {
    text,
    clickableCitations = false,
    defaultDocId = null,
    onCitationClick,
    compact = false,
  }: Props = $props();

  let katexMod = $state<KatexModule | null>(getLoadedKatex());

  $effect(() => {
    if (!katexMod && hasMathDelimiters(text)) {
      let cancelled = false;
      loadKatex()
        .then((mod) => {
          if (!cancelled) katexMod = mod;
        })
        .catch(() => {});
      return () => {
        cancelled = true;
      };
    }
  });

  let renderedHtml = $derived(
    renderMathMarkdown(text, katexMod, { clickableCitations, defaultDocId })
  );

  function handleClick(ev: MouseEvent) {
    if (!onCitationClick) return;
    const target = ev.target as HTMLElement | null;
    const btn = target?.closest<HTMLButtonElement>('button[data-cite-page]');
    if (!btn) return;
    const docId = Number(btn.dataset.citeDoc ?? defaultDocId ?? 0);
    const pageOrd = Number(btn.dataset.citePage ?? 0);
    if (pageOrd > 0) {
      ev.preventDefault();
      ev.stopPropagation();
      onCitationClick(docId, pageOrd);
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="math-markdown" class:compact onclick={handleClick}>
  {@html renderedHtml}
</div>

<style>
  .math-markdown {
    font-size: 13px;
    line-height: 1.55;
    color: var(--text);
    word-break: break-word;
  }

  .math-markdown.compact {
    font-size: 12px;
    line-height: 1.48;
  }

  .math-markdown :global(p) {
    margin: 0 0 0.55em;
  }

  .math-markdown :global(p:last-child) {
    margin-bottom: 0;
  }

  .math-markdown :global(ul),
  .math-markdown :global(ol) {
    margin: 0.35em 0 0.55em;
    padding-left: 1.35em;
  }

  .math-markdown :global(li) {
    margin-bottom: 0.2em;
  }

  .math-markdown :global(.mm-heading) {
    margin: 0.65em 0 0.3em;
    font-size: 1em;
    font-weight: 700;
  }

  .math-markdown :global(.mm-quote) {
    margin: 0.45em 0;
    padding: 4px 10px;
    border-left: 3px solid var(--accent);
    background: var(--bg-subtle, rgba(127, 127, 127, 0.08));
    border-radius: 0 4px 4px 0;
  }

  .math-markdown :global(.mm-pre) {
    margin: 0.45em 0;
    padding: 8px 10px;
    border-radius: 6px;
    background: var(--bg-subtle, rgba(127, 127, 127, 0.1));
    overflow-x: auto;
    font-size: 11.5px;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }

  .math-markdown :global(.mm-code-inline),
  .math-markdown :global(.mm-math-fallback) {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.92em;
    padding: 1px 4px;
    border-radius: 3px;
    background: var(--bg-subtle, rgba(127, 127, 127, 0.12));
  }

  .math-markdown :global(.mm-math-block) {
    margin: 0.5em 0;
    overflow-x: auto;
    text-align: center;
  }

  .math-markdown :global(.mm-cite-pill) {
    display: inline;
    padding: 0 5px;
    margin: 0 1px;
    border-radius: 4px;
    border: 1px solid color-mix(in srgb, var(--accent) 40%, transparent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    color: var(--accent);
    font-size: 0.88em;
    font-weight: 600;
    cursor: pointer;
    vertical-align: baseline;
  }

  .math-markdown :global(.mm-cite-pill:hover) {
    background: color-mix(in srgb, var(--accent) 22%, transparent);
  }
</style>
