<script lang="ts">
  import { tr, type Lang } from '../i18n';
  import { foldMathDiacritics } from '../inDocSearch';
  import type { FileAnnotation, NormalizedRect } from '../types';
  import MathMarkdown from './MathMarkdown.svelte';

  interface Props {
    lang?: Lang;
    annotations: FileAnnotation[];
    onJump: (pageOrd: number, rects: NormalizedRect[]) => void;
    onUpdateNote: (id: number, noteMarkdown: string, color?: string) => Promise<void>;
    onDelete: (id: number) => Promise<void>;
    onExportMarkdown: () => Promise<void>;
  }

  let {
    lang = 'it',
    annotations,
    onJump,
    onUpdateNote,
    onDelete,
    onExportMarkdown,
  }: Props = $props();

  let filterText = $state('');
  let editingId = $state<number | null>(null);
  let draftNote = $state('');
  let draftColor = $state('yellow');
  let exporting = $state(false);

  const filtered = $derived.by(() => {
    const q = foldMathDiacritics(filterText).trim();
    if (!q) return annotations;
    return annotations.filter((a) => {
      const hay = foldMathDiacritics(
        `${a.quoted_text} ${a.note_markdown} ${a.kind} p.${a.page_ord}`
      );
      return hay.includes(q);
    });
  });

  function startEdit(a: FileAnnotation) {
    editingId = a.id;
    draftNote = a.note_markdown;
    draftColor = a.color;
  }

  async function saveEdit(id: number) {
    await onUpdateNote(id, draftNote, draftColor);
    editingId = null;
  }

  async function triggerExport() {
    exporting = true;
    try {
      await onExportMarkdown();
    } finally {
      exporting = false;
    }
  }

  function kindLabel(kind: string): string {
    if (kind === 'bookmark') return tr(lang, 'kindBookmark');
    if (kind === 'note') return tr(lang, 'kindNote');
    if (kind === 'snip') return tr(lang, 'kindSnip');
    return tr(lang, 'kindHighlight');
  }
</script>

