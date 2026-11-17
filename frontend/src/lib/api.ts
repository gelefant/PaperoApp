import { Channel, invoke } from '@tauri-apps/api/core';
import type {
  AltFormulaImportReport,
  BackupReport,
  BulkMetadataProgressEvent,
  ClusteringReport,
  ContentSearchDocResult,
  DocumentListItem,
  DocumentMetadataView,
  DownloadProgressEvent,
  DuplicateGroupView,
  DuplicateReport,
  ExternalReaderPreset,
  ExtractedSpan,
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
  ManualMetadataEdit,
  MetadataSearchFilter,
  MlEnvironmentStatus,
  MscTreeNode,
  OcrEnvironmentStatus,
  OcrFileReport,
  OcrProgressEvent,
  ResolveOutcome,
  RestoreReport,
  SaveAndFetchResult,
  ScanProgressEvent,
  SourceRoot,
  TagSummary,
  TopicClusterView,
  UiSettingsPayload,
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
    workCategory?: string | null
  ) =>
    invoke<ContentSearchDocResult[]>('search_content_bm25', {
      query,
      rootIds: rootIds ?? null,
      limit,
      workCategory: workCategory && workCategory !== 'all' ? workCategory : null,
    }),
  searchContentHybrid: (
    query: string,
    rootIds?: string[],
    rerankOverride?: boolean | null,
    limitOverride?: number | null,
    workCategory?: string | null
  ) =>
    invoke<HybridSearchResponse>('search_content_hybrid', {
      query,
      rootIds: rootIds ?? null,
      rerankOverride: rerankOverride ?? null,
      limitOverride: limitOverride ?? null,
      workCategory: workCategory && workCategory !== 'all' ? workCategory : null,
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
    resetUnlocked = true
  ) =>
    invoke<SaveAndFetchResult>('save_and_fetch_document_metadata', {
      edit,
      lockedFields: lockedFields ?? null,
      resetUnlocked,
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
  confirmCandidate: (documentId: number, candidateId: number) =>
    invoke<DocumentMetadataView>('confirm_candidate', { documentId, candidateId }),
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
};
