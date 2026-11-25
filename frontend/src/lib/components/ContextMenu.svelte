<script lang="ts">
  import { tr, type Lang } from '../i18n';
  import type { CollectionView, ExternalReaderPreset } from '../types';

  export interface ContextTarget {
    x: number;
    y: number;
    documentId: number;
    fileId: number;
    format: string;
    isAvailable: boolean;
    pageOrd?: number;
  }

  interface Props {
    lang?: Lang;
    target: ContextTarget | null;
    presets: ExternalReaderPreset[];
    collections?: CollectionView[];
    onClose: () => void;
    onOpenInternal: (fileId: number, pageOrd?: number) => void;
    onOpenExternal: (fileId: number, pageOrd?: number, templateOverride?: string | null) => void;
    onInspectMetadata: (documentId: number) => void;
    onCopyBibtex: (documentId: number) => void;
    onToggleCollection?: (collectionId: number, documentId: number, currentlyIn: boolean) => void;
    onRunOcr?: (fileId: number) => void;
    onRevealInFolder: (fileId: number) => void;
    onCopyPath: (fileId: number) => void;
  }

  let {
    lang = 'it',
    target,
    presets,
    collections = [],
    onClose,
    onOpenInternal,
    onOpenExternal,
    onInspectMetadata,
    onCopyBibtex,
    onToggleCollection,
    onRunOcr,
    onRevealInFolder,
    onCopyPath,
  }: Props = $props();

  let collectionsOpen = $state(false);
  const unavailableReason = $derived(tr(lang, 'unavailableFolderReason'));

  $effect(() => {
    if (!target) {
      collectionsOpen = false;
    }
  });
</script>

{#if target}
  <div
    class="backdrop"
    role="presentation"
    onclick={onClose}
    oncontextmenu={(e) => {
      e.preventDefault();
      onClose();
    }}
  >
    <div
      class="menu"
      style="left: {Math.min(target.x, window.innerWidth - 270)}px; top: {Math.min(target.y, window.innerHeight - 360)}px;"
      role="menu"
      tabindex="-1"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => {
        if (e.key === 'Escape') onClose();
        e.stopPropagation();
      }}
    >
      <button
        type="button"
        disabled={!target.isAvailable}
        title={target.isAvailable ? '' : unavailableReason}
        onclick={() => {
          onOpenInternal(target.fileId, target.pageOrd);
          onClose();
        }}
      >
        {tr(lang, 'ctxOpenInternal')}
      </button>

      <button
        type="button"
        disabled={!target.isAvailable}
        title={target.isAvailable ? '' : unavailableReason}
        onclick={() => {
          onOpenExternal(target.fileId, target.pageOrd, null);
          onClose();
        }}
      >
        {tr(lang, 'ctxOpenExternal')}
      </button>

      {#each presets.filter((p) => p.formats.includes(target.format)) as preset}
        <button
          type="button"
          class="subitem"
          disabled={!target.isAvailable}
          title={!target.isAvailable ? unavailableReason : (preset.note ?? preset.template)}
          onclick={() => {
            onOpenExternal(target.fileId, target.pageOrd, preset.template);
            onClose();
          }}
        >
          ↳ {preset.label}
        </button>
      {/each}

      <div class="sep"></div>

      {#if onToggleCollection && collections.length > 0}
        <button
          type="button"
          class="sub-toggle"
          onclick={() => (collectionsOpen = !collectionsOpen)}
        >
          <span>{tr(lang, 'ctxAddToCollection')}</span>
          <span>{collectionsOpen ? '▾' : '▸'}</span>
        </button>
        {#if collectionsOpen}
          <div class="col-submenu">
            {#each collections as col (col.id)}
              {@const inCol = col.document_ids.includes(target.documentId)}
              <button
                type="button"
                class="subitem col-item"
                class:checked={inCol}
                onclick={() => {
                  onToggleCollection(col.id, target.documentId, inCol);
                  onClose();
                }}
              >
                <span>{inCol ? '✓' : '+'} {col.name}</span>
                {#if col.bib_sync_enabled && col.bib_sync_path}
                  <span class="bib-pill">.bib</span>
                {/if}
              </button>
            {/each}
          </div>
        {/if}
        <div class="sep"></div>
      {/if}

      <button
        type="button"
        disabled={!target.isAvailable}
        title={target.isAvailable ? '' : unavailableReason}
        onclick={() => {
          onRevealInFolder(target.fileId);
          onClose();
        }}
      >
        {tr(lang, 'ctxRevealInFolder')}
      </button>

      <button
        type="button"
        onclick={() => {
          onInspectMetadata(target.documentId);
          onClose();
        }}
      >
        {tr(lang, 'ctxOpenInspector')}
      </button>

      <button
        type="button"
        onclick={() => {
          onCopyBibtex(target.documentId);
          onClose();
        }}
      >
        {tr(lang, 'ctxCopyBibtex')}
      </button>

      {#if onRunOcr}
        <button
          type="button"
          disabled={!target.isAvailable}
          title={target.isAvailable
            ? lang === 'en'
              ? 'Extract text with Tesseract OCR (original file untouched)'
              : 'Estrai testo con Tesseract OCR (file originale invariato)'
            : unavailableReason}
          onclick={() => {
            onRunOcr(target.fileId);
            onClose();
          }}
        >
          {tr(lang, 'ctxRunOcr')}
        </button>
      {/if}

      <button
        type="button"
        onclick={() => {
          onCopyPath(target.fileId);
          onClose();
        }}
      >
        {tr(lang, 'ctxCopyPath')}
      </button>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 1000;
  }
  .menu {
    position: fixed;
    width: 260px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.22);
    padding: 5px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .menu button {
    text-align: left;
    background: transparent;
    border: none;
    border-radius: 5px;
    padding: 6px 8px;
    font-size: 12px;
  }
  .menu button:hover:not(:disabled) {
    background: var(--accent-soft);
  }
  .menu button.sub-toggle {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-weight: 500;
  }
  .col-submenu {
    display: flex;
    flex-direction: column;
    gap: 1px;
    max-height: 150px;
    overflow-y: auto;
  }
  .menu button.subitem {
    padding-left: 16px;
    color: var(--text-muted);
    font-size: 11px;
  }
  .menu button.col-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
  }
  .menu button.col-item.checked {
    color: var(--accent);
    font-weight: 600;
  }
  .bib-pill {
    font-size: 9.5px;
    padding: 0 4px;
    border-radius: 4px;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .sep {
    height: 1px;
    background: var(--border);
    margin: 3px 0;
  }
</style>
