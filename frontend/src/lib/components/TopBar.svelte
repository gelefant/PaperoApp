<script lang="ts">
  import { onDestroy } from 'svelte';
  import { APP_VERSION } from '../appVersion';
  import { tr, type Lang } from '../i18n';
  import type {
    DownloadProgressEvent,
    EmbedProgressStatus,
    ExternalReaderPreset,
    MetadataSearchFilter,
    MlEnvironmentStatus,
    SortDirection,
    SortField,
    SourceRoot,
    UiSettingsPayload,
    VerifiedFilter,
    WorkCategory,
  } from '../types';

  interface Props {
    lang?: Lang;
    query: string;
    searchMode: 'content' | 'metadata';
    metaFilter: MetadataSearchFilter;
    activeLeftView: 'library' | 'duplicates';
    pendingDuplicatesCount: number;
    scanning: boolean;
    scanStatusText: string;
    totalDocsCount?: number;
    unresolvedMetaCount?: number;
    fetchingBulkMeta?: boolean;
    embedProgress?: EmbedProgressStatus | null;
    roots: SourceRoot[];
    settings: UiSettingsPayload | null;
    mlStatus: MlEnvironmentStatus | null;
    mlDownloadProgress: DownloadProgressEvent | null;
    presets: ExternalReaderPreset[];
    inspectorOpen: boolean;
    notesScopeActive?: boolean;
    hasNotes?: boolean;
    updateAvailableVersion?: string | null;
    onSearch: (q: string) => void;
    onClearSearch: () => void;
    onSearchModeChange: (mode: 'content' | 'metadata') => void;
    onMetaFilterChange: (next: MetadataSearchFilter) => void;
    onWorkCategoryChange?: (cat: WorkCategory) => void;
    onToggleNotesScope?: (active: boolean) => void;
    onOpenClassics?: () => void;
    onVerifiedFilterChange?: (vf: VerifiedFilter) => void;
    onSortByChange?: (sortBy: SortField) => void;
    onToggleSortDir?: () => void;
    onToggleLeftView: (view: 'library' | 'duplicates') => void;
    onToggleInspector: () => void;
    onAddFolder: () => void;
    onRescan: () => void;
    onControlRescan: (action: 'pause' | 'resume' | 'cancel') => void;
    onToggleEmbedPaused?: (paused: boolean) => void;
    onFetchAllMetadata?: () => void;
    onCancelBulkMetadata?: () => void;
    onExportBibtex: () => void;
    onExportCsv: () => void;
    onImportMetadata: () => void;
    onSaveSettings: (next: UiSettingsPayload) => Promise<void>;
    onRefreshMlStatus: () => void;
    onDownloadMlProfile: (profile: 'light' | 'quality') => Promise<void>;
    onUnloadMlModels: () => Promise<void>;
    onOpenSettings?: () => void;
  }

  let {
    lang = 'it',
    query = $bindable(),
    searchMode,
    metaFilter,
    activeLeftView,
    pendingDuplicatesCount,
    scanning,
    scanStatusText,
    totalDocsCount = 0,
    unresolvedMetaCount = 0,
    fetchingBulkMeta = false,
    embedProgress = null,
    roots,
    settings,
    mlStatus,
    mlDownloadProgress,
    presets,
    inspectorOpen,
    notesScopeActive = false,
    hasNotes = false,
    updateAvailableVersion = null,
    onSearch,
    onClearSearch,
    onSearchModeChange,
    onMetaFilterChange,
    onWorkCategoryChange,
    onToggleNotesScope,
    onOpenClassics,
    onVerifiedFilterChange,
    onSortByChange,
    onToggleSortDir,
    onToggleLeftView,
    onToggleInspector,
    onAddFolder,
    onRescan,
    onControlRescan,
    onToggleEmbedPaused,
    onFetchAllMetadata,
    onCancelBulkMetadata,
    onExportBibtex,
    onExportCsv,
    onImportMetadata,
    onSaveSettings,
    onRefreshMlStatus,
    onDownloadMlProfile,
    onUnloadMlModels,
    onOpenSettings,
  }: Props = $props();

  let paused = $state(false);
  let showSettingsPopover = $state(false);
  let emailDraft = $state('');
  let downloadingProfile = $state<string | null>(null);
  let mlActionError = $state('');
  let searchInputEl = $state<HTMLInputElement | null>(null);
  let metaDebounceTimer: ReturnType<typeof setTimeout> | null = null;
  let filterDebounceTimer: ReturnType<typeof setTimeout> | null = null;

  const effectiveWorkCat = $derived<WorkCategory>(
    metaFilter.work_category === 'paper' || metaFilter.work_category === 'book'
      ? metaFilter.work_category
      : 'all'
  );
  const effectiveVerifiedFilter = $derived<VerifiedFilter>(
    metaFilter.verified_filter === 'unverified' || metaFilter.verified_filter === 'verified'
      ? metaFilter.verified_filter
      : 'all'
  );
  const effectiveSortBy = $derived<SortField>(metaFilter.sort_by ?? 'updated');
  const effectiveSortDir = $derived<SortDirection>(
    metaFilter.sort_dir ??
      (effectiveSortBy === 'title' || effectiveSortBy === 'author' ? 'asc' : 'desc')
  );

  function scheduleMetadataSearch(q: string) {
    if (metaDebounceTimer) clearTimeout(metaDebounceTimer);
    metaDebounceTimer = setTimeout(() => {
      metaDebounceTimer = null;
      onSearch(q);
    }, 160);
  }

  function scheduleMetaFilterChange(patch: Partial<MetadataSearchFilter>) {
    if (filterDebounceTimer) clearTimeout(filterDebounceTimer);
    filterDebounceTimer = setTimeout(() => {
      filterDebounceTimer = null;
      onMetaFilterChange({ ...metaFilter, ...patch });
    }, 160);
  }

  onDestroy(() => {
    if (metaDebounceTimer) {
      clearTimeout(metaDebounceTimer);
      metaDebounceTimer = null;
    }
    if (filterDebounceTimer) {
      clearTimeout(filterDebounceTimer);
      filterDebounceTimer = null;
    }
  });

  export function focusSearch() {
    searchInputEl?.focus();
    searchInputEl?.select();
  }

  $effect(() => {
    if (settings) {
      emailDraft = settings.polite_email ?? '';
    }
  });

  $effect(() => {
    if (showSettingsPopover) {
      onRefreshMlStatus();
    }
  });

  async function handleDownloadProfileClick(profile: 'light' | 'quality') {
    downloadingProfile = profile;
    mlActionError = '';
    try {
      await onDownloadMlProfile(profile);
    } catch (e) {
      mlActionError = `Errore download modello: ${e}`;
    } finally {
      downloadingProfile = null;
    }
  }

  function formatMb(bytes: number): string {
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }

  function submitSearch(e: Event) {
    e.preventDefault();
    if (metaDebounceTimer) {
      clearTimeout(metaDebounceTimer);
      metaDebounceTimer = null;
    }
    onSearch(query);
  }

  function patchSettings(patch: Partial<UiSettingsPayload>) {
    if (!settings) return;
    onSaveSettings({ ...settings, ...patch });
  }
