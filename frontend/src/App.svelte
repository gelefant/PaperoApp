<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { open as openFileDialog, save as saveFileDialog } from '@tauri-apps/plugin-dialog';
  import { api } from './lib/api';
  import { normalizeLang, tr } from './lib/i18n';
  import type {
    BackupReport,
    BibtexSyncReport,
    CollectionView,
    ContentSearchDocResult,
    ContentSearchFilter,
    ContentSearchPassageHit,
    DocumentListItem,
    DocumentMetadataView,
    DownloadProgressEvent,
    DuplicateFileMember,
    DuplicateGroupView,
    EmbedProgressStatus,
    ExternalReaderPreset,
    HybridSearchStats,
    LlmAnswerResult,
    LlmCitation,
    LlmPromptPreview,
    LlmProvider,
    ManualMetadataEdit,
    MetadataSearchFilter,
    MlEnvironmentStatus,
    MscTreeNode,
    NormalizedRect,
    OcrEnvironmentStatus,
    ReadingStatus,
    SavedSearchView,
    ScanProgressEvent,
    SettingsTabId,
    SmartPasteApplyResult,
    SortDirection,
    SortField,
    SourceRoot,
    TagSummary,
    TopicClusterView,
    UiSettingsPayload,
    VerifiedFilter,
    ViewerTab,
    ViewerTargetPassage,
    WorkCategory,
  } from './lib/types';
  import TopBar from './lib/components/TopBar.svelte';
  import LeftSidebar from './lib/components/LeftSidebar.svelte';
  import DocumentList from './lib/components/DocumentList.svelte';
  import ContentResults from './lib/components/ContentResults.svelte';
  import ViewerTabs from './lib/components/ViewerTabs.svelte';
  import UnifiedViewer from './lib/components/UnifiedViewer.svelte';
  import ContextMenu, { type ContextTarget } from './lib/components/ContextMenu.svelte';
  import RightInspector from './lib/components/RightInspector.svelte';
  import DuplicatesView from './lib/components/DuplicatesView.svelte';
  import FirstRunWizard from './lib/components/FirstRunWizard.svelte';
  import SettingsModal from './lib/components/SettingsModal.svelte';

  // Check if launched as a detached viewer window (`?viewerFileId=...&page=...`)
  const urlParams = new URLSearchParams(window.location.search);
  const detachedFileId = urlParams.has('viewerFileId')
    ? Number(urlParams.get('viewerFileId'))
    : null;
  const detachedPage = Number(urlParams.get('page') ?? '1') || 1;
  function parseDetachedBboxes(): NormalizedRect[] {
    if (!urlParams.has('bboxes')) return [];
    try {
      return JSON.parse(urlParams.get('bboxes') ?? '[]');
    } catch {
      return [];
    }
  }
  const detachedBboxes = parseDetachedBboxes();

  let roots = $state.raw<SourceRoot[]>([]);
  let documents = $state.raw<DocumentListItem[]>([]);
  let allDocuments = $state.raw<DocumentListItem[]>([]);
  let query = $state('');
  let committedContentQuery = $state('');
  let searchMode = $state<'content' | 'metadata'>('content');
  let metaFilter = $state<MetadataSearchFilter>({
    work_category: 'all',
    verified_filter: 'all',
    sort_by: 'updated',
    sort_dir: 'desc',
  });
  let searchResults = $state.raw<ContentSearchDocResult[] | null>(null);
  let searchStats = $state<HybridSearchStats | null>(null);
  let mlStatus = $state<MlEnvironmentStatus | null>(null);
  let mlDownloadProgress = $state<DownloadProgressEvent | null>(null);
  let embedProgress = $state<EmbedProgressStatus | null>(null);
  let activeLeftView = $state<'library' | 'duplicates'>('library');
  let duplicateGroups = $state.raw<DuplicateGroupView[]>([]);
  let scanningDedup = $state(false);
  let scanning = $state(false);
  let scanStatusText = $state('');
  let sidebarCollapsed = $state(false);
  let mscTree = $state.raw<MscTreeNode[]>([]);
  let clusters = $state.raw<TopicClusterView[]>([]);
  let tags = $state.raw<TagSummary[]>([]);
  let collections = $state.raw<CollectionView[]>([]);
  let savedSearches = $state.raw<SavedSearchView[]>([]);
  let selectedDocIds = $state<number[]>([]);
  let knnBusy = $state(false);
  let clusteringBusy = $state(false);
  let settings = $state<UiSettingsPayload | null>(null);
  let presets = $state.raw<ExternalReaderPreset[]>([]);
  let tabs = $state<ViewerTab[]>([]);
  let activeTabId = $state<string | null>(null);
  let contextTarget = $state<ContextTarget | null>(null);
  let topBarRef = $state<{ focusSearch?: () => void } | null>(null);
  let inspectorRef = $state<{
    savePendingIfForDoc: (docId: number) => Promise<boolean>;
    hasUnsavedChanges: () => boolean;
    triggerSave: () => Promise<boolean>;
  } | null>(null);
  let pendingDocSwitch = $state<{ targetDocId: number; afterSwitch?: () => void } | null>(null);
  let settingsOpen = $state(false);
  let settingsInitialTab = $state<SettingsTabId | null>(null);
  let wizardOpen = $state(false);
  let unreachableBannerDismissed = $state(false);
  let unreachableRoots = $derived(roots.filter((r) => !r.is_reachable));
  let ocrStatus = $state<OcrEnvironmentStatus | null>(null);
  let llmPreview = $state<LlmPromptPreview | null>(null);
  let llmAnswer = $state<LlmAnswerResult | null>(null);
  let llmStreamText = $state('');
  let llmLoading = $state(false);
  let llmError = $state('');
  let cloudConsentModal = $state<{ provider: LlmProvider; preview: LlmPromptPreview } | null>(null);
  let backupBusy = $state(false);
  let backupDialogOpen = false;
  let backupNotice = $state('');
  let backupError = $state('');
  let lang = $derived(normalizeLang(settings?.ui_language));
  let unlistenFs: UnlistenFn | null = null;
  let unlistenBackup: UnlistenFn | null = null;
  let unlistenDuplicates: UnlistenFn | null = null;
  let unlistenEmbed: UnlistenFn | null = null;
  let unlistenBibtexSync: UnlistenFn | null = null;
  let appDestroyed = false;
  let fsRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  let docsReqSeq = 0;
  let searchReqSeq = 0;
  let inspectorReqSeq = 0;
  let singleFetchSeq = 0;

  $effect(() => {
    document.documentElement.dataset.theme = settings?.ui_theme ?? 'system';
  });

  const activeWorkCategory = $derived<WorkCategory>(
    metaFilter.work_category === 'paper' || metaFilter.work_category === 'book'
      ? metaFilter.work_category
      : 'all'
  );
  const activeVerifiedFilter = $derived<VerifiedFilter>(
    metaFilter.verified_filter === 'unverified' || metaFilter.verified_filter === 'verified'
      ? metaFilter.verified_filter
      : 'all'
  );
  const activeSortBy = $derived<SortField>(metaFilter.sort_by ?? 'updated');
  const activeSortDir = $derived<SortDirection>(
    metaFilter.sort_dir ??
      (activeSortBy === 'title' || activeSortBy === 'author' ? 'asc' : 'desc')
  );

  let selectedDocId = $state<number | null>(null);

  const catalogStats = $derived.by(() => {
    const byDocId = new Map<number, DocumentListItem>();
    const byFileId = new Map<number, DocumentListItem>();
    const verifiedDocIds = new Set<number>();
    let unverifiedCount = 0;
    let unresolvedCount = 0;

    for (const d of allDocuments) {
      byDocId.set(d.document_id, d);
      byFileId.set(d.file_id, d);
      if (d.metadata_verified) {
        verifiedDocIds.add(d.document_id);
      } else {
        unverifiedCount++;
        if (!d.metadata_source || d.metadata_source === 'heuristic') {
          unresolvedCount++;
        }
      }
    }

    return { byDocId, byFileId, verifiedDocIds, unverifiedCount, unresolvedCount };
  });

  const selectedFileId = $derived(
    selectedDocId !== null
      ? ((catalogStats.byDocId.get(selectedDocId) ??
          documents.find((d) => d.document_id === selectedDocId))?.file_id ?? null)
      : null
  );
  let inspectorOpen = $state(true);
  let selectedMeta = $state<DocumentMetadataView | null>(null);
  let loadingMeta = $state(false);
  let fetchingMeta = $state(false);
  let fetchingBulkMeta = $state(false);
  let fetchStatusMsg = $state('');

  const unverifiedDocsCount = $derived(catalogStats.unverifiedCount);

  const hasNextUnverified = $derived(
    documents.some((d) => d.document_id !== selectedDocId && !d.metadata_verified)
  );

  const displayedSearchResults = $derived.by(() => {
    if (searchResults === null) return null;
    if (activeVerifiedFilter === 'all') return searchResults;
    const verifiedSet = catalogStats.verifiedDocIds;
    return searchResults.filter((r) =>
      activeVerifiedFilter === 'verified'
        ? verifiedSet.has(r.document_id)
        : !verifiedSet.has(r.document_id)
    );
  });

  const unresolvedMetaCount = $derived(catalogStats.unresolvedCount);

  const pendingDuplicatesCount = $derived(
    duplicateGroups.filter((g) => g.status === 'pending' || g.status === 'info_merged').length
  );

  async function refreshDuplicates() {
    duplicateGroups = await api.listDuplicates();
  }

  async function refreshTaxonomy() {
    const [tree, cl, tg, cols, saved] = await Promise.all([
      api.getMscTree(),
      api.listTopicClusters(),
      api.listAllTags(),
      api.listCollections().catch(() => [] as CollectionView[]),
      api.listSavedSearches().catch(() => [] as SavedSearchView[]),
    ]);
    mscTree = tree;
    clusters = cl;
    tags = tg;
    collections = cols;
    savedSearches = saved;
  }

  function hasSidebarFilter() {
    return Boolean(
      metaFilter.msc_prefix ||
        metaFilter.cluster_id ||
        metaFilter.tag ||
        metaFilter.root_id ||
        metaFilter.collection_id ||
        metaFilter.reading_status ||
        metaFilter.starred_only ||
        metaFilter.recent_only ||
        metaFilter.author ||
        metaFilter.year_from !== null && metaFilter.year_from !== undefined ||
        metaFilter.year_to !== null && metaFilter.year_to !== undefined
    );
  }

  /** Builds ContentSearchFilter propagating Tag/Cluster/Collection/Author/Year/Root/Category/Reading/Starred — intentionally excluding MSC2020. */
  function buildActiveContentFilter(): ContentSearchFilter {
    return {
      root_ids: metaFilter.root_id ? [metaFilter.root_id] : null,
      tag: metaFilter.tag || null,
      cluster_id: metaFilter.cluster_id ?? null,
      collection_id: metaFilter.collection_id ?? null,
      author: metaFilter.author || null,
      year_from: metaFilter.year_from ?? null,
      year_to: metaFilter.year_to ?? null,
      work_category: activeWorkCategory !== 'all' ? activeWorkCategory : null,
      reading_status: metaFilter.reading_status || null,
      starred_only: metaFilter.starred_only ? true : null,
    };
  }

  const activeContentFilterChips = $derived.by(() => {
    const chips: string[] = [];
    if (metaFilter.tag) chips.push(`#${metaFilter.tag}`);
    if (metaFilter.cluster_id) {
      const cl = clusters.find((c) => c.id === metaFilter.cluster_id);
      chips.push(cl ? cl.label : `Cluster #${metaFilter.cluster_id}`);
    }
    if (metaFilter.collection_id) {
      const col = collections.find((c) => c.id === metaFilter.collection_id);
      chips.push(col ? col.name : `Collezione #${metaFilter.collection_id}`);
    }
    if (metaFilter.root_id) {
      const rt = roots.find((r) => r.id === metaFilter.root_id);
      chips.push(rt ? rt.label : 'Cartella');
    }
    if (metaFilter.author) chips.push(`Autore: ${metaFilter.author}`);
    if (metaFilter.year_from || metaFilter.year_to) {
      chips.push(`${metaFilter.year_from ?? '…'}–${metaFilter.year_to ?? '…'}`);
    }
    if (metaFilter.reading_status) {
      chips.push(
        metaFilter.reading_status === 'reading'
          ? 'In lettura'
          : metaFilter.reading_status === 'read'
            ? 'Letto'
            : 'Da leggere'
      );
    }
    if (metaFilter.starred_only) chips.push('★ Preferiti');
    return chips;
  });

  function hasListFilterOrCustomSort() {
    return Boolean(
      searchMode === 'metadata' ||
        hasSidebarFilter() ||
        activeWorkCategory !== 'all' ||
        activeVerifiedFilter !== 'all' ||
        activeSortBy !== 'updated' ||
        activeSortDir !== 'desc'
    );
  }

  async function refreshDocuments(reloadAll = false) {
    const seq = ++docsReqSeq;
    const needAll = reloadAll || allDocuments.length === 0;
    const filteredMode = hasListFilterOrCustomSort();

    if (filteredMode) {
      if (needAll) {
        const [filtered, all] = await Promise.all([
          api.searchMetadata({
            ...metaFilter,
            text: searchMode === 'metadata' ? query.trim() || null : null,
          }),
          api.listDocuments(),
        ]);
        if (seq !== docsReqSeq) return;
        documents = filtered;
        allDocuments = all;
      } else {
        const filtered = await api.searchMetadata({
          ...metaFilter,
          text: searchMode === 'metadata' ? query.trim() || null : null,
        });
        if (seq !== docsReqSeq) return;
        documents = filtered;
      }
    } else if (needAll) {
      const all = await api.listDocuments();
      if (seq !== docsReqSeq) return;
      documents = all;
      allDocuments = all;
    } else {
      documents = allDocuments;
    }
  }

  async function refreshLibrary() {
    const [rList] = await Promise.all([
      api.listRoots(),
      refreshDuplicates(),
      refreshTaxonomy(),
    ]);
    roots = rList;

    // Clear any sidebar filter whose root, MSC2020 class, tag, or cluster no longer has documents
    let filterChanged = false;
    const nextFilter: MetadataSearchFilter = { ...metaFilter };
    if (nextFilter.root_id && !rList.some((r) => r.id === nextFilter.root_id)) {
      nextFilter.root_id = null;
      filterChanged = true;
    }
    if (
      nextFilter.tag &&
      !tags.some((t) => t.name.toLowerCase() === nextFilter.tag!.toLowerCase())
    ) {
      nextFilter.tag = null;
      filterChanged = true;
    }
    if (nextFilter.cluster_id && !clusters.some((c) => c.id === nextFilter.cluster_id)) {
      nextFilter.cluster_id = null;
      filterChanged = true;
    }
    if (nextFilter.collection_id && !collections.some((c) => c.id === nextFilter.collection_id)) {
      nextFilter.collection_id = null;
      filterChanged = true;
    }
    if (nextFilter.msc_prefix) {
      const pfx = nextFilter.msc_prefix.toUpperCase();
      const mscStillPopulated = mscTree.some(
        (n) =>
          n.total_count > 0 &&
          (n.code.toUpperCase().startsWith(pfx) ||
            n.children.some((ch) => ch.total_count > 0 && ch.code.toUpperCase().startsWith(pfx)))
      );
      if (!mscStillPopulated) {
        nextFilter.msc_prefix = null;
        filterChanged = true;
      }
    }
    if (filterChanged) {
      metaFilter = nextFilter;
    }

    await refreshDocuments(true);
    void refreshEmbedProgress();
    const allLibraryDocs = allDocuments;

    // Close viewer tabs pointing to files that were removed
    if (tabs.length > 0) {
      const validFileIds = catalogStats.byFileId;
      const remainingTabs = tabs.filter((t) => validFileIds.has(t.fileId));
      if (remainingTabs.length !== tabs.length) {
        tabs = remainingTabs;
        if (!remainingTabs.some((t) => t.tabId === activeTabId)) {
          activeTabId = remainingTabs[0]?.tabId ?? null;
        }
      }
    }

    if (selectedDocId !== null) {
      if (catalogStats.byDocId.has(selectedDocId) || allLibraryDocs.some((d) => d.document_id === selectedDocId)) {
        await loadInspectorMetadata(selectedDocId);
      } else {
        selectedDocId = null;
        selectedMeta = null;
        fetchStatusMsg = '';
      }
    }
  }

  async function applySidebarFilter(patch: Partial<MetadataSearchFilter>) {
    activeLeftView = 'library';
    metaFilter = { ...metaFilter, ...patch };
    // MSC2020 is intentionally excluded from content search -> switch to metadata catalog
    if ('msc_prefix' in patch && patch.msc_prefix) {
      searchResults = null;
      searchStats = null;
      await refreshDocuments();
      return;
    }
    // Keep content search results open & filter them coherently when clicking Tag/Cluster/Collection/Folder!
    if (searchResults !== null && committedContentQuery.trim()) {
      await Promise.all([handleSearch(committedContentQuery), refreshDocuments()]);
      return;
    }
    await refreshDocuments();
  }

  async function clearSidebarFilters() {
    metaFilter = {
      ...metaFilter,
      msc_prefix: null,
      cluster_id: null,
      tag: null,
      root_id: null,
      collection_id: null,
      reading_status: null,
      starred_only: null,
      recent_only: null,
      author: null,
      year_from: null,
      year_to: null,
    };
    if (searchResults !== null && committedContentQuery.trim()) {
      await Promise.all([handleSearch(committedContentQuery), refreshDocuments()]);
      return;
    }
    await refreshDocuments();
  }

  async function handleSelectQuickView(view: 'all' | 'recent' | 'reading' | 'starred') {
    activeLeftView = 'library';
    if (view === 'all') {
      metaFilter = {
        ...metaFilter,
        collection_id: null,
        reading_status: null,
        starred_only: null,
        recent_only: null,
        sort_by: 'updated',
        sort_dir: 'desc',
      };
    } else if (view === 'recent') {
      searchResults = null;
      searchStats = null;
      metaFilter = {
        ...metaFilter,
        collection_id: null,
        reading_status: null,
        starred_only: null,
        recent_only: true,
        sort_by: 'last_opened',
        sort_dir: 'desc',
      };
      await refreshDocuments();
      return;
    } else if (view === 'reading') {
      metaFilter = {
        ...metaFilter,
        recent_only: null,
        starred_only: null,
        reading_status: metaFilter.reading_status === 'reading' ? null : 'reading',
      };
    } else if (view === 'starred') {
      metaFilter = {
        ...metaFilter,
        recent_only: null,
        reading_status: null,
        starred_only: !metaFilter.starred_only,
      };
    }
    if (searchResults !== null && committedContentQuery.trim()) {
      await Promise.all([handleSearch(committedContentQuery), refreshDocuments()]);
      return;
    }
    await refreshDocuments();
  }

  async function handleCreateCollection(name: string) {
    await api.createCollection(name);
    await refreshTaxonomy();
  }

  async function handleRenameCollection(collectionId: number, name: string) {
    await api.renameCollection(collectionId, name);
    await refreshTaxonomy();
  }

  async function handleDeleteCollection(collectionId: number) {
    await api.deleteCollection(collectionId);
    if (metaFilter.collection_id === collectionId) {
      metaFilter = { ...metaFilter, collection_id: null };
    }
    await Promise.all([refreshTaxonomy(), refreshDocuments()]);
  }

  async function handleReorderCollections(orderedIds: number[]) {
    await api.reorderCollections(orderedIds);
    await refreshTaxonomy();
  }

  async function handleAddDocToCollection(collectionId: number, docId: number) {
    await api.addDocsToCollection(collectionId, [docId]);
    await refreshTaxonomy();
    if (selectedDocId === docId) await loadInspectorMetadata(docId);
  }

  async function handleRemoveDocFromCollection(collectionId: number, docId: number) {
    await api.removeDocsFromCollection(collectionId, [docId]);
    await Promise.all([refreshTaxonomy(), refreshDocuments()]);
    if (selectedDocId === docId) await loadInspectorMetadata(docId);
  }

  async function handleSaveCurrentView(name: string) {
    const mode = searchResults !== null || searchMode === 'content' ? 'content' : 'metadata';
    const q = mode === 'content' ? committedContentQuery || query : query;
    await api.createSavedSearch(name, mode, q.trim(), JSON.stringify(metaFilter));
    await refreshTaxonomy();
  }

  async function handleApplySavedSearch(view: SavedSearchView) {
    activeLeftView = 'library';
    let parsed: MetadataSearchFilter = {
      work_category: 'all',
      verified_filter: 'all',
      sort_by: 'updated',
      sort_dir: 'desc',
    };
    try {
      const obj = JSON.parse(view.filter_json);
      if (obj && typeof obj === 'object') parsed = { ...parsed, ...obj };
    } catch {
      // Fallback to default filter
    }
    metaFilter = parsed;
    query = view.query_text;
    searchMode = view.search_mode === 'content' ? 'content' : 'metadata';
    if (searchMode === 'content' && view.query_text.trim()) {
      await handleSearch(view.query_text);
    } else {
      searchResults = null;
      searchStats = null;
      await refreshDocuments();
    }
  }

  async function handleDeleteSavedSearch(id: number) {
    await api.deleteSavedSearch(id);
    await refreshTaxonomy();
  }

  async function handleSetReadingStatus(docId: number, status: ReadingStatus) {
    await api.setDocumentReadingStatus(docId, status);
    const patch = (d: DocumentListItem) =>
      d.document_id === docId ? { ...d, reading_status: status } : d;
    allDocuments = allDocuments.map(patch);
    documents = documents.map(patch);
    if (selectedMeta && selectedMeta.document_id === docId) {
      selectedMeta = { ...selectedMeta, reading_status: status };
    }
    if (metaFilter.reading_status) {
      await refreshDocuments(false);
    }
  }

  async function handleToggleStarred(docId: number, starred: boolean) {
    await api.setDocumentStarred(docId, starred);
    const patch = (d: DocumentListItem) =>
      d.document_id === docId ? { ...d, is_starred: starred } : d;
    allDocuments = allDocuments.map(patch);
    documents = documents.map(patch);
    if (selectedMeta && selectedMeta.document_id === docId) {
      selectedMeta = { ...selectedMeta, is_starred: starred };
    }
    if (metaFilter.starred_only) {
      await refreshDocuments(false);
    }
  }

  async function handleCopyCiteCommand(docId: number): Promise<string> {
    const res = await api.getDocumentCiteKey(docId);
    try {
      await navigator.clipboard.writeText(res.cite_command);
      scanStatusText = `Copiato ${res.cite_command}`;
    } catch {
      // Ignore clipboard error
    }
    return res.cite_command;
  }

  async function handleApplySmartPaste(
    docId: number,
    rawInput: string
  ): Promise<SmartPasteApplyResult> {
    fetchingMeta = true;
    fetchStatusMsg = 'Riconoscimento identificatore / BibTeX…';
    try {
      const res = await api.applySmartPaste(docId, rawInput);
      selectedMeta = res.metadata;
      fetchStatusMsg =
        res.outcome?.status === 'updated'
          ? `${res.summary} → aggiornato da ${res.outcome.source ?? 'fonte esterna'}.`
          : res.outcome?.status === 'candidates_found'
            ? `${res.summary} → trovati ${res.outcome.candidates_count} candidati.`
            : res.summary;
      await Promise.all([refreshDocuments(true), refreshTaxonomy()]);
      return res;
    } finally {
      fetchingMeta = false;
    }
  }

  async function handleBatchAddTag(docIds: number[], tagName: string) {
    const n = await api.batchAddTag(docIds, tagName);
    scanStatusText = `Tag #${tagName} aggiunto a ${n} documenti.`;
    await Promise.all([refreshDocuments(true), refreshTaxonomy()]);
    if (selectedDocId !== null && docIds.includes(selectedDocId)) {
      await loadInspectorMetadata(selectedDocId);
    }
  }

  async function handleBatchRemoveTag(docIds: number[], tagName: string) {
    const n = await api.batchRemoveTag(docIds, tagName);
    scanStatusText = `Tag #${tagName} rimosso da ${n} documenti.`;
    await Promise.all([refreshDocuments(true), refreshTaxonomy()]);
    if (selectedDocId !== null && docIds.includes(selectedDocId)) {
      await loadInspectorMetadata(selectedDocId);
    }
  }

  async function handleBatchSetVerified(docIds: number[], verified: boolean) {
    const n = await api.batchSetVerified(docIds, verified);
    scanStatusText = verified
      ? `${n} documenti segnati come verificati ✓.`
      : `Verifica rimossa da ${n} documenti.`;
    await refreshDocuments(true);
    if (selectedDocId !== null && docIds.includes(selectedDocId)) {
      await loadInspectorMetadata(selectedDocId);
    }
  }

  async function handleBatchAddToCollection(collectionId: number, docIds: number[]) {
    const n = await api.addDocsToCollection(collectionId, docIds);
    const col = collections.find((c) => c.id === collectionId);
    scanStatusText = `Aggiunti ${n} documenti alla collezione «${col?.name ?? collectionId}».`;
    await refreshTaxonomy();
    if (selectedDocId !== null && docIds.includes(selectedDocId)) {
      await loadInspectorMetadata(selectedDocId);
    }
  }

  async function handleBatchExportBibtex(docIds: number[]) {
    const outPath = await saveFileDialog({
      defaultPath: 'papero_selezionati.bib',
      filters: [{ name: 'BibTeX', extensions: ['bib'] }],
    });
    if (!outPath) return;
    await api.exportBibtexMany(docIds, outPath);
    scanStatusText = `Esportati ${docIds.length} record BibTeX in ${outPath}`;
  }

  async function handleBatchExportCsv(docIds: number[]) {
    const outPath = await saveFileDialog({
      defaultPath: 'papero_selezionati.csv',
      filters: [{ name: 'CSV', extensions: ['csv'] }],
    });
    if (!outPath) return;
    await api.exportCsvMany(docIds, outPath);
    scanStatusText = `Esportati ${docIds.length} record CSV in ${outPath}`;
  }

  async function handleBatchEnqueueOcr(docIds: number[]) {
    const queued = await api.batchEnqueueOcr(docIds);
    if (queued === 0) {
      scanStatusText = 'Nessun file da accodare per OCR nella selezione.';
      return;
    }
    scanStatusText = `${queued} file accodati per OCR. Avvio elaborazione…`;
    await handleRunPendingOcr();
  }

  async function handleRunKnnProposals() {
    knnBusy = true;
    try {
      const rep = await api.runKnnMscProposals(7);
      scanStatusText = `k-NN MSC: ${rep.proposed_count} proposte create (su ${rep.unlabeled_docs} senza MSC, ${rep.labeled_docs} confermati).`;
      await Promise.all([refreshDocuments(true), refreshTaxonomy()]);
    } catch (e) {
      scanStatusText = `Errore k-NN MSC: ${String(e)}`;
    } finally {
      knnBusy = false;
    }
  }

  async function handleConfirmMsc(documentId: number, mscCode: string | null) {
    await api.confirmMscClassification(documentId, mscCode);
    await Promise.all([refreshDocuments(true), refreshTaxonomy()]);
    if (selectedDocId === documentId) {
      await loadInspectorMetadata(documentId);
    }
  }

  async function handleRejectMsc(documentId: number) {
    await api.rejectMscProposal(documentId);
    await Promise.all([refreshDocuments(true), refreshTaxonomy()]);
    if (selectedDocId === documentId) {
      await loadInspectorMetadata(documentId);
    }
  }

  async function handleRunClustering() {
    clusteringBusy = true;
    try {
      const rep = await api.runTopicClustering(null);
      scanStatusText = `Gruppi tematici: ${rep.clusters_created} cluster su ${rep.documents_clustered} documenti (silhouette ${rep.silhouette_score.toFixed(2)}).`;
      await Promise.all([refreshDocuments(true), refreshTaxonomy()]);
    } catch (e) {
      scanStatusText = `Errore clustering: ${String(e)}`;
    } finally {
      clusteringBusy = false;
    }
  }

  async function handleRenameCluster(clusterId: number, label: string) {
    await api.renameTopicCluster(clusterId, label);
    await refreshTaxonomy();
  }

  async function handleSetDocCluster(documentId: number, clusterId: number) {
    await api.setDocumentCluster(documentId, clusterId);
    await Promise.all([refreshDocuments(true), refreshTaxonomy()]);
  }

  async function handleRemoveDocCluster(documentId: number, clusterId: number) {
    await api.removeDocumentCluster(documentId, clusterId);
    await Promise.all([refreshDocuments(true), refreshTaxonomy()]);
  }

  async function handleAddDocTag(documentId: number, tagName: string) {
    await api.addDocumentTag(documentId, tagName);
    await Promise.all([refreshDocuments(true), refreshTaxonomy()]);
    if (selectedDocId === documentId) {
      await loadInspectorMetadata(documentId);
    }
  }

  async function handleRemoveDocTag(documentId: number, tagName: string) {
    await api.removeDocumentTag(documentId, tagName);
    await Promise.all([refreshDocuments(true), refreshTaxonomy()]);
    if (selectedDocId === documentId) {
      await loadInspectorMetadata(documentId);
    }
  }

  async function loadInspectorMetadata(docId: number) {
    const seq = ++inspectorReqSeq;
    selectedDocId = docId;
    loadingMeta = true;
    fetchStatusMsg = '';
    try {
      const meta = await api.getDocumentMetadata(docId);
      if (seq !== inspectorReqSeq) return;
      selectedMeta = meta;
    } finally {
      if (seq === inspectorReqSeq) {
        loadingMeta = false;
      }
    }
  }

  function requestSelectDocument(docId: number, afterSwitch?: () => void) {
    if (
      inspectorOpen &&
      selectedDocId !== null &&
      selectedDocId !== docId &&
      inspectorRef?.hasUnsavedChanges()
    ) {
      pendingDocSwitch = { targetDocId: docId, afterSwitch };
      return;
    }
    void loadInspectorMetadata(docId);
    afterSwitch?.();
  }

  async function resolvePendingDocSwitch(action: 'save' | 'discard' | 'cancel') {
    const pending = pendingDocSwitch;
    if (!pending) return;
    if (action === 'cancel') {
      pendingDocSwitch = null;
      return;
    }
    if (action === 'save') {
      const ok = await inspectorRef?.triggerSave();
      if (ok === false) {
        pendingDocSwitch = null;
        return;
      }
    }
    pendingDocSwitch = null;
    await loadInspectorMetadata(pending.targetDocId);
    pending.afterSwitch?.();
  }

  async function refreshEmbedProgress() {
    try {
      embedProgress = await api.getEmbedProgress();
    } catch {
      // Ignore transient startup race
    }
  }

  async function handleToggleEmbedPaused(paused: boolean) {
    try {
      embedProgress = await api.setEmbedPaused(paused);
    } catch (e) {
      scanStatusText = `Errore controllo embedding ML: ${String(e)}`;
    }
  }

  async function refreshMlStatus() {
    mlStatus = await api.listModelStatuses();
  }

  async function refreshOcrStatus() {
    ocrStatus = await api.getOcrStatus();
  }

  async function handleRunOcrForFile(fileId: number) {
    scanStatusText = `OCR in corso su file #${fileId}…`;
    try {
      const rep = await api.runOcrForFile(fileId, (ev) => {
        scanStatusText = `OCR file #${ev.file_id}: pagina ${ev.page_ord}/${ev.total_pages}…`;
      });
      scanStatusText = `OCR completato (${rep.languages_used}): ${rep.pages_processed} pagine, ${rep.passages_indexed} passaggi indicizzati.`;
      await refreshLibrary();
    } catch (e) {
      scanStatusText = `Errore OCR: ${e}`;
    }
  }

  async function handleRunPendingOcr() {
    scanStatusText = 'Avvio OCR su file scansionati senza livello di testo…';
    try {
      const reports = await api.runPendingOcr((ev) => {
        scanStatusText = `OCR file #${ev.file_id}: pagina ${ev.page_ord}/${ev.total_pages}…`;
      });
      scanStatusText =
        reports.length === 0
          ? 'Nessun file scansionato in attesa di OCR.'
          : `OCR batch completato su ${reports.length} file.`;
      await refreshLibrary();
    } catch (e) {
      scanStatusText = `Errore OCR: ${e}`;
    }
  }

  async function handleDownloadMlProfile(profile: 'light' | 'quality') {
    mlDownloadProgress = null;
    const updated = await api.downloadMlProfile(profile, (ev) => {
      mlDownloadProgress = ev;
    });
    mlStatus = updated;
    if (updated.profiles.find((p) => p.profile === profile)?.ready) {
      scanStatusText = `Modelli ${profile} scaricati e verificati (SHA-256). Indicizzazione vettoriale avviata in background.`;
      void refreshEmbedProgress();
    }
  }

  async function handleUnloadMlModels() {
    mlStatus = await api.unloadMlModels();
  }

  async function handleImportAltFormula(fileId: number, markdownText: string) {
    const rep = await api.importAltFormulaText(fileId, markdownText);
    scanStatusText = `Importati ${rep.passages_inserted} passaggi LaTeX in cache (${rep.pages_parsed} pagine).`;
  }

  onMount(async () => {
    if (detachedFileId !== null) return;
    const [cfg, pr, ml, ocr, emb] = await Promise.all([
      api.getUiSettings(),
      api.listExternalPresets(),
      api.listModelStatuses(),
      api.getOcrStatus(),
      api.getEmbedProgress().catch(() => null),
    ]);
    settings = cfg;
    presets = pr;
    mlStatus = ml;
    ocrStatus = ocr;
    embedProgress = emb;
    if (!cfg.first_run_completed) {
      wizardOpen = true;
    }
    await refreshLibrary();
    if (appDestroyed) return;
    const unFs = await listen('library-fs-updated', () => {
      if (appDestroyed) return;
      if (fsRefreshTimer) clearTimeout(fsRefreshTimer);
      fsRefreshTimer = setTimeout(() => {
        fsRefreshTimer = null;
        if (!appDestroyed) void refreshLibrary();
      }, 250);
    });
    if (appDestroyed) {
      unFs();
      return;
    }
    unlistenFs = unFs;

    const unBk = await listen<BackupReport>('papero://backup-updated', async (ev) => {
      if (appDestroyed) return;
      try {
        settings = await api.getUiSettings();
        const rep = ev.payload;
        const kb = Math.max(1, Math.round(rep.total_bytes / 1024));
        const msg =
          lang === 'en'
            ? `Backup updated (${rep.files_count} files, ${kb} KB) at ${rep.archive_path}`
            : `Backup aggiornato (${rep.files_count} file, ${kb} KB) in ${rep.archive_path}`;
        backupNotice = msg;
        backupError = '';
        scanStatusText = msg;
      } catch {
        // Ignore transient refresh race
      }
    });
    if (appDestroyed) {
      unBk();
      return;
    }
    unlistenBackup = unBk;

    const unDup = await listen('papero://duplicates-updated', () => {
      if (!appDestroyed) {
        void refreshDuplicates();
      }
    });
    if (appDestroyed) {
      unDup();
      return;
    }
    unlistenDuplicates = unDup;

    const unEmb = await listen<EmbedProgressStatus>('papero://embed-progress', (ev) => {
      if (!appDestroyed) {
        embedProgress = ev.payload;
      }
    });
    if (appDestroyed) {
      unEmb();
      return;
    }
    unlistenEmbed = unEmb;

    const unBib = await listen<BibtexSyncReport>('papero://bibtex-synced', (ev) => {
      if (!appDestroyed) {
        scanStatusText =
          lang === 'en'
            ? `Auto-synced ${ev.payload.count} BibTeX entries to ${ev.payload.path}`
            : `Sincronizzate ${ev.payload.count} voci BibTeX in ${ev.payload.path}`;
      }
    });
    if (appDestroyed) {
      unBib();
      return;
    }
    unlistenBibtexSync = unBib;
  });

  onDestroy(() => {
    appDestroyed = true;
    if (fsRefreshTimer) {
      clearTimeout(fsRefreshTimer);
      fsRefreshTimer = null;
    }
    unlistenFs?.();
    unlistenBackup?.();
    unlistenDuplicates?.();
    unlistenEmbed?.();
    unlistenBibtexSync?.();
  });

  async function handleAddFolder() {
    const selected = await openFileDialog({ directory: true, multiple: false });
    if (typeof selected === 'string' && selected) {
      await api.addSourceRoot(selected);
      await refreshLibrary();
    }
  }

  async function handleRescan() {
    scanning = true;
    scanStatusText = 'Avvio scansione cartelle…';
    try {
      await api.triggerRescan((ev: ScanProgressEvent) => {
        if (ev.stage === 'scan_done') {
          const s = ev.summary;
          scanStatusText = `Scansione: ${s.cnt_new} nuovi, ${s.cnt_unchanged} invariati, ${s.cnt_moved_renamed} spostati, ${s.cnt_modified} modificati, ${s.cnt_removed} rimossi. Estrazione testo…`;
        } else if (ev.stage === 'indexing_done') {
          const r = ev.report;
          scanStatusText = `Indicizzazione completata: ${r.succeeded} file (${r.passages_indexed} passaggi BM25), ${r.failed} errori.`;
        }
      });
      await refreshLibrary();
    } catch (e) {
      scanStatusText = `Errore scansione: ${e}`;
    } finally {
      scanning = false;
    }
  }

  async function refreshLlmPreview(): Promise<LlmPromptPreview | null> {
    if (
      !settings ||
      settings.llm_provider === 'disabled' ||
      !searchResults ||
      searchResults.length === 0 ||
      !query.trim()
    ) {
      llmPreview = null;
      return null;
    }
    try {
      const p = await api.previewLlmPrompt(query.trim(), searchResults);
      llmPreview = p;
      return p;
    } catch {
      llmPreview = null;
      return null;
    }
  }

  async function executeGenerateLlmAnswer() {
    const activeQ = (committedContentQuery || query).trim();
    if (!searchResults || searchResults.length === 0 || !activeQ) return;
    llmLoading = true;
    llmError = '';
    llmAnswer = null;
    llmStreamText = '';
    try {
      const ans = await api.generateLlmAnswerStream(activeQ, searchResults, (ev) => {
        if (ev.event === 'delta') {
          llmStreamText += ev.text;
        } else if (ev.event === 'done') {
          llmAnswer = ev.result;
        } else if (ev.event === 'error') {
          llmError = ev.message;
        }
      });
      llmAnswer = ans;
    } catch (e) {
      llmError = String(e);
    } finally {
      llmLoading = false;
    }
  }

  async function handleGenerateLlmAnswer() {
    if (!searchResults || searchResults.length === 0 || !query.trim()) return;
    llmError = '';
    const preview = await refreshLlmPreview();
    if (preview?.requires_cloud_consent) {
      cloudConsentModal = { provider: preview.provider, preview };
      return;
    }
    await executeGenerateLlmAnswer();
  }

  async function confirmAndRunCloudConsent() {
    const modal = cloudConsentModal;
    if (!modal) return;
    await api.acceptLlmCloudConsent(modal.provider);
    if (settings) {
      if (modal.provider === 'anthropic') {
        settings = { ...settings, llm_consent_anthropic: true };
      } else if (modal.provider === 'openai') {
        settings = { ...settings, llm_consent_openai: true };
      } else if (modal.provider === 'gemini') {
        settings = { ...settings, llm_consent_gemini: true };
      }
    }
    cloudConsentModal = null;
    await refreshLlmPreview();
    await executeGenerateLlmAnswer();
  }

  async function handleSearch(q: string) {
    const seq = ++searchReqSeq;
    llmAnswer = null;
    llmStreamText = '';
    llmError = '';
    llmPreview = null;
    cloudConsentModal = null;
    if (searchMode === 'metadata') {
      searchResults = null;
      searchStats = null;
      await refreshDocuments();
      return;
    }
    const trimmed = q.trim();
    if (!trimmed) {
      committedContentQuery = '';
      searchResults = null;
      searchStats = null;
      return;
    }
    activeLeftView = 'library';
    const contentFilter = buildActiveContentFilter();
    const resp = await api.searchContentHybrid(
      trimmed,
      contentFilter.root_ids ?? undefined,
      null,
      null,
      activeWorkCategory,
      contentFilter
    );
    if (seq !== searchReqSeq) return;
    committedContentQuery = trimmed;
    searchResults = resp.results;
    searchStats = resp.stats;
    await refreshLlmPreview();
  }

  async function handleSearchModeChange(mode: 'content' | 'metadata') {
    searchMode = mode;
    llmAnswer = null;
    llmError = '';
    llmPreview = null;
    cloudConsentModal = null;
    if (mode === 'metadata') {
      searchResults = null;
      searchStats = null;
      await refreshDocuments();
    } else if (!query.trim()) {
      searchResults = null;
      searchStats = null;
      await refreshDocuments();
    } else {
      await handleSearch(query);
    }
  }

  async function handleMetaFilterChange(next: MetadataSearchFilter) {
    metaFilter = next;
    await refreshDocuments();
  }

  async function handleWorkCategoryChange(cat: WorkCategory) {
    metaFilter = { ...metaFilter, work_category: cat };
    if (searchMode === 'content' && (searchResults !== null || query.trim())) {
      if (query.trim()) {
        await Promise.all([handleSearch(query), refreshDocuments()]);
      } else {
        searchResults = null;
        searchStats = null;
        await refreshDocuments();
      }
      return;
    }
    await refreshDocuments();
  }

  async function handleVerifiedFilterChange(vf: VerifiedFilter) {
    metaFilter = { ...metaFilter, verified_filter: vf };
    await refreshDocuments();
    if (
      vf === 'unverified' &&
      documents.length > 0 &&
      (selectedDocId === null || !documents.some((d) => d.document_id === selectedDocId))
    ) {
      await loadInspectorMetadata(documents[0].document_id);
    }
  }

  async function handleToggleDocVerified(
    documentId: number,
    verified: boolean,
    advanceNext = false
  ) {
    const prevList = documents;
    const prevIdx = prevList.findIndex((d) => d.document_id === documentId);

    if (verified && fetchingMeta && selectedDocId === documentId) {
      singleFetchSeq++;
      fetchingMeta = false;
      fetchStatusMsg = '';
      void api.cancelSingleMetadata();
    }

    if (verified && inspectorOpen && selectedDocId === documentId) {
      const saved = await inspectorRef?.savePendingIfForDoc(documentId);
      if (saved === false) return;
    }

    const updated = await api.setDocumentMetadataVerified(documentId, verified);
    if (selectedDocId === documentId) {
      selectedMeta = updated;
    }
    const patchDoc = (d: DocumentListItem): DocumentListItem =>
      d.document_id === documentId
        ? {
            ...d,
            title: updated.title,
            year: updated.year,
            work_type: updated.work_type,
            metadata_source: updated.metadata_source,
            metadata_verified: verified,
          }
        : d;
    allDocuments = allDocuments.map(patchDoc);
    documents = documents.map(patchDoc);

    const shouldAdvance =
      verified &&
      (advanceNext || (activeVerifiedFilter === 'unverified' && selectedDocId === documentId));
    if (shouldAdvance) {
      await refreshDocuments(false);
      let nextDoc: DocumentListItem | undefined;
      if (activeVerifiedFilter === 'unverified') {
        if (documents.length > 0) {
          const targetIdx = prevIdx >= 0 ? Math.min(prevIdx, documents.length - 1) : 0;
          nextDoc = documents[targetIdx];
        }
      } else if (prevIdx >= 0) {
        nextDoc =
          documents.slice(prevIdx + 1).find((d) => !d.metadata_verified) ??
          documents.find((d) => !d.metadata_verified && d.document_id !== documentId);
      }
      if (nextDoc) {
        inspectorOpen = true;
        await loadInspectorMetadata(nextDoc.document_id);
      } else if (activeVerifiedFilter === 'unverified' && documents.length === 0) {
        selectedDocId = null;
        selectedMeta = null;
        fetchStatusMsg = '';
      }
    } else {
      void refreshDocuments(false);
    }
  }

  async function handleSortByChange(sortBy: SortField) {
    const defaultDir: SortDirection = sortBy === 'title' || sortBy === 'author' ? 'asc' : 'desc';
    metaFilter = { ...metaFilter, sort_by: sortBy, sort_dir: defaultDir };
    await refreshDocuments();
  }

  async function handleToggleSortDir() {
    const nextDir: SortDirection = activeSortDir === 'asc' ? 'desc' : 'asc';
    metaFilter = { ...metaFilter, sort_dir: nextDir };
    await refreshDocuments();
  }

  async function openInViewer(
    fileId: number,
    title: string,
    format: string,
    pageOrd = 1,
    highlightRects: NormalizedRect[] = [],
    matches: ViewerTargetPassage[] = [],
    activeMatchIndex = 0,
    explicitPageJump = false
  ) {
    if (settings?.viewer_open_in_separate_window) {
      await api.openDetachedViewerWindow(
        fileId,
        pageOrd,
        title,
        highlightRects.length ? JSON.stringify(highlightRects) : null
      );
      return;
    }

    const existing = tabs.find((t) => t.fileId === fileId);
    if (existing) {
      tabs = tabs.map((t) =>
        t.tabId === existing.tabId
          ? {
              ...t,
              targetPage: pageOrd,
              explicitPageJump: explicitPageJump || highlightRects.length > 0 || pageOrd > 1,
              highlightRects,
              matches: matches.length ? matches : t.matches,
              activeMatchIndex,
            }
          : t
      );
      activeTabId = existing.tabId;
      return;
    }

    const tabId = `tab-${fileId}`;
    tabs = [
      ...tabs,
      {
        tabId,
        fileId,
        title,
        format,
        targetPage: pageOrd,
        explicitPageJump: explicitPageJump || highlightRects.length > 0 || pageOrd > 1,
        highlightRects,
        matches,
        activeMatchIndex,
      },
    ];
    activeTabId = tabId;
  }

  function handleOpenPassage(doc: ContentSearchDocResult, pass: ContentSearchPassageHit) {
    const matches: ViewerTargetPassage[] = doc.passages.map((p) => ({
      page_ord: p.page_ord,
      printed_label: p.printed_label,
      bboxes: p.bboxes,
      text_content: p.text_content,
    }));
    const idx = doc.passages.findIndex((p) => p.passage_id === pass.passage_id);
    requestSelectDocument(doc.document_id, () => {
      openInViewer(
        pass.file_id,
        doc.title,
        doc.format,
        pass.page_ord,
        pass.bboxes,
        matches,
        Math.max(0, idx),
        true
      );
    });
  }

  function handleOpenCitation(c: LlmCitation) {
    const matchingDoc = searchResults?.find((d) => d.document_id === c.document_id);
    const matches: ViewerTargetPassage[] = matchingDoc
      ? matchingDoc.passages.map((p) => ({
          page_ord: p.page_ord,
          printed_label: p.printed_label,
          bboxes: p.bboxes,
          text_content: p.text_content,
        }))
      : [
          {
            page_ord: c.page_ord,
            printed_label: c.printed_label,
            bboxes: c.bboxes,
            text_content: c.snippet,
          },
        ];
    const idx = matches.findIndex(
      (m) => m.page_ord === c.page_ord && m.bboxes.length === c.bboxes.length
    );
    requestSelectDocument(c.document_id, () => {
      openInViewer(
        c.file_id,
        c.doc_title,
        matchingDoc?.format ?? 'pdf',
        c.page_ord,
        c.bboxes,
        matches,
        Math.max(0, idx),
        true
      );
    });
  }

  function handleSelectMatchIndex(tabId: string, idx: number) {
    tabs = tabs.map((t) => {
      if (t.tabId !== tabId || !t.matches[idx]) return t;
      const m = t.matches[idx];
      return {
        ...t,
        activeMatchIndex: idx,
        targetPage: m.page_ord,
        highlightRects: m.bboxes,
      };
    });
  }

  async function handleCopyPath(fileId: number) {
    const info = await api.getViewerDoc(fileId);
    await navigator.clipboard.writeText(info.abs_path);
  }

  async function handleCopyBibtex(docId: number): Promise<string> {
    const bib = await api.getDocumentBibtex(docId);
    try {
      await navigator.clipboard.writeText(bib);
      scanStatusText = `BibTeX copiato negli appunti (${docId})`;
    } catch {
      // Ignored if clipboard permission unavailable
    }
    return bib;
  }

  async function handleSaveSettings(next: UiSettingsPayload) {
    settings = next;
    await api.saveUiSettings(next);
    await Promise.all([refreshLlmPreview(), refreshEmbedProgress()]);
  }

  async function handleToggleRoot(rootId: string, enabled: boolean) {
    await api.toggleSourceRoot(rootId, enabled);
    await refreshLibrary();
    await refreshLlmPreview();
  }

  async function handleRebindRoot(rootId: string) {
    const targetRoot = roots.find((r) => r.id === rootId);
    const selected = await openFileDialog({
      directory: true,
      multiple: false,
      title:
        lang === 'en'
          ? `Select local folder for "${targetRoot?.label ?? 'Library'}"`
          : `Seleziona cartella locale per "${targetRoot?.label ?? 'Libreria'}"`,
    });
    if (typeof selected !== 'string' || !selected) return;
    try {
      const updated = await api.rebindSourceRoot(rootId, selected);
      unreachableBannerDismissed = false;
      await refreshLibrary();
      scanStatusText =
        lang === 'en'
          ? `Folder "${updated.label}" relocated to ${updated.root_path}`
          : `Cartella "${updated.label}" ricollegata in ${updated.root_path}`;
    } catch (e) {
      scanStatusText = `Errore ricollegamento cartella: ${String(e)}`;
    }
  }

  async function handleSetRootCloudExclusion(rootId: string, excluded: boolean) {
    await api.setRootCloudExclusion(rootId, excluded);
    await refreshLibrary();
    await refreshLlmPreview();
  }

  async function handleDeleteRoot(rootId: string) {
    await api.deleteSourceRoot(rootId);
    await refreshLibrary();
    if (searchResults !== null) {
      if (query.trim() && roots.length > 0) {
        await handleSearch(query);
      } else {
        searchResults = null;
        searchStats = null;
        llmAnswer = null;
        llmPreview = null;
        llmError = '';
      }
    }
  }

  function formatBackupSuccessMsg(rep: BackupReport, created: boolean): string {
    const sizeText =
      rep.total_bytes >= 1024 * 1024
        ? `${(rep.total_bytes / (1024 * 1024)).toFixed(1)} MB`
        : `${Math.max(1, Math.round(rep.total_bytes / 1024))} KB`;
    if (lang === 'en') {
      return `${created ? 'Backup created' : 'Backup updated'} (${rep.files_count} files, ${sizeText}) at ${rep.archive_path}`;
    }
    return `${created ? 'Backup creato' : 'Backup aggiornato'} (${rep.files_count} file, ${sizeText}) in ${rep.archive_path}`;
  }

  async function runBackupAction(action: () => Promise<void>) {
    backupBusy = true;
    backupError = '';
    try {
      await action();
    } catch (e) {
      const msg = String(e);
      backupError = msg;
      scanStatusText = msg;
    } finally {
      backupBusy = false;
    }
  }

  async function pickBackupSavePath(): Promise<string | null> {
    const filters = [{ name: 'Papero Backup', extensions: ['papero-backup'] }];
    const preferred = settings?.backup_path?.trim();
    if (preferred) {
      try {
        return await saveFileDialog({ defaultPath: preferred, filters });
      } catch {
        // Fallback to default filename if stored backup path parent no longer exists
      }
    }
    return await saveFileDialog({
      defaultPath: 'papero-library.papero-backup',
      filters,
    });
  }

  async function handleCreateBackup() {
    if (backupBusy || backupDialogOpen) return;
    backupError = '';
    backupDialogOpen = true;
    let outPath: string | null = null;
    try {
      outPath = await pickBackupSavePath();
    } catch (e) {
      const msg = String(e);
      backupError = msg;
      scanStatusText = msg;
      return;
    } finally {
      backupDialogOpen = false;
    }
    if (!outPath) return;
    await runBackupAction(async () => {
      const rep = await api.createBackup(outPath);
      settings = await api.getUiSettings();
      const msg = formatBackupSuccessMsg(rep, true);
      backupNotice = msg;
      scanStatusText = msg;
    });
  }

  async function handleUpdateBackupNow() {
    if (backupBusy || backupDialogOpen) return;
    await runBackupAction(async () => {
      const rep = await api.updateBackupNow();
      settings = await api.getUiSettings();
      const msg = formatBackupSuccessMsg(rep, false);
      backupNotice = msg;
      scanStatusText = msg;
    });
  }

  async function handleRestoreBackup() {
    if (backupBusy || backupDialogOpen) return;
    backupError = '';
    backupDialogOpen = true;
    let selected: string | string[] | null = null;
    try {
      selected = await openFileDialog({
        multiple: false,
        filters: [{ name: 'Papero Backup', extensions: ['papero-backup'] }],
      });
    } catch (e) {
      const msg = String(e);
      backupError = msg;
      scanStatusText = msg;
      return;
    } finally {
      backupDialogOpen = false;
    }
    if (typeof selected !== 'string' || !selected) return;
    backupBusy = true;
    backupError = '';
    try {
      const rep = await api.restoreBackup(selected);
      const msg =
        lang === 'en'
          ? `Backup verified (${rep.files_restored} files). Restarting Papero to complete restore…`
          : `Backup verificato (${rep.files_restored} file). Riavvio di Papero per completare il ripristino…`;
      backupNotice = msg;
      scanStatusText = msg;
    } catch (e) {
      const msg = String(e);
      backupError = msg;
      scanStatusText = msg;
      backupBusy = false;
    }
  }

  function handleGlobalKeydown(e: KeyboardEvent) {
    if (detachedFileId !== null) return;
    const mod = e.metaKey || e.ctrlKey;
    const target = e.target as HTMLElement | null;
    const inEditable =
      target !== null &&
      (target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.tagName === 'SELECT' ||
        target.isContentEditable);

    if (e.key === 'Escape') {
      if (pendingDocSwitch) pendingDocSwitch = null;
      else if (cloudConsentModal) cloudConsentModal = null;
      else if (contextTarget) contextTarget = null;
      else if (settingsOpen) settingsOpen = false;
      else if (wizardOpen) wizardOpen = false;
      return;
    }

    if (mod && e.key.toLowerCase() === 's') {
      if (inspectorOpen && selectedDocId !== null && inspectorRef && !settingsOpen && !wizardOpen) {
        e.preventDefault();
        void inspectorRef.triggerSave();
      }
      return;
    }

    if ((e.key === '/' && !mod && !inEditable) || (mod && e.key.toLowerCase() === 'k')) {
      e.preventDefault();
      topBarRef?.focusSearch?.();
      return;
    }

    if (mod && e.key.toLowerCase() === 'r') {
      e.preventDefault();
      if (!scanning && roots.length > 0) {
        handleRescan();
      }
      return;
    }

    if (mod && e.key.toLowerCase() === 'b') {
      e.preventDefault();
      sidebarCollapsed = !sidebarCollapsed;
      return;
    }

    if (mod && e.key === ',') {
      e.preventDefault();
      settingsOpen = true;
    }
  }

  function handleCancelSingleMetadata() {
    singleFetchSeq++;
    fetchingMeta = false;
    fetchStatusMsg = 'Ricerca online interrotta.';
    void api.cancelSingleMetadata();
  }

  async function handleFlushDocMetadata(edit: ManualMetadataEdit, lockedFields?: string[]) {
    await api.saveDocumentMetadata(edit, lockedFields);
  }

  async function handleSaveDocMetadata(edit: ManualMetadataEdit, lockedFields?: string[]) {
    if (fetchingMeta) {
      singleFetchSeq++;
      fetchingMeta = false;
      void api.cancelSingleMetadata();
    }
    const updated = await api.saveDocumentMetadata(edit, lockedFields);
    selectedMeta = updated;
    const patchDoc = (d: DocumentListItem): DocumentListItem =>
      d.document_id === updated.document_id
        ? {
            ...d,
            title: updated.title,
            year: updated.year,
            work_type: updated.work_type,
            metadata_source: updated.metadata_source,
            metadata_verified: updated.metadata_verified,
          }
        : d;
    allDocuments = allDocuments.map(patchDoc);
    documents = documents.map(patchDoc);
    void Promise.all([refreshDocuments(true), refreshTaxonomy(), refreshLlmPreview()]);
  }

  const SOURCE_SHORT_NAMES: Record<string, string> = {
    mrlookup: 'MRLookup',
    zbmath: 'zbMATH',
    crossref: 'Crossref',
    dblp: 'DBLP',
    openalex: 'OpenAlex',
    inspirehep: 'INSPIRE',
    arxiv: 'arXiv',
    datacite: 'DataCite',
    semanticscholar: 'S2',
    europepmc: 'Europe PMC',
    harvard: 'Harvard',
    loc: 'LoC',
    googlebooks: 'Google Books',
    openlibrary: 'Open Library',
  };

  function formatActivePriorityChain(cfg: UiSettingsPayload | null): string {
    const order =
      cfg?.metadata_source_priority && cfg.metadata_source_priority.length > 0
        ? cfg.metadata_source_priority
        : [
            'mrlookup',
            'zbmath',
            'crossref',
            'dblp',
            'openalex',
            'inspirehep',
            'arxiv',
            'datacite',
            'semanticscholar',
            'europepmc',
            'harvard',
            'loc',
            'googlebooks',
            'openlibrary',
          ];
    const active = order.filter((id) => {
      if (!cfg) return true;
      if (id === 'mrlookup') return cfg.mrlookup_enabled_single;
      if (id === 'zbmath') return cfg.zbmath_enabled;
      if (id === 'crossref') return cfg.crossref_enabled;
      if (id === 'dblp') return cfg.dblp_enabled;
      if (id === 'openalex') return cfg.openalex_enabled;
      if (id === 'inspirehep') return cfg.inspirehep_enabled;
      if (id === 'arxiv') return cfg.arxiv_enabled;
      if (id === 'datacite') return cfg.datacite_enabled;
      if (id === 'semanticscholar') return cfg.semanticscholar_enabled;
      if (id === 'europepmc') return cfg.europepmc_enabled;
      if (id === 'harvard') return cfg.harvard_enabled;
      if (id === 'loc') return cfg.loc_enabled;
      if (id === 'googlebooks') return cfg.googlebooks_enabled;
      if (id === 'openlibrary') return cfg.openlibrary_enabled;
      return false;
    });
    const labels = active.map((id) => SOURCE_SHORT_NAMES[id] ?? id);
    if (labels.length <= 5) return labels.join(' → ');
    return `${labels.slice(0, 5).join(' → ')} → …`;
  }

  async function handleSaveAndFetchDocMetadata(
    edit: ManualMetadataEdit,
    lockedFields: string[],
    overwriteQueryFields = false
  ) {
    const reqSeq = ++singleFetchSeq;
    fetchingMeta = true;
    fetchStatusMsg = `Ricerca guidata secondo priorità (${formatActivePriorityChain(settings)})…`;
    try {
      const res = await api.saveAndFetchDocumentMetadata(
        edit,
        lockedFields,
        true,
        overwriteQueryFields
      );
      if (reqSeq !== singleFetchSeq) return;
      selectedMeta = res.metadata;
      const outcome = res.outcome;
      if (outcome.status === 'cancelled') {
        fetchStatusMsg = 'Ricerca online interrotta.';
        return outcome;
      }
      if (outcome.status === 'updated') {
        fetchStatusMsg = overwriteQueryFields
          ? `Metadati (inclusi titolo e autori ufficiali) aggiornati da ${outcome.source ?? 'fonte esterna'}.`
          : `Metadati completati da ${outcome.source ?? 'fonte esterna'} (campi bloccati preservati).`;
      } else if (outcome.status === 'verified_locked') {
        fetchStatusMsg = 'Metadati confermati e protetti: rimuovi la spunta ✓ prima di avviare una ricerca online.';
      } else if (outcome.status === 'candidates_found') {
        fetchStatusMsg = `Trovati ${outcome.candidates_count} candidati da confermare qui sotto.`;
      } else if (outcome.status === 'offline_only') {
        fetchStatusMsg = 'Modalità offline attiva: modifiche salvate localmente, nessuna richiesta di rete inviata.';
      } else {
        fetchStatusMsg = 'Nessuna corrispondenza trovata con i campi inseriti. Prova ad aggiungere o precisare Titolo/Autore.';
      }
      void Promise.all([refreshDocuments(true), refreshTaxonomy(), refreshLlmPreview()]);
      return outcome;
    } catch (e) {
      if (reqSeq === singleFetchSeq) {
        fetchStatusMsg = `Errore ricerca guidata metadati: ${e}`;
      }
    } finally {
      if (reqSeq === singleFetchSeq) {
        fetchingMeta = false;
      }
    }
  }

  async function handleFetchDocMetadata(documentId: number) {
    const reqSeq = ++singleFetchSeq;
    fetchingMeta = true;
    fetchStatusMsg = `Ricerca metadati secondo priorità (${formatActivePriorityChain(settings)})…`;
    try {
      const outcome = await api.fetchDocumentMetadata(documentId);
      if (reqSeq !== singleFetchSeq) return;
      if (outcome.status === 'cancelled') {
        fetchStatusMsg = 'Ricerca online interrotta.';
        return outcome;
      }
      let msg = '';
      if (outcome.status === 'updated') {
        msg = `Metadati aggiornati da ${outcome.source ?? 'fonte esterna'}.`;
      } else if (outcome.status === 'verified_locked') {
        msg = 'Metadati confermati e protetti: rimuovi la spunta ✓ prima di avviare una ricerca automatica.';
      } else if (outcome.status === 'candidates_found') {
        msg = `Trovati ${outcome.candidates_count} candidati ambigui da confermare.`;
      } else if (outcome.status === 'offline_only') {
        msg = 'Modalità offline attiva: nessuna richiesta di rete inviata.';
      } else {
        msg = 'Nessun risultato migliore trovato nelle fonti abilitate.';
      }
      const latestMeta = await api.getDocumentMetadata(documentId);
      if (reqSeq !== singleFetchSeq) return;
      selectedMeta = latestMeta;
      fetchStatusMsg = msg;
      void Promise.all([refreshDocuments(true), refreshTaxonomy()]);
      return outcome;
    } catch (e) {
      if (reqSeq === singleFetchSeq) {
        fetchStatusMsg = `Errore ricerca metadati: ${e}`;
      }
    } finally {
      if (reqSeq === singleFetchSeq) {
        fetchingMeta = false;
      }
    }
  }

  async function handleFetchAllMetadata() {
    if (fetchingBulkMeta) return;
    fetchingBulkMeta = true;
    let lastRefreshUpdated = 0;
    let lastRefreshAt = 0;
    let refreshInFlight = false;
    scanStatusText =
      lang === 'en'
        ? 'Starting bulk bibliographic metadata lookup…'
        : 'Avvio scansione metadati bibliografici di tutti i documenti…';
    try {
      const summary = await api.fetchAllMetadata((ev) => {
        if (!ev.finished) {
          scanStatusText =
            lang === 'en'
              ? `Metadata (${ev.current}/${ev.total}): "${ev.current_title}" — ${ev.updated} updated, ${ev.candidates} pending review…`
              : `Metadati (${ev.current}/${ev.total}): «${ev.current_title}» — ${ev.updated} aggiornati, ${ev.candidates} da confermare…`;
          const now = Date.now();
          if (
            !refreshInFlight &&
            ev.updated - lastRefreshUpdated >= 5 &&
            now - lastRefreshAt >= 2500
          ) {
            lastRefreshUpdated = ev.updated;
            lastRefreshAt = now;
            refreshInFlight = true;
            void refreshDocuments(true).finally(() => {
              refreshInFlight = false;
            });
          }
        }
      }, true);
      await refreshLibrary();
      if (summary.cancelled) {
        scanStatusText =
          lang === 'en'
            ? `Metadata scan stopped (${summary.current}/${summary.total}): ${summary.updated} updated, ${summary.candidates} with candidates.`
            : `Scansione metadati interrotta (${summary.current}/${summary.total}): ${summary.updated} aggiornati, ${summary.candidates} con candidati da confermare.`;
      } else {
        scanStatusText =
          lang === 'en'
            ? `Metadata scan completed (${summary.total} docs): ${summary.updated} updated, ${summary.candidates} with candidates, ${summary.unchanged} unchanged${summary.failed > 0 ? `, ${summary.failed} errors` : ''}.`
            : `Metadati completati (${summary.total} doc): ${summary.updated} aggiornati automaticamente, ${summary.candidates} con candidati da confermare, ${summary.unchanged} invariati${summary.failed > 0 ? `, ${summary.failed} errori` : ''}.`;
      }
    } catch (e) {
      scanStatusText = `Errore scansione metadati: ${e}`;
    } finally {
      fetchingBulkMeta = false;
    }
  }

  async function handleCancelBulkMetadata() {
    await api.cancelMetadataBatch();
  }

  async function handleConfirmCandidate(
    documentId: number,
    candidateId: number,
    overwriteQueryFields = true
  ) {
    selectedMeta = await api.confirmCandidate(documentId, candidateId, overwriteQueryFields);
    fetchStatusMsg = 'Candidato confermato e applicato.';
    await Promise.all([refreshDocuments(true), refreshDuplicates(), refreshTaxonomy()]);
  }

  async function handleDismissCandidate(candidateId: number) {
    await api.dismissCandidate(candidateId);
    if (selectedDocId !== null) {
      await loadInspectorMetadata(selectedDocId);
    }
  }

  async function exportCatalog(
    label: string,
    defaultPath: string,
    filter: { name: string; extensions: string[] },
    exportFn: (docIds: number[], outPath: string) => Promise<string>
  ) {
    try {
      const docIds = documents.map((d) => d.document_id);
      const outPath = await saveFileDialog({ defaultPath, filters: [filter] });
      if (!outPath) return;
      await exportFn(docIds, outPath);
      scanStatusText = `Esportati ${docIds.length} record ${label} in ${outPath}`;
    } catch (e) {
      scanStatusText = `Errore esportazione ${label}: ${String(e)}`;
    }
  }

  async function handleExportBibtex() {
    await exportCatalog(
      'BibTeX',
      'papero_catalogo.bib',
      { name: 'BibTeX', extensions: ['bib'] },
      api.exportBibtexMany
    );
  }

  async function handleExportCsv() {
    await exportCatalog(
      'CSV',
      'papero_catalogo.csv',
      { name: 'CSV', extensions: ['csv'] },
      api.exportCsvMany
    );
  }

  async function handleImportMetadata() {
    try {
      const selected = await openFileDialog({
        multiple: false,
        filters: [{ name: 'BibTeX / Zotero CSL-JSON', extensions: ['bib', 'json'] }],
      });
      if (typeof selected !== 'string' || !selected) return;
      const fmt = selected.toLowerCase().endsWith('.json') ? 'csl_json' : 'bib';
      const report = await api.importMetadataFile({
        path: selected,
        format: fmt,
        targetDocumentId: selectedDocId,
      });
      scanStatusText = `Importati: ${report.matched_documents}/${report.total_entries} voci abbinate.`;
      await refreshLibrary();
    } catch (e) {
      scanStatusText = `Errore importazione metadati: ${String(e)}`;
    }
  }

  async function handleRunDedup() {
    scanningDedup = true;
    try {
      const rep = await api.runDedupScan();
      await refreshDuplicates();
      scanStatusText = `Verifica doppioni: ${rep.pairs_checked} coppie analizzate, ${rep.new_groups_created} nuovi gruppi.`;
    } finally {
      scanningDedup = false;
    }
  }

  async function handleMergeDuplicates(groupId: number, keepDocumentId: number) {
    await api.mergeDuplicates(groupId, keepDocumentId);
    if (selectedDocId !== null) {
      selectedDocId = keepDocumentId;
    }
    await refreshLibrary();
  }

  async function handleResolveDuplicate(
    groupId: number,
    decision: 'distinct' | 'versions' | 'editions'
  ) {
    await api.resolveDuplicate(groupId, decision);
    await refreshDuplicates();
  }

  async function handleOpenSideBySide(members: DuplicateFileMember[]) {
    if (settings?.viewer_open_in_separate_window) {
      await api.saveUiSettings({ ...settings, viewer_open_in_separate_window: false });
      settings = { ...settings, viewer_open_in_separate_window: false };
    }
    for (const m of members) {
      if (m.is_available) {
        await openInViewer(m.file_id, m.title, m.format, 1);
      }
    }
  }
