import { Channel, invoke } from '@tauri-apps/api/core';
import type {
  AltFormulaImportReport,
  AnnotationSearchHit,
  BackupReport,
  BibtexSyncReport,
  BulkMetadataProgressEvent,
  CiteKeyPayload,
  ClusteringReport,
  CollectionView,
  ContentSearchDocResult,
  ContentSearchFilter,
  DocLlmScope,
  DocumentListItem,
  DocumentMetadataView,
  DocumentRelationsView,
  DownloadProgressEvent,
  DuplicateGroupView,
  DuplicateReport,
  EmbedProgressStatus,
  ExternalReaderPreset,
  ExtractedSpan,
  FileAnnotation,
  FileReadingState,
  FileViewerInfo,
  HybridSearchResponse,
  ImportMetadataReport,
  KnnProposeReport,
  LlmAnswerResult,
  LlmKeyStatus,
  LlmKeyVerifyResult,
  LlmModelInfo,
  LlmPromptPreview,
  LlmProvider,
  LlmStreamEvent,
  ManualMetadataEdit,
  MetadataSearchFilter,
  MlEnvironmentStatus,
  MscTreeNode,
  NewFileAnnotation,
  OcrEnvironmentStatus,
  OcrFileReport,
  OcrProgressEvent,
  ReadingStatePatch,
  ResolveOutcome,
  RestoreReport,
  SaveAndFetchResult,
  SavedSearchView,
  ScanProgressEvent,
  SmartPasteApplyResult,
  SourceRoot,
  TagSummary,
  TopicClusterView,
  UiSettingsPayload,
  UpdateFileAnnotation,
} from './types';

