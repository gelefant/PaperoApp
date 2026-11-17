<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { open as openFileDialog, save as saveFileDialog } from '@tauri-apps/plugin-dialog';
  import { api } from './lib/api';
  import { normalizeLang, tr } from './lib/i18n';
  import type {
    BackupReport,
    ContentSearchDocResult,
    ContentSearchPassageHit,
    DocumentListItem,
    DocumentMetadataView,
    DownloadProgressEvent,
    DuplicateFileMember,
    DuplicateGroupView,
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
    ScanProgressEvent,
    SettingsTabId,
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
  let activeLeftView = $state<'library' | 'duplicates'>('library');
  let duplicateGroups = $state.raw<DuplicateGroupView[]>([]);
  let scanningDedup = $state(false);
  let scanning = $state(false);
  let scanStatusText = $state('');
  let sidebarCollapsed = $state(false);
  let mscTree = $state.raw<MscTreeNode[]>([]);
  let clusters = $state.raw<TopicClusterView[]>([]);
  let tags = $state.raw<TagSummary[]>([]);
  let knnBusy = $state(false);
  let clusteringBusy = $state(false);
  let settings = $state<UiSettingsPayload | null>(null);
  let presets = $state.raw<ExternalReaderPreset[]>([]);
  let tabs = $state<ViewerTab[]>([]);
  let activeTabId = $state<string | null>(null);
  let contextTarget = $state<ContextTarget | null>(null);
  let topBarRef = $state<{ focusSearch?: () => void } | null>(null);
  let settingsOpen = $state(false);
  let settingsInitialTab = $state<SettingsTabId | null>(null);
  let wizardOpen = $state(false);
  let unreachableBannerDismissed = $state(false);
  let unreachableRoots = $derived(roots.filter((r) => !r.is_reachable));
  let ocrStatus = $state<OcrEnvironmentStatus | null>(null);
  let llmPreview = $state<LlmPromptPreview | null>(null);
  let llmAnswer = $state<LlmAnswerResult | null>(null);
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
  let appDestroyed = false;
  let fsRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  let docsReqSeq = 0;
  let searchReqSeq = 0;
  let inspectorReqSeq = 0;

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
    const [tree, cl, tg] = await Promise.all([
      api.getMscTree(),
      api.listTopicClusters(),
      api.listAllTags(),
    ]);
    mscTree = tree;
    clusters = cl;
    tags = tg;
  }

  function hasSidebarFilter() {
    return Boolean(
      metaFilter.msc_prefix || metaFilter.cluster_id || metaFilter.tag || metaFilter.root_id
    );
  }

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
    searchResults = null;
    searchStats = null;
    metaFilter = { ...metaFilter, ...patch };
    await refreshDocuments();
  }

  async function clearSidebarFilters() {
    metaFilter = {
      ...metaFilter,
      msc_prefix: null,
      cluster_id: null,
      tag: null,
      root_id: null,
    };
    await refreshDocuments();
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
    const [cfg, pr, ml, ocr] = await Promise.all([
      api.getUiSettings(),
      api.listExternalPresets(),
      api.listModelStatuses(),
      api.getOcrStatus(),
    ]);
    settings = cfg;
    presets = pr;
    mlStatus = ml;
    ocrStatus = ocr;
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
  });

  onDestroy(() => {
    appDestroyed = true;
    if (fsRefreshTimer) {
      clearTimeout(fsRefreshTimer);
      fsRefreshTimer = null;
    }
    unlistenFs?.();
    unlistenBackup?.();
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
    if (!searchResults || searchResults.length === 0 || !query.trim()) return;
    llmLoading = true;
    llmError = '';
    try {
      llmAnswer = await api.generateLlmAnswer(query.trim(), searchResults);
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
    const resp = await api.searchContentHybrid(
      trimmed,
      metaFilter.root_id ? [metaFilter.root_id] : undefined,
      null,
      null,
      activeWorkCategory
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

    const updated = await api.setDocumentMetadataVerified(documentId, verified);
    if (selectedDocId === documentId) {
      selectedMeta = updated;
    }
    allDocuments = allDocuments.map((d) =>
      d.document_id === documentId ? { ...d, metadata_verified: verified } : d
    );
    await refreshDocuments(false);

    const shouldAdvance =
      verified &&
      (advanceNext || (activeVerifiedFilter === 'unverified' && selectedDocId === documentId));
    if (shouldAdvance) {
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
    activeMatchIndex = 0
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
        highlightRects,
        matches,
        activeMatchIndex,
      },
    ];
    activeTabId = tabId;
  }

  function handleOpenPassage(doc: ContentSearchDocResult, pass: ContentSearchPassageHit) {
    loadInspectorMetadata(doc.document_id);
    const matches: ViewerTargetPassage[] = doc.passages.map((p) => ({
      page_ord: p.page_ord,
      printed_label: p.printed_label,
      bboxes: p.bboxes,
      text_content: p.text_content,
    }));
    const idx = doc.passages.findIndex((p) => p.passage_id === pass.passage_id);
    openInViewer(
      pass.file_id,
      doc.title,
      doc.format,
      pass.page_ord,
      pass.bboxes,
      matches,
      Math.max(0, idx)
    );
  }

  function handleOpenCitation(c: LlmCitation) {
    loadInspectorMetadata(c.document_id);
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
    openInViewer(
      c.file_id,
      c.doc_title,
      matchingDoc?.format ?? 'pdf',
      c.page_ord,
      c.bboxes,
      matches,
      Math.max(0, idx)
    );
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
    await refreshLlmPreview();
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
      if (cloudConsentModal) cloudConsentModal = null;
      else if (contextTarget) contextTarget = null;
      else if (settingsOpen) settingsOpen = false;
      else if (wizardOpen) wizardOpen = false;
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

  async function handleSaveDocMetadata(edit: ManualMetadataEdit, lockedFields?: string[]) {
    selectedMeta = await api.saveDocumentMetadata(edit, lockedFields);
    await Promise.all([
      refreshDocuments(true),
      refreshDuplicates(),
      refreshTaxonomy(),
      refreshLlmPreview(),
    ]);
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
      if (id === 'openlibrary') return cfg.openlibrary_enabled;
      return false;
    });
    const labels = active.map((id) => SOURCE_SHORT_NAMES[id] ?? id);
    if (labels.length <= 5) return labels.join(' → ');
    return `${labels.slice(0, 5).join(' → ')} → …`;
  }

  async function handleSaveAndFetchDocMetadata(edit: ManualMetadataEdit, lockedFields: string[]) {
    fetchingMeta = true;
    fetchStatusMsg = `Ricerca guidata secondo priorità (${formatActivePriorityChain(settings)})…`;
    try {
      const res = await api.saveAndFetchDocumentMetadata(edit, lockedFields, true);
      selectedMeta = res.metadata;
      const outcome = res.outcome;
      if (outcome.status === 'updated') {
        fetchStatusMsg = `Metadati completati da ${outcome.source ?? 'fonte esterna'} (campi bloccati preservati).`;
      } else if (outcome.status === 'verified_locked') {
        fetchStatusMsg = 'Metadati confermati e protetti: rimuovi la spunta ✓ prima di avviare una ricerca online.';
      } else if (outcome.status === 'candidates_found') {
        fetchStatusMsg = `Trovati ${outcome.candidates_count} candidati da confermare qui sotto.`;
      } else if (outcome.status === 'offline_only') {
        fetchStatusMsg = 'Modalità offline attiva: campi bloccati salvati, nessuna richiesta di rete inviata.';
      } else {
        fetchStatusMsg = 'Nessuna corrispondenza trovata con i campi inseriti. Prova ad aggiungere o precisare Titolo/Autore.';
      }
      await Promise.all([
        refreshDocuments(true),
        refreshDuplicates(),
        refreshTaxonomy(),
        refreshLlmPreview(),
      ]);
      return outcome;
    } catch (e) {
      fetchStatusMsg = `Errore ricerca guidata metadati: ${e}`;
    } finally {
      fetchingMeta = false;
    }
  }

  async function handleFetchDocMetadata(documentId: number) {
    fetchingMeta = true;
    fetchStatusMsg = `Ricerca metadati secondo priorità (${formatActivePriorityChain(settings)})…`;
    try {
      const outcome = await api.fetchDocumentMetadata(documentId);
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
      selectedMeta = await api.getDocumentMetadata(documentId);
      fetchStatusMsg = msg;
      await Promise.all([refreshDocuments(true), refreshDuplicates(), refreshTaxonomy()]);
      return outcome;
    } catch (e) {
      fetchStatusMsg = `Errore ricerca metadati: ${e}`;
    } finally {
      fetchingMeta = false;
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

  async function handleConfirmCandidate(documentId: number, candidateId: number) {
    selectedMeta = await api.confirmCandidate(documentId, candidateId);
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
          activeMscPrefix={metaFilter.msc_prefix ?? null}
          activeClusterId={metaFilter.cluster_id ?? null}
          activeTag={metaFilter.tag ?? null}
          activeRootId={metaFilter.root_id ?? null}
          {knnBusy}
          {clusteringBusy}
          onSelectMsc={(code) => applySidebarFilter({ msc_prefix: code })}
          onSelectCluster={(cid) => applySidebarFilter({ cluster_id: cid })}
          onSelectTag={(tg) => applySidebarFilter({ tag: tg })}
          onSelectRoot={(rid) => applySidebarFilter({ root_id: rid })}
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
              loadInspectorMetadata(m.document_id);
              openInViewer(m.file_id, m.title, m.format, 1);
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
            onChangeWorkCategory={handleWorkCategoryChange}
            llmProvider={settings?.llm_provider ?? 'disabled'}
            {llmPreview}
            {llmAnswer}
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
            onSelectDoc={(doc) => {
              loadInspectorMetadata(doc.document_id);
            }}
            onOpenDoc={(doc) => {
              loadInspectorMetadata(doc.document_id);
              openInViewer(doc.file_id, doc.title, doc.format, 1);
            }}
            onConfirmMsc={handleConfirmMsc}
            onRejectMsc={handleRejectMsc}
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
          onSelectTab={(id) => (activeTabId = id)}
          onCloseTab={(id) => {
            tabs = tabs.filter((t) => t.tabId !== id);
            if (activeTabId === id) {
              activeTabId = tabs[0]?.tabId ?? null;
            }
          }}
          onSelectMatchIndex={handleSelectMatchIndex}
          onOpenExternal={(fid, page) => api.openFileExternal(fid, page, null)}
        />
      </section>

      {#if inspectorOpen}
        <section class="inspector-pane">
          <RightInspector
            meta={selectedMeta}
            {selectedFileId}
            {clusters}
            loading={loadingMeta}
            fetchingOnline={fetchingMeta}
            lastFetchMessage={fetchStatusMsg}
            onSave={handleSaveDocMetadata}
            onSaveAndFetch={handleSaveAndFetchDocMetadata}
            onFetchMetadata={handleFetchDocMetadata}
            onToggleVerified={handleToggleDocVerified}
            {hasNextUnverified}
            onConfirmCandidate={handleConfirmCandidate}
            onDismissCandidate={handleDismissCandidate}
            onCopyBibtex={handleCopyBibtex}
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
        loadInspectorMetadata(docId);
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