</script>

<svelte:window onkeydown={handleGlobalKeydown} />

{#if detachedFileId !== null}
  <main class="detached-root">
    <UnifiedViewer
      fileId={detachedFileId}
      targetPage={detachedPage}
      highlightRects={detachedBboxes}
      onOpenExternal={(fid, p) => api.openFileExternal(fid, p, null)}
    />
  </main>
{:else}
  <div class="app-shell">
    <TopBar
      bind:this={topBarRef}
      {lang}
      bind:query
      {searchMode}
      {metaFilter}
      {activeLeftView}
      {pendingDuplicatesCount}
      {scanning}
      {scanStatusText}
      totalDocsCount={allDocuments.length}
      {unresolvedMetaCount}
      {fetchingBulkMeta}
      {embedProgress}
      {roots}
      {settings}
      {mlStatus}
      {mlDownloadProgress}
      {presets}
      {inspectorOpen}
      onSearch={handleSearch}
      onClearSearch={async () => {
        searchReqSeq++;
        query = '';
        committedContentQuery = '';
        searchResults = null;
        searchStats = null;
        await refreshDocuments();
      }}
      onSearchModeChange={handleSearchModeChange}
      onMetaFilterChange={handleMetaFilterChange}
      onWorkCategoryChange={handleWorkCategoryChange}
      onVerifiedFilterChange={handleVerifiedFilterChange}
      onSortByChange={handleSortByChange}
      onToggleSortDir={handleToggleSortDir}
      onToggleLeftView={(view) => (activeLeftView = view)}
      onToggleInspector={() => (inspectorOpen = !inspectorOpen)}
      onAddFolder={handleAddFolder}
      onRescan={handleRescan}
      onControlRescan={(act) => api.controlRescan(act)}
      onToggleEmbedPaused={handleToggleEmbedPaused}
      onFetchAllMetadata={handleFetchAllMetadata}
      onCancelBulkMetadata={handleCancelBulkMetadata}
      onExportBibtex={handleExportBibtex}
      onExportCsv={handleExportCsv}
      onImportMetadata={handleImportMetadata}
      onSaveSettings={handleSaveSettings}
      onRefreshMlStatus={refreshMlStatus}
      onDownloadMlProfile={handleDownloadMlProfile}
      onUnloadMlModels={handleUnloadMlModels}
      onOpenSettings={() => {
        settingsInitialTab = null;
        settingsOpen = true;
      }}
    />

    {#if !wizardOpen && !unreachableBannerDismissed && unreachableRoots.length > 0}
      <div class="unreachable-banner" role="status">
        <span class="unreachable-msg">
          <strong>{unreachableRoots.map((r) => r.label).join(', ')}:</strong>
          {tr(lang, 'unreachableRootsBanner')}
        </span>
        <div class="unreachable-actions">
          {#if unreachableRoots.length === 1}
            <button
              type="button"
              class="primary"
              onclick={() => handleRebindRoot(unreachableRoots[0].id)}
            >
              {tr(lang, 'rebindNowBtn')}
            </button>
          {/if}
          <button
            type="button"
            onclick={() => {
              settingsInitialTab = 'folders';
              settingsOpen = true;
            }}
          >
            {tr(lang, 'tabFolders')}
          </button>
          <button
            type="button"
            title={tr(lang, 'close')}
            onclick={() => (unreachableBannerDismissed = true)}
          >
            ✕
          </button>
        </div>
      </div>
    {/if}

    <div class="workspace-body">
      <div class="sidebar-slot" class:collapsed={sidebarCollapsed}>
        <LeftSidebar
          {lang}
          collapsed={sidebarCollapsed}
          onToggleCollapse={() => (sidebarCollapsed = !sidebarCollapsed)}
          {mscTree}
          {clusters}
          {tags}
          {roots}
          {collections}
          {savedSearches}
          activeMscPrefix={metaFilter.msc_prefix ?? null}
          activeClusterId={metaFilter.cluster_id ?? null}
          activeTag={metaFilter.tag ?? null}
          activeRootId={metaFilter.root_id ?? null}
          activeCollectionId={metaFilter.collection_id ?? null}
          activeReadingStatus={metaFilter.reading_status ?? null}
          activeStarredOnly={Boolean(metaFilter.starred_only)}
          activeRecentOnly={Boolean(metaFilter.recent_only)}
          {knnBusy}
          {clusteringBusy}
          onSelectMsc={(code) => applySidebarFilter({ msc_prefix: code })}
          onSelectCluster={(cid) => applySidebarFilter({ cluster_id: cid })}
          onSelectTag={(tg) => applySidebarFilter({ tag: tg })}
          onSelectRoot={(rid) => applySidebarFilter({ root_id: rid })}
          onSelectCollection={(colId) => applySidebarFilter({ collection_id: colId })}
          onSelectQuickView={handleSelectQuickView}
          onCreateCollection={handleCreateCollection}
          onRenameCollection={handleRenameCollection}
          onDeleteCollection={handleDeleteCollection}
          onReorderCollections={handleReorderCollections}
          onSaveCurrentView={handleSaveCurrentView}
          onApplySavedSearch={handleApplySavedSearch}
          onDeleteSavedSearch={handleDeleteSavedSearch}
          onClearAllFilters={clearSidebarFilters}
          onRunKnnProposals={handleRunKnnProposals}
          onRunClustering={handleRunClustering}
          onRenameCluster={handleRenameCluster}
        />
      </div>

      <div class="main-split" class:with-inspector={inspectorOpen}>
      <section class="left-pane">
        {#if activeLeftView === 'duplicates'}
          <DuplicatesView
            groups={duplicateGroups}
            {scanningDedup}
            onRunDedup={handleRunDedup}
            onMerge={handleMergeDuplicates}
            onResolve={handleResolveDuplicate}
            onOpenSideBySide={handleOpenSideBySide}
            onOpenMember={(m) => {
              requestSelectDocument(m.document_id, () => {
                openInViewer(m.file_id, m.title, m.format, 1);
              });
            }}
            onRevealFile={(fid) => api.revealFileInFolder(fid)}
          />
        {:else if displayedSearchResults !== null}
          <ContentResults
            {lang}
            query={committedContentQuery || query}
            results={displayedSearchResults}
            {searchStats}
            workCategory={activeWorkCategory}
            activeFilterChips={activeContentFilterChips}
            authorFilter={metaFilter.author ?? ''}
            yearFromFilter={metaFilter.year_from ?? null}
            yearToFilter={metaFilter.year_to ?? null}
            onChangeWorkCategory={handleWorkCategoryChange}
            onChangeContentMetaFilters={(patch) => applySidebarFilter(patch)}
            onClearContentFilters={clearSidebarFilters}
            llmProvider={settings?.llm_provider ?? 'disabled'}
            {llmPreview}
            {llmAnswer}
            {llmStreamText}
            {llmLoading}
            {llmError}
            onGenerateAnswer={handleGenerateLlmAnswer}
            onOpenCitation={handleOpenCitation}
            onOpenDocTopMatch={(doc) => {
              const first = doc.passages[0];
              if (first) handleOpenPassage(doc, first);
            }}
            onOpenPassage={handleOpenPassage}
            onContextMenu={(e, doc, pageOrd) => {
              contextTarget = {
                x: e.clientX,
                y: e.clientY,
                documentId: doc.document_id,
                fileId: doc.file_id,
                format: doc.format,
                isAvailable: doc.is_available,
                pageOrd,
              };
            }}
          />
        {:else}
          <DocumentList
            {lang}
            {documents}
            {selectedDocId}
            {selectedDocIds}
            {collections}
            workCategory={activeWorkCategory}
            verifiedFilter={activeVerifiedFilter}
            unverifiedCount={unverifiedDocsCount}
            sortBy={activeSortBy}
            sortDir={activeSortDir}
            onChangeWorkCategory={handleWorkCategoryChange}
            onChangeVerifiedFilter={handleVerifiedFilterChange}
            onChangeSortBy={handleSortByChange}
            onToggleSortDir={handleToggleSortDir}
            onToggleVerified={(docId, verified) => handleToggleDocVerified(docId, verified, false)}
            onToggleStarred={handleToggleStarred}
            onSelectDoc={(doc) => {
              requestSelectDocument(doc.document_id);
            }}
            onChangeSelectedDocIds={(ids) => (selectedDocIds = ids)}
            onOpenDoc={(doc) => {
              requestSelectDocument(doc.document_id, () => {
                openInViewer(doc.file_id, doc.title, doc.format, 1);
              });
            }}
            onConfirmMsc={handleConfirmMsc}
            onRejectMsc={handleRejectMsc}
            onBatchAddTag={handleBatchAddTag}
            onBatchRemoveTag={handleBatchRemoveTag}
            onBatchSetVerified={handleBatchSetVerified}
            onBatchAddToCollection={handleBatchAddToCollection}
            onBatchExportBibtex={handleBatchExportBibtex}
            onBatchExportCsv={handleBatchExportCsv}
            onBatchEnqueueOcr={handleBatchEnqueueOcr}
            onContextMenu={(e, doc) => {
              contextTarget = {
                x: e.clientX,
                y: e.clientY,
                documentId: doc.document_id,
                fileId: doc.file_id,
                format: doc.format,
                isAvailable: doc.is_available,
                pageOrd: 1,
              };
            }}
          />
        {/if}
      </section>

      <section class="center-pane">
        <ViewerTabs
          {tabs}
          {activeTabId}
          llmProvider={settings?.llm_provider ?? 'disabled'}
          onSelectTab={(id) => (activeTabId = id)}
          onCloseTab={(id) => {
            tabs = tabs.filter((t) => t.tabId !== id);
            if (activeTabId === id) {
              activeTabId = tabs[0]?.tabId ?? null;
            }
          }}
          onSelectMatchIndex={handleSelectMatchIndex}
          onOpenExternal={(fid, page) => api.openFileExternal(fid, page, null)}
          onConsentAccepted={(prov) => {
            if (settings) {
              if (prov === 'anthropic') settings = { ...settings, llm_consent_anthropic: true };
              else if (prov === 'openai') settings = { ...settings, llm_consent_openai: true };
              else if (prov === 'gemini') settings = { ...settings, llm_consent_gemini: true };
            }
          }}
          onStatusMessage={(msg) => (scanStatusText = msg)}
        />
      </section>

      {#if inspectorOpen}
        <section class="inspector-pane">
          <RightInspector
            bind:this={inspectorRef}
            meta={selectedMeta}
            {selectedFileId}
            {clusters}
            {collections}
            loading={loadingMeta}
            fetchingOnline={fetchingMeta}
            lastFetchMessage={fetchStatusMsg}
            onSave={handleSaveDocMetadata}
            onFlushSave={handleFlushDocMetadata}
            onSaveAndFetch={handleSaveAndFetchDocMetadata}
            onFetchMetadata={handleFetchDocMetadata}
            onCancelFetch={handleCancelSingleMetadata}
            onToggleVerified={handleToggleDocVerified}
            {hasNextUnverified}
            onConfirmCandidate={handleConfirmCandidate}
            onDismissCandidate={handleDismissCandidate}
            onCopyBibtex={handleCopyBibtex}
            onCopyCiteCommand={handleCopyCiteCommand}
            onApplySmartPaste={handleApplySmartPaste}
            onSetReadingStatus={handleSetReadingStatus}
            onSetStarred={handleToggleStarred}
            onAddToCollection={handleAddDocToCollection}
            onRemoveFromCollection={handleRemoveDocFromCollection}
            onSelectRelatedDoc={(docId) => requestSelectDocument(docId)}
            onImportAltFormula={handleImportAltFormula}
            onRunOcr={handleRunOcrForFile}
            onConfirmMsc={handleConfirmMsc}
            onRejectMsc={handleRejectMsc}
            onSetCluster={handleSetDocCluster}
            onRemoveCluster={handleRemoveDocCluster}
            onAddTag={handleAddDocTag}
            onRemoveTag={handleRemoveDocTag}
            onOpenInViewer={() => {
              if (selectedFileId !== null && selectedMeta) {
                const doc = catalogStats.byFileId.get(selectedFileId);
                openInViewer(selectedFileId, selectedMeta.title, doc?.format ?? 'pdf', 1);
              }
            }}
            onClose={() => (inspectorOpen = false)}
          />
        </section>
      {/if}
    </div>
    </div>

    <ContextMenu
      target={contextTarget}
      {presets}
      onClose={() => (contextTarget = null)}
      onOpenInternal={(fid, page) => {
        const doc = catalogStats.byFileId.get(fid);
        openInViewer(fid, doc?.title ?? `File #${fid}`, doc?.format ?? 'pdf', page ?? 1);
      }}
      onOpenExternal={(fid, page, tpl) => api.openFileExternal(fid, page, tpl)}
      onInspectMetadata={(docId) => {
        inspectorOpen = true;
        requestSelectDocument(docId);
      }}
      onCopyBibtex={(docId) => handleCopyBibtex(docId)}
      onRunOcr={(fid) => handleRunOcrForFile(fid)}
      onRevealInFolder={(fid) => api.revealFileInFolder(fid)}
      onCopyPath={handleCopyPath}
    />

    {#if settings}
      <FirstRunWizard
        open={wizardOpen}
        {settings}
        {roots}
        {mlStatus}
        {mlDownloadProgress}
        onAddFolder={handleAddFolder}
        onRestoreBackup={handleRestoreBackup}
        {backupBusy}
        {backupNotice}
        {backupError}
        onDownloadMlProfile={handleDownloadMlProfile}
        onSaveSettings={handleSaveSettings}
        onCompleteAndScan={async () => {
          wizardOpen = false;
          await handleRescan();
        }}
        onClose={() => (wizardOpen = false)}
      />

      <SettingsModal
        open={settingsOpen}
        initialTab={settingsInitialTab}
        {settings}
        {roots}
        {presets}
        {mlStatus}
        {mlDownloadProgress}
        {ocrStatus}
        onSaveSettings={handleSaveSettings}
        onToggleRoot={handleToggleRoot}
        onSetRootCloudExclusion={handleSetRootCloudExclusion}
        onRebindRoot={handleRebindRoot}
        onDeleteRoot={handleDeleteRoot}
        onAddFolder={handleAddFolder}
        onDownloadMlProfile={handleDownloadMlProfile}
        onUnloadMlModels={handleUnloadMlModels}
        onRefreshMlStatus={refreshMlStatus}
        onRefreshOcrStatus={refreshOcrStatus}
        onRunPendingOcr={handleRunPendingOcr}
        {backupBusy}
        {backupNotice}
        {backupError}
        onCreateBackup={handleCreateBackup}
        onUpdateBackupNow={handleUpdateBackupNow}
        onRestoreBackup={handleRestoreBackup}
        onLaunchWizard={() => {
          settingsOpen = false;
          settingsInitialTab = null;
          wizardOpen = true;
        }}
        onClose={() => {
          settingsOpen = false;
          settingsInitialTab = null;
        }}
      />

      {#if cloudConsentModal}
        <div
          class="consent-backdrop"
          role="dialog"
          aria-modal="true"
          aria-label={lang === 'en' ? 'Cloud LLM Privacy Confirmation' : 'Conferma Privacy Invio LLM Cloud'}
        >
          <div class="consent-dialog">
            <h3>
              {lang === 'en'
                ? `First transmission to ${cloudConsentModal.provider.toUpperCase()}`
                : `Primo invio verso ${cloudConsentModal.provider.toUpperCase()}`}
            </h3>
            <p>
              {lang === 'en'
                ? `Papero is about to send ONLY your search query and ${cloudConsentModal.preview.passages_included} retrieved passage snippet(s) (~${cloudConsentModal.preview.estimated_input_tokens} estimated input tokens) to ${cloudConsentModal.provider} (model: ${cloudConsentModal.preview.model ?? '-'}). Full PDF/DjVu files are NEVER uploaded.`
                : `Papero sta per inviare ESCLUSIVAMENTE la tua query e ${cloudConsentModal.preview.passages_included} passaggi estratti (~${cloudConsentModal.preview.estimated_input_tokens} token stimati) a ${cloudConsentModal.provider} (modello: ${cloudConsentModal.preview.model ?? '-'}). I file PDF/DjVu interi NON vengono mai inviati.`}
            </p>
            {#if cloudConsentModal.preview.passages_excluded_cloud > 0}
              <p class="consent-excl">
                {lang === 'en'
                  ? `${cloudConsentModal.preview.passages_excluded_cloud} passage(s) from excluded documents/folders were removed before transmission.`
                  : `${cloudConsentModal.preview.passages_excluded_cloud} passaggi provenienti da documenti o cartelle esclusi sono già stati rimossi prima dell'invio.`}
              </p>
            {/if}
            {#if cloudConsentModal.provider === 'gemini'}
              <div class="consent-warn">
                <strong>
                  {lang === 'en'
                    ? 'Important Google Gemini Free Tier Notice:'
                    : 'Avviso importante per Google Gemini (Piano Gratuito):'}
                </strong>
                {lang === 'en'
                  ? ' Under Google Gemini API Unpaid/Free Tier terms, submitted prompts and outputs may be reviewed and used to improve Google products. On a Paid billing project, API inputs are not used for model training.'
                  : ' Nel piano gratuito (Unpaid) della Gemini API, prompt e risposte possono essere revisionati e usati da Google per migliorare i prodotti. Nei progetti con fatturazione attiva (Paid Tier) gli input API non vengono usati per addestramento.'}
              </div>
            {:else}
              <p class="consent-note">
                {lang === 'en'
                  ? 'Under commercial API terms, inputs sent via API are not used to train models by default (unlike consumer web plans).'
                  : 'Secondo le policy API commerciali, gli input inviati tramite API non vengono usati per addestramento per impostazione predefinita.'}
              </p>
            {/if}
            <div class="consent-actions">
              <button type="button" onclick={() => (cloudConsentModal = null)}>
                {lang === 'en' ? 'Cancel' : 'Annulla'}
              </button>
              <button type="button" class="primary" onclick={confirmAndRunCloudConsent}>
                {lang === 'en' ? 'I understand, send query' : 'Ho capito, invia ora'}
              </button>
            </div>
          </div>
        </div>
      {/if}
      {#if pendingDocSwitch}
        <div
          class="consent-backdrop"
          role="dialog"
          aria-modal="true"
          aria-label={lang === 'en' ? 'Unsaved Metadata Changes' : 'Modifiche Metadati Non Salvate'}
        >
          <div class="consent-dialog">
            <h3>
              {lang === 'en'
                ? 'Unsaved changes in the Metadata Inspector'
                : 'Modifiche non salvate nell’Ispettore Metadati'}
            </h3>
            <p>
              {lang === 'en'
                ? 'You have unsaved edits on the current document. Save them before switching, or discard them?'
                : 'Ci sono modifiche non ancora salvate sul documento corrente. Vuoi salvarle prima di cambiare documento o scartarle?'}
            </p>
            <div class="consent-actions">
              <button type="button" onclick={() => resolvePendingDocSwitch('cancel')}>
                {lang === 'en' ? 'Cancel' : 'Annulla'}
              </button>
              <button type="button" onclick={() => resolvePendingDocSwitch('discard')}>
                {lang === 'en' ? 'Discard changes' : 'Scarta modifiche'}
              </button>
              <button type="button" class="primary" onclick={() => resolvePendingDocSwitch('save')}>
                {lang === 'en' ? 'Save & switch' : 'Salva e cambia'}
              </button>
            </div>
          </div>
        </div>
      {/if}
    {/if}
  </div>
{/if}

<style>
  .detached-root {
    height: 100vh;
    width: 100vw;
  }
  .app-shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }
  .workspace-body {
    flex: 1;
    display: flex;
    overflow: hidden;
  }
  .sidebar-slot {
    width: 245px;
    flex-shrink: 0;
    height: 100%;
    overflow: hidden;
  }
  .sidebar-slot.collapsed {
    width: 28px;
  }
  .main-split {
    flex: 1;
    min-width: 0;
    display: grid;
    grid-template-columns: minmax(300px, 34%) 1fr;
    overflow: hidden;
  }
  .main-split.with-inspector {
    grid-template-columns: minmax(290px, 30%) 1fr minmax(300px, 340px);
  }
  .left-pane {
    border-right: 1px solid var(--border);
    background: var(--bg-panel);
    overflow: hidden;
  }
  .center-pane {
    overflow: hidden;
  }
  .inspector-pane {
    overflow: hidden;
  }
  .consent-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(15, 23, 42, 0.58);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 70;
    padding: 16px;
  }
  .consent-dialog {
    width: min(520px, 100%);
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 16px 18px;
    box-shadow: 0 20px 55px rgba(0, 0, 0, 0.38);
    display: flex;
    flex-direction: column;
    gap: 10px;
    font-size: 13px;
  }
  .consent-dialog h3 {
    margin: 0;
    font-size: 16px;
  }
  .consent-dialog p {
    margin: 0;
    line-height: 1.5;
  }
  .consent-excl {
    font-size: 12px;
    color: var(--accent);
    font-weight: 600;
  }
  .consent-warn {
    padding: 8px 10px;
    border-radius: 8px;
    background: rgba(245, 158, 11, 0.16);
    border: 1px solid rgba(245, 158, 11, 0.45);
    font-size: 12px;
    line-height: 1.45;
  }
  .consent-note {
    font-size: 12px;
    color: var(--text-muted);
  }
  .consent-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }
  .unreachable-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    padding: 6px 12px;
    background: rgba(245, 158, 11, 0.14);
    border-bottom: 1px solid rgba(217, 119, 6, 0.45);
    font-size: 12px;
  }
  .unreachable-msg {
    min-width: 0;
    line-height: 1.35;
  }
  .unreachable-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }
</style>
