<script lang="ts">
  import type {
    BibliographicAuthor,
    DocumentMetadataView,
    ManualMetadataEdit,
    ResolveOutcome,
    TopicClusterView,
  } from '../types';

  interface Props {
    meta: DocumentMetadataView | null;
    selectedFileId?: number | null;
    clusters?: TopicClusterView[];
    loading: boolean;
    fetchingOnline: boolean;
    lastFetchMessage: string;
    onSave: (edit: ManualMetadataEdit, lockedFields?: string[]) => Promise<void>;
    onSaveAndFetch?: (
      edit: ManualMetadataEdit,
      lockedFields: string[],
      overwriteQueryFields?: boolean
    ) => Promise<ResolveOutcome | void>;
    onFetchMetadata: (documentId: number) => Promise<ResolveOutcome | void>;
    onToggleVerified?: (documentId: number, verified: boolean, advanceNext?: boolean) => Promise<void>;
    hasNextUnverified?: boolean;
    onConfirmCandidate: (
      documentId: number,
      candidateId: number,
      overwriteQueryFields?: boolean
    ) => Promise<void>;
    onDismissCandidate: (candidateId: number) => Promise<void>;
    onCopyBibtex: (documentId: number) => Promise<string>;
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
    meta,
    selectedFileId = null,
    clusters = [],
    loading,
    fetchingOnline,
    lastFetchMessage,
    onSave,
    onSaveAndFetch,
    onFetchMetadata,
    onToggleVerified,
    hasNextUnverified = false,
    onConfirmCandidate,
    onDismissCandidate,
    onCopyBibtex,
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
  let tagsText = $state('');
  let excludeFromCloud = $state(false);
  let lockedFields = $state<string[]>([]);
  let locksModified = $state(false);
  let touched = $state<string[]>([]);
  let overwriteQueryFields = $state(true);
  let lastDocId = $state<number | null>(null);
  let bibPreview = $state('');
  let copyFeedback = $state('');
  let altFormulaFeedback = $state('');
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

  function formatAuthorsForInput(authors: BibliographicAuthor[]): string {
    return authors
      .map((a) => (a.given_name.trim() ? `${a.family_name}, ${a.given_name}` : a.family_name))
      .join('; ');
  }

  function parseAuthorsInput(raw: string): BibliographicAuthor[] {
    return raw
      .split(';')
      .map((s) => s.trim())
      .filter(Boolean)
      .map((part) => {
        const idx = part.indexOf(',');
        if (idx >= 0) {
          return {
            family_name: part.slice(0, idx).trim(),
            given_name: part.slice(idx + 1).trim(),
            role: 'author',
          };
        }
        const tokens = part.split(/\s+/);
        if (tokens.length >= 2) {
          return {
            family_name: tokens[tokens.length - 1],
            given_name: tokens.slice(0, -1).join(' '),
            role: 'author',
          };
        }
        return { family_name: part, given_name: '', role: 'author' };
      });
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
    tagsText = meta.tags.join(', ');
    excludeFromCloud = meta.exclude_from_cloud_llm;
    lockedFields = meta.manual_locked_fields.filter(
      (f) => f !== 'tags' && f !== 'exclude_from_cloud_llm'
    );
    if (meta.document_id !== lastDocId) {
      lastDocId = meta.document_id;
      overwriteQueryFields = true;
    }
    locksModified = false;
    touched = [];
    bibPreview = meta.raw_mrlookup_bibtex ?? '';
    copyFeedback = '';
  });

  function touch(field: string, currentValue?: string) {
    if (!touched.includes(field)) {
      touched = [...touched, field];
    }
    if (field === 'tags' || field === 'exclude_from_cloud_llm') return;
    if ((field === 'title' || field === 'authors') && overwriteQueryFields) {
      return;
    }
    if (currentValue !== undefined) {
      if (currentValue.trim()) {
        if (!lockedFields.includes(field)) {
          lockedFields = [...lockedFields, field];
        }
      } else {
        lockedFields = lockedFields.filter((f) => f !== field);
      }
      locksModified = true;
    } else if (!lockedFields.includes(field)) {
      lockedFields = [...lockedFields, field];
      locksModified = true;
    }
  }

  function isLocked(field: string): boolean {
    return lockedFields.includes(field);
  }

  function toggleLock(field: string) {
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
    }
  }

  function unlockAllFields() {
    lockedFields = [];
    locksModified = true;
  }

  function optStr(val: string): string | null {
    const t = val.trim();
    return t ? t : null;
  }

  function buildEditPayload(allowEmptyTitle = false): ManualMetadataEdit | null {
    if (!meta) return null;
    const parsedYear = yearStr.trim() ? Number(yearStr.trim()) : null;
    return {
      document_id: meta.document_id,
      work_type: workType,
      title: allowEmptyTitle ? title.trim() : title.trim() || meta.title,
      authors: parseAuthorsInput(authorsText),
      journal_abbrev: optStr(journalAbbrev),
      journal_full: optStr(journalFull),
      booktitle: optStr(booktitle),
      series: optStr(series),
      volume: optStr(volume),
      issue_number: optStr(issueNumber),
      year: parsedYear && Number.isFinite(parsedYear) ? parsedYear : null,
      pages: optStr(pages),
      edition: optStr(edition),
      publisher: optStr(publisher),
      address: optStr(address),
      isbn: optStr(isbn),
      issn: optStr(issn),
      doi: optStr(doi),
      arxiv_id: optStr(arxivId),
      mr_number: optStr(mrNumber),
      mr_reviewer: optStr(mrReviewer),
      zbl_number: optStr(zblNumber),
      url: optStr(url),
      msc_primary: optStr(mscPrimary),
      msc_secondary: mscSecondaryText
        .split(',')
        .map((s) => s.trim())
        .filter(Boolean),
      abstract_text: optStr(abstractText),
      exclude_from_cloud_llm: excludeFromCloud,
      tags: tagsText
        .split(',')
        .map((s) => s.trim())
        .filter(Boolean),
      edited_fields: touched,
    };
  }

  async function handleSave(e: Event) {
    e.preventDefault();
    const payload = buildEditPayload(false);
    if (!payload) return;
    const saveLocks = [...lockedFields];
    if (touched.includes('title') && title.trim() && !saveLocks.includes('title')) {
      saveLocks.push('title');
    }
    if (touched.includes('authors') && authorsText.trim() && !saveLocks.includes('authors')) {
      saveLocks.push('authors');
    }
    saving = true;
    try {
      await onSave(payload, saveLocks);
      lockedFields = saveLocks;
      touched = [];
      locksModified = false;
    } finally {
      saving = false;
    }
  }

  async function handleGuidedFetchClick() {
    if (!meta) return;
    let nextLocks = [...lockedFields];
    if (overwriteQueryFields) {
      nextLocks = nextLocks.filter((f) => f !== 'title' && f !== 'authors');
      if (yearStr.trim() && touched.includes('year') && !nextLocks.includes('year')) {
        nextLocks.push('year');
      }
    } else {
      const hasPrimaryAnchor = nextLocks.some((f) => PRIMARY_ANCHOR_KEYS.includes(f));
      if (!hasPrimaryAnchor) {
        if (title.trim()) nextLocks.push('title');
        if (authorsText.trim()) nextLocks.push('authors');
        if (yearStr.trim() && touched.includes('year')) nextLocks.push('year');
      }
    }
    lockedFields = nextLocks;
    const payload = buildEditPayload(true);
    if (!payload) return;
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
    } finally {
      saving = false;
    }
  }

  async function handleTopFetchClick() {
    if (!meta) return;
    if (touched.length > 0 || locksModified) {
      const payload = buildEditPayload(false);
      if (payload) {
        await onSave(payload, lockedFields);
        touched = [];
        locksModified = false;
      }
    }
    await onFetchMetadata(meta.document_id);
  }

  async function handleToggleVerifiedClick(advanceNext = false) {
    if (!meta || !onToggleVerified) return;
    const nextState = advanceNext ? true : !meta.metadata_verified;
    saving = true;
    try {
      if (nextState && (touched.length > 0 || locksModified)) {
        const payload = buildEditPayload(false);
        if (payload) {
          await onSave(payload, lockedFields);
          touched = [];
          locksModified = false;
        }
      }
      await onToggleVerified(meta.document_id, nextState, advanceNext);
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
</script>

<aside class="inspector">
  <div class="inspector-header">
    <strong>Ispettore Metadati</strong>
    <button type="button" class="close-btn" onclick={onClose} title="Chiudi pannello">✕</button>
  </div>

  {#if loading}
    <div class="status-msg">Caricamento metadati…</div>
  {:else if !meta}
    <div class="status-msg">Seleziona un documento dalla lista per visualizzare o modificare i metadati.</div>
  {:else}
    <div class="action-bar">
      {#if selectedFileId !== null && onOpenInViewer}
        <button type="button" onclick={onOpenInViewer}>
          Apri nel lettore
        </button>
      {/if}
      <button
        type="button"
        class="primary"
        disabled={fetchingOnline || saving || meta.metadata_verified}
        onclick={handleTopFetchClick}
        title={meta.metadata_verified
          ? 'Metadati confermati: rimuovi la spunta ✓ prima di avviare la ricerca online'
          : 'Cerca/aggiorna metadati rispettando i campi bloccati'}
      >
        {fetchingOnline ? 'Ricerca…' : 'Aggiorna metadati'}
      </button>
      <button type="button" onclick={handleCopyBibtexClick}>
        Copia BibTeX (MRLookup)
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

    {#if lastFetchMessage}
      <div class="notice">{lastFetchMessage}</div>
    {/if}
    {#if copyFeedback}
      <div class="notice">{copyFeedback}</div>
    {/if}
    {#if altFormulaFeedback}
      <div class="notice">{altFormulaFeedback}</div>
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
          disabled={saving || fetchingOnline}
          onclick={() => handleToggleVerifiedClick(false)}
          title={meta.metadata_verified
            ? 'Clicca per rimuovere il check di controllo metadati'
            : 'Salva eventuali modifiche e segna i metadati come controllati e corretti'}
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
            disabled={saving || fetchingOnline}
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
      {#if lockedFields.length > 0}
        <div class="lock-summary">
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
        </div>
      {/if}
    </div>

    {#if meta.metadata_license_note || meta.metadata_source === 'zbmath'}
      <div class="license-banner">
        {meta.metadata_license_note ?? 'Metadata via zbMATH Open (CC-BY-SA 4.0)'}
      </div>
    {/if}

    {#if meta.pending_candidates.length > 0}
      <section class="candidates-box">
        <strong>Candidati ambigui ({meta.pending_candidates.length})</strong>
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
          <input type="number" bind:value={yearStr} oninput={() => touch('year', yearStr)} />
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

      <div class="two-col">
        <div class="field-group">
          {@render fieldHead('Booktitle', 'booktitle')}
          <input type="text" bind:value={booktitle} oninput={() => touch('booktitle', booktitle)} />
        </div>
        <div class="field-group">
          {@render fieldHead('Serie', 'series')}
          <input type="text" bind:value={series} oninput={() => touch('series', series)} />
        </div>
      </div>

      <div class="two-col">
        <div class="field-group">
          {@render fieldHead('Editore', 'publisher')}
          <input type="text" bind:value={publisher} oninput={() => touch('publisher', publisher)} />
        </div>
        <div class="field-group">
          {@render fieldHead('Edizione', 'edition')}
          <input type="text" bind:value={edition} oninput={() => touch('edition', edition)} />
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
        <span>Tag (separati da virgola)</span>
        <input type="text" bind:value={tagsText} oninput={() => touch('tags')} />
      </label>

      <div class="field-group">
        {@render fieldHead('Abstract', 'abstract_text')}
        <textarea rows="3" bind:value={abstractText} oninput={() => touch('abstract_text', abstractText)}></textarea>
      </div>

      <label class="check-row">
        <input
          type="checkbox"
          bind:checked={excludeFromCloud}
          onchange={() => touch('exclude_from_cloud_llm')}
        />
        <span>Escludi documento da servizi LLM cloud</span>
      </label>

      <div class="bottom-actions">
        <button type="submit" class="primary save-btn" disabled={saving || fetchingOnline}>
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

    {#if bibPreview}
      <div class="bib-box">
        <div class="bib-label">Anteprima BibTeX (MRLookup)</div>
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
  .notice {
    padding: 5px 8px;
    border-radius: 4px;
    background: var(--accent-soft);
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
</style>
