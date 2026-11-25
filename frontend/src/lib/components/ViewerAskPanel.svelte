<script lang="ts">
  import { api } from '../api';
  import { tr, type Lang } from '../i18n';
  import type {
    DocLlmScope,
    LlmAnswerResult,
    LlmPromptPreview,
    LlmProvider,
  } from '../types';
  import MathMarkdown from './MathMarkdown.svelte';

  interface Props {
    lang?: Lang;
    documentId: number;
    fileId: number;
    currentPage: number;
    llmProvider: LlmProvider;
    initialScope?: DocLlmScope;
    selectionText?: string;
    onJumpToPage: (pageOrd: number) => void;
    onClose: () => void;
    onConsentAccepted?: (provider: LlmProvider) => void;
  }

  let {
    lang = 'it',
    documentId,
    fileId,
    currentPage,
    llmProvider,
    initialScope = 'document',
    selectionText = '',
    onJumpToPage,
    onClose,
    onConsentAccepted,
  }: Props = $props();

  let scope = $state<DocLlmScope>('document');
  let question = $state('');
  let streaming = $state(false);
  let streamText = $state('');
  let answer = $state<LlmAnswerResult | null>(null);
  let errorMsg = $state('');
  let pendingConsent = $state<LlmPromptPreview | null>(null);
  let lastPreview = $state<LlmPromptPreview | null>(null);

  $effect(() => {
    scope = initialScope;
  });

  async function runStream(activeQuestion: string) {
    streaming = true;
    streamText = '';
    answer = null;
    errorMsg = '';
    try {
      const res = await api.askDocumentLlmStream(
        {
          documentId,
          fileId,
          scope,
          pageOrd: currentPage,
          selectionText: scope === 'selection' ? selectionText : null,
          question: activeQuestion,
        },
        (ev) => {
          if (ev.event === 'delta') {
            streamText += ev.text;
          } else if (ev.event === 'done') {
            answer = ev.result;
          } else if (ev.event === 'error') {
            errorMsg = ev.message;
          }
        }
      );
      answer = res;
      if (!streamText) streamText = res.answer_markdown;
    } catch (e) {
      errorMsg = String(e);
    } finally {
      streaming = false;
    }
  }

  async function handleSubmit(e: Event) {
    e.preventDefault();
    const q = question.trim();
    if (!q || streaming) return;
    errorMsg = '';
    pendingConsent = null;
    try {
      const preview = await api.previewDocLlmPrompt({
        documentId,
        fileId,
        scope,
        pageOrd: currentPage,
        selectionText: scope === 'selection' ? selectionText : null,
        question: q,
      });
      lastPreview = preview;
      if (preview.passages_excluded_cloud > 0 && preview.passages_included === 0) {
        errorMsg =
          lang === 'en'
            ? 'Document or folder excluded from cloud LLM transmission. Use local Ollama or adjust exclusion in the Inspector.'
            : 'Documento o cartella esclusi dall’invio LLM cloud. Usa Ollama locale oppure modifica l’esclusione nell’Ispettore.';
        return;
      }
      if (!preview.model) {
        errorMsg =
          lang === 'en'
            ? 'No LLM model selected in Settings.'
            : 'Nessun modello LLM selezionato nelle Impostazioni.';
        return;
      }
      if (preview.requires_cloud_consent) {
        pendingConsent = preview;
        return;
      }
      await runStream(q);
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function confirmConsentAndAsk() {
    if (!pendingConsent) return;
    const prov = pendingConsent.provider;
    await api.acceptLlmCloudConsent(prov);
    onConsentAccepted?.(prov);
    pendingConsent = null;
    const q = question.trim();
    if (q) await runStream(q);
  }
</script>

<aside class="ask-panel" aria-label={tr(lang, 'askDocumentTitle')}>
  <header class="ask-head">
    <span class="ask-title">{tr(lang, 'askDocumentTitle')}</span>
    <span class="ask-prov">{llmProvider}</span>
    <button type="button" class="close-btn" title={tr(lang, 'close')} onclick={onClose}>×</button>
  </header>

  <div class="scope-row" role="group" aria-label={lang === 'en' ? 'Question scope' : 'Ambito domanda'}>
    <button
      type="button"
      class="scope-pill"
      class:active={scope === 'document'}
      onclick={() => (scope = 'document')}
    >
      {tr(lang, 'scopeDocument')}
    </button>
    <button
      type="button"
      class="scope-pill"
      class:active={scope === 'page'}
      onclick={() => (scope = 'page')}
    >
      {tr(lang, 'scopePage')} {currentPage}
    </button>
    <button
      type="button"
      class="scope-pill"
      class:active={scope === 'selection'}
      disabled={!selectionText.trim()}
      title={selectionText.trim()
        ? lang === 'en'
          ? 'Use selected text'
          : 'Usa testo selezionato'
        : lang === 'en'
          ? 'Select text on the page first'
          : 'Seleziona prima un testo nella pagina'}
      onclick={() => (scope = 'selection')}
    >
      {tr(lang, 'scopeSelection')}
    </button>
  </div>

  {#if scope === 'selection' && selectionText.trim()}
    <div class="sel-quote">
      “{selectionText.trim().slice(0, 220)}{selectionText.trim().length > 220 ? '…' : ''}”
    </div>
  {/if}

  <form class="ask-form" onsubmit={handleSubmit}>
    <textarea
      rows="2"
      placeholder={tr(lang, 'askPlaceholder')}
      bind:value={question}
      onkeydown={(e) => {
        if (e.key === 'Enter' && !e.shiftKey) {
          e.preventDefault();
          void handleSubmit(e);
        }
      }}
    ></textarea>
    <div class="ask-form-foot">
      {#if lastPreview}
        <span class="tok-hint">~{lastPreview.estimated_input_tokens} tok</span>
      {:else}
        <span class="tok-hint">
          {lang === 'en' ? 'Enter to ask · [p. X] citations' : 'Invio per chiedere · citazioni [p. X]'}
        </span>
      {/if}
      <button type="submit" class="primary ask-send" disabled={streaming || !question.trim()}>
        {streaming ? tr(lang, 'askSendingBtn') : tr(lang, 'askSendBtn')}
      </button>
    </div>
  </form>

  {#if pendingConsent}
    <div class="consent-box">
      <p>
        {lang === 'en' ? 'Sending to' : 'Invio verso'} <strong>{pendingConsent.provider.toUpperCase()}</strong> ({pendingConsent.passages_included}
        {lang === 'en' ? 'excerpts' : 'estratti'}, ~{pendingConsent.estimated_input_tokens} token). {lang === 'en' ? 'Full files are never uploaded.' : 'Il file intero non viene mai caricato.'}
      </p>
      <div class="consent-btns">
        <button type="button" onclick={() => (pendingConsent = null)}>{tr(lang, 'cancel')}</button>
        <button type="button" class="primary" onclick={confirmConsentAndAsk}>
          {lang === 'en' ? 'Allow & send' : 'Consenti e invia'}
        </button>
      </div>
    </div>
  {/if}

  {#if errorMsg}
    <div class="ask-error">{errorMsg}</div>
  {/if}

  <div class="ask-output">
    {#if streamText || answer}
      <div class="answer-card" class:refused={answer?.refused}>
        <MathMarkdown
          text={answer ? answer.answer_markdown : streamText}
          clickableCitations={true}
          defaultDocId={documentId}
          onCitationClick={(_docId, pageOrd) => onJumpToPage(pageOrd)}
        />
        {#if streaming}
          <span class="stream-cursor" aria-hidden="true">▍</span>
        {/if}
        {#if answer && answer.citations.length > 0}
          <div class="cite-row">
            {#each answer.citations as c (c.marker + '-' + c.page_ord)}
              <button
                type="button"
                class="cite-jump"
                onclick={() => onJumpToPage(c.page_ord)}
              >
                p. {c.page_ord}
              </button>
            {/each}
          </div>
        {/if}
        {#if answer?.usage?.total_tokens}
          <div class="usage-line">
            {answer.model} · {answer.usage.total_tokens} tok
          </div>
        {/if}
      </div>
    {:else if !streaming && !errorMsg}
      <p class="ask-empty">
        {tr(lang, 'askEmptyHint')}
      </p>
    {/if}
  </div>
</aside>

<style>
  .ask-panel {
    width: 316px;
    flex-shrink: 0;
    border-left: 1px solid var(--border);
    background: var(--bg-panel);
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: hidden;
  }
  .ask-head {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 7px 10px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-muted);
  }
  .ask-title {
    font-size: 12px;
    font-weight: 700;
    flex: 1;
  }
  .ask-prov {
    font-size: 10px;
    text-transform: uppercase;
    padding: 1px 6px;
    border-radius: 999px;
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
  }
  .close-btn {
    border: none;
    background: transparent;
    font-size: 16px;
    line-height: 1;
    color: var(--text-muted);
    cursor: pointer;
    padding: 0 4px;
  }
  .scope-row {
    display: flex;
    gap: 4px;
    padding: 7px 10px 4px;
  }
  .scope-pill {
    flex: 1;
    font-size: 11px;
    padding: 3px 6px;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text-muted);
    cursor: pointer;
  }
  .scope-pill.active {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--accent);
    font-weight: 600;
  }
  .scope-pill:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .sel-quote {
    margin: 4px 10px 0;
    padding: 5px 8px;
    font-size: 11px;
    font-style: italic;
    color: var(--text-muted);
    background: var(--bg-muted);
    border-left: 2px solid var(--accent);
    border-radius: 0 4px 4px 0;
    max-height: 68px;
    overflow-y: auto;
  }
  .ask-form {
    padding: 6px 10px 8px;
    border-bottom: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .ask-form textarea {
    width: 100%;
    resize: vertical;
    font-size: 12px;
    padding: 6px 8px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
    box-sizing: border-box;
    font-family: inherit;
  }
  .ask-form-foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
  }
  .tok-hint {
    font-size: 10.5px;
    color: var(--text-muted);
  }
  .ask-send {
    padding: 3px 10px;
    font-size: 11.5px;
    border-radius: 5px;
  }
  .consent-box {
    margin: 8px 10px 0;
    padding: 8px;
    border-radius: 6px;
    background: rgba(245, 158, 11, 0.12);
    border: 1px solid rgba(245, 158, 11, 0.4);
    font-size: 11.5px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .consent-box p {
    margin: 0;
    line-height: 1.4;
  }
  .consent-btns {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
  }
  .ask-error {
    margin: 8px 10px 0;
    padding: 6px 8px;
    border-radius: 6px;
    background: rgba(220, 38, 38, 0.1);
    color: var(--danger);
    font-size: 11.5px;
  }
  .ask-output {
    flex: 1;
    overflow-y: auto;
    padding: 10px;
  }
  .answer-card {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .stream-cursor {
    display: inline-block;
    color: var(--accent);
    animation: blink 0.9s steps(2, start) infinite;
  }
  @keyframes blink {
    to {
      opacity: 0;
    }
  }
  .cite-row {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding-top: 6px;
    border-top: 1px dashed var(--border);
  }
  .cite-jump {
    font-size: 11px;
    padding: 1px 6px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--bg-muted);
    color: var(--accent);
    font-weight: 600;
    cursor: pointer;
  }
  .usage-line {
    font-size: 10.5px;
    color: var(--text-muted);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }
  .ask-empty {
    margin: 4px 0;
    font-size: 12px;
    color: var(--text-muted);
    line-height: 1.45;
  }
</style>