</script>

<header class="topbar">
  <div class="brand">
    <svg class="brand-logo" viewBox="0 0 24 24" aria-hidden="true">
      <polygon points="3,15 10,9 14,15" fill="#f59e0b" />
      <polygon points="10,9 16,12 14,15" fill="#d97706" />
      <polygon points="13,11 17,5 19,9" fill="#fbbf24" />
      <polygon points="17,5 22,7 19,8.5" fill="#ea580c" />
      <polygon points="6,15 17,15 13,20 4,18" fill="#fcd34d" />
    </svg>
    <strong class="brand-name" title={`Papero v${APP_VERSION}`}>Papero</strong>
    <span class="brand-version" title={`Papero v${APP_VERSION}`}>{APP_VERSION}</span>
    <span
      class="roots-pill"
      title={roots.length > 0
        ? roots.map((r) => `${r.label}: ${r.root_path}`).join('\n')
        : lang === 'en'
          ? 'No source folders configured'
          : 'Nessuna cartella sorgente configurata'}
    >
      <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
      </svg>
      <span>{roots.length}</span>
    </span>
  </div>

  <form class="search-form" onsubmit={submitSearch}>
    <select
      class="search-mode-select"
      aria-label="Modalità di ricerca"
      title={searchMode === 'content'
        ? 'Ricerca nel testo completo (BM25 + semantica ibrida)'
        : 'Filtra per titolo, autore, anno, rivista, MSC'}
      value={searchMode}
      onchange={(e) => {
        if (metaDebounceTimer) {
          clearTimeout(metaDebounceTimer);
          metaDebounceTimer = null;
        }
        onSearchModeChange((e.currentTarget as HTMLSelectElement).value as 'content' | 'metadata');
      }}
    >
      <option value="content">{tr(lang, 'modeContent')}</option>
      <option value="metadata">{tr(lang, 'modeMetadata')}</option>
    </select>
    <select
      class="search-scope-select"
      class:filtered={notesScopeActive || effectiveWorkCat !== 'all'}
      aria-label={lang === 'en' ? 'Filter scope' : 'Ambito ricerca'}
      title={lang === 'en'
        ? 'Contextual scope: All, Papers, Books' + (hasNotes ? ', or Notes' : '')
        : 'Ambito ricerca: Tutti, solo Paper, solo Libri' + (hasNotes ? ' o solo Note' : '')}
      value={notesScopeActive && hasNotes ? 'notes' : effectiveWorkCat}
      onchange={(e) => {
        const raw = (e.currentTarget as HTMLSelectElement).value;
        if (raw === 'notes') {
          onToggleNotesScope?.(true);
        } else {
          if (notesScopeActive) onToggleNotesScope?.(false);
          const val = raw as WorkCategory;
          if (onWorkCategoryChange) onWorkCategoryChange(val);
          else onMetaFilterChange({ ...metaFilter, work_category: val });
        }
      }}
    >
      <option value="all">{tr(lang, 'filterAll')}</option>
      <option value="paper">{tr(lang, 'filterPapers')}</option>
      <option value="book">{tr(lang, 'filterBooks')}</option>
      {#if hasNotes}
        <option value="notes">{tr(lang, 'filterNotes')}</option>
      {/if}
    </select>
    <input
      class="search-input"
      bind:this={searchInputEl}
      type="search"
      placeholder={notesScopeActive
        ? lang === 'en'
          ? 'Search your personal notes, highlights & bookmarks…'
          : 'Cerca nelle tue note, evidenziazioni e segnalibri…'
        : searchMode === 'content'
          ? tr(lang, 'searchContentPlaceholder')
          : tr(lang, 'searchMetadataPlaceholder')}
      bind:value={query}
      oninput={() => {
        if (notesScopeActive || searchMode === 'metadata') scheduleMetadataSearch(query);
      }}
    />
    {#if query.trim()}
      <button
        type="button"
        class="search-clear-btn"
        aria-label={tr(lang, 'btnClear')}
        title={tr(lang, 'btnClear')}
        onclick={() => {
          if (metaDebounceTimer) {
            clearTimeout(metaDebounceTimer);
            metaDebounceTimer = null;
          }
          onClearSearch();
        }}
      >
        <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" aria-hidden="true">
          <path d="M18 6 6 18M6 6l12 12" />
        </svg>
      </button>
    {/if}
    <button
      type="submit"
      class="search-submit-btn primary"
      aria-label={tr(lang, 'btnSearch')}
      title={tr(lang, 'btnSearch')}
    >
      <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <circle cx="11" cy="11" r="7" />
        <path d="m20 20-3.5-3.5" />
      </svg>
    </button>
  </form>

  <div class="actions">
    {#if embedProgress && embedProgress.profile_ready && (embedProgress.pending_files > 0 || embedProgress.running)}
      <div
        class="embed-pill"
        class:paused={embedProgress.paused}
        title={lang === 'en'
          ? `Background ML embedding (${embedProgress.profile}): ${embedProgress.embedded_files}/${embedProgress.total_files} files${embedProgress.paused ? ' (paused)' : ''}`
          : `Indicizzazione vettoriale ML (${embedProgress.profile}): ${embedProgress.embedded_files}/${embedProgress.total_files} file${embedProgress.paused ? ' (in pausa)' : ''}`}
      >
        <span class="embed-dot" class:paused={embedProgress.paused} aria-hidden="true"></span>
        <span class="embed-label">
          Embedding {embedProgress.embedded_files}/{embedProgress.total_files}
        </span>
        {#if onToggleEmbedPaused}
          <button
            type="button"
            class="embed-ctrl-btn"
            aria-label={embedProgress.user_paused
              ? lang === 'en'
                ? 'Resume ML embedding'
                : 'Riprendi embedding ML'
              : lang === 'en'
                ? 'Pause ML embedding'
                : 'Metti in pausa embedding ML'}
            title={embedProgress.user_paused
              ? lang === 'en'
                ? 'Resume background ML embedding'
                : 'Riprendi indicizzazione ML in background'
              : lang === 'en'
                ? 'Pause background ML embedding to save CPU/battery'
                : 'Metti in pausa indicizzazione ML per risparmiare CPU/batteria'}
            onclick={() => onToggleEmbedPaused(!embedProgress.user_paused)}
          >
            {#if embedProgress.user_paused}
              <svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor" aria-hidden="true">
                <polygon points="6,4 20,12 6,20" />
              </svg>
            {:else}
              <svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor" aria-hidden="true">
                <rect x="6" y="4" width="4" height="16" rx="1" />
                <rect x="14" y="4" width="4" height="16" rx="1" />
              </svg>
            {/if}
          </button>
        {/if}
      </div>
    {/if}

    <!-- Gruppo 1: Cartelle, Scansione & Metadati -->
    <div class="btn-group">
      <button
        type="button"
        class="icon-btn"
        data-tip={lang === 'en' ? 'Add source folder…' : 'Aggiungi cartella…'}
        aria-label={tr(lang, 'addFolder')}
        onclick={onAddFolder}
      >
        <svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
          <path d="M12 10v6M9 13h6" />
        </svg>
      </button>

      <button
        type="button"
        class="icon-btn"
        class:primary={!scanning && roots.length > 0 && totalDocsCount === 0}
        disabled={scanning || roots.length === 0}
        data-tip={scanning
          ? tr(lang, 'scanning')
          : lang === 'en'
            ? 'Rescan folders (⌘R)'
            : 'Riscansiona cartelle (⌘R)'}
        aria-label={scanning ? tr(lang, 'scanning') : tr(lang, 'rescan')}
        onclick={onRescan}
      >
        <svg class:spin={scanning} viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M21 12a9 9 0 0 0-15.5-6.36L3 8" />
          <path d="M3 3v5h5" />
          <path d="M3 12a9 9 0 0 0 15.5 6.36L21 16" />
          <path d="M21 21v-5h-5" />
        </svg>
      </button>

      {#if scanning}
        <button
          type="button"
          class="icon-btn"
          data-tip={paused ? 'Riprendi scansione' : 'Metti in pausa'}
          aria-label={paused ? 'Riprendi scansione' : 'Metti in pausa'}
          onclick={() => {
            paused = !paused;
            onControlRescan(paused ? 'pause' : 'resume');
          }}
        >
          {#if paused}
            <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" aria-hidden="true">
              <polygon points="6,4 20,12 6,20" />
            </svg>
          {:else}
            <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" aria-hidden="true">
              <rect x="6" y="4" width="4" height="16" rx="1" />
              <rect x="14" y="4" width="4" height="16" rx="1" />
            </svg>
          {/if}
        </button>
        <button
          type="button"
          class="icon-btn danger-hover"
          data-tip="Annulla scansione"
          aria-label="Annulla scansione"
          onclick={() => onControlRescan('cancel')}
        >
          <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" aria-hidden="true">
            <rect x="6" y="6" width="12" height="12" rx="2" />
          </svg>
        </button>
      {/if}

      {#if onFetchAllMetadata}
        {#if fetchingBulkMeta}
          <button
            type="button"
            class="icon-btn active-pulse"
            disabled
            data-tip={lang === 'en' ? 'Fetching metadata…' : 'Ricerca metadati in corso…'}
            aria-label={lang === 'en' ? 'Fetching metadata…' : 'Ricerca metadati in corso…'}
          >
            <svg class="spin" viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
              <path d="M12 3a9 9 0 1 0 9 9" />
            </svg>
          </button>
          {#if onCancelBulkMetadata}
            <button
              type="button"
              class="icon-btn danger-hover"
              data-tip={lang === 'en' ? 'Stop metadata scan' : 'Interrompi scansione metadati'}
              aria-label={lang === 'en' ? 'Stop metadata scan' : 'Interrompi scansione metadati'}
              onclick={onCancelBulkMetadata}
            >
              <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" aria-hidden="true">
                <rect x="6" y="6" width="12" height="12" rx="2" />
              </svg>
            </button>
          {/if}
        {:else}
          <button
            type="button"
            class="icon-btn"
            class:primary={unresolvedMetaCount > 0}
            disabled={scanning || totalDocsCount === 0}
            data-tip={lang === 'en'
              ? `Scan metadata for all papers${unresolvedMetaCount > 0 ? ` (${unresolvedMetaCount} pending)` : ''}`
              : `Aggiorna metadati di tutti i documenti${unresolvedMetaCount > 0 ? ` (${unresolvedMetaCount} da completare)` : ''}`}
            aria-label={lang === 'en' ? 'Scan all metadata' : 'Metadati tutti'}
            onclick={onFetchAllMetadata}
          >
            <svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <ellipse cx="11" cy="6" rx="7" ry="3" />
              <path d="M4 6v5c0 1.66 3.13 3 7 3 1.4 0 2.7-.18 3.8-.48" />
              <path d="M4 11v5c0 1.66 3.13 3 7 3 .85 0 1.67-.07 2.42-.19" />
              <path d="m18.5 13.5 1.1 2.4 2.4 1.1-2.4 1.1-1.1 2.4-1.1-2.4-2.4-1.1 2.4-1.1z" />
            </svg>
            {#if unresolvedMetaCount > 0}
              <span class="btn-badge">{unresolvedMetaCount}</span>
            {/if}
          </button>
        {/if}
      {/if}
    </div>

    <span class="v-divider" aria-hidden="true"></span>

    <!-- Gruppo 2: Importa / Esporta -->
    <div class="btn-group">
      <button
        type="button"
        class="icon-btn"
        data-tip={tr(lang, 'importBib')}
        aria-label={tr(lang, 'importBib')}
        onclick={onImportMetadata}
      >
        <svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M12 3v12" />
          <path d="m8 11 4 4 4-4" />
          <path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2" />
        </svg>
      </button>

      <button
        type="button"
        class="icon-btn"
        data-tip={tr(lang, 'exportBib')}
        aria-label={tr(lang, 'exportBib')}
        onclick={onExportBibtex}
      >
        <svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M9 5C7 5 6 6 6 8v2c0 1.2-1 2-2 2 1 0 2 .8 2 2v2c0 2 1 3 3 3" />
          <path d="M15 5c2 0 3 1 3 3v2c0 1.2 1 2 2 2-1 0-2 .8-2 2v2c0 2-1 3-3 3" />
        </svg>
      </button>

      <button
        type="button"
        class="icon-btn"
        data-tip={tr(lang, 'exportCsv')}
        aria-label={tr(lang, 'exportCsv')}
        onclick={onExportCsv}
      >
        <svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <rect x="3" y="4" width="18" height="16" rx="2" />
          <path d="M3 10h18M3 15h18M9 4v16M15 4v16" />
        </svg>
      </button>
    </div>

    <span class="v-divider" aria-hidden="true"></span>

    <!-- Gruppo 3: Doppioni, Ispettore & Impostazioni -->
    <div class="btn-group">
      <button
        type="button"
        class="icon-btn"
        class:active-toggle={activeLeftView === 'duplicates'}
        data-tip={`${tr(lang, 'duplicates')} (${pendingDuplicatesCount})`}
        aria-label={`${tr(lang, 'duplicates')} (${pendingDuplicatesCount})`}
        onclick={() => onToggleLeftView(activeLeftView === 'duplicates' ? 'library' : 'duplicates')}
      >
        <svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <rect x="8" y="8" width="12" height="12" rx="2" />
          <path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" />
        </svg>
        {#if pendingDuplicatesCount > 0}
          <span class="btn-badge warn">{pendingDuplicatesCount}</span>
        {/if}
      </button>

      <button
        type="button"
        class="icon-btn tip-end"
        class:active-toggle={inspectorOpen}
        data-tip={lang === 'en' ? 'Toggle Metadata Inspector' : 'Mostra/nascondi Ispettore Metadati'}
        aria-label={tr(lang, 'inspector')}
        onclick={onToggleInspector}
      >
        <svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <rect x="3" y="4" width="18" height="16" rx="2" />
          <path d="M15 4v16" />
          <path d="M17.5 8h1.5M17.5 12h1.5M17.5 16h1.5" />
        </svg>
      </button>

      {#if onOpenClassics}
        <button
          type="button"
          class="icon-btn tip-end"
          data-tip={lang === 'en'
            ? 'Suggested Classics (most-cited missing papers & books)'
            : 'Classici suggeriti (paper e libri più citati che mancano)'}
          aria-label={tr(lang, 'suggestedClassicsTitle')}
          onclick={onOpenClassics}
        >
          <svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20" />
            <path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z" />
            <path d="m12 6.5 1 2.2 2.2 1-2.2 1-1 2.2-1-2.2-2.2-1 2.2-1z" />
          </svg>
        </button>
      {/if}

      <button
        type="button"
        class="icon-btn tip-end"
        data-tip={updateAvailableVersion
          ? lang === 'en'
            ? `Settings (⌘,) · Update v${updateAvailableVersion} available`
            : `Impostazioni (⌘,) · Aggiornamento v${updateAvailableVersion} disponibile`
          : lang === 'en'
            ? 'Settings (⌘,)'
            : 'Impostazioni (⌘,)'}
        aria-label={tr(lang, 'settings')}
        onclick={() => (onOpenSettings ? onOpenSettings() : (showSettingsPopover = !showSettingsPopover))}
      >
        <svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
        </svg>
        {#if updateAvailableVersion}
          <span class="update-dot" aria-hidden="true"></span>
        {/if}
      </button>
    </div>
  </div>
</header>

{#if searchMode === 'metadata'}
  <div class="meta-filter-bar">
    <label>
      Tipo:
      <select
        value={effectiveWorkCat}
        onchange={(e) => {
          const val = (e.currentTarget as HTMLSelectElement).value as WorkCategory;
          if (onWorkCategoryChange) onWorkCategoryChange(val);
          else onMetaFilterChange({ ...metaFilter, work_category: val });
        }}
      >
        <option value="all">{tr(lang, 'filterAll')}</option>
        <option value="paper">{tr(lang, 'filterPapers')}</option>
        <option value="book">{tr(lang, 'filterBooks')}</option>
      </select>
    </label>
    <label>
      Controllo:
      <select
        value={effectiveVerifiedFilter}
        onchange={(e) => {
          const val = (e.currentTarget as HTMLSelectElement).value as VerifiedFilter;
          if (onVerifiedFilterChange) onVerifiedFilterChange(val);
          else onMetaFilterChange({ ...metaFilter, verified_filter: val });
        }}
      >
        <option value="all">{tr(lang, 'filterAll')}</option>
        <option value="unverified">{tr(lang, 'filterUnverified')}</option>
        <option value="verified">✓ {tr(lang, 'filterVerified')}</option>
      </select>
    </label>
    <label>
      Formato:
      <select
        value={metaFilter.format ?? ''}
        onchange={(e) =>
          onMetaFilterChange({ ...metaFilter, format: (e.currentTarget as HTMLSelectElement).value || null })}
      >
        <option value="">Tutti</option>
        <option value="pdf">PDF</option>
        <option value="djvu">DjVu</option>
      </select>
    </label>
    <label>
      MSC:
      <input
        type="text"
        placeholder="es. 14"
        value={metaFilter.msc_prefix ?? ''}
        oninput={(e) =>
          scheduleMetaFilterChange({ ...metaFilter, msc_prefix: (e.currentTarget as HTMLInputElement).value || null })}
      />
    </label>
    <label>
      Da anno:
      <input
        type="number"
        placeholder="1950"
        value={metaFilter.year_from ?? ''}
        oninput={(e) => {
          const v = (e.currentTarget as HTMLInputElement).value;
          scheduleMetaFilterChange({ ...metaFilter, year_from: v ? Number(v) : null });
        }}
      />
    </label>
    <label>
      A anno:
      <input
        type="number"
        placeholder="2026"
        value={metaFilter.year_to ?? ''}
        oninput={(e) => {
          const v = (e.currentTarget as HTMLInputElement).value;
          scheduleMetaFilterChange({ ...metaFilter, year_to: v ? Number(v) : null });
        }}
      />
    </label>
    <div class="meta-sort-group">
      <label>
        Ordina:
        <select
          value={effectiveSortBy}
          onchange={(e) => {
            const val = (e.currentTarget as HTMLSelectElement).value as SortField;
            if (onSortByChange) onSortByChange(val);
            else {
              const defDir: SortDirection = val === 'title' || val === 'author' ? 'asc' : 'desc';
              onMetaFilterChange({ ...metaFilter, sort_by: val, sort_dir: defDir });
            }
          }}
        >
          <option value="updated">{tr(lang, 'sortUpdated')}</option>
          <option value="title">{tr(lang, 'sortTitle')}</option>
          <option value="author">{tr(lang, 'sortAuthor')}</option>
          <option value="year">{tr(lang, 'sortYear')}</option>
        </select>
      </label>
      <button
        type="button"
        class="meta-sort-dir-btn"
        title={effectiveSortDir === 'asc' ? tr(lang, 'sortOrderAsc') : tr(lang, 'sortOrderDesc')}
        aria-label={effectiveSortDir === 'asc' ? tr(lang, 'sortOrderAsc') : tr(lang, 'sortOrderDesc')}
        onclick={() => {
          if (onToggleSortDir) onToggleSortDir();
          else {
            const nextDir: SortDirection = effectiveSortDir === 'asc' ? 'desc' : 'asc';
            onMetaFilterChange({ ...metaFilter, sort_dir: nextDir });
          }
        }}
      >
        {#if effectiveSortDir === 'asc'}
          ↑
        {:else}
          ↓
        {/if}
      </button>
    </div>
  </div>
{/if}

{#if scanStatusText}
  <div class="scan-banner">{scanStatusText}</div>
{/if}

{#if showSettingsPopover && settings}
  <div class="settings-popover">
    <div class="settings-section">
      <strong>Visualizzatore e lettori esterni</strong>
      <label class="row">
        <input
          type="checkbox"
          checked={settings.viewer_open_in_separate_window}
          onchange={(e) =>
            patchSettings({ viewer_open_in_separate_window: (e.currentTarget as HTMLInputElement).checked })}
        />
        Apri il visualizzatore interno in una finestra separata (invece che in schede)
      </label>

      <div class="preset-grid">
        <label>
          Lettore esterno PDF:
          <select
            value={settings.external_pdf_command ?? ''}
            onchange={(e) =>
              patchSettings({ external_pdf_command: (e.currentTarget as HTMLSelectElement).value.trim() || null })}
          >
            <option value="">Applicazione predefinita di sistema</option>
            {#each presets.filter((p) => p.formats.includes('pdf')) as p}
              <option value={p.template}>
                {p.label}{p.requires_accessibility ? ' (Accessibilità)' : ''}
              </option>
            {/each}
          </select>
        </label>

        <label>
          Lettore esterno DjVu:
          <select
            value={settings.external_djvu_command ?? ''}
            onchange={(e) =>
              patchSettings({ external_djvu_command: (e.currentTarget as HTMLSelectElement).value.trim() || null })}
          >
            <option value="">Applicazione predefinita di sistema</option>
            {#each presets.filter((p) => p.formats.includes('djvu')) as p}
              <option value={p.template}>{p.label}</option>
            {/each}
          </select>
        </label>
      </div>
    </div>

    <div class="settings-section">
      <strong>Metadati bibliografici online &amp; ToS</strong>
      <label class="row">
        <input
          type="checkbox"
          checked={!settings.network_lookups_enabled}
          onchange={(e) =>
            patchSettings({ network_lookups_enabled: !(e.currentTarget as HTMLInputElement).checked })}
        />
        Modalità 100% offline (disattiva tutte le richieste di rete)
      </label>

      <div class="source-toggles">
        <label class="row">
          <input
            type="checkbox"
            checked={settings.mrlookup_enabled_single}
            disabled={!settings.network_lookups_enabled}
            onchange={(e) =>
              patchSettings({ mrlookup_enabled_single: (e.currentTarget as HTMLInputElement).checked })}
          />
          MRLookup singolo record (on-demand utente)
        </label>
        <label class="row">
          <input
            type="checkbox"
            checked={settings.mrlookup_enabled_bulk}
            disabled={!settings.network_lookups_enabled}
            onchange={(e) =>
              patchSettings({ mrlookup_enabled_bulk: (e.currentTarget as HTMLInputElement).checked })}
          />
          MRLookup bulk in background (disattivo di default per policy AMS Content-Signal / ToS; max 1 req/5s)
        </label>
        <label class="row">
          <input
            type="checkbox"
            checked={settings.zbmath_enabled}
            disabled={!settings.network_lookups_enabled}
            onchange={(e) =>
              patchSettings({ zbmath_enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          zbMATH Open (CC-BY-SA 4.0)
        </label>
        <label class="row">
          <input
            type="checkbox"
            checked={settings.dblp_enabled}
            disabled={!settings.network_lookups_enabled}
            onchange={(e) =>
              patchSettings({ dblp_enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          DBLP Computer Science Bibliography (CC0 1.0)
        </label>
        <label class="row">
          <input
            type="checkbox"
            checked={settings.crossref_enabled}
            disabled={!settings.network_lookups_enabled}
            onchange={(e) =>
              patchSettings({ crossref_enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          Crossref API
        </label>
        <label class="row">
          <input
            type="checkbox"
            checked={settings.openalex_enabled}
            disabled={!settings.network_lookups_enabled}
            onchange={(e) =>
              patchSettings({ openalex_enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          OpenAlex (CC0 1.0)
        </label>
        <label class="row">
          <input
            type="checkbox"
            checked={settings.datacite_enabled}
            disabled={!settings.network_lookups_enabled}
            onchange={(e) =>
              patchSettings({ datacite_enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          DataCite (CC0 1.0 — arXiv / Zenodo / ETH / HAL)
        </label>
        <label class="row">
          <input
            type="checkbox"
            checked={settings.inspirehep_enabled}
            disabled={!settings.network_lookups_enabled}
            onchange={(e) =>
              patchSettings({ inspirehep_enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          INSPIRE-HEP (CC0 1.0 — Fisica Matematica &amp; HEP)
        </label>
        <label class="row">
          <input
            type="checkbox"
            checked={settings.arxiv_enabled}
            disabled={!settings.network_lookups_enabled}
            onchange={(e) =>
              patchSettings({ arxiv_enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          arXiv API
        </label>
        <label class="row">
          <input
            type="checkbox"
            checked={settings.semanticscholar_enabled}
            disabled={!settings.network_lookups_enabled}
            onchange={(e) =>
              patchSettings({ semanticscholar_enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          Semantic Scholar Graph API (CS / ML)
        </label>
        <label class="row">
          <input
            type="checkbox"
            checked={settings.europepmc_enabled}
            disabled={!settings.network_lookups_enabled}
            onchange={(e) =>
              patchSettings({ europepmc_enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          Europe PMC (Biomatematica &amp; Scienze della Vita)
        </label>
        <label class="row">
          <input
            type="checkbox"
            checked={settings.harvard_enabled}
            disabled={!settings.network_lookups_enabled}
            onchange={(e) =>
              patchSettings({ harvard_enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          Harvard LibraryCloud (MARC21 — editore, città, collana, pagine, ISBN)
        </label>
        <label class="row">
          <input
            type="checkbox"
            checked={settings.loc_enabled}
            disabled={!settings.network_lookups_enabled}
            onchange={(e) =>
              patchSettings({ loc_enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          Library of Congress SRU (libri e monografie accademiche)
        </label>
        <label class="row">
          <input
            type="checkbox"
            checked={settings.googlebooks_enabled}
            disabled={!settings.network_lookups_enabled}
            onchange={(e) =>
              patchSettings({ googlebooks_enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          Google Books (libri scientifici e commerciali)
        </label>
        <label class="row">
          <input
            type="checkbox"
            checked={settings.openlibrary_enabled}
            disabled={!settings.network_lookups_enabled}
            onchange={(e) =>
              patchSettings({ openlibrary_enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          Open Library (fallback finale libri, max 1 req/s con cache permanente)
        </label>
      </div>

      <div class="row email-row">
        <label class="row">
          Email opzionale (Polite Pool Crossref/OpenAlex/OpenLibrary, salvata nel Portachiavi OS):
          <input
            type="email"
            placeholder="nome@esempio.org (opzionale)"
            bind:value={emailDraft}
          />
        </label>
        <button
          type="button"
          class="primary"
          onclick={() => patchSettings({ polite_email: emailDraft.trim() || null })}
        >
          Salva email
        </button>
      </div>
    </div>

    <div class="settings-section">
      <strong>Ricerca Ibrida ONNX, Reranker &amp; Profili Modello (Hugging Face)</strong>
      {#if mlStatus}
        <div class="ml-status-line">
          <span>
            Runtime ONNX: <strong>{mlStatus.onnxruntime_found ? 'Attivo' : 'Non trovato (solo BM25)'}</strong>
          </span>
          <span>RAM processo: <strong>{mlStatus.rss_mb.toFixed(1)} MB</strong></span>
          <span>
            In RAM: Embedder={mlStatus.embedder_loaded ? 'sì' : 'no'} · Reranker={mlStatus.reranker_loaded ? 'sì' : 'no'}
          </span>
          {#if mlStatus.embedder_loaded || mlStatus.reranker_loaded}
            <button type="button" onclick={() => onUnloadMlModels()}>
              Scarica modelli da RAM ora
            </button>
          {/if}
        </div>

        <div class="ml-profiles-grid">
          {#each mlStatus.profiles as prof (prof.profile)}
            <div class="ml-profile-card" class:selected={settings.ml_profile === prof.profile}>
              <div class="row">
                <input
                  type="radio"
                  name="ml-profile"
                  checked={settings.ml_profile === prof.profile}
                  onchange={() => patchSettings({ ml_profile: prof.profile })}
                />
                <strong>{prof.label}</strong>
              </div>
              <div class="ml-meta">
                <span>Embedder: <code>{prof.embedder_repo}</code></span>
                <span>Reranker: <code>{prof.reranker_repo}</code></span>
              </div>
              <div class="row">
                {#if prof.ready}
                  <span class="ready-pill">Pronto offline ({formatMb(prof.total_bytes)})</span>
                {:else}
                  <button
                    type="button"
                    class="primary"
                    disabled={downloadingProfile !== null}
                    onclick={() => handleDownloadProfileClick(prof.profile as 'light' | 'quality')}
                  >
                    {downloadingProfile === prof.profile
                      ? 'Download in corso…'
                      : `Scarica ${prof.profile} (${formatMb(prof.total_bytes)})`}
                  </button>
                {/if}
              </div>
            </div>
          {/each}
        </div>
      {/if}

      {#if mlDownloadProgress && !mlDownloadProgress.finished}
        <div class="ml-progress">
          <div class="row">
            <span>{mlDownloadProgress.artifact_name}</span>
            <span>
              {formatMb(mlDownloadProgress.profile_bytes_done)} / {formatMb(mlDownloadProgress.profile_bytes_total)}
              ({(mlDownloadProgress.bytes_per_sec / (1024 * 1024)).toFixed(1)} MB/s)
            </span>
          </div>
          <progress
            value={mlDownloadProgress.profile_bytes_done}
            max={Math.max(1, mlDownloadProgress.profile_bytes_total)}
          ></progress>
        </div>
      {/if}

      {#if mlActionError}
        <div class="ml-error">{mlActionError}</div>
      {/if}

      <div class="preset-grid">
        <label class="row">
          <input
            type="checkbox"
            checked={settings.reranker_enabled}
            onchange={(e) =>
              patchSettings({ reranker_enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          Abilita Cross-Encoder Reranker (disattiva se su CPU molto lenta)
        </label>

        <label>
          Top-N candidati per ramo:
          <input
            type="number"
            min="10"
            max="200"
            style="width: 64px"
            value={settings.hybrid_top_n}
            onchange={(e) =>
              patchSettings({ hybrid_top_n: Math.max(10, Number((e.currentTarget as HTMLInputElement).value) || 50) })}
          />
        </label>

        <label>
          Rerank top-M (Light / Quality):
          <input
            type="number"
            min="5"
            max="100"
            style="width: 56px"
            value={settings.rerank_top_m_light}
            onchange={(e) =>
              patchSettings({ rerank_top_m_light: Math.max(5, Number((e.currentTarget as HTMLInputElement).value) || 25) })}
          />
          /
          <input
            type="number"
            min="4"
            max="60"
            style="width: 56px"
            value={settings.rerank_top_m_quality}
            onchange={(e) =>
              patchSettings({ rerank_top_m_quality: Math.max(4, Number((e.currentTarget as HTMLInputElement).value) || 12) })}
          />
        </label>

        <label>
          Max documenti (K, 0=tutti):
          <input
            type="number"
            min="0"
            max="200"
            style="width: 60px"
            value={settings.search_top_k_docs}
            onchange={(e) =>
              patchSettings({ search_top_k_docs: Math.max(0, Number((e.currentTarget as HTMLInputElement).value) || 0) })}
          />
        </label>

        <label>
          Aggregazione punteggio documento:
          <select
            value={settings.doc_score_aggregation}
            onchange={(e) =>
              patchSettings({ doc_score_aggregation: (e.currentTarget as HTMLSelectElement).value })}
          >
            <option value="max">Max (passaggio migliore)</option>
            <option value="top2_mean">Media dei top 2 passaggi</option>
          </select>
        </label>

        <label>
          Unload inattività RAM (s):
          <input
            type="number"
            min="30"
            max="3600"
            style="width: 64px"
            value={settings.model_idle_unload_secs}
            onchange={(e) =>
              patchSettings({ model_idle_unload_secs: Math.max(30, Number((e.currentTarget as HTMLInputElement).value) || 180) })}
          />
        </label>
      </div>
    </div>
  </div>
{/if}

<style>
  .topbar {
    position: relative;
    z-index: 40;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 12px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
    min-height: 46px;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 7px;
    white-space: nowrap;
    flex-shrink: 0;
  }
  .brand-logo {
    width: 22px;
    height: 22px;
    flex-shrink: 0;
    filter: drop-shadow(0 1px 1px rgba(0, 0, 0, 0.14));
  }
  .brand-name {
    font-size: 14px;
    letter-spacing: -0.01em;
  }
  .brand-version {
    font-size: 10.5px;
    font-weight: 500;
    color: var(--text-muted);
    opacity: 0.8;
    margin-left: -2px;
  }
  .roots-pill {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 11px;
    font-weight: 600;
    padding: 2px 7px;
    border-radius: 999px;
    background: var(--bg-muted);
    color: var(--text-muted);
    border: 1px solid var(--border);
  }
  .search-form {
    flex: 1 1 220px;
    min-width: 180px;
    max-width: 620px;
    display: flex;
    align-items: center;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 2px;
    gap: 2px;
    transition: border-color 0.15s, box-shadow 0.15s;
  }
  .search-form:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-soft);
  }
  .search-mode-select {
    border: none;
    background: var(--bg-muted);
    border-radius: 6px;
    padding: 4px 7px;
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    flex-shrink: 0;
  }
  .search-mode-select:focus {
    outline: none;
  }
  .search-scope-select {
    border: none;
    background: var(--bg-muted);
    border-radius: 6px;
    padding: 4px 6px;
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    flex-shrink: 0;
  }
  .search-scope-select.filtered {
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
  }
  .search-scope-select:focus {
    outline: none;
  }
  .search-input {
    flex: 1;
    min-width: 90px;
    border: none;
    background: transparent;
    padding: 4px 8px;
    font-size: 13px;
  }
  .search-input:focus {
    outline: none;
  }
  .search-clear-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border: none;
    border-radius: 5px;
    background: transparent;
    color: var(--text-muted);
    flex-shrink: 0;
  }
  .search-clear-btn:hover {
    background: var(--bg-muted);
    color: var(--text);
  }
  .search-submit-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border-radius: 6px;
    flex-shrink: 0;
  }
  .actions {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
    white-space: nowrap;
  }
  .embed-pill {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 3px 6px 3px 8px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--bg-muted);
    font-size: 11px;
    color: var(--text);
    white-space: nowrap;
  }
  .embed-pill.paused {
    border-color: rgba(217, 119, 6, 0.5);
    background: rgba(245, 158, 11, 0.14);
  }
  .embed-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
    animation: embed-pulse 1.5s ease-in-out infinite;
    flex-shrink: 0;
  }
  .embed-dot.paused {
    background: #d97706;
    animation: none;
  }
  @keyframes embed-pulse {
    0%, 100% {
      opacity: 1;
    }
    50% {
      opacity: 0.35;
    }
  }
  .embed-label {
    font-variant-numeric: tabular-nums;
    font-weight: 500;
  }
  .embed-ctrl-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--bg-panel);
    color: var(--text);
    cursor: pointer;
  }
  .embed-ctrl-btn:hover {
    border-color: var(--accent);
    color: var(--accent);
  }
  .btn-group {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }
  .v-divider {
    width: 1px;
    height: 20px;
    background: var(--border);
    margin: 0 2px;
    flex-shrink: 0;
  }
  .icon-btn {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 4px;
    min-width: 32px;
    height: 32px;
    padding: 0 7px;
    border-radius: 7px;
    background: var(--bg-muted);
    border: 1px solid var(--border);
    color: var(--text);
    flex-shrink: 0;
    transition: background 0.12s, border-color 0.12s, color 0.12s;
  }
  .icon-btn:hover:not(:disabled) {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .icon-btn.primary {
    background: var(--accent);
    color: #fff;
    border-color: transparent;
  }
  .icon-btn.primary:hover:not(:disabled) {
    filter: brightness(1.07);
    background: var(--accent);
  }
  .icon-btn.active-toggle {
    background: var(--accent-soft);
    color: var(--accent);
    border-color: var(--accent);
  }
  .icon-btn.danger-hover:hover:not(:disabled) {
    border-color: var(--danger);
    color: var(--danger);
    background: rgba(220, 38, 38, 0.1);
  }
  .btn-badge {
    font-size: 10.5px;
    font-weight: 700;
    line-height: 1;
    padding: 2px 5px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.24);
    color: inherit;
  }
  .icon-btn:not(.primary) .btn-badge {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .icon-btn:not(.primary) .btn-badge.warn {
    background: rgba(217, 119, 6, 0.18);
    color: var(--warn);
  }
  .icon-btn[data-tip]::after {
    content: attr(data-tip);
    position: absolute;
    top: calc(100% + 7px);
    left: 50%;
    transform: translateX(-50%) translateY(-2px);
    padding: 4px 8px;
    border-radius: 6px;
    background: #0f172a;
    color: #f8fafc;
    font-size: 11.5px;
    font-weight: 500;
    white-space: nowrap;
    pointer-events: none;
    opacity: 0;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.24);
    transition: opacity 0.14s ease, transform 0.14s ease;
    z-index: 90;
  }
  .icon-btn.tip-end[data-tip]::after {
    left: auto;
    right: 0;
    transform: translateY(-2px);
  }
  .icon-btn[data-tip]:hover::after {
    opacity: 1;
    transform: translateX(-50%) translateY(0);
  }
  .icon-btn.tip-end[data-tip]:hover::after {
    transform: translateY(0);
  }
  @keyframes spin360 {
    to {
      transform: rotate(360deg);
    }
  }
  .spin {
    animation: spin360 1s linear infinite;
  }
  .meta-filter-bar {
    display: flex;
    gap: 12px;
    align-items: center;
    padding: 6px 12px;
    background: var(--bg-muted);
    border-bottom: 1px solid var(--border);
    font-size: 12px;
    flex-wrap: wrap;
  }
  .meta-filter-bar input[type='number'],
  .meta-filter-bar input[type='text'] {
    width: 84px;
  }
  .meta-sort-group {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .meta-sort-dir-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
    font-size: 12px;
    font-weight: 700;
    cursor: pointer;
  }
  .meta-sort-dir-btn:hover {
    border-color: var(--accent);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .scan-banner {
    padding: 5px 12px;
    font-size: 12px;
    background: var(--accent-soft);
    border-bottom: 1px solid var(--border);
  }
  .settings-popover {
    padding: 10px 12px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 12px;
    font-size: 12px;
  }
  .settings-section {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .source-toggles {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(290px, 1fr));
    gap: 4px 12px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .email-row {
    flex-wrap: wrap;
  }
  .preset-grid {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
  }
  .preset-grid label {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .ml-status-line {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    align-items: center;
    padding: 4px 0;
  }
  .ml-profiles-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
    gap: 8px;
  }
  .ml-profile-card {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    background: var(--bg);
  }
  .ml-profile-card.selected {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .ml-meta {
    display: flex;
    flex-direction: column;
    font-size: 11px;
    color: var(--text-muted);
  }
  .ready-pill {
    font-size: 11px;
    padding: 2px 7px;
    border-radius: 999px;
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
  }
  .ml-progress {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .ml-progress progress {
    width: 100%;
    height: 8px;
  }
  .ml-error {
    color: #c62828;
    font-size: 12px;
  }
  .update-dot {
    position: absolute;
    top: 4px;
    right: 4px;
    width: 7px;
    height: 7px;
    border-radius: 999px;
    background: #16a34a;
    box-shadow: 0 0 0 1.5px var(--bg-panel);
  }
</style>
