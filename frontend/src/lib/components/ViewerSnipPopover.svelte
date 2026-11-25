<script lang="ts">
  import { api } from '../api';
  import { tr, type Lang } from '../i18n';
  import type { LlmProvider, NormalizedRect } from '../types';
  import { dataUrlToPngBlob } from '../viewerSnipUtils';
  import MathMarkdown from './MathMarkdown.svelte';

  interface Props {
    lang?: Lang;
    documentId: number;
    pageOrd: number;
    rect: NormalizedRect;
    imageDataUrl: string;
    x: number;
    y: number;
    llmProvider?: LlmProvider;
    onSaveSnipAnnotation: (opts: {
      pageOrd: number;
      rect: NormalizedRect;
      imageDataUrl: string;
      noteMarkdown: string;
      color: 'yellow' | 'green' | 'blue' | 'pink';
    }) => Promise<void>;
    onConsentAccepted?: (provider: LlmProvider) => void;
    onStatusMessage?: (msg: string) => void;
    onClose: () => void;
  }

  let {
    lang = 'it',
    documentId,
    pageOrd,
    rect,
    imageDataUrl,
    x,
    y,
    llmProvider = 'disabled',
    onSaveSnipAnnotation,
    onConsentAccepted,
    onStatusMessage,
    onClose,
  }: Props = $props();

  let noteDraft = $state('');
  let noteColor = $state<'yellow' | 'green' | 'blue' | 'pink'>('blue');
  let saving = $state(false);
  let converting = $state(false);
  let latexResult = $state('');
  let errorMsg = $state('');
  let feedbackMsg = $state('');
  let pendingConsentProvider = $state<LlmProvider | null>(null);

  async function handleCopyPng() {
    errorMsg = '';
    feedbackMsg = '';
    try {
      const blob = dataUrlToPngBlob(imageDataUrl);
      if (!blob || typeof ClipboardItem === 'undefined' || !navigator.clipboard?.write) {
        throw new Error(
          lang === 'en' ? 'Image clipboard API unavailable' : 'Appunti immagine non disponibili'
        );
      }
      await navigator.clipboard.write([new ClipboardItem({ 'image/png': blob })]);
      feedbackMsg = tr(lang, 'snipCopiedPngMsg');
      onStatusMessage?.(tr(lang, 'snipCopiedPngMsg'));
    } catch (e) {
      errorMsg = String(e);
    }
  }

  async function runConvertToLatex() {
    if (converting || llmProvider === 'disabled') return;
    converting = true;
    errorMsg = '';
    feedbackMsg = '';
    pendingConsentProvider = null;
    try {
      const res = await api.snipImageToLatex(documentId, imageDataUrl, null);
      latexResult = res.latex;
      if (!noteDraft.trim() && res.latex.trim()) {
        const isTikz = res.latex.includes('\\begin{tikzcd}');
        noteDraft = isTikz ? `\`\`\`latex\n${res.latex}\n\`\`\`` : `$$\n${res.latex}\n$$`;
      }
    } catch (e) {
      const msg = String(e);
      const consentPrefix = 'CLOUD_CONSENT_REQUIRED:';
      const idx = msg.indexOf(consentPrefix);
      if (idx >= 0) {
        const prov = msg.slice(idx + consentPrefix.length).trim() as LlmProvider;
        pendingConsentProvider = prov;
      } else {
        errorMsg = msg;
      }
    } finally {
      converting = false;
    }
  }

  async function confirmConsentAndConvert() {
    const prov = pendingConsentProvider;
    if (!prov) return;
    await api.acceptLlmCloudConsent(prov);
    onConsentAccepted?.(prov);
    pendingConsentProvider = null;
    await runConvertToLatex();
  }

  async function handleCopyLatex() {
    if (!latexResult.trim()) return;
    try {
      await navigator.clipboard.writeText(latexResult);
      feedbackMsg = lang === 'en' ? 'LaTeX copied!' : 'LaTeX copiato!';
    } catch (e) {
      errorMsg = String(e);
    }
  }

  async function handleSave() {
    if (saving) return;
    saving = true;
    errorMsg = '';
    try {
      await onSaveSnipAnnotation({
        pageOrd,
        rect,
        imageDataUrl,
        noteMarkdown: noteDraft.trim(),
        color: noteColor,
      });
      onClose();
    } catch (e) {
      errorMsg = String(e);
    } finally {
      saving = false;
    }
  }
</script>

<div
  class="snip-popover"
  style="left: {x}px; top: {y}px;"
  role="dialog"
  aria-label={tr(lang, 'snipTitle')}