<div class="ann-pane">
  <div class="ann-head">
    <input
      type="search"
      class="ann-filter"
      placeholder={tr(lang, 'filterNotesPlaceholder')}
      bind:value={filterText}
    />
    <button
      type="button"
      class="exp-btn"
      disabled={exporting || annotations.length === 0}
      title={tr(lang, 'exportNotesMdTip')}
      onclick={triggerExport}
    >
      .md
    </button>
  </div>

  {#if filtered.length === 0}
    <div class="ann-empty">
      {annotations.length === 0
        ? tr(lang, 'emptyAnnotationsMsg')
        : tr(lang, 'noMatchingAnnotationsMsg')}
    </div>
  {:else}
    <div class="ann-list">
      {#each filtered as a (a.id)}
        <div class="ann-card color-{a.color}">
          <div class="ann-card-head">
            <button
              type="button"
              class="ann-jump-btn"
              onclick={() => onJump(a.page_ord, a.rects)}
            >
              <span class="ann-kind">{kindLabel(a.kind)}</span>
              <span class="ann-pg">p. {a.page_ord}</span>
            </button>
            <div class="ann-card-actions">
              <button
                type="button"
                class="tiny-act"
                title={lang === 'en' ? 'Edit note' : 'Modifica nota'}
                onclick={() => startEdit(a)}
              >
                ✎
              </button>
              <button
                type="button"
                class="tiny-act danger"
                title={lang === 'en' ? 'Delete' : 'Elimina'}
                onclick={() => onDelete(a.id)}
              >
                ×
              </button>
            </div>
          </div>

          {#if a.image_data_url}
            <button
              type="button"
              class="ann-snip-thumb-btn"
              onclick={() => onJump(a.page_ord, a.rects)}
            >
              <img src={a.image_data_url} alt={`Snip p. ${a.page_ord}`} class="ann-snip-thumb" />
            </button>
          {/if}

          {#if a.quoted_text}
            <button
              type="button"
              class="ann-quote"
              onclick={() => onJump(a.page_ord, a.rects)}
            >
              “{a.quoted_text.slice(0, 180)}{a.quoted_text.length > 180 ? '…' : ''}”
            </button>
          {/if}

          {#if editingId === a.id}
            <div class="ann-editor">
              <textarea
                rows="3"
                placeholder={tr(lang, 'snipNotePlaceholder')}
                bind:value={draftNote}
              ></textarea>
              <div class="ann-edit-bar">
                <div class="color-dots">
                  {#each ['yellow', 'green', 'blue', 'pink'] as c}
                    <button
                      type="button"
                      class="cdot dot-{c}"
                      class:selected={draftColor === c}
                      aria-label={c}
                      onclick={() => (draftColor = c)}
                    ></button>
                  {/each}
                </div>
                <div class="edit-btns">
                  <button type="button" class="mini" onclick={() => (editingId = null)}>
                    {tr(lang, 'cancel')}
                  </button>
                  <button type="button" class="mini primary" onclick={() => saveEdit(a.id)}>
                    {tr(lang, 'save')}
                  </button>
                </div>
              </div>
            </div>
          {:else if a.note_markdown}
            <div class="ann-note">
              <MathMarkdown text={a.note_markdown} compact={true} />
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .ann-pane {
    display: flex;
    flex-direction: column;
    gap: 6px;
    height: 100%;
  }
  .ann-head {
    display: flex;
    gap: 4px;
    align-items: center;
  }
  .ann-filter {
    flex: 1;
    min-width: 0;
    font-size: 11.5px;
    padding: 3px 6px;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
  }
  .exp-btn {
    padding: 3px 7px;
    font-size: 11px;
    font-weight: 600;
    border-radius: 5px;
    flex-shrink: 0;
  }
  .ann-empty {
    padding: 10px 4px;
    font-size: 11.5px;
    color: var(--text-muted);
    line-height: 1.45;
  }
  .ann-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .ann-card {
    border: 1px solid var(--border);
    border-left-width: 3px;
    border-radius: 6px;
    padding: 6px 7px;
    background: var(--bg);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .ann-card.color-yellow {
    border-left-color: #eab308;
  }
  .ann-card.color-green {
    border-left-color: #22c55e;
  }
  .ann-card.color-blue {
    border-left-color: #3b82f6;
  }
  .ann-card.color-pink {
    border-left-color: #ec4899;
  }
  .ann-card-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 4px;
  }
  .ann-jump-btn {
    border: none;
    background: transparent;
    padding: 0;
    display: flex;
    align-items: center;
    gap: 5px;
    cursor: pointer;
    color: var(--text);
  }
  .ann-kind {
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    color: var(--text-muted);
  }
  .ann-pg {
    font-size: 11px;
    font-weight: 700;
    color: var(--accent);
  }
  .ann-card-actions {
    display: flex;
    gap: 2px;
  }
  .tiny-act {
    border: none;
    background: transparent;
    color: var(--text-muted);
    font-size: 12px;
    padding: 0 4px;
    cursor: pointer;
  }
  .tiny-act:hover {
    color: var(--text);
  }
  .tiny-act.danger:hover {
    color: var(--danger);
  }
  .ann-snip-thumb-btn {
    border: 1px solid var(--border);
    border-radius: 4px;
    background: #ffffff;
    padding: 3px;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    max-height: 110px;
    overflow: hidden;
  }
  .ann-snip-thumb {
    max-width: 100%;
    max-height: 100px;
    object-fit: contain;
    display: block;
  }
  .ann-quote {
    border: none;
    background: transparent;
    text-align: left;
    padding: 0;
    font-size: 11px;
    font-style: italic;
    color: var(--text-muted);
    line-height: 1.35;
    cursor: pointer;
  }
  .ann-note {
    padding-top: 2px;
  }
  .ann-editor {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .ann-editor textarea {
    width: 100%;
    font-size: 11.5px;
    padding: 5px;
    border-radius: 4px;
    border: 1px solid var(--accent);
    background: var(--bg-panel);
    color: var(--text);
    box-sizing: border-box;
    font-family: inherit;
  }
  .ann-edit-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 4px;
  }
  .color-dots {
    display: flex;
    gap: 4px;
  }
  .cdot {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 1.5px solid transparent;
    padding: 0;
    cursor: pointer;
  }
  .cdot.selected {
    border-color: var(--text);
  }
  .dot-yellow {
    background: #facc15;
  }
  .dot-green {
    background: #4ade80;
  }
  .dot-blue {
    background: #60a5fa;
  }
  .dot-pink {
    background: #f472b6;
  }
  .edit-btns {
    display: flex;
    gap: 4px;
  }
  .mini {
    font-size: 10.5px;
    padding: 2px 6px;
    border-radius: 4px;
  }
</style>
