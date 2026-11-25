<script lang="ts">
  import { api } from '../api';
  import { tr, type Lang } from '../i18n';
  import { hasMathDelimiters } from '../katexLoader';
  import type {
    CollectionView,
    DocumentMetadataView,
    DocumentRelationsView,
    ManualMetadataEdit,
    ReadingStatus,
    ResolveOutcome,
    SmartPasteApplyResult,
    TopicClusterView,
  } from '../types';
  import {
    buildManualMetadataEditPayload,
    clearUnlockedFields,
    computeNextLocksOnTouch,
    formatAuthorsForInput,
    hasInspectorFormDiff,
    hasNonEmptyUnlockedFields,
    normalizeInputText,
    type InspectorFormFields,
  } from '../inspectorUtils';
  import MathMarkdown from './MathMarkdown.svelte';

  interface Props {
    lang?: Lang;
    meta: DocumentMetadataView | null;
    selectedFileId?: number | null;
    clusters?: TopicClusterView[];
    collections?: CollectionView[];
    loading: boolean;
    fetchingOnline: boolean;
    lastFetchMessage: string;
    onSave: (edit: ManualMetadataEdit, lockedFields?: string[]) => Promise<void>;
    onFlushSave?: (edit: ManualMetadataEdit, lockedFields?: string[]) => Promise<void>;
    onSaveAndFetch?: (
      edit: ManualMetadataEdit,
      lockedFields: string[],
      overwriteQueryFields?: boolean
    ) => Promise<ResolveOutcome | void>;
    onFetchMetadata: (documentId: number) => Promise<ResolveOutcome | void>;
    onCancelFetch?: () => void;
    onToggleVerified?: (documentId: number, verified: boolean, advanceNext?: boolean) => Promise<void>;
    hasNextUnverified?: boolean;
    onConfirmCandidate: (
      documentId: number,
      candidateId: number,
      overwriteQueryFields?: boolean
    ) => Promise<void>;
    onDismissCandidate: (candidateId: number) => Promise<void>;
    onCopyBibtex: (documentId: number) => Promise<string>;
    onCopyCiteCommand?: (documentId: number) => Promise<string>;
    onApplySmartPaste?: (documentId: number, rawInput: string) => Promise<SmartPasteApplyResult | void>;
    onSetReadingStatus?: (documentId: number, status: ReadingStatus) => Promise<void>;
    onSetStarred?: (documentId: number, starred: boolean) => Promise<void>;
    onAddToCollection?: (collectionId: number, documentId: number) => Promise<void>;
    onRemoveFromCollection?: (collectionId: number, documentId: number) => Promise<void>;
    onSelectRelatedDoc?: (documentId: number) => void;
    onImportAltFormula?: (fileId: number, markdownText: string) => Promise<void>;
    onRunOcr?: (fileId: number) => Promise<void>;
    onConfirmMsc?: (documentId: number, mscCode: string | null) => Promise<void>;
    onRejectMsc?: (documentId: number) => Promise<void>;
    onSetCluster?: (documentId: number, clusterId: number) => Promise<void>;
    onRemoveCluster?: (documentId: number, clusterId: number) => Promise<void>;
    onAddTag?: (documentId: number, tagName: string) => Promise<void>;
    onRemoveTag?: (documentId: number, tagName: string) => Promise<void>;
    onOpenInViewer?: () => void;
    onClose: () => void;
  }

  let {
    lang = 'it',
    meta,
    selectedFileId = null,
    clusters = [],
    collections = [],
    loading,
    fetchingOnline,
    lastFetchMessage,
    onSave,
    onFlushSave,
    onSaveAndFetch,
    onFetchMetadata,
    onCancelFetch,
    onToggleVerified,
    hasNextUnverified = false,
    onConfirmCandidate,
    onDismissCandidate,
    onCopyBibtex,
    onCopyCiteCommand,
    onApplySmartPaste,
    onSetReadingStatus,
    onSetStarred,
    onAddToCollection,
    onRemoveFromCollection,
    onSelectRelatedDoc,
    onImportAltFormula,
    onRunOcr,
    onConfirmMsc,
    onRejectMsc,
    onSetCluster,
    onRemoveCluster,
    onAddTag,
    onRemoveTag,
    onOpenInViewer,
    onClose,
  }: Props = $props();

  let newTagInput = $state('');
  let smartPasteInput = $state('');
  let smartPasteBusy = $state(false);
  let relations = $state.raw<DocumentRelationsView | null>(null);
  let relationsReqSeq = 0;

  let workType = $state('article');
  let title = $state('');
  let authorsText = $state('');
  let yearStr = $state('');
  let journalAbbrev = $state('');
  let journalFull = $state('');
  let booktitle = $state('');
  let series = $state('');
  let volume = $state('');
  let issueNumber = $state('');
  let pages = $state('');
  let edition = $state('');
  let publisher = $state('');
  let address = $state('');
  let doi = $state('');
  let arxivId = $state('');
  let mrNumber = $state('');
  let mrReviewer = $state('');
  let zblNumber = $state('');
  let isbn = $state('');
  let issn = $state('');
  let url = $state('');
  let mscPrimary = $state('');
  let mscSecondaryText = $state('');
  let abstractText = $state('');
  let citationKeyOverride = $state('');
  let tagsText = $state('');
  let excludeFromCloud = $state(false);
  let lockedFields = $state<string[]>([]);
  let locksModified = $state(false);
  let touched = $state<string[]>([]);
  let overwriteQueryFields = $state(true);
  let lastDocId: number | null = null;
  let bibPreview = $state('');
  let copyFeedback = $state('');
  let altFormulaFeedback = $state('');
  let saveFeedback = $state('');
  let saveError = $state('');
  let altFileInput: HTMLInputElement | undefined = $state();
  let saving = $state(false);
  const currentCluster = $derived(
    meta ? (clusters.find((c) => c.document_ids.includes(meta.document_id)) ?? null) : null
  );

  const ANCHOR_LABELS: Record<string, string> = {
    title: 'Titolo',
    authors: 'Autori',
    year: 'Anno',
    doi: 'DOI',
    arxiv_id: 'arXiv',
    mr_number: 'MR',
    zbl_number: 'Zbl',
    isbn: 'ISBN',
  };

  const PRIMARY_ANCHOR_KEYS = [
    'title',
    'authors',
    'doi',
    'arxiv_id',
    'mr_number',
    'zbl_number',
    'isbn',
  ];

  const activeLockedAnchors = $derived(
    Object.keys(ANCHOR_LABELS).filter((k) => lockedFields.includes(k))
  );

  async function handleAltFormulaSelected(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = '';
    if (!file || selectedFileId === null || !onImportAltFormula) return;
    const text = await file.text();
    await onImportAltFormula(selectedFileId, text);
    altFormulaFeedback = `Importato testo formule (${file.name}) per ricerca ibrida.`;
  }

  $effect(() => {
    if (!meta) return;
    workType = meta.work_type || 'article';
    title = meta.title || '';
    authorsText = formatAuthorsForInput(meta.authors);
    yearStr = meta.year !== null && meta.year !== undefined ? String(meta.year) : '';
    journalAbbrev = meta.journal_abbrev ?? '';
    journalFull = meta.journal_full ?? '';
    booktitle = meta.booktitle ?? '';
    series = meta.series ?? '';
    volume = meta.volume ?? '';
    issueNumber = meta.issue_number ?? '';
    pages = meta.pages ?? '';
    edition = meta.edition ?? '';
    publisher = meta.publisher ?? '';
    address = meta.address ?? '';
    doi = meta.doi ?? '';
    arxivId = meta.arxiv_id ?? '';
    mrNumber = meta.mr_number ?? '';
    mrReviewer = meta.mr_reviewer ?? '';
    zblNumber = meta.zbl_number ?? '';
    isbn = meta.isbn ?? '';
    issn = meta.issn ?? '';
    url = meta.url ?? '';
    mscPrimary = meta.msc_primary ?? '';
    mscSecondaryText = meta.msc_secondary.join(', ');
    abstractText = meta.abstract_text ?? '';
    citationKeyOverride = meta.citation_key_override ?? '';
    tagsText = meta.tags.join(', ');
    excludeFromCloud = meta.exclude_from_cloud_llm;
    const nextLocked = meta.manual_locked_fields.filter(
      (f) => f !== 'tags' && f !== 'exclude_from_cloud_llm'
    );
    lockedFields = nextLocked;
    if (meta.document_id !== lastDocId) {
      lastDocId = meta.document_id;
      overwriteQueryFields = !nextLocked.includes('title') && !nextLocked.includes('authors');
      saveFeedback = '';
      saveError = '';
      smartPasteInput = '';
      relations = null;
      const req = ++relationsReqSeq;
      const targetDocId = meta.document_id;
      void api
        .getDocumentRelations(targetDocId)
        .then((rel) => {
          if (req === relationsReqSeq) relations = rel;
        })
        .catch(() => {});
    } else if (nextLocked.includes('title') || nextLocked.includes('authors')) {
      overwriteQueryFields = false;
    }
    locksModified = false;
    touched = [];
    bibPreview = meta.raw_mrlookup_bibtex ?? '';
    copyFeedback = '';
  });

  function touch(field: string, currentValue?: string | number | null) {
    saveFeedback = '';
    saveError = '';
    if (!touched.includes(field)) {
      touched = [...touched, field];
    }
    const next = computeNextLocksOnTouch(
      lockedFields,
      field,
      overwriteQueryFields,
      currentValue
    );
    lockedFields = next.lockedFields;
    if (next.modified) {
      locksModified = true;
    }
  }

  function isLocked(field: string): boolean {
    return lockedFields.includes(field);
  }

  function toggleLock(field: string) {
    saveFeedback = '';
    saveError = '';
    if (lockedFields.includes(field)) {
      lockedFields = lockedFields.filter((f) => f !== field);
    } else {
      lockedFields = [...lockedFields, field];
      if (field === 'title' || field === 'authors') {
        overwriteQueryFields = false;
      }
    }
    locksModified = true;
  }

  function handleOverwriteToggle() {
    if (overwriteQueryFields) {
      const next = lockedFields.filter((f) => f !== 'title' && f !== 'authors');
      if (next.length !== lockedFields.length) {
        lockedFields = next;
        locksModified = true;
      }
    } else {
      const next = [...lockedFields];
      let changed = false;
      if (title.trim() && !next.includes('title')) {
        next.push('title');
        changed = true;
      }
      if (authorsText.trim() && !next.includes('authors')) {
        next.push('authors');
        changed = true;
      }
      if (changed) {
        lockedFields = next;
        locksModified = true;
      }
    }
  }

  function unlockAllFields() {
    saveFeedback = '';
    saveError = '';
    lockedFields = [];
    locksModified = true;
  }

  function getFormFields(): InspectorFormFields {
    return {
      workType,
      title,
      authorsText,
      yearStr,
      journalAbbrev,
      journalFull,
      booktitle,
      series,
      volume,
      issueNumber,
      pages,
      edition,
      publisher,
      address,
      doi,
      arxivId,
      mrNumber,
      mrReviewer,
      zblNumber,
      isbn,
      issn,
      url,
      mscPrimary,
      mscSecondaryText,
      abstractText,
      citationKeyOverride,
      tagsText,
      excludeFromCloud,
      touched,
    };
  }

  const hasClearableUnlocked = $derived(
    meta !== null && hasNonEmptyUnlockedFields(getFormFields(), lockedFields)
  );

  function handleClearUnlockedFields() {
    if (!meta) return;
    const { next, clearedFields } = clearUnlockedFields(getFormFields(), lockedFields);
    if (clearedFields.length === 0) return;
    authorsText = next.authorsText;
    yearStr = String(next.yearStr ?? '');
    journalAbbrev = next.journalAbbrev;
    journalFull = next.journalFull;
    booktitle = next.booktitle;
    series = next.series;
    volume = next.volume;
    issueNumber = next.issueNumber;
    pages = next.pages;
    edition = next.edition;
    publisher = next.publisher;
    address = next.address;
    doi = next.doi;
    arxivId = next.arxivId;
    mrNumber = next.mrNumber;
    mrReviewer = next.mrReviewer;
    zblNumber = next.zblNumber;
    isbn = next.isbn;
    issn = next.issn;
    url = next.url;
    mscPrimary = next.mscPrimary;
    mscSecondaryText = next.mscSecondaryText;
    abstractText = next.abstractText;
    touched = next.touched;
    saveError = '';
    saveFeedback = `Svuotati ${clearedFields.length} campi non bloccati: premi Salva (⌘S) o ✓ per confermare.`;
  }

  function buildEditPayload(allowEmptyTitle = false): ManualMetadataEdit | null {
    if (!meta) return null;
    const res = buildManualMetadataEditPayload(meta, getFormFields(), allowEmptyTitle);
    if (!res.ok) {
      saveError = res.error;
      return null;
    }
    return res.payload;
  }

  function hasUnsavedFormDiff(): boolean {
    if (!meta) return false;
    return hasInspectorFormDiff(meta, getFormFields(), locksModified);
  }

  export function hasUnsavedChanges(): boolean {
    return hasUnsavedFormDiff();
  }

  function computeEffectiveSaveLocks(): string[] {
    const saveLocks = [...lockedFields];
    const titleChanged =
      touched.includes('title') || (meta !== null && title.trim() !== (meta.title ?? '').trim());
    const authorsChanged =
      touched.includes('authors') ||
      (meta !== null && authorsText.trim() !== formatAuthorsForInput(meta.authors).trim());
    if (titleChanged && title.trim() && !saveLocks.includes('title')) {
      saveLocks.push('title');
    }
    if (authorsChanged && authorsText.trim() && !saveLocks.includes('authors')) {
      saveLocks.push('authors');
    }
    return saveLocks;
  }

  export async function savePendingIfForDoc(docId: number): Promise<boolean> {
    if (!meta || meta.document_id !== docId) return true;
    if (!hasUnsavedFormDiff()) return true;
    saveError = '';
    const payload = buildEditPayload(false);
    if (!payload) return false;
    const saveLocks = computeEffectiveSaveLocks();
    await (onFlushSave ?? onSave)(payload, saveLocks);
    lockedFields = saveLocks;
    if (saveLocks.includes('title') || saveLocks.includes('authors')) {
      overwriteQueryFields = false;
    }
    touched = [];
    locksModified = false;
    return true;
  }

  export async function triggerSave(): Promise<boolean> {
    if (!meta || saving) return false;
    saveError = '';
    saveFeedback = '';
    const payload = buildEditPayload(false);
    if (!payload) return false;
    if (fetchingOnline && onCancelFetch) {
      onCancelFetch();
    }
    const saveLocks = computeEffectiveSaveLocks();
    saving = true;
    try {
      await onSave(payload, saveLocks);
      lockedFields = saveLocks;
      if (saveLocks.includes('title') || saveLocks.includes('authors')) {
        overwriteQueryFields = false;
      }
      touched = [];
      locksModified = false;
      saveFeedback = 'Metadati salvati.';
      return true;
    } catch (err) {
      saveError = `Errore salvataggio metadati: ${String(err)}`;
      return false;
    } finally {
      saving = false;
    }
  }

  async function handleSave(e: Event) {
    e.preventDefault();
    await triggerSave();
  }

  async function handleGuidedFetchClick() {
    if (!meta) return;
    saveError = '';
    saveFeedback = '';
    const cleanedYear = normalizeInputText(yearStr);
    let nextLocks = [...lockedFields];
    if (overwriteQueryFields) {
      nextLocks = nextLocks.filter((f) => f !== 'title' && f !== 'authors');
      if (cleanedYear && touched.includes('year') && !nextLocks.includes('year')) {
        nextLocks.push('year');
      }
    } else {
      const hasPrimaryAnchor = nextLocks.some((f) => PRIMARY_ANCHOR_KEYS.includes(f));
      if (!hasPrimaryAnchor) {
        if (title.trim()) nextLocks.push('title');
        if (authorsText.trim()) nextLocks.push('authors');
        if (cleanedYear && touched.includes('year')) nextLocks.push('year');
      }
    }
    const payload = buildEditPayload(true);
    if (!payload) return;
    lockedFields = nextLocks;
    saving = true;
    try {
      if (onSaveAndFetch) {
        await onSaveAndFetch(payload, nextLocks, overwriteQueryFields);
      } else {
        await onSave(payload, nextLocks);
        await onFetchMetadata(meta.document_id);
      }
      touched = [];
      locksModified = false;
    } catch (err) {
      saveError = `Errore salvataggio/ricerca metadati: ${String(err)}`;
    } finally {
      saving = false;
    }
  }

  async function handleTopFetchClick() {
    if (!meta) return;
    saveError = '';
    saveFeedback = '';
    const docId = meta.document_id;
    if (hasUnsavedFormDiff()) {
      const payload = buildEditPayload(false);
      if (!payload) return;
      const saveLocks = computeEffectiveSaveLocks();
      try {
        await onSave(payload, saveLocks);
      } catch (err) {
        saveError = `Errore salvataggio metadati: ${String(err)}`;
        return;
      }
      lockedFields = saveLocks;
      if (saveLocks.includes('title') || saveLocks.includes('authors')) {
        overwriteQueryFields = false;
      }
      touched = [];
      locksModified = false;
    }
    await onFetchMetadata(docId);
  }

  async function handleToggleVerifiedClick(advanceNext = false) {
    if (!meta || !onToggleVerified) return;
    saveError = '';
    saveFeedback = '';
    const docId = meta.document_id;
    const nextState = advanceNext ? true : !meta.metadata_verified;
    saving = true;
    try {
      await onToggleVerified(docId, nextState, advanceNext);
    } catch (err) {
      saveError = `Errore conferma metadati: ${String(err)}`;
    } finally {
      saving = false;
    }
  }

  async function handleCopyBibtexClick() {
    if (!meta) return;
    const bib = await onCopyBibtex(meta.document_id);
    bibPreview = bib;
    try {
      await navigator.clipboard.writeText(bib);
      copyFeedback = 'BibTeX copiato!';
    } catch {
      copyFeedback = 'BibTeX generato (seleziona e copia qui sotto)';
    }
  }

  async function handleCopyCiteClick() {
    if (!meta) return;
    try {
      const cmd = onCopyCiteCommand
        ? await onCopyCiteCommand(meta.document_id)
        : (await api.getDocumentCiteKey(meta.document_id)).cite_command;
      await navigator.clipboard.writeText(cmd);
      copyFeedback = `Copiato ${cmd}`;
    } catch (e) {
      saveError = `Errore copia \\cite: ${String(e)}`;
    }
  }

  async function handleSmartPasteSubmit(e?: Event) {
    e?.preventDefault();
    if (!meta || !onApplySmartPaste || !smartPasteInput.trim() || smartPasteBusy) return;
    smartPasteBusy = true;
    saveError = '';
    saveFeedback = '';
    try {
      const res = await onApplySmartPaste(meta.document_id, smartPasteInput.trim());
      smartPasteInput = '';
      if (res?.summary) {
        saveFeedback = res.summary;
      }
    } catch (err) {
      saveError = String(err);
    } finally {
      smartPasteBusy = false;
    }
  }

  async function handleApplyPublishedNotice() {
    if (!meta || !onApplySmartPaste || !relations?.published_notice) return;
    const pub = relations.published_notice;
    const raw = pub.doi ? `https://doi.org/${pub.doi}` : pub.mr_number ? `MR${pub.mr_number}` : '';
    if (!raw) return;
    smartPasteInput = raw;
    await handleSmartPasteSubmit();
  }

  const docCollections = $derived(
    meta ? collections.filter((c) => (meta.collection_ids ?? []).includes(c.id)) : []
  );
  const availableCollections = $derived(
    meta ? collections.filter((c) => !(meta.collection_ids ?? []).includes(c.id)) : []
  );
  const totalRelationsCount = $derived(
    (relations?.similar.length ?? 0) +
      (relations?.cites.length ?? 0) +
      (relations?.cited_by.length ?? 0)
  );