>
  <div class="snip-head">
    <strong>{tr(lang, 'snipTitle')} · p. {pageOrd}</strong>
    <button type="button" class="snip-close" title={tr(lang, 'close')} onclick={onClose}>×</button>
  </div>

  <div class="snip-img-wrap">
    <img src={imageDataUrl} alt={`Snip p. ${pageOrd}`} class="snip-img" />
  </div>

  <div class="snip-actions-row">
    <button type="button" class="snip-btn" onclick={handleCopyPng}>
      {tr(lang, 'snipCopyPngBtn')}
    </button>
    {#if llmProvider !== 'disabled'}
      <button
        type="button"
        class="snip-btn accent"
        disabled={converting}
        onclick={runConvertToLatex}
      >
        {converting ? tr(lang, 'snipConvertingMsg') : tr(lang, 'snipConvertLatexBtn')}
      </button>
    {/if}
  </div>

  {#if pendingConsentProvider}
    <div class="snip-consent">
      <p>
        {lang === 'en'
          ? `Send ONLY this cropped image snippet to ${pendingConsentProvider.toUpperCase()} for LaTeX transcription? The full PDF/DjVu is never uploaded.`
          : `Inviare SOLO questo ritaglio immagine a ${pendingConsentProvider.toUpperCase()} per la trascrizione LaTeX? Il file intero non viene mai caricato.`}
      </p>
      <div class="snip-consent-btns">
        <button type="button" class="snip-btn" onclick={() => (pendingConsentProvider = null)}>
          {tr(lang, 'cancel')}
        </button>
        <button type="button" class="snip-btn primary" onclick={confirmConsentAndConvert}>
          {lang === 'en' ? 'Allow & convert' : 'Consenti e converti'}
        </button>
      </div>
    </div>
  {/if}

  {#if latexResult}
    <div class="snip-latex-box">
      <div class="snip-latex-head">
        <span>LaTeX</span>
        <button type="button" class="snip-copy-tex" onclick={handleCopyLatex}>
          {tr(lang, 'snipCopyLatexBtn')}
        </button>
      </div>
      <pre class="snip-tex-code">{latexResult}</pre>
      {#if !latexResult.includes('\\begin{tikzcd}')}
        <div class="snip-katex-prev">
          <MathMarkdown text={`$$\n${latexResult}\n$$`} compact={true} />
        </div>
      {/if}
    </div>
  {/if}

  <textarea
    class="snip-note-input"
    rows="2"
    placeholder={tr(lang, 'snipNotePlaceholder')}
    bind:value={noteDraft}
  ></textarea>

  {#if errorMsg}
    <div class="snip-err">{errorMsg}</div>
  {/if}
  {#if feedbackMsg}
    <div class="snip-ok">{feedbackMsg}</div>
  {/if}

  <div class="snip-foot">
    <div class="color-dots">
      {#each ['yellow', 'green', 'blue', 'pink'] as c}
        <button
          type="button"
          class="cdot dot-{c}"
          class:selected={noteColor === c}
          aria-label={c}
          onclick={() => (noteColor = c as 'yellow' | 'green' | 'blue' | 'pink')}
        ></button>
      {/each}
    </div>
    <div class="snip-foot-btns">
      <button type="button" class="snip-btn" onclick={onClose}>{tr(lang, 'cancel')}</button>
      <button type="button" class="snip-btn primary" disabled={saving} onclick={handleSave}>
        {tr(lang, 'snipSaveNoteBtn')}
      </button>
    </div>
  </div>
</div>

<style>
  .snip-popover {
    position: absolute;
    z-index: 38;
    width: 330px;
    max-width: calc(100% - 24px);
    padding: 9px 10px;
    border-radius: 9px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    box-shadow: 0 14px 36px rgba(0, 0, 0, 0.32);
    display: flex;
    flex-direction: column;
    gap: 7px;
    font-size: 12px;
  }
  .snip-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
  }
  .snip-close {
    border: none;
    background: transparent;
    color: var(--text-muted);
    font-size: 15px;
    padding: 0 4px;
    cursor: pointer;
  }
  .snip-img-wrap {
    max-height: 150px;
    overflow: auto;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: #ffffff;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
  }
  .snip-img {
    max-width: 100%;
    max-height: 140px;
    object-fit: contain;
    display: block;
  }
  .snip-actions-row {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .snip-btn {
    font-size: 11.5px;
    padding: 3px 8px;
    border-radius: 5px;
    font-weight: 600;
    cursor: pointer;
  }
  .snip-btn.accent {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--accent);
  }
  .snip-consent {
    padding: 7px 8px;
    border-radius: 6px;
    background: rgba(245, 158, 11, 0.14);
    border: 1px solid rgba(245, 158, 11, 0.42);
    font-size: 11px;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .snip-consent p {
    margin: 0;
    line-height: 1.4;
  }
  .snip-consent-btns {
    display: flex;
    justify-content: flex-end;
    gap: 5px;
  }
  .snip-latex-box {
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    padding: 5px 7px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .snip-latex-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: 10.5px;
    font-weight: 700;
    color: var(--text-muted);
  }
  .snip-copy-tex {
    font-size: 10.5px;
    padding: 1px 6px;
    border-radius: 4px;
  }
  .snip-tex-code {
    margin: 0;
    max-height: 76px;
    overflow: auto;
    font-size: 11px;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .snip-katex-prev {
    border-top: 1px dashed var(--border);
    padding-top: 4px;
    max-height: 84px;
    overflow: auto;
  }
  .snip-note-input {
    width: 100%;
    font-size: 11.5px;
    padding: 5px 6px;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
    box-sizing: border-box;
    font-family: inherit;
  }
  .snip-err {
    font-size: 11px;
    color: var(--danger);
  }
  .snip-ok {
    font-size: 11px;
    color: var(--accent);
    font-weight: 600;
  }
  .snip-foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
  }
  .snip-foot-btns {
    display: flex;
    gap: 5px;
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
</style>
