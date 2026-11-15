<script lang="ts">
  import type { ExternalReaderPreset } from '../types';

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
    target: ContextTarget | null;
    presets: ExternalReaderPreset[];
    onClose: () => void;
    onOpenInternal: (fileId: number, pageOrd?: number) => void;
    onOpenExternal: (fileId: number, pageOrd?: number, templateOverride?: string | null) => void;
    onInspectMetadata: (documentId: number) => void;
    onCopyBibtex: (documentId: number) => void;
    onRunOcr?: (fileId: number) => void;
    onRevealInFolder: (fileId: number) => void;
    onCopyPath: (fileId: number) => void;
  }

  let {
    target,
    presets,
    onClose,
    onOpenInternal,
    onOpenExternal,
    onInspectMetadata,
    onCopyBibtex,
    onRunOcr,
    onRevealInFolder,
    onCopyPath,
  }: Props = $props();

  const unavailableReason = 'Cartella sorgente non raggiungibile';
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
      style="left: {Math.min(target.x, window.innerWidth - 260)}px; top: {Math.min(target.y, window.innerHeight - 280)}px;"
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
        Apri nel visualizzatore interno
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
        Apri con applicazione esterna
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

      <button
        type="button"
        disabled={!target.isAvailable}
        title={target.isAvailable ? '' : unavailableReason}
        onclick={() => {
          onRevealInFolder(target.fileId);
          onClose();
        }}
      >
        Mostra nella cartella
      </button>

      <button
        type="button"
        onclick={() => {
          onInspectMetadata(target.documentId);
          onClose();
        }}
      >
        Apri Ispettore Metadati
      </button>

      <button
        type="button"
        onclick={() => {
          onCopyBibtex(target.documentId);
          onClose();
        }}
      >
        Copia BibTeX (MRLookup)
      </button>

      {#if onRunOcr}
        <button
          type="button"
          disabled={!target.isAvailable}
          title={target.isAvailable ? 'Estrai testo con Tesseract OCR (file originale invariato)' : unavailableReason}
          onclick={() => {
            onRunOcr(target.fileId);
            onClose();
          }}
        >
          Esegui OCR (Tesseract)
        </button>
      {/if}

      <button
        type="button"
        onclick={() => {
          onCopyPath(target.fileId);
          onClose();
        }}
      >
        Copia percorso
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
    width: 250px;
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
  .menu button.subitem {
    padding-left: 16px;
    color: var(--text-muted);
    font-size: 11px;
  }
  .sep {
    height: 1px;
    background: var(--border);
    margin: 3px 0;
  }
</style>