</script>

<aside class="inspector">
  <div class="inspector-header">
    <strong>{tr(lang, 'inspectorTitle')}</strong>
    <button type="button" class="close-btn" onclick={onClose} title={tr(lang, 'close')}>✕</button>
  </div>

  {#if loading}
    <div class="status-msg">{lang === 'en' ? 'Loading metadata…' : 'Caricamento metadati…'}</div>
  {:else if !meta}
    <div class="status-msg">
      {lang === 'en'
        ? 'Select a document from the catalog to inspect or edit its metadata.'
        : 'Seleziona un documento dalla lista per visualizzare o modificare i metadati.'}
    </div>
  {:else}
    <div class="action-bar">
      {#if selectedFileId !== null && onOpenInViewer}
        <button type="button" onclick={onOpenInViewer}>
          {tr(lang, 'openInViewerBtn')}
        </button>
      {/if}
      <button
        type="button"
        class="primary"
        disabled={fetchingOnline || saving || meta.metadata_verified}
        onclick={handleTopFetchClick}
        title={meta.metadata_verified
          ? lang === 'en'
            ? 'Verified metadata: uncheck ✓ before running online lookup'
            : 'Metadati confermati: rimuovi la spunta ✓ prima di avviare la ricerca online'
          : lang === 'en'
            ? 'Lookup/update metadata respecting locked fields'
            : 'Cerca/aggiorna metadati rispettando i campi bloccati'}
      >
        {fetchingOnline ? tr(lang, 'searchingBtn') : tr(lang, 'refreshMetadataBtn')}
      </button>
      {#if fetchingOnline && onCancelFetch}
        <button
          type="button"
          class="mini-no"
          onclick={onCancelFetch}
          title={lang === 'en' ? 'Cancel active online lookup' : 'Interrompi subito la ricerca online in corso'}
        >
          {tr(lang, 'stopBtn')}
        </button>
      {/if}
      <button type="button" onclick={handleCopyBibtexClick}>
        {tr(lang, 'copyBibtexBtn')}
      </button>
      <button
        type="button"
        title={lang === 'en' ? 'Copy disambiguated \\cite{key} to clipboard' : 'Copia subito \\cite{chiave} negli appunti'}
        onclick={handleCopyCiteClick}
      >
        {tr(lang, 'copyCiteBtn')}
      </button>
      {#if selectedFileId !== null && onImportAltFormula}
        <button
          type="button"
          title="Importa testo LaTeX/Markdown generato da Nougat o Marker senza modificare la cartella sorgente"
          onclick={() => altFileInput?.click()}
        >
          Importa LaTeX (.mmd/.md)
        </button>
        <input
          bind:this={altFileInput}
          type="file"
          accept=".mmd,.md,.tex"
          style="display:none"
          onchange={handleAltFormulaSelected}
        />
      {/if}
      {#if selectedFileId !== null && onRunOcr}
        <button
          type="button"
          title="Esegui OCR con Tesseract su file scansionati (originale mai modificato)"
          onclick={() => onRunOcr(selectedFileId)}
        >
          Esegui OCR
        </button>
      {/if}
    </div>

    {#if saveError}
      <div class="error-notice" role="alert">{saveError}</div>
    {/if}
    {#if saveFeedback}
      <div class="notice">{saveFeedback}</div>
    {/if}
    {#if lastFetchMessage}
      <div class="notice">{lastFetchMessage}</div>
    {/if}
    {#if copyFeedback}
      <div class="notice">{copyFeedback}</div>
    {/if}
    {#if altFormulaFeedback}
      <div class="notice">{altFormulaFeedback}</div>
    {/if}

    <!-- Smart Paste: DOI / arXiv / MR / ISBN / URL / BibTeX (NO thumbnails!) -->
    {#if onApplySmartPaste}
      <form class="smart-paste-bar" onsubmit={handleSmartPasteSubmit}>
        <input
          type="text"
          class="smart-paste-input"
          placeholder="Incolla DOI / arXiv / MR / ISBN / BibTeX…"
          disabled={smartPasteBusy || fetchingOnline}
          bind:value={smartPasteInput}
        />
        <button
          type="submit"
          class="smart-paste-btn"
          disabled={smartPasteBusy || fetchingOnline || !smartPasteInput.trim()}
        >
          {smartPasteBusy ? '…' : 'Applica'}
        </button>
      </form>
    {/if}

    <!-- Reading status & Star -->
    {#if onSetReadingStatus || onSetStarred}
      <div class="reading-row">
        {#if onSetReadingStatus}
          <select
            class="reading-select"
            aria-label="Stato lettura"
            value={meta.reading_status ?? 'unread'}
            onchange={(e) =>
              onSetReadingStatus(
                meta.document_id,
                (e.currentTarget as HTMLSelectElement).value as ReadingStatus
              )}
          >
            <option value="unread">Da leggere</option>
            <option value="reading">In lettura</option>
            <option value="read">Letto</option>
          </select>
        {/if}
        {#if onSetStarred}
          <button
            type="button"
            class="star-toggle-btn"
            class:starred={Boolean(meta.is_starred)}
            onclick={() => onSetStarred(meta.document_id, !meta.is_starred)}
          >
            {meta.is_starred ? '★ Preferito' : '☆ Preferito'}
          </button>
        {/if}
      </div>
    {/if}

    <!-- Preprint published version badge -->
    {#if relations?.published_notice}
      <div class="pub-notice-banner">
        <div class="pub-notice-text">
          <strong>Versione pubblicata:</strong>
          {#if relations.published_notice.journal}{relations.published_notice.journal}{/if}
          {#if relations.published_notice.year} ({relations.published_notice.year}){/if}
          {#if relations.published_notice.doi} · DOI {relations.published_notice.doi}{/if}
          {#if relations.published_notice.mr_number} · MR{relations.published_notice.mr_number}{/if}
        </div>
        {#if onApplySmartPaste && (relations.published_notice.doi || relations.published_notice.mr_number)}
          <button
            type="button"
            class="mini-ok"
            disabled={smartPasteBusy}
            onclick={handleApplyPublishedNotice}
          >
            Aggiorna
          </button>
        {/if}
      </div>
    {/if}

    <!-- Collections assignment -->
    {#if collections.length > 0 && onAddToCollection && onRemoveFromCollection}
      <div class="tax-box">
        <span class="tax-label">Collezioni</span>
        {#if docCollections.length > 0}
          <div class="tag-chips">
            {#each docCollections as c (c.id)}
              <span class="tag-chip">
                {c.name}
                <button
                  type="button"
                  aria-label="Rimuovi da collezione {c.name}"
                  onclick={() => onRemoveFromCollection(c.id, meta.document_id)}
                >
                  ×
                </button>
              </span>
            {/each}
          </div>
        {/if}
        {#if availableCollections.length > 0}
          <select
            value=""
            onchange={async (e) => {
              const cid = Number((e.currentTarget as HTMLSelectElement).value);
              (e.currentTarget as HTMLSelectElement).value = '';
              if (cid > 0) await onAddToCollection(cid, meta.document_id);
            }}
          >
            <option value="">+ Aggiungi a collezione…</option>
            {#each availableCollections as c (c.id)}
              <option value={String(c.id)}>{c.name}</option>
            {/each}
          </select>
        {/if}
      </div>
    {/if}

    {#if meta.msc_primary && meta.msc_primary_status === 'proposed' && onConfirmMsc && onRejectMsc}
      <div class="proposal-banner">
        <span>Proposta MSC (k-NN): <strong>{meta.msc_primary}</strong></span>
        <div class="prop-actions">
          <button
            type="button"
            class="mini-ok"
            onclick={() => onConfirmMsc(meta.document_id, meta.msc_primary)}
          >
            Conferma
          </button>
          <button
            type="button"
            class="mini-no"
            onclick={() => onRejectMsc(meta.document_id)}
          >
            Rifiuta
          </button>
        </div>
      </div>
    {/if}

    {#if clusters.length > 0 && onSetCluster && onRemoveCluster}
      <div class="tax-box">
        <span class="tax-label">Gruppo Tematico</span>
        <select
          value={currentCluster ? String(currentCluster.id) : ''}
          onchange={async (e) => {
            const val = (e.currentTarget as HTMLSelectElement).value;
            if (!val) {
              if (currentCluster) {
                await onRemoveCluster(meta.document_id, currentCluster.id);
              }
            } else {
              await onSetCluster(meta.document_id, Number(val));
            }
          }}
        >
          <option value="">— Nessun gruppo —</option>
          {#each clusters as cl (cl.id)}
            <option value={String(cl.id)}>{cl.label} ({cl.document_count})</option>
          {/each}
        </select>
      </div>
    {/if}

    {#if onAddTag && onRemoveTag}
      <div class="tax-box">
        <span class="tax-label">Tag rapidi</span>
        <div class="tag-chips">
          {#each meta.tags as t (t)}
            <span class="tag-chip">
              #{t}
              <button
                type="button"
                aria-label="Rimuovi tag {t}"
                onclick={() => onRemoveTag(meta.document_id, t)}
              >
                ×
              </button>
            </span>
          {/each}
        </div>
        <div class="tag-add-row">
          <input
            type="text"
            placeholder="Nuovo tag…"
            bind:value={newTagInput}
            onkeydown={async (e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                const name = newTagInput.trim();
                if (name) {
                  await onAddTag(meta.document_id, name);
                  newTagInput = '';
                }
              }
            }}
          />
          <button
            type="button"
            onclick={async () => {
              const name = newTagInput.trim();
              if (name) {
                await onAddTag(meta.document_id, name);
                newTagInput = '';
              }
            }}
          >
            + Tag
          </button>
        </div>
      </div>
    {/if}

    {#if onToggleVerified}
      <div class="verify-banner" class:verified={meta.metadata_verified}>
        <button
          type="button"
          class="verify-main-btn"
          class:verified={meta.metadata_verified}
          disabled={saving}
          onclick={() => handleToggleVerifiedClick(false)}
          title={meta.metadata_verified
            ? 'Clicca per rimuovere il check di controllo metadati'
            : 'Salva eventuali modifiche e segna subito i metadati come controllati e corretti'}
        >
          <span class="verify-box">{meta.metadata_verified ? '✓' : ''}</span>
          <span>
            {meta.metadata_verified
              ? 'Metadati controllati e corretti'
              : 'Segna metadati come controllati'}
          </span>
        </button>
        {#if !meta.metadata_verified && hasNextUnverified}
          <button
            type="button"
            class="verify-next-btn"
            disabled={saving}
            onclick={() => handleToggleVerifiedClick(true)}
            title="Segna questo documento come controllato e passa subito al prossimo non checkato"
          >
            ✓ + Prossimo →
          </button>
        {/if}
      </div>
      {#if meta.metadata_verified}
        <div class="verify-lock-note">
          🔒 Metadati confermati: la ricerca automatica non li modificherà (rimuovi la spunta ✓ per cercare di nuovo).
        </div>
      {/if}
    {/if}

    <div class="source-row">
      <span>Fonte: <strong>{meta.metadata_source ?? 'locale/euristica'}</strong></span>
      <div class="lock-summary">
        {#if lockedFields.length > 0}
          <span class="lock-pill" title={lockedFields.join(', ')}>
            🔒 {lockedFields.length} {lockedFields.length === 1 ? 'bloccato' : 'bloccati'}
          </span>
          <button
            type="button"
            class="unlock-all-btn"
            onclick={unlockAllFields}
            title="Sblocca tutti i campi per consentirne la sovrascrittura"
          >
            Sblocca tutti
          </button>
        {/if}
        {#if hasClearableUnlocked}
          <button
            type="button"
            class="unlock-all-btn clear-unlocked-btn"
            onclick={handleClearUnlockedFields}
            title="Svuota tutti i campi non bloccati lasciando intatti i campi bloccati 🔒"
          >
            Pulisci non bloccati
          </button>
        {/if}
      </div>
    </div>

    {#if meta.metadata_license_note || meta.metadata_source === 'zbmath'}
      <div class="license-banner">
        {meta.metadata_license_note ?? 'Metadata via zbMATH Open (CC-BY-SA 4.0)'}
      </div>
    {/if}

    {#if meta.pending_candidates.length > 0}
      <section class="candidates-box">
        <div class="cand-box-header">
          <strong>Candidati ambigui ({meta.pending_candidates.length})</strong>
          <button
            type="button"
            class="mini-no"
            disabled={saving}
            onclick={() => handleToggleVerifiedClick(false)}
            title="Ignora questi candidati errati, mantieni il tuo Titolo/Autore e segna i metadati come controllati"
          >
            ✓ Tieni i miei dati
          </button>
        </div>
        {#each meta.pending_candidates as cand (cand.id)}
          {@const venue =
            cand.record.booktitle || cand.record.journal_abbrev || cand.record.journal_full}
          {@const typeLabel =
            cand.record.work_type === 'inproceedings'
              ? 'Atti'
              : cand.record.work_type === 'incollection'
                ? 'Capitolo'
                : cand.record.work_type === 'book'
                  ? 'Libro'
                  : cand.record.work_type === 'phdthesis'
                    ? 'Tesi'
                    : cand.record.work_type === 'misc'
                      ? 'Preprint'
                      : 'Articolo'}
          <div class="cand-card">
            <div class="cand-head">
              <span class="cand-source">{cand.source} · {typeLabel}</span>
              <span class="cand-score">{Math.round(cand.match_score * 100)}%</span>
            </div>
            <div class="cand-title">{cand.record.title}</div>
            <div class="cand-sub">
              {cand.record.authors.map((a) => a.family_name).join(', ')}
              {#if cand.record.year} · {cand.record.year}{/if}
              {#if venue} · {venue}{/if}
              {#if cand.record.pages} · pp. {cand.record.pages}{/if}
              {#if cand.record.mr_number} · MR{cand.record.mr_number}{/if}
              {#if cand.record.doi} · DOI {cand.record.doi}{/if}
            </div>
            <div class="cand-actions">
              <button
                type="button"
                class="primary"
                onclick={() =>
                  onConfirmCandidate(meta.document_id, cand.id, overwriteQueryFields)}
              >
                Conferma
              </button>
              <button type="button" onclick={() => onDismissCandidate(cand.id)}>Ignora</button>
            </div>
          </div>
        {/each}
      </section>
    {/if}

    {#snippet fieldHead(label: string, field: string)}
      <div class="field-head">
        <span class="field-label">{label}</span>
        <button
          type="button"
          class="lock-toggle"
          class:locked={isLocked(field)}
          onclick={() => toggleLock(field)}
          title={isLocked(field)
            ? 'Campo bloccato: non verrà sovrascritto dalla ricerca. Clicca per sbloccare.'
            : 'Campo libero: verrà compilato dalla ricerca automatica. Clicca per bloccare.'}
        >
          {isLocked(field) ? '🔒' : '🔓'}
        </button>
      </div>
    {/snippet}

    <form class="fields-form" onsubmit={handleSave}>
      <div class="field-group">
        {@render fieldHead('Tipo', 'work_type')}
        <select bind:value={workType} onchange={() => touch('work_type', workType)}>
          <option value="article">Articolo (@article)</option>
          <option value="book">Libro (@book)</option>
          <option value="incollection">Capitolo / Raccolta (@incollection)</option>
          <option value="inproceedings">Atti (@inproceedings)</option>
          <option value="phdthesis">Tesi (@phdthesis)</option>
          <option value="misc">Altro / Preprint (@misc)</option>
        </select>
      </div>

      <div class="field-group">
        {@render fieldHead('Titolo', 'title')}
        <textarea rows="2" bind:value={title} oninput={() => touch('title', title)}></textarea>
      </div>

      <div class="field-group">
        {@render fieldHead('Autori (Cognome, Nome; …)', 'authors')}
        <input type="text" bind:value={authorsText} oninput={() => touch('authors', authorsText)} />
      </div>

      <div class="two-col">
        <div class="field-group">
          {@render fieldHead('Anno', 'year')}
          <input
            type="text"
            inputmode="numeric"
            placeholder="es. 2002"
            bind:value={yearStr}
            oninput={() => touch('year', yearStr)}
          />
        </div>
        <div class="field-group">
          {@render fieldHead('Pagine', 'pages')}
          <input type="text" placeholder="es. 577--685" bind:value={pages} oninput={() => touch('pages', pages)} />
        </div>
      </div>

      <div class="guided-box">
        <div class="guided-info">
          {#if overwriteQueryFields}
            <span>
              Scrivi parole del <strong>Titolo</strong> (anche parziali o senza sottotitolo) e/o <strong>Autori</strong>: Papero cercherà il record completo online.
            </span>
          {:else if activeLockedAnchors.length > 0}
            <span>
              Guida bloccata:
              <strong>{activeLockedAnchors.map((k) => ANCHOR_LABELS[k]).join(', ')}</strong>
            </span>
          {:else}
            <span>
              Scrivi <strong>Titolo</strong> e/o <strong>Autori</strong> qui sopra: verranno bloccati 🔒 e il resto sarà cercato online.
            </span>
          {/if}
        </div>
        <label class="guided-overwrite-row">
          <input
            type="checkbox"
            bind:checked={overwriteQueryFields}
            onchange={handleOverwriteToggle}
          />
          <span>Sovrascrivi anche Titolo e Autori con i dati ufficiali trovati</span>
        </label>
        <button
          type="button"
          class="guided-btn"
          disabled={saving || fetchingOnline || meta.metadata_verified || (!title.trim() && !authorsText.trim() && activeLockedAnchors.length === 0)}
          onclick={handleGuidedFetchClick}
          title="Cerca per parole contenute nel titolo (anche senza sottotitolo o con ':' / '-') e/o per autore; se la spunta sopra è attiva aggiorna anche titolo, autori e BibTeX ufficiale"
        >
          {fetchingOnline
            ? 'Ricerca in corso…'
            : overwriteQueryFields
              ? '🔍 Cerca per titolo/autore'
              : '🔍 Blocca e cerca il resto'}
        </button>
      </div>

      <div class="field-group">
        {@render fieldHead('Rivista (abbrev.)', 'journal_abbrev')}
        <input type="text" bind:value={journalAbbrev} oninput={() => touch('journal_abbrev', journalAbbrev)} />
      </div>

      <div class="field-group">
        {@render fieldHead('Rivista (estesa)', 'journal_full')}
        <input type="text" bind:value={journalFull} oninput={() => touch('journal_full', journalFull)} />
      </div>

      <div class="two-col">
        <div class="field-group">
          {@render fieldHead('Volume', 'volume')}
          <input type="text" bind:value={volume} oninput={() => touch('volume', volume)} />
        </div>
        <div class="field-group">
          {@render fieldHead('Numero / Issue', 'issue_number')}
          <input type="text" bind:value={issueNumber} oninput={() => touch('issue_number', issueNumber)} />
        </div>
      </div>

      <div class="field-group">
        {@render fieldHead('Booktitle', 'booktitle')}
        <input type="text" bind:value={booktitle} oninput={() => touch('booktitle', booktitle)} />
      </div>

      <div class="two-col">
        <div class="field-group">
          {@render fieldHead('Serie', 'series')}
          <input type="text" bind:value={series} oninput={() => touch('series', series)} />
        </div>
        <div class="field-group">
          {@render fieldHead('Edizione', 'edition')}
          <input type="text" bind:value={edition} oninput={() => touch('edition', edition)} />
        </div>
      </div>

      <div class="two-col">
        <div class="field-group">
          {@render fieldHead('Editore', 'publisher')}
          <input type="text" bind:value={publisher} oninput={() => touch('publisher', publisher)} />
        </div>
        <div class="field-group">
          {@render fieldHead('Città / Indirizzo', 'address')}
          <input
            type="text"
            placeholder="es. New York"
            bind:value={address}
            oninput={() => touch('address', address)}
          />
        </div>
      </div>

      <div class="two-col">
        <div class="field-group">
          {@render fieldHead('DOI', 'doi')}
          <input type="text" bind:value={doi} oninput={() => touch('doi', doi)} />
        </div>
        <div class="field-group">
          {@render fieldHead('arXiv ID', 'arxiv_id')}
          <input type="text" bind:value={arxivId} oninput={() => touch('arxiv_id', arxivId)} />
        </div>
      </div>

      <div class="two-col">
        <div class="field-group">
          {@render fieldHead('MR Number', 'mr_number')}
          <input type="text" bind:value={mrNumber} oninput={() => touch('mr_number', mrNumber)} />
        </div>
        <div class="field-group">
          {@render fieldHead('zbMATH (Zbl)', 'zbl_number')}
          <input type="text" bind:value={zblNumber} oninput={() => touch('zbl_number', zblNumber)} />
        </div>
      </div>

      <div class="two-col">
        <div class="field-group">
          {@render fieldHead('ISBN', 'isbn')}
          <input type="text" bind:value={isbn} oninput={() => touch('isbn', isbn)} />
        </div>
        <div class="field-group">
          {@render fieldHead('ISSN', 'issn')}
          <input type="text" bind:value={issn} oninput={() => touch('issn', issn)} />
        </div>
      </div>

      <div class="two-col">
        <div class="field-group">
          {@render fieldHead('MSC Primaria', 'msc_primary')}
          <input type="text" placeholder="es. 14A20" bind:value={mscPrimary} oninput={() => touch('msc_primary', mscPrimary)} />
        </div>
        <div class="field-group">
          {@render fieldHead('MSC Secondarie', 'msc_secondary')}
          <input type="text" bind:value={mscSecondaryText} oninput={() => touch('msc_secondary', mscSecondaryText)} />
        </div>
      </div>

      <div class="two-col">
        <div class="field-group">
          {@render fieldHead('Recensore MR', 'mr_reviewer')}
          <input type="text" bind:value={mrReviewer} oninput={() => touch('mr_reviewer', mrReviewer)} />
        </div>
        <div class="field-group">
          {@render fieldHead('URL', 'url')}
          <input type="text" bind:value={url} oninput={() => touch('url', url)} />
        </div>
      </div>

      <label>
        <span>{tr(lang, 'citationKeyOverrideLabel')}</span>
        <input
          type="text"
          placeholder={tr(lang, 'citationKeyOverridePlaceholder')}
          bind:value={citationKeyOverride}
          oninput={() => touch('citation_key_override', citationKeyOverride)}
        />
      </label>

      <label>
        <span>{lang === 'en' ? 'Tags (comma-separated)' : 'Tag (separati da virgola)'}</span>
        <input type="text" bind:value={tagsText} oninput={() => touch('tags')} />
      </label>

      <div class="field-group">
        {@render fieldHead('Abstract', 'abstract_text')}
        <textarea rows="3" bind:value={abstractText} oninput={() => touch('abstract_text', abstractText)}></textarea>
        {#if abstractText.trim() && hasMathDelimiters(abstractText)}
          <div class="abstract-katex-preview">
            <MathMarkdown text={abstractText} compact={true} />
          </div>
        {/if}
      </div>

      <label class="check-row">
        <input
          type="checkbox"
          bind:checked={excludeFromCloud}
          onchange={() => touch('exclude_from_cloud_llm')}
        />
        <span>Escludi documento da servizi LLM cloud</span>
      </label>

      {#if saveError}
        <div class="error-notice" role="alert">{saveError}</div>
      {/if}
      {#if saveFeedback}
        <div class="notice">{saveFeedback}</div>
      {/if}

      <div class="bottom-actions">
        <button
          type="submit"
          class="primary save-btn"
          disabled={saving}
          title="Salva metadati (⌘S / Ctrl+S)"
        >
          {saving ? 'Salvataggio…' : touched.length > 0 || locksModified ? 'Salva modifiche e blocchi' : 'Salva metadati'}
        </button>
        <button
          type="button"
          class="save-btn"
          disabled={
            saving ||
            fetchingOnline ||
            meta.metadata_verified ||
            (!title.trim() && !authorsText.trim() && activeLockedAnchors.length === 0)
          }
          onclick={handleGuidedFetchClick}
          title={meta.metadata_verified
            ? 'Metadati confermati: rimuovi la spunta ✓ prima di cercare online'
            : overwriteQueryFields
              ? 'Cerca per parole del titolo e/o autore e sovrascrivi anche titolo e autori con i dati ufficiali'
              : 'Salva i campi bloccati, azzera i restanti e cerca online'}
        >
          {overwriteQueryFields ? '🔍 Cerca per titolo/autore' : '🔍 Blocca e cerca il resto'}
        </button>
      </div>
    </form>

    {#if relations && totalRelationsCount > 0}
      <details class="rel-details">
        <summary class="rel-summary">
          <span>{tr(lang, 'relatedDocsSection')}</span>
          <span class="rel-count">{totalRelationsCount}</span>
        </summary>
        <div class="rel-body">
          {#if relations.similar.length > 0}
            <div class="rel-group">
              <div class="rel-subhead">{tr(lang, 'similarInLibrary')}</div>
              {#each relations.similar as sim (sim.document_id)}
                <button
                  type="button"
                  class="rel-item"
                  onclick={() => onSelectRelatedDoc?.(sim.document_id)}
                >
                  <span class="rel-item-title">{sim.title}</span>
                  <span class="rel-item-meta">
                    {sim.authors_summary || '—'}{#if sim.year} · {sim.year}{/if} · {Math.round(sim.similarity * 100)}%
                  </span>
                </button>
              {/each}
            </div>
          {/if}
          {#if relations.cites.length > 0}
            <div class="rel-group">
              <div class="rel-subhead">{tr(lang, 'citesInLibrary')}</div>
              {#each relations.cites as c (c.document_id)}
                <button
                  type="button"
                  class="rel-item"
                  onclick={() => onSelectRelatedDoc?.(c.document_id)}
                >
                  <span class="rel-item-title">{c.title}</span>
                  <span class="rel-item-meta">{c.authors_summary || '—'}{#if c.year} · {c.year}{/if}</span>
                </button>
              {/each}
            </div>
          {/if}
          {#if relations.cited_by.length > 0}
            <div class="rel-group">
              <div class="rel-subhead">{tr(lang, 'citedByInLibrary')}</div>
              {#each relations.cited_by as cb (cb.document_id)}
                <button
                  type="button"
                  class="rel-item"
                  onclick={() => onSelectRelatedDoc?.(cb.document_id)}
                >
                  <span class="rel-item-title">{cb.title}</span>
                  <span class="rel-item-meta">{cb.authors_summary || '—'}{#if cb.year} · {cb.year}{/if}</span>
                </button>
              {/each}
            </div>
          {/if}
        </div>
      </details>
    {/if}

    {#if bibPreview}
      <div class="bib-box">
        <div class="bib-label">Anteprima BibTeX</div>
        <pre>{bibPreview}</pre>
      </div>
    {/if}
  {/if}
</aside>

<style>
  .inspector {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow-y: auto;
    padding: 10px 12px;
    background: var(--bg-panel);
    border-left: 1px solid var(--border);
    gap: 8px;
    font-size: 12px;
  }
  .inspector-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .close-btn {
    padding: 2px 7px;
  }
  .action-bar {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .verify-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    padding: 5px 8px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--bg-muted);
  }
  .verify-banner.verified {
    border-color: rgba(22, 163, 74, 0.45);
    background: rgba(22, 163, 74, 0.11);
  }
  .verify-lock-note {
    font-size: 11px;
    color: #15803d;
    background: rgba(22, 163, 74, 0.08);
    border-radius: 5px;
    padding: 4px 8px;
  }
  .verify-main-btn {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    border: none;
    background: transparent;
    color: var(--text);
    font-size: 12px;
    font-weight: 600;
    padding: 2px 2px;
    cursor: pointer;
    text-align: left;
  }
  .verify-main-btn.verified {
    color: #15803d;
  }
  .verify-box {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 17px;
    height: 17px;
    border-radius: 4px;
    border: 1.5px solid var(--text-muted);
    background: var(--bg-panel);
    font-size: 11px;
    font-weight: 800;
    color: #fff;
    flex-shrink: 0;
  }
  .verify-main-btn:hover .verify-box {
    border-color: #16a34a;
  }
  .verify-main-btn.verified .verify-box {
    border-color: #15803d;
    background: #16a34a;
  }
  .verify-next-btn {
    font-size: 11px;
    font-weight: 600;
    padding: 3px 8px;
    border-radius: 5px;
    border: 1px solid #16a34a;
    background: rgba(22, 163, 74, 0.12);
    color: #15803d;
    cursor: pointer;
    white-space: nowrap;
    flex-shrink: 0;
  }
  .verify-next-btn:hover:not(:disabled) {
    background: #16a34a;
    color: #fff;
  }
  .source-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    flex-wrap: wrap;
    font-size: 11px;
    color: var(--text-muted);
  }
  .lock-summary {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .lock-pill {
    background: var(--bg-muted);
    padding: 1px 6px;
    border-radius: 4px;
    font-size: 10px;
  }
  .unlock-all-btn {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-muted);
    border-radius: 4px;
    padding: 1px 6px;
    font-size: 10px;
    cursor: pointer;
  }
  .unlock-all-btn:hover {
    color: var(--text);
    background: var(--bg-muted);
  }
  .clear-unlocked-btn:hover {
    border-color: rgba(220, 38, 38, 0.5);
    color: #b91c1c;
    background: rgba(220, 38, 38, 0.08);
  }
  .notice {
    padding: 5px 8px;
    border-radius: 4px;
    background: var(--accent-soft);
    font-size: 11px;
  }
  .error-notice {
    padding: 5px 8px;
    border-radius: 4px;
    border: 1px solid rgba(220, 38, 38, 0.45);
    background: rgba(220, 38, 38, 0.12);
    color: #b91c1c;
    font-size: 11px;
  }
  .proposal-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    padding: 6px 8px;
    border-radius: 5px;
    border: 1px dashed #d97706;
    background: #fef3c7;
    color: #92400e;
    font-size: 11.5px;
  }
  .prop-actions {
    display: flex;
    gap: 4px;
  }
  .mini-ok {
    border: 1px solid #16a34a;
    background: #dcfce7;
    color: #166534;
    border-radius: 4px;
    padding: 2px 7px;
    font-size: 11px;
    cursor: pointer;
  }
  .mini-no {
    border: 1px solid #dc2626;
    background: #fee2e2;
    color: #991b1b;
    border-radius: 4px;
    padding: 2px 7px;
    font-size: 11px;
    cursor: pointer;
  }
  .tax-box {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-muted);
  }
  .tax-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-muted);
  }
  .tag-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .tag-chip {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 1px 7px;
    font-size: 11px;
  }
  .tag-chip button {
    border: none;
    background: transparent;
    cursor: pointer;
    padding: 0 2px;
    font-size: 12px;
    color: var(--text-muted);
  }
  .tag-add-row {
    display: flex;
    gap: 4px;
  }
  .tag-add-row input {
    flex: 1;
    min-width: 0;
  }
  .license-banner {
    padding: 4px 8px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--bg-muted);
    font-size: 11px;
  }
  .candidates-box {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-muted);
  }
  .cand-box-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
  }
  .cand-card {
    padding: 6px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 4px;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .cand-head {
    display: flex;
    justify-content: space-between;
    font-size: 10px;
    text-transform: uppercase;
    color: var(--text-muted);
  }
  .cand-title {
    font-weight: 600;
  }
  .cand-sub {
    font-size: 11px;
    color: var(--text-muted);
  }
  .cand-actions {
    display: flex;
    gap: 6px;
    margin-top: 3px;
  }
  .fields-form {
    display: flex;
    flex-direction: column;
    gap: 7px;
  }
  .fields-form label,
  .field-group {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .fields-form label span,
  .field-label {
    font-size: 11px;
    color: var(--text-muted);
  }
  .field-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 4px;
  }
  .lock-toggle {
    border: none;
    background: transparent;
    padding: 0 3px;
    font-size: 11px;
    line-height: 1.2;
    cursor: pointer;
    opacity: 0.45;
    border-radius: 3px;
  }
  .lock-toggle:hover {
    opacity: 0.9;
    background: var(--bg-muted);
  }
  .lock-toggle.locked {
    opacity: 1;
    background: var(--accent-soft);
  }
  .guided-box {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 7px 8px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--accent-soft);
  }
  .guided-info {
    font-size: 11px;
    line-height: 1.35;
  }
  .guided-overwrite-row {
    display: flex !important;
    flex-direction: row !important;
    align-items: flex-start;
    gap: 6px !important;
    cursor: pointer;
    user-select: none;
  }
  .guided-overwrite-row input {
    margin-top: 2px;
  }
  .guided-overwrite-row span {
    color: var(--text-main, inherit) !important;
    font-size: 11px;
    line-height: 1.3;
  }
  .guided-btn {
    align-self: flex-start;
    font-weight: 600;
    padding: 4px 10px;
    border-radius: 5px;
    border: 1px solid var(--accent, #2563eb);
    background: var(--bg-panel);
    cursor: pointer;
  }
  .guided-btn:hover:not(:disabled) {
    filter: brightness(0.97);
  }
  .two-col {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
  }
  .check-row {
    flex-direction: row !important;
    align-items: center;
    gap: 6px !important;
  }
  .bottom-actions {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    margin-top: 4px;
  }
  .save-btn {
    flex: 1;
    min-width: 130px;
  }
  .bib-box pre {
    margin: 4px 0 0;
    padding: 8px;
    border-radius: 4px;
    background: var(--bg-muted);
    border: 1px solid var(--border);
    font-size: 11px;
    overflow-x: auto;
    white-space: pre-wrap;
  }
  .bib-label {
    font-size: 11px;
    color: var(--text-muted);
  }
  .status-msg {
    color: var(--text-muted);
  }
  .smart-paste-bar {
    display: flex;
    gap: 4px;
    align-items: center;
  }
  .smart-paste-input {
    flex: 1;
    min-width: 0;
    font-size: 11.5px;
    padding: 4px 7px;
  }
  .smart-paste-btn {
    font-size: 11px;
    padding: 4px 8px;
    flex-shrink: 0;
  }
  .reading-row {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .reading-select {
    flex: 1;
    font-size: 11.5px;
    padding: 3px 6px;
  }
  .star-toggle-btn {
    font-size: 11.5px;
    padding: 3px 8px;
    border-radius: 5px;
  }
  .star-toggle-btn.starred {
    border-color: #d97706;
    background: rgba(245, 158, 11, 0.14);
    color: #b45309;
    font-weight: 600;
  }
  .pub-notice-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    padding: 6px 8px;
    border-radius: 6px;
    border: 1px solid rgba(16, 185, 129, 0.4);
    background: rgba(16, 185, 129, 0.1);
    font-size: 11px;
  }
  .pub-notice-text {
    line-height: 1.35;
  }
  .abstract-katex-preview {
    margin-top: 4px;
    padding: 7px 8px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--bg-muted);
  }
  .rel-details {
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-muted);
    padding: 5px 8px;
  }
  .rel-summary {
    display: flex;
    align-items: center;
    justify-content: space-between;
    cursor: pointer;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--text);
    list-style: none;
  }
  .rel-count {
    font-size: 10px;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    color: var(--text-muted);
  }
  .rel-body {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 7px;
  }
  .rel-group {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .rel-subhead {
    font-size: 10.5px;
    font-weight: 700;
    text-transform: uppercase;
    color: var(--text-muted);
  }
  .rel-item {
    display: flex;
    flex-direction: column;
    gap: 1px;
    text-align: left;
    padding: 5px 7px;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg-panel);
    cursor: pointer;
  }
  .rel-item:hover {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .rel-item-title {
    font-size: 11.5px;
    font-weight: 600;
    color: var(--text);
    line-height: 1.3;
  }
  .rel-item-meta {
    font-size: 10.5px;
    color: var(--text-muted);
  }
</style>
