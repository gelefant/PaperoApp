<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { open as openFileDialog, save as saveFileDialog } from '@tauri-apps/plugin-dialog';
  import { api } from './lib/api';
  import { normalizeLang, tr } from './lib/i18n';
  import { createLayoutState } from './lib/layoutState.svelte';
  import { createViewerTabsState } from './lib/viewerTabsState.svelte';
  import type {
    AnnotationSearchHit,
    BackupReport,
    BibtexSyncReport,
    CitationLinkItem,
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
    UpdateCheckResult,
    VerifiedFilter,
    ViewerTargetPassage,
    WorkCategory,
  } from './lib/types';
  import TopBar from './lib/components/TopBar.svelte';
  import LeftSidebar from './lib/components/LeftSidebar.svelte';
  import DocumentList from './lib/components/DocumentList.svelte';
  import ContentResults from './lib/components/ContentResults.svelte';
  import NotesSearchResults from './lib/components/NotesSearchResults.svelte';
  import ViewerTabs from './lib/components/ViewerTabs.svelte';
  import UnifiedViewer from './lib/components/UnifiedViewer.svelte';
  import ContextMenu, { type ContextTarget } from './lib/components/ContextMenu.svelte';
  import RightInspector from './lib/components/RightInspector.svelte';
  import DuplicatesView from './lib/components/DuplicatesView.svelte';
  import FirstRunWizard from './lib/components/FirstRunWizard.svelte';
  import SettingsModal from './lib/components/SettingsModal.svelte';
  import CommandPalette, { type PaletteAction } from './lib/components/CommandPalette.svelte';
  import SuggestedClassicsModal from './lib/components/SuggestedClassicsModal.svelte';

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
  const layout = createLayoutState();
  const viewerTabs = createViewerTabsState();

  let scanning = $state(false);
  let scanStatusText = $state('');
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
  let totalAnnotationsCount = $state(0);
  let notesScopeActive = $state(false);
  let notesHits = $state.raw<AnnotationSearchHit[]>([]);
  let notesLoading = $state(false);
  let statementsOnly = $state(false);
  let statementKindGroup = $state<string>('all');
  let commandPaletteOpen = $state(false);
  let classicsModalOpen = $state(false);
  let collectionBibModal = $state<CollectionView | null>(null);
  let collectionBibBusy = $state(false);
  let collectionBibErr = $state('');
  let updateInfo = $state<UpdateCheckResult | null>(null);
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

  function resolveStatementKinds(group: string): string[] | null {
    switch (group) {
      case 'theorem':
        return ['theorem', 'lemma', 'proposition', 'corollary'];
      case 'definition':
        return ['definition'];
      case 'remark':
        return ['remark'];
      case 'example':
        return ['example'];
      case 'conjecture':
        return ['conjecture'];
      default:
        return null;
    }
  }

  /** Builds ContentSearchFilter propagating MSC2020, Tag, Cluster, Collection, Author, Year, Root, Category, Reading, Starred & Statement subtypes. */
  function buildActiveContentFilter(): ContentSearchFilter {
    return {
      root_ids: metaFilter.root_id ? [metaFilter.root_id] : null,
      msc_prefix: metaFilter.msc_prefix || null,
      tag: metaFilter.tag || null,
      cluster_id: metaFilter.cluster_id ?? null,
      collection_id: metaFilter.collection_id ?? null,
      author: metaFilter.author || null,
      year_from: metaFilter.year_from ?? null,
      year_to: metaFilter.year_to ?? null,
      work_category: activeWorkCategory !== 'all' ? activeWorkCategory : null,
      reading_status: metaFilter.reading_status || null,
      starred_only: metaFilter.starred_only ? true : null,
      statements_only: statementsOnly,
      statement_kinds: statementsOnly ? resolveStatementKinds(statementKindGroup) : null,
    };
  }

  const activeContentFilterChips = $derived.by(() => {
    const chips: string[] = [];
    if (statementsOnly) {
      const subLabel =
        statementKindGroup === 'theorem'
          ? tr(lang, 'stmtTheoremGroup')
          : statementKindGroup === 'definition'
            ? tr(lang, 'stmtDefinitions')
            : statementKindGroup === 'remark'
              ? tr(lang, 'stmtRemarks')
              : statementKindGroup === 'example'
                ? tr(lang, 'stmtExamples')
                : statementKindGroup === 'conjecture'
                  ? tr(lang, 'stmtConjectures')
                  : tr(lang, 'statementsOnlyLabel');
      chips.push(subLabel);
    }
    if (metaFilter.msc_prefix) chips.push(`MSC ${metaFilter.msc_prefix}`);
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
    viewerTabs.pruneRemovedFiles(catalogStats.byFileId);

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
    // Keep content search results open & filter them coherently when clicking MSC2020/Tag/Cluster/Collection/Folder!
    if (searchResults !== null && committedContentQuery.trim()) {
      await Promise.all([handleSearch(committedContentQuery), refreshDocuments()]);
      return;
    }
    await refreshDocuments();
  }

  async function clearSidebarFilters() {
    statementsOnly = false;
    statementKindGroup = 'all';
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

  async function refreshAnnotationsCount() {
    try {
      const cnt = await api.countTotalAnnotations();
      totalAnnotationsCount = cnt;
      if (cnt === 0 && notesScopeActive) {
        notesScopeActive = false;
        notesHits = [];
      } else if (notesScopeActive) {
        void runNotesSearch(query);
      }
    } catch {
      // Ignore transient startup race
    }
  }

  async function runNotesSearch(q: string) {
    notesLoading = true;
    try {
      notesHits = await api.searchAnnotations(q.trim(), null);
    } catch {
      notesHits = [];
    } finally {
      notesLoading = false;
    }
  }

  async function handleToggleNotesScope(active: boolean) {
    if (active && totalAnnotationsCount === 0) return;
    notesScopeActive = active;
    if (active) {
      activeLeftView = 'library';
      await runNotesSearch(query);
    } else {
      notesHits = [];
    }
  }

  function handleOpenNoteHit(hit: AnnotationSearchHit) {
    if (!hit.file_id) return;
    const doc = hit.document_id !== null ? catalogStats.byDocId.get(hit.document_id) : undefined;
    const fid = hit.file_id;
    const title = hit.doc_title || doc?.title || `File #${fid}`;
    const fmt = doc?.format ?? 'pdf';
    if (hit.document_id !== null) {
      requestSelectDocument(hit.document_id, () => {
        openInViewer(fid, title, fmt, hit.annotation.page_ord, hit.annotation.rects);
      });
    } else {
      openInViewer(fid, title, fmt, hit.annotation.page_ord, hit.annotation.rects);
    }
  }

  async function handleToggleStatementsOnly(active: boolean) {
    statementsOnly = active;
    if (!active) statementKindGroup = 'all';
    if (searchResults !== null && committedContentQuery.trim()) {
      await handleSearch(committedContentQuery);
    }
  }

  async function handleChangeStatementKindGroup(group: string) {
    statementKindGroup = group;
    if (statementsOnly && searchResults !== null && committedContentQuery.trim()) {
      await handleSearch(committedContentQuery);
    }
  }

  function handleToggleZenMode() {
    if (!layout.toggleZenMode(viewerTabs.tabs.length > 0)) {
      scanStatusText =
        lang === 'en'
          ? 'Open a document first to use Zen reading mode.'
          : 'Apri prima un documento per usare la modalità lettura Zen.';
    }
  }

  async function handleConfigureCollectionBibSync(col: CollectionView) {
    collectionBibErr = '';
    collectionBibModal = col;
  }

  async function handleChooseCollectionBibPath() {
    const col = collectionBibModal;
    if (!col) return;
    collectionBibBusy = true;
    collectionBibErr = '';
    try {
      const safeName =
        col.name
          .trim()
          .toLowerCase()
          .replace(/[^a-z0-9_-]+/g, '_')
          .replace(/^_+|_+$/g, '') || `collezione_${col.id}`;
      const chosen = await saveFileDialog({
        defaultPath: col.bib_sync_path || `${safeName}.bib`,
        filters: [{ name: 'BibTeX', extensions: ['bib'] }],
      });
      if (!chosen) return;
      const updated = await api.setCollectionBibSync(col.id, chosen, true);
      collectionBibModal = updated;
      await refreshTaxonomy();
      scanStatusText =
        lang === 'en'
          ? `Linked "${updated.name}" to ${chosen} (Auto-Sync active)`
          : `Collezione «${updated.name}» collegata a ${chosen} (Auto-Sync attivo)`;
    } catch (e) {
      collectionBibErr = String(e);
    } finally {
      collectionBibBusy = false;
    }
  }

  async function handleUnlinkCollectionBib() {
    const col = collectionBibModal;
    if (!col) return;
    collectionBibBusy = true;
    collectionBibErr = '';
    try {
      const updated = await api.setCollectionBibSync(col.id, null, false);
      collectionBibModal = updated;
      await refreshTaxonomy();
      scanStatusText =
        lang === 'en'
          ? `Unlinked .bib auto-sync for "${updated.name}"`
          : `Auto-Sync .bib scollegato per «${updated.name}»`;
    } catch (e) {
      collectionBibErr = String(e);
    } finally {
      collectionBibBusy = false;
    }
  }

  async function handleSyncCollectionBibNow(col: CollectionView) {
    collectionBibBusy = true;
    collectionBibErr = '';
    try {
      const [path, count] = await api.syncCollectionBibtexNow(col.id);
      scanStatusText =
        lang === 'en'
          ? `Synced ${count} entries from "${col.name}" to ${path}`
          : `Sincronizzate ${count} voci di «${col.name}» in ${path}`;
    } catch (e) {
      const msg = String(e);
      collectionBibErr = msg;
      scanStatusText = msg;
    } finally {
      collectionBibBusy = false;
    }
  }

  const paletteActions = $derived.by((): PaletteAction[] => {
    const list: PaletteAction[] = [
      {
        id: 'classics',
        label: tr(lang, 'suggestedClassicsTitle'),
        hint: lang === 'en' ? 'Missing most-cited papers & books' : 'Paper e libri più citati che mancano',
        run: () => (classicsModalOpen = true),
      },
      {
        id: 'zen',
        label: layout.zenMode
          ? lang === 'en'
            ? 'Exit Zen reading mode'
            : 'Esci da modalità lettura Zen'
          : lang === 'en'
            ? 'Enter Zen reading mode'
            : 'Attiva modalità lettura Zen',
        hint: '⌘\\',
        run: handleToggleZenMode,
      },
      {
        id: 'toggle-catalog',
        label: layout.catalogCollapsed
          ? tr(lang, 'expandCatalogTip')
          : tr(lang, 'collapseCatalogTip'),
        hint: '⇧⌘B',
        run: () => layout.toggleCatalogCollapsed(),
      },
      {
        id: 'rescan',
        label: tr(lang, 'rescan'),
        hint: '⌘R',
        run: () => {
          if (!scanning && roots.length > 0) void handleRescan();
        },
      },
      {
        id: 'settings',
        label: tr(lang, 'settings'),
        hint: '⌘,',
        run: () => {
          settingsInitialTab = null;
          settingsOpen = true;
        },
      },
      {
        id: 'export-bib',
        label: tr(lang, 'exportBib'),
        hint: lang === 'en' ? 'Export visible catalog to .bib' : 'Esporta catalogo corrente in .bib',
        run: () => void handleExportBibtex(),
      },
      {
        id: 'backup',
        label: tr(lang, 'createBackupBtn'),
        hint: '.papero-backup',
        run: () => void handleCreateBackup(),
      },
    ];
    if (selectedDocId !== null) {
      const docId = selectedDocId;
      list.unshift({
        id: 'copy-cite',
        label: lang === 'en' ? 'Copy \\cite{key} of selected document' : 'Copia \\cite{key} del documento selezionato',
        hint: selectedMeta?.title ?? '',
        run: () => {
          void api.getDocumentCiteKey(docId, metaFilter.collection_id ?? null).then(async (p) => {
            await navigator.clipboard.writeText(p.cite_command);
            scanStatusText = `Copiato ${p.cite_command}`;
          });
        },
      });
    }
    if (settings?.bibtex_auto_sync_enabled && settings?.bibtex_auto_sync_path) {
      list.push({
        id: 'sync-bib',
        label: lang === 'en' ? 'Sync global .bib now' : 'Sincronizza file .bib globale ora',
        hint: settings.bibtex_auto_sync_path,
        run: () => void api.syncBibtexNow().catch(() => {}),
      });
    }
    return list;
  });

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
    const res = await api.getDocumentCiteKey(docId, metaFilter.collection_id ?? null);
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
      layout.inspectorOpen &&
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
    void refreshAnnotationsCount();
    if (cfg.check_updates_on_startup !== false && cfg.network_lookups_enabled) {
      api
        .checkAppUpdate(false)
        .then((res) => {
          if (!appDestroyed) updateInfo = res;
        })
        .catch(() => {});
    }
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
    if (notesScopeActive) {
      await runNotesSearch(q);
      return;
    }
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

    if (verified && layout.inspectorOpen && selectedDocId === documentId) {
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
        layout.inspectorOpen = true;
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
    explicitPageJump = false,
    documentId?: number
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
    viewerTabs.openInPrimary({
      fileId,
      documentId,
      title,
      format,
      pageOrd,
      highlightRects,
      matches,
      activeMatchIndex,
      explicitPageJump,
    });
  }

  function handleOpenLibraryDocFromCite(item: CitationLinkItem, inSplit: boolean) {
    const doc = catalogStats.byDocId.get(item.document_id);
    const fmt = doc?.format ?? 'pdf';
    if (inSplit) {
      viewerTabs.openInSplit({
        fileId: item.file_id,
        documentId: item.document_id,
        title: item.title,
        format: fmt,
        pageOrd: 1,
      });
      return;
    }
    requestSelectDocument(item.document_id, () => {
      void openInViewer(item.file_id, item.title, fmt, 1, [], [], 0, false, item.document_id);
    });
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
    viewerTabs.selectMatchIndex(tabId, idx);
  }

  async function handleCopyPath(fileId: number) {
    const info = await api.getViewerDoc(fileId);
    await navigator.clipboard.writeText(info.abs_path);
  }

  async function runRevealFileInFolder(fileId: number) {
    try {
      await api.revealFileInFolder(fileId);
    } catch (e) {
      scanStatusText =
        lang === 'en'
          ? `Could not reveal file in folder: ${String(e)}`
          : `Impossibile mostrare il file nella cartella: ${String(e)}`;
    }
  }

  async function runOpenFileExternal(
    fileId: number,
    pageOrd?: number | null,
    templateOverride?: string | null
  ) {
    try {
      await api.openFileExternal(
        fileId,
        pageOrd ?? undefined,
        templateOverride ?? undefined
      );
    } catch (e) {
      scanStatusText =
        lang === 'en'
          ? `Could not open external reader: ${String(e)}`
          : `Impossibile aprire il lettore esterno: ${String(e)}`;
    }
  }

  async function handleCopyBibtex(docId: number): Promise<string> {
    const bib = await api.getDocumentBibtex(docId, metaFilter.collection_id ?? null);
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
      if (commandPaletteOpen) commandPaletteOpen = false;
      else if (classicsModalOpen) classicsModalOpen = false;
      else if (collectionBibModal) collectionBibModal = null;
      else if (pendingDocSwitch) pendingDocSwitch = null;
      else if (cloudConsentModal) cloudConsentModal = null;
      else if (contextTarget) contextTarget = null;
      else if (settingsOpen) settingsOpen = false;
      else if (wizardOpen) wizardOpen = false;
      else if (layout.zenMode) handleToggleZenMode();
      return;
    }

    if (mod && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      commandPaletteOpen = !commandPaletteOpen;
      return;
    }

    if (mod && e.key === '\\') {
      e.preventDefault();
      handleToggleZenMode();
      return;
    }

    if (mod && !e.shiftKey && e.key.toLowerCase() === 's') {
      if (
        layout.inspectorOpen &&
        selectedDocId !== null &&
        inspectorRef &&
        !settingsOpen &&
        !wizardOpen
      ) {
        e.preventDefault();
        void inspectorRef.triggerSave();
      }
      return;
    }

    if (e.key === '/' && !mod && !inEditable) {
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

    if (mod && e.shiftKey && e.key.toLowerCase() === 'b') {
      e.preventDefault();
      layout.toggleCatalogCollapsed();
      return;
    }

    if (mod && !e.shiftKey && e.key.toLowerCase() === 'b') {
      e.preventDefault();
      layout.toggleSidebarCollapsed();
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
      onOpenExternal={(fid, p) => void runOpenFileExternal(fid, p, null)}
    />
  </main>
{:else}
  <div class="app-shell" class:is-resizing={layout.resizingPane !== null}>
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
      inspectorOpen={layout.inspectorOpen}
      {notesScopeActive}
      hasNotes={totalAnnotationsCount > 0}
      {statementsOnly}
      {statementKindGroup}
      catalogCollapsed={layout.catalogCollapsed}
      updateAvailableVersion={updateInfo?.update_available ? updateInfo.latest_version : null}
      onSearch={handleSearch}
      onClearSearch={async () => {
        searchReqSeq++;
        query = '';
        committedContentQuery = '';
        searchResults = null;
        searchStats = null;
        if (notesScopeActive) {
          await runNotesSearch('');
        } else {
          await refreshDocuments();
        }
      }}
      onSearchModeChange={handleSearchModeChange}
      onMetaFilterChange={handleMetaFilterChange}
      onWorkCategoryChange={handleWorkCategoryChange}
      onToggleNotesScope={handleToggleNotesScope}
      onToggleStatementsOnly={handleToggleStatementsOnly}
      onStatementKindGroupChange={handleChangeStatementKindGroup}
      onToggleCatalog={() => layout.toggleCatalogCollapsed()}
      onOpenClassics={() => (classicsModalOpen = true)}
      onVerifiedFilterChange={handleVerifiedFilterChange}
      onSortByChange={handleSortByChange}
      onToggleSortDir={handleToggleSortDir}
      onToggleLeftView={(view) => (activeLeftView = view)}
      onToggleInspector={() => layout.toggleInspectorOpen()}
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
      {#if !layout.zenMode}
        <div
          class="sidebar-slot"
          class:collapsed={layout.sidebarCollapsed}
          style="width: {layout.sidebarCollapsed ? 28 : layout.sidebarWidth}px;"
        >
          <LeftSidebar
            {lang}
            collapsed={layout.sidebarCollapsed}
            onToggleCollapse={() => layout.toggleSidebarCollapsed()}
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
            onConfigureCollectionBibSync={handleConfigureCollectionBibSync}
            onSyncCollectionBibNow={handleSyncCollectionBibNow}
            onSaveCurrentView={handleSaveCurrentView}
            onApplySavedSearch={handleApplySavedSearch}
            onDeleteSavedSearch={handleDeleteSavedSearch}
            onClearAllFilters={clearSidebarFilters}
            onRunKnnProposals={handleRunKnnProposals}
            onRunClustering={handleRunClustering}
            onRenameCluster={handleRenameCluster}
          />
        </div>
        {#if !layout.sidebarCollapsed}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="pane-resizer"
            class:active={layout.resizingPane === 'sidebar'}
            onpointerdown={(e) => layout.startResize('sidebar', e)}
          ></div>
        {/if}
      {/if}

      <div class="main-split">
        {#if !layout.zenMode}
          {#if layout.catalogCollapsed}
            <button
              type="button"
              class="catalog-collapsed-rail"
              title={tr(lang, 'expandCatalogTip')}
              aria-label={tr(lang, 'expandCatalogTip')}
              onclick={() => layout.toggleCatalogCollapsed()}
            >
              <span>»</span>
              <span class="rail-vert-label">{tr(lang, 'catalogHeader')}</span>
            </button>
          {:else}
            <section class="left-pane" style="width: {layout.catalogWidth}px;">
              {#if notesScopeActive}
                <NotesSearchResults
                  {lang}
                  {query}
                  hits={notesHits}
                  loading={notesLoading}
                  onOpenHit={handleOpenNoteHit}
                  onExitNotesScope={() => handleToggleNotesScope(false)}
                />
              {:else if activeLeftView === 'duplicates'}
                <DuplicatesView
                  {lang}
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
                  onRevealFile={(fid) => void runRevealFileInFolder(fid)}
                />
              {:else if displayedSearchResults !== null}
                <ContentResults
                  {lang}
                  query={committedContentQuery || query}
                  results={displayedSearchResults}
                  {searchStats}
                  workCategory={activeWorkCategory}
                  {statementsOnly}
                  {statementKindGroup}
                  activeFilterChips={activeContentFilterChips}
                  authorFilter={metaFilter.author ?? ''}
                  yearFromFilter={metaFilter.year_from ?? null}
                  yearToFilter={metaFilter.year_to ?? null}
                  onChangeWorkCategory={handleWorkCategoryChange}
                  onToggleStatementsOnly={handleToggleStatementsOnly}
                  onChangeStatementKindGroup={handleChangeStatementKindGroup}
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
                  onToggleVerified={(docId, verified) =>
                    handleToggleDocVerified(docId, verified, false)}
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
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class="pane-resizer"
              class:active={layout.resizingPane === 'catalog'}
              onpointerdown={(e) => layout.startResize('catalog', e)}
            ></div>
          {/if}
        {/if}

        <section class="center-pane">
          <ViewerTabs
            {lang}
            tabs={viewerTabs.tabs}
            activeTabId={viewerTabs.activeTabId}
            splitTabId={viewerTabs.splitTabId}
            focusedPane={viewerTabs.focusedPane}
            zenMode={layout.zenMode}
            llmProvider={settings?.llm_provider ?? 'disabled'}
            onSelectTab={(id) => viewerTabs.selectTab(id)}
            onCloseTab={(id) => {
              viewerTabs.closeTab(id);
              if (viewerTabs.tabs.length === 0 && layout.zenMode) {
                handleToggleZenMode();
              }
            }}
            onSetSplitTab={(id) => viewerTabs.setSplitTab(id)}
            onDuplicateTabToSplit={(id) => viewerTabs.duplicateTabToSplit(id)}
            onFocusPane={(pane) => (viewerTabs.focusedPane = pane)}
            onToggleZen={handleToggleZenMode}
            onSelectMatchIndex={handleSelectMatchIndex}
            onOpenExternal={(fid, page) => void runOpenFileExternal(fid, page, null)}
            onOpenLibraryDoc={handleOpenLibraryDocFromCite}
            onConsentAccepted={(prov) => {
              if (settings) {
                if (prov === 'anthropic') settings = { ...settings, llm_consent_anthropic: true };
                else if (prov === 'openai') settings = { ...settings, llm_consent_openai: true };
                else if (prov === 'gemini') settings = { ...settings, llm_consent_gemini: true };
              }
            }}
            onAnnotationsChanged={refreshAnnotationsCount}
            onStatusMessage={(msg) => (scanStatusText = msg)}
          />
        </section>

        {#if !layout.zenMode && layout.inspectorOpen}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="pane-resizer"
            class:active={layout.resizingPane === 'inspector'}
            onpointerdown={(e) => layout.startResize('inspector', e)}
          ></div>
          <section class="inspector-pane" style="width: {layout.inspectorWidth}px;">
            <RightInspector
              bind:this={inspectorRef}
              {lang}
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
              onClose={() => (layout.inspectorOpen = false)}
            />
          </section>
        {/if}
      </div>
    </div>

    <ContextMenu
      {lang}
      target={contextTarget}
      {presets}
      {collections}
      onClose={() => (contextTarget = null)}
      onOpenInternal={(fid, page) => {
        const doc = catalogStats.byFileId.get(fid);
        openInViewer(fid, doc?.title ?? `File #${fid}`, doc?.format ?? 'pdf', page ?? 1);
      }}
      onOpenExternal={(fid, page, tpl) => void runOpenFileExternal(fid, page, tpl)}
      onInspectMetadata={(docId) => {
        layout.inspectorOpen = true;
        requestSelectDocument(docId);
      }}
      onCopyBibtex={(docId) => handleCopyBibtex(docId)}
      onToggleCollection={(colId, docId, currentlyIn) => {
        if (currentlyIn) {
          void handleRemoveDocFromCollection(colId, docId);
        } else {
          void handleAddDocToCollection(colId, docId);
        }
      }}
      onRunOcr={(fid) => handleRunOcrForFile(fid)}
      onRevealInFolder={(fid) => void runRevealFileInFolder(fid)}
      onCopyPath={handleCopyPath}
    />

    <CommandPalette
      open={commandPaletteOpen}
      {lang}
      documents={allDocuments}
      {collections}
      {savedSearches}
      actions={paletteActions}
      onOpenDocument={(doc) => {
        requestSelectDocument(doc.document_id, () => {
          openInViewer(doc.file_id, doc.title, doc.format, 1);
        });
      }}
      onSelectCollection={(colId) => void applySidebarFilter({ collection_id: colId })}
      onApplySavedSearch={(sv) => void handleApplySavedSearch(sv)}
      onClose={() => (commandPaletteOpen = false)}
    />

    <SuggestedClassicsModal
      open={classicsModalOpen}
      {lang}
      onSelectLibraryDoc={(docId) => {
        layout.inspectorOpen = true;
        requestSelectDocument(docId);
      }}
      onStatusMessage={(msg) => (scanStatusText = msg)}
      onClose={() => (classicsModalOpen = false)}
    />

    {#if collectionBibModal}
      <div
        class="consent-backdrop"
        role="dialog"
        aria-modal="true"
        aria-label="Sincronizzazione .bib collezione"
      >
        <div class="consent-dialog">
          <h3>
            {lang === 'en'
              ? `Dedicated .bib file — ${collectionBibModal.name}`
              : `File .bib dedicato — ${collectionBibModal.name}`}
          </h3>
          <p>
            {lang === 'en'
              ? 'Papero keeps this external .bib file automatically synced whenever you add/remove documents from this collection or edit their metadata (strictly outside source folders).'
              : 'Papero mantiene sincronizzato automaticamente questo file .bib esterno ogni volta che aggiungi/rimuovi documenti da questa collezione o modifichi i loro metadati (sempre fuori dalle cartelle sorgente).'}
          </p>
          {#if collectionBibModal.bib_sync_enabled && collectionBibModal.bib_sync_path}
            <p class="consent-excl" style="word-break: break-all;">
              {collectionBibModal.bib_sync_path}
            </p>
          {:else}
            <p class="consent-note">
              {lang === 'en'
                ? 'No .bib file linked to this collection yet.'
                : 'Nessun file .bib ancora associato a questa collezione.'}
            </p>
          {/if}
          {#if collectionBibErr}
            <div class="consent-warn">{collectionBibErr}</div>
          {/if}
          <div class="consent-actions">
            {#if collectionBibModal.bib_sync_enabled && collectionBibModal.bib_sync_path}
              <button
                type="button"
                disabled={collectionBibBusy}
                onclick={handleUnlinkCollectionBib}
              >
                {lang === 'en' ? 'Unlink' : 'Scollega'}
              </button>
              <button
                type="button"
                disabled={collectionBibBusy}
                onclick={() => collectionBibModal && handleSyncCollectionBibNow(collectionBibModal)}
              >
                {lang === 'en' ? 'Sync now' : 'Sincronizza ora'}
              </button>
            {/if}
            <button
              type="button"
              class="primary"
              disabled={collectionBibBusy}
              onclick={handleChooseCollectionBibPath}
            >
              {collectionBibModal.bib_sync_path
                ? lang === 'en'
                  ? 'Change .bib file…'
                  : 'Cambia file .bib…'
                : lang === 'en'
                  ? 'Create / Choose .bib file…'
                  : 'Crea / Scegli file .bib…'}
            </button>
            <button type="button" onclick={() => (collectionBibModal = null)}>
              {tr(lang, 'close')}
            </button>
          </div>
        </div>
      </div>
    {/if}

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
        {updateInfo}
        onUpdateInfoChange={(info) => (updateInfo = info)}
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
  .app-shell.is-resizing {
    user-select: none;
    cursor: col-resize;
  }
  .workspace-body {
    flex: 1;
    display: flex;
    overflow: hidden;
  }
  .sidebar-slot {
    flex-shrink: 0;
    height: 100%;
    overflow: hidden;
  }
  .sidebar-slot.collapsed {
    width: 28px !important;
  }
  .main-split {
    flex: 1;
    min-width: 0;
    display: flex;
    overflow: hidden;
  }
  .left-pane {
    flex-shrink: 0;
    background: var(--bg-panel);
    overflow: hidden;
  }
  .center-pane {
    flex: 1;
    min-width: 240px;
    overflow: hidden;
  }
  .inspector-pane {
    flex-shrink: 0;
    overflow: hidden;
  }
  .pane-resizer {
    width: 5px;
    flex-shrink: 0;
    cursor: col-resize;
    background: var(--border);
    position: relative;
    transition: background 0.12s;
    z-index: 6;
  }
  .pane-resizer:hover,
  .pane-resizer.active {
    background: var(--accent);
  }
  .catalog-collapsed-rail {
    width: 26px;
    flex-shrink: 0;
    border: none;
    border-right: 1px solid var(--border);
    border-radius: 0;
    background: var(--bg-panel);
    color: var(--text-muted);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    padding: 8px 0;
    cursor: pointer;
    font-size: 12px;
    font-weight: 600;
  }
  .catalog-collapsed-rail:hover {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .rail-vert-label {
    writing-mode: vertical-rl;
    text-orientation: mixed;
    font-size: 10.5px;
    letter-spacing: 0.04em;
    text-transform: uppercase;
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