export const api = {
  listRoots: () => invoke<SourceRoot[]>('list_roots'),
  addSourceRoot: (path: string, label?: string, isNetworkOrCloud = false) =>
    invoke<SourceRoot>('add_source_root', {
      path,
      label: label ?? null,
      isNetworkOrCloud,
    }),
  rebindSourceRoot: (rootId: string, newPath: string) =>
    invoke<SourceRoot>('rebind_source_root', { rootId, newPath }),
  toggleSourceRoot: (rootId: string, enabled: boolean) =>
    invoke<void>('toggle_source_root', { rootId, enabled }),
  setRootCloudExclusion: (rootId: string, excluded: boolean) =>
    invoke<void>('set_root_cloud_exclusion', { rootId, excluded }),
  deleteSourceRoot: (rootId: string) =>
    invoke<void>('delete_source_root', { rootId }),

  triggerRescan: (
    onEvent: (ev: ScanProgressEvent) => void,
    onlyRootId?: string
  ) => {
    const channel = new Channel<ScanProgressEvent>();
    channel.onmessage = onEvent;
    return invoke<void>('trigger_rescan', {
      onlyRootId: onlyRootId ?? null,
      onEvent: channel,
    });
  },
  controlRescan: (action: 'pause' | 'resume' | 'cancel') =>
    invoke<void>('control_rescan', { action }),

  listDocuments: () => invoke<DocumentListItem[]>('list_documents'),
  searchMetadata: (filter: MetadataSearchFilter) =>
    invoke<DocumentListItem[]>('search_metadata', { filter }),
  searchContentBm25: (
    query: string,
    rootIds?: string[],
    limit = 50,
    workCategory?: string | null,
    filter?: ContentSearchFilter | null
  ) =>
    invoke<ContentSearchDocResult[]>('search_content_bm25', {
      query,
      rootIds: rootIds ?? null,
      limit,
      workCategory: workCategory && workCategory !== 'all' ? workCategory : null,
      filter: filter ?? null,
    }),
  searchContentHybrid: (
    query: string,
    rootIds?: string[],
    rerankOverride?: boolean | null,
    limitOverride?: number | null,
    workCategory?: string | null,
    filter?: ContentSearchFilter | null
  ) =>
    invoke<HybridSearchResponse>('search_content_hybrid', {
      query,
      rootIds: rootIds ?? null,
      rerankOverride: rerankOverride ?? null,
      limitOverride: limitOverride ?? null,
      workCategory: workCategory && workCategory !== 'all' ? workCategory : null,
      filter: filter ?? null,
    }),
  listModelStatuses: () => invoke<MlEnvironmentStatus>('list_model_statuses'),
  downloadMlProfile: (
    profile: 'light' | 'quality',
    onEvent: (ev: DownloadProgressEvent) => void
  ) => {
    const channel = new Channel<DownloadProgressEvent>();
    channel.onmessage = onEvent;
    return invoke<MlEnvironmentStatus>('download_ml_profile', {
      profile,
      onEvent: channel,
    });
  },
  unloadMlModels: () => invoke<MlEnvironmentStatus>('unload_ml_models'),
  importAltFormulaText: (fileId: number, markdownText: string) =>
    invoke<AltFormulaImportReport>('import_alt_formula_text', {
      fileId,
      markdownText,
    }),

  getDocumentMetadata: (documentId: number) =>
    invoke<DocumentMetadataView>('get_document_metadata', { documentId }),
  setDocumentMetadataVerified: (documentId: number, verified: boolean) =>
    invoke<DocumentMetadataView>('set_document_metadata_verified_cmd', {
      documentId,
      verified,
    }),
  saveDocumentMetadata: (edit: ManualMetadataEdit, lockedFields?: string[]) =>
    invoke<DocumentMetadataView>('save_document_metadata', {
      edit,
      lockedFields: lockedFields ?? null,
    }),
  saveAndFetchDocumentMetadata: (
    edit: ManualMetadataEdit,
    lockedFields?: string[],
    resetUnlocked = true,
    overwriteQueryFields = false
  ) =>
    invoke<SaveAndFetchResult>('save_and_fetch_document_metadata', {
      edit,
      lockedFields: lockedFields ?? null,
      resetUnlocked,
      overwriteQueryFields,
    }),
  fetchDocumentMetadata: (documentId: number) =>
    invoke<ResolveOutcome>('fetch_document_metadata', { documentId }),
  fetchAllMetadata: (
    onEvent: (ev: BulkMetadataProgressEvent) => void,
    onlyUnresolved = true
  ) => {
    const channel = new Channel<BulkMetadataProgressEvent>();
    channel.onmessage = onEvent;
    return invoke<BulkMetadataProgressEvent>('fetch_all_metadata_cmd', {
      onlyUnresolved,
      onEvent: channel,
    });
  },
  cancelMetadataBatch: () => invoke<void>('cancel_metadata_batch_cmd'),
  cancelSingleMetadata: () => invoke<void>('cancel_single_metadata_cmd'),
  getEmbedProgress: () => invoke<EmbedProgressStatus>('get_embed_progress_cmd'),
  setEmbedPaused: (paused: boolean) =>
    invoke<EmbedProgressStatus>('set_embed_paused_cmd', { paused }),
  confirmCandidate: (documentId: number, candidateId: number, overwriteQueryFields = false) =>
    invoke<DocumentMetadataView>('confirm_candidate', {
      documentId,
      candidateId,
      overwriteQueryFields,
    }),
  dismissCandidate: (candidateId: number) =>
    invoke<void>('dismiss_candidate', { candidateId }),

  getDocumentBibtex: (documentId: number) =>
    invoke<string>('get_document_bibtex', { documentId }),
  exportBibtexMany: (documentIds: number[], filePath?: string | null) =>
    invoke<string>('export_bibtex_many', {
      documentIds,
      filePath: filePath ?? null,
    }),
  exportCsvMany: (documentIds: number[], filePath?: string | null) =>
    invoke<string>('export_csv_many', {
      documentIds,
      filePath: filePath ?? null,
    }),
  importMetadataFile: (opts: {
    path?: string | null;
    content?: string | null;
    format?: 'bib' | 'csl_json' | null;
    targetDocumentId?: number | null;
  }) =>
    invoke<ImportMetadataReport>('import_metadata_file', {
      path: opts.path ?? null,
      content: opts.content ?? null,
      format: opts.format ?? null,
      targetDocumentId: opts.targetDocumentId ?? null,
    }),

  listDuplicates: () => invoke<DuplicateGroupView[]>('list_duplicates'),
  runDedupScan: () => invoke<DuplicateReport>('run_dedup_scan'),
  mergeDuplicates: (groupId: number, keepDocumentId: number) =>
    invoke<void>('merge_duplicates', { groupId, keepDocumentId }),
  resolveDuplicate: (
    groupId: number,
    decision: 'distinct' | 'versions' | 'editions'
  ) => invoke<void>('resolve_duplicate', { groupId, decision }),

  getViewerDoc: (fileId: number) =>
    invoke<FileViewerInfo>('get_viewer_doc', { fileId }),
  readPdfRange: (fileId: number, start: number, length: number) =>
    invoke<ArrayBuffer>('read_pdf_range_cmd', { fileId, start, length }),
  renderDjvuPage: (fileId: number, pageOrd: number, dpi = 150) =>
    invoke<ArrayBuffer>('render_djvu_page_cmd', { fileId, pageOrd, dpi }),
  getDjvuPageText: (fileId: number, pageOrd: number) =>
    invoke<ExtractedSpan[]>('get_djvu_page_text', { fileId, pageOrd }),

  openFileExternal: (
    fileId: number,
    pageOrd?: number,
    templateOverride?: string | null
  ) =>
    invoke<void>('open_file_external', {
      fileId,
      pageOrd: pageOrd ?? null,
      templateOverride: templateOverride ?? null,
    }),
  revealFileInFolder: (fileId: number) =>
    invoke<void>('reveal_file_in_folder', { fileId }),
  listExternalPresets: () =>
    invoke<ExternalReaderPreset[]>('list_external_presets'),

  getUiSettings: () => invoke<UiSettingsPayload>('get_ui_settings'),
  saveUiSettings: (settings: UiSettingsPayload) =>
    invoke<void>('save_ui_settings', { settings }),

  openDetachedViewerWindow: (
    fileId: number,
    pageOrd: number,
    title: string,
    bboxesJson?: string | null
  ) =>
    invoke<void>('open_detached_viewer_window', {
      fileId,
      pageOrd,
      title,
      bboxesJson: bboxesJson ?? null,
    }),

  getMscTree: () => invoke<MscTreeNode[]>('get_msc_tree'),
  runKnnMscProposals: (k?: number | null) =>
    invoke<KnnProposeReport>('run_knn_msc_proposals', { k: k ?? null }),
  confirmMscClassification: (documentId: number, mscCode?: string | null) =>
    invoke<void>('confirm_msc_classification', {
      documentId,
      mscCode: mscCode ?? null,
    }),
  rejectMscProposal: (documentId: number) =>
    invoke<void>('reject_msc_proposal', { documentId }),

  runTopicClustering: (requestedK?: number | null) =>
    invoke<ClusteringReport>('run_topic_clustering_cmd', {
      requestedK: requestedK ?? null,
    }),
  listTopicClusters: () =>
    invoke<TopicClusterView[]>('list_topic_clusters'),
  renameTopicCluster: (clusterId: number, label: string) =>
    invoke<void>('rename_topic_cluster_cmd', { clusterId, label }),
  setDocumentCluster: (documentId: number, clusterId: number) =>
    invoke<void>('set_document_cluster_cmd', { documentId, clusterId }),
  removeDocumentCluster: (documentId: number, clusterId: number) =>
    invoke<void>('remove_document_cluster_cmd', { documentId, clusterId }),

  listAllTags: () => invoke<TagSummary[]>('list_all_tags_cmd'),
  addDocumentTag: (documentId: number, tagName: string) =>
    invoke<void>('add_document_tag_cmd', { documentId, tagName }),
  removeDocumentTag: (documentId: number, tagName: string) =>
    invoke<void>('remove_document_tag_cmd', { documentId, tagName }),

  createBackup: (destPath: string) =>
    invoke<BackupReport>('create_backup_cmd', { destPath }),
  updateBackupNow: () =>
    invoke<BackupReport>('update_backup_now_cmd'),
  restoreBackup: (srcPath: string) =>
    invoke<RestoreReport>('restore_backup_cmd', { srcPath }),

  getOcrStatus: () => invoke<OcrEnvironmentStatus>('get_ocr_status'),
  runOcrForFile: (fileId: number, onEvent?: (ev: OcrProgressEvent) => void) => {
    const channel = new Channel<OcrProgressEvent>();
    if (onEvent) channel.onmessage = onEvent;
    return invoke<OcrFileReport>('run_ocr_for_file_cmd', { fileId, onEvent: channel });
  },
  runPendingOcr: (onEvent?: (ev: OcrProgressEvent) => void) => {
    const channel = new Channel<OcrProgressEvent>();
    if (onEvent) channel.onmessage = onEvent;
    return invoke<OcrFileReport[]>('run_pending_ocr_cmd', { onEvent: channel });
  },

  getLlmKeyStatus: () => invoke<LlmKeyStatus>('get_llm_key_status'),
  saveLlmApiKey: (provider: LlmProvider, apiKey: string) =>
    invoke<void>('save_llm_api_key', { provider, apiKey }),
  removeLlmApiKey: (provider: LlmProvider) =>
    invoke<void>('remove_llm_api_key', { provider }),
  verifyLlmKey: (
    provider: LlmProvider,
    apiKeyOverride?: string | null,
    ollamaUrlOverride?: string | null
  ) =>
    invoke<LlmKeyVerifyResult>('verify_llm_key_cmd', {
      provider,
      apiKeyOverride: apiKeyOverride ?? null,
      ollamaUrlOverride: ollamaUrlOverride ?? null,
    }),
  listLlmModels: (
    provider: LlmProvider,
    ollamaUrlOverride?: string | null,
    apiKeyOverride?: string | null
  ) =>
    invoke<LlmModelInfo[]>('list_llm_models_cmd', {
      provider,
      ollamaUrlOverride: ollamaUrlOverride ?? null,
      apiKeyOverride: apiKeyOverride ?? null,
    }),
  previewLlmPrompt: (
    query: string,
    results: ContentSearchDocResult[],
    maxPassages?: number | null,
    maxInputTokens?: number | null
  ) =>
    invoke<LlmPromptPreview>('preview_llm_prompt_cmd', {
      query,
      results,
      maxPassages: maxPassages ?? null,
      maxInputTokens: maxInputTokens ?? null,
    }),
  acceptLlmCloudConsent: (provider: LlmProvider) =>
    invoke<void>('accept_llm_cloud_consent_cmd', { provider }),
  generateLlmAnswer: (
    query: string,
    results: ContentSearchDocResult[],
    maxPassages?: number | null,
    maxInputTokens?: number | null
  ) =>
    invoke<LlmAnswerResult>('generate_llm_answer_cmd', {
      query,
      results,
      maxPassages: maxPassages ?? null,
      maxInputTokens: maxInputTokens ?? null,
    }),
  generateLlmAnswerStream: (
    query: string,
    results: ContentSearchDocResult[],
    onEvent: (ev: LlmStreamEvent) => void,
    maxPassages?: number | null,
    maxInputTokens?: number | null
  ) => {
    const channel = new Channel<LlmStreamEvent>();
    channel.onmessage = onEvent;
    return invoke<LlmAnswerResult>('generate_llm_answer_stream_cmd', {
      query,
      results,
      maxPassages: maxPassages ?? null,
      maxInputTokens: maxInputTokens ?? null,
      onEvent: channel,
    });
  },
  previewDocLlmPrompt: (opts: {
    documentId: number;
    fileId: number;
    scope: DocLlmScope;
    pageOrd: number;
    selectionText?: string | null;
    question: string;
    maxPassages?: number | null;
    maxInputTokens?: number | null;
  }) =>
    invoke<LlmPromptPreview>('preview_doc_llm_prompt_cmd', {
      documentId: opts.documentId,
      fileId: opts.fileId,
      scope: opts.scope,
      pageOrd: opts.pageOrd,
      selectionText: opts.selectionText ?? null,
      question: opts.question,
      maxPassages: opts.maxPassages ?? null,
      maxInputTokens: opts.maxInputTokens ?? null,
    }),
  askDocumentLlmStream: (
    opts: {
      documentId: number;
      fileId: number;
      scope: DocLlmScope;
      pageOrd: number;
      selectionText?: string | null;
      question: string;
      maxPassages?: number | null;
      maxInputTokens?: number | null;
    },
    onEvent: (ev: LlmStreamEvent) => void
  ) => {
    const channel = new Channel<LlmStreamEvent>();
    channel.onmessage = onEvent;
    return invoke<LlmAnswerResult>('ask_document_llm_stream_cmd', {
      documentId: opts.documentId,
      fileId: opts.fileId,
      scope: opts.scope,
      pageOrd: opts.pageOrd,
      selectionText: opts.selectionText ?? null,
      question: opts.question,
      maxPassages: opts.maxPassages ?? null,
      maxInputTokens: opts.maxInputTokens ?? null,
      onEvent: channel,
    });
  },

  // Citation key & Live BibTeX Auto-Sync (v0.5.0)
  getDocumentCiteKey: (documentId: number) =>
    invoke<CiteKeyPayload>('get_document_cite_key_cmd', { documentId }),
  syncBibtexNow: () => invoke<BibtexSyncReport>('sync_bibtex_now_cmd'),

  // Smart Paste & Document Relations (v0.5.0)
  applySmartPaste: (documentId: number, rawInput: string) =>
    invoke<SmartPasteApplyResult>('apply_smart_paste_cmd', { documentId, rawInput }),
  getDocumentRelations: (documentId: number) =>
    invoke<DocumentRelationsView>('get_document_relations_cmd', { documentId }),

  // Non-destructive Annotations, Bookmarks, Notes & Reading State (v0.5.0)
  listFileAnnotations: (fileId: number) =>
    invoke<FileAnnotation[]>('list_file_annotations_cmd', { fileId }),
  createFileAnnotation: (fileId: number, input: NewFileAnnotation) =>
    invoke<FileAnnotation>('create_file_annotation_cmd', { fileId, input }),
  updateFileAnnotation: (annotationId: number, patch: UpdateFileAnnotation) =>
    invoke<FileAnnotation>('update_file_annotation_cmd', { annotationId, patch }),
  deleteFileAnnotation: (annotationId: number) =>
    invoke<void>('delete_file_annotation_cmd', { annotationId }),
  togglePageBookmark: (fileId: number, pageOrd: number, label?: string | null) =>
    invoke<FileAnnotation | null>('toggle_page_bookmark_cmd', {
      fileId,
      pageOrd,
      label: label ?? null,
    }),
  searchAnnotations: (query: string, fileId?: number | null) =>
    invoke<AnnotationSearchHit[]>('search_annotations_cmd', {
      query,
      fileId: fileId ?? null,
    }),
  exportAnnotationsMarkdown: (fileId?: number | null, filePath?: string | null) =>
    invoke<string>('export_annotations_markdown_cmd', {
      fileId: fileId ?? null,
      filePath: filePath ?? null,
    }),
  upsertReadingState: (fileId: number, patch: ReadingStatePatch) =>
    invoke<FileReadingState>('upsert_reading_state_cmd', { fileId, patch }),
  setDocumentReadingStatus: (documentId: number, status: string) =>
    invoke<void>('set_document_reading_status_cmd', { documentId, status }),
  setDocumentStarred: (documentId: number, starred: boolean) =>
    invoke<void>('set_document_starred_cmd', { documentId, starred }),

  // Collections & Saved Views (v0.5.0)
  listCollections: () => invoke<CollectionView[]>('list_collections_cmd'),
  createCollection: (name: string, colorHex?: string | null) =>
    invoke<CollectionView>('create_collection_cmd', { name, colorHex: colorHex ?? null }),
  renameCollection: (collectionId: number, name: string) =>
    invoke<void>('rename_collection_cmd', { collectionId, name }),
  deleteCollection: (collectionId: number) =>
    invoke<void>('delete_collection_cmd', { collectionId }),
  reorderCollections: (orderedIds: number[]) =>
    invoke<void>('reorder_collections_cmd', { orderedIds }),
  addDocsToCollection: (collectionId: number, documentIds: number[]) =>
    invoke<number>('add_docs_to_collection_cmd', { collectionId, documentIds }),
  removeDocsFromCollection: (collectionId: number, documentIds: number[]) =>
    invoke<number>('remove_docs_from_collection_cmd', { collectionId, documentIds }),
  listSavedSearches: () => invoke<SavedSearchView[]>('list_saved_searches_cmd'),
  createSavedSearch: (
    name: string,
    searchMode: 'metadata' | 'content',
    queryText: string,
    filterJson: string
  ) =>
    invoke<SavedSearchView>('create_saved_search_cmd', {
      name,
      searchMode,
      queryText,
      filterJson,
    }),
  deleteSavedSearch: (savedSearchId: number) =>
    invoke<void>('delete_saved_search_cmd', { savedSearchId }),

  // Batch Catalog Actions (v0.5.0)
  batchAddTag: (documentIds: number[], tagName: string) =>
    invoke<number>('batch_add_tag_cmd', { documentIds, tagName }),
  batchRemoveTag: (documentIds: number[], tagName: string) =>
    invoke<number>('batch_remove_tag_cmd', { documentIds, tagName }),
  batchSetVerified: (documentIds: number[], verified: boolean) =>
    invoke<number>('batch_set_verified_cmd', { documentIds, verified }),
  batchEnqueueOcr: (documentIds: number[]) =>
    invoke<number>('batch_enqueue_ocr_cmd', { documentIds }),
};
