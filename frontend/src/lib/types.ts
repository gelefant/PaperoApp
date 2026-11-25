export interface NormalizedRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface SourceRoot {
  id: string;
  label: string;
  root_path: string;
  is_enabled: boolean;
  is_network_or_cloud: boolean;
  is_reachable: boolean;
  exclude_from_cloud_llm: boolean;
  last_scan_at: number | null;
}

export interface DocumentListItem {
  document_id: number;
  file_id: number;
  root_id: string;
  root_label: string;
  work_type?: string;
  title: string;
  authors_summary: string;
  year: number | null;
  format: 'pdf' | 'djvu' | string;
  rel_path: string;
  page_count: number | null;
  is_available: boolean;
  index_state: string;
  error_message: string | null;
  msc_primary: string | null;
  msc_primary_status: string;
  has_text_layer: boolean;
  ocr_status: 'none' | 'queued' | 'running' | 'done' | 'failed' | string;
  metadata_source?: string | null;
  metadata_verified?: boolean;
  reading_status?: 'unread' | 'reading' | 'read' | string;
  is_starred?: boolean;
  last_opened_at?: number | null;
  last_page_ord?: number | null;
}

export interface ContentSearchPassageHit {
  passage_id: number;
  file_id: number;
  page_ord: number;
  printed_label: string | null;
  score: number;
  text_content: string;
  bboxes: NormalizedRect[];
  is_alt_formula?: boolean;
  statement_label?: string | null;
  statement_kind?: string | null;
}

export interface ContentSearchDocResult {
  document_id: number;
  file_id: number;
  work_type?: string;
  title: string;
  format: 'pdf' | 'djvu' | string;
  rel_path: string;
  is_available: boolean;
  max_score: number;
  passages: ContentSearchPassageHit[];
}

export interface FilePageInfo {
  page_ord: number;
  printed_label: string | null;
  width_pts: number;
  height_pts: number;
}

export interface OutlineItem {
  title: string;
  page_ord: number | null;
  children: OutlineItem[];
}

export interface FileViewerInfo {
  file_id: number;
  document_id: number;
  blake3_hash?: string;
  title: string;
  format: 'pdf' | 'djvu' | string;
  page_count: number;
  byte_size: number;
  is_available: boolean;
  abs_path: string;
  pages: FilePageInfo[];
  outline: OutlineItem[];
  last_page_ord?: number | null;
  zoom_mode?: 'fit-width' | 'fit-page' | 'custom' | string | null;
  custom_zoom?: number | null;
  rotation_deg?: number;
}

export interface ExtractedSpan {
  text: string;
  bbox: NormalizedRect;
}

export interface ViewerTargetPassage {
  page_ord: number;
  printed_label: string | null;
  bboxes: NormalizedRect[];
  text_content: string;
}

export type BibtexKeyPattern =
  | 'mr_default'
  | 'author_year'
  | 'author_year_word'
  | 'author_lower_year';

export type BibtexFormat = 'bibtex' | 'biblatex';

export interface CiteKeyPayload {
  key: string;
  cite_command: string;
}

export interface BibtexSyncReport {
  path: string;
  count: number;
}

export interface ViewerTab {
  tabId: string;
  fileId: number;
  documentId?: number;
  title: string;
  format: string;
  targetPage: number;
  /** True when opened from a search hit or explicit citation jump (do not override with last_page_ord). */
  explicitPageJump?: boolean;
  highlightRects: NormalizedRect[];
  matches: ViewerTargetPassage[];
  activeMatchIndex: number;
}

export interface ExternalReaderPreset {
  id: string;
  label: string;
  os: string;
  formats: string[];
  template: string;
  requires_accessibility: boolean;
  note: string | null;
}

export interface UiSettingsPayload {
  viewer_open_in_separate_window: boolean;
  external_pdf_command: string | null;
  external_djvu_command: string | null;
  linux_reveal_mode: 'dbus_with_fallback' | 'parent_xdg_open_only';
  max_worker_threads: number;
  cache_limit_mb: number;
  polite_email: string | null;
  network_lookups_enabled: boolean;
  mrlookup_enabled_single: boolean;
  mrlookup_enabled_bulk: boolean;
  zbmath_enabled: boolean;
  crossref_enabled: boolean;
  arxiv_enabled: boolean;
  harvard_enabled: boolean;
  loc_enabled: boolean;
  googlebooks_enabled: boolean;
  openlibrary_enabled: boolean;
  dblp_enabled: boolean;
  openalex_enabled: boolean;
  datacite_enabled: boolean;
  inspirehep_enabled: boolean;
  europepmc_enabled: boolean;
  semanticscholar_enabled: boolean;
  metadata_source_priority: string[];
  ml_profile: 'light' | 'quality' | string;
  reranker_enabled: boolean;
  model_idle_unload_secs: number;
  hybrid_top_n: number;
  rerank_top_m_light: number;
  rerank_top_m_quality: number;
  search_top_k_docs: number;
  search_min_score: number;
  doc_score_aggregation: 'max' | 'top2_mean' | string;
  ui_language: 'it' | 'en' | string;
  ui_theme: 'system' | 'light' | 'dark' | string;
  first_run_completed: boolean;
  fs_watcher_enabled: boolean;
  tesseract_bin: string | null;
  ocr_languages: string | null;
  ocr_dpi: number;
  ocr_front_pages_only?: boolean;
  ocr_front_pages_count?: number;
  check_updates_on_startup?: boolean;
  llm_provider: LlmProvider;
  llm_model: string | null;
  llm_ollama_url: string;
  llm_max_passages: number;
  llm_max_input_tokens: number;
  llm_max_output_tokens: number;
  llm_consent_anthropic: boolean;
  llm_consent_openai: boolean;
  llm_consent_gemini: boolean;
  backup_path?: string | null;
  auto_backup_enabled?: boolean;
  last_backup_at?: number | null;
  last_backup_bytes?: number | null;
  bibtex_key_pattern?: BibtexKeyPattern;
  bibtex_format?: BibtexFormat;
  bibtex_auto_sync_path?: string | null;
  bibtex_auto_sync_enabled?: boolean;
}

export interface HybridSearchStats {
  mode: string;
  bm25_hits: number;
  vector_hits: number;
  rrf_candidates: number;
  reranked_count: number;
  total_ms: number;
  bm25_ms: number;
  embed_query_ms: number;
  vector_ms: number;
  rerank_ms: number;
}

export interface HybridSearchResponse {
  results: ContentSearchDocResult[];
  stats: HybridSearchStats;
}

export interface ModelProfileStatus {
  profile: 'light' | 'quality' | string;
  label: string;
  ready: boolean;
  total_bytes: number;
  downloaded_bytes: number;
  embedder_repo: string;
  reranker_repo: string;
  missing_files: string[];
}

export interface MlEnvironmentStatus {
  onnxruntime_found: boolean;
  onnxruntime_path: string | null;
  embedder_loaded: boolean;
  reranker_loaded: boolean;
  rss_mb: number;
  active_profile: 'light' | 'quality' | string;
  profiles: ModelProfileStatus[];
}

export interface DownloadProgressEvent {
  profile: 'light' | 'quality' | string;
  artifact_name: string;
  artifact_bytes_done: number;
  artifact_bytes_total: number;
  profile_bytes_done: number;
  profile_bytes_total: number;
  bytes_per_sec: number;
  finished: boolean;
}

export interface AltFormulaImportReport {
  file_id: number;
  document_id: number;
  cache_path: string;
  passages_inserted: number;
  pages_parsed: number;
}

export interface BibliographicAuthor {
  family_name: string;
  given_name: string;
  role: 'author' | 'editor' | string;
}

export interface BibliographicRecord {
  work_type: string;
  title: string;
  authors: BibliographicAuthor[];
  journal_abbrev: string | null;
  journal_full: string | null;
  booktitle: string | null;
  series: string | null;
  volume: string | null;
  issue_number: string | null;
  year: number | null;
  pages: string | null;
  edition: string | null;
  note: string | null;
  publisher: string | null;
  address: string | null;
  isbn: string | null;
  issn: string | null;
  coden: string | null;
  doi: string | null;
  arxiv_id: string | null;
  arxiv_version: string | null;
  mr_number: string | null;
  mr_number_display: string | null;
  mr_reviewer: string | null;
  zbl_number: string | null;
  url: string | null;
  msc_primary: string | null;
  msc_secondary: string[];
  abstract_text: string | null;
  raw_mrlookup_bibtex: string | null;
  source: string;
  license_note: string | null;
}

export interface CandidateRow {
  id: number;
  document_id: number;
  source: string;
  match_score: number;
  record: BibliographicRecord;
}

export interface DocumentMetadataView {
  document_id: number;
  work_type: string;
  title: string;
  authors: BibliographicAuthor[];
  journal_abbrev: string | null;
  journal_full: string | null;
  booktitle: string | null;
  series: string | null;
  volume: string | null;
  issue_number: string | null;
  year: number | null;
  pages: string | null;
  edition: string | null;
  publisher: string | null;
  address: string | null;
  isbn: string | null;
  issn: string | null;
  doi: string | null;
  arxiv_id: string | null;
  arxiv_version: string | null;
  mr_number: string | null;
  mr_number_display: string | null;
  mr_reviewer: string | null;
  zbl_number: string | null;
  url: string | null;
  msc_primary: string | null;
  msc_primary_status: string;
  msc_secondary: string[];
  abstract_text: string | null;
  citation_key_override?: string | null;
  raw_mrlookup_bibtex: string | null;
  metadata_source: string | null;
  metadata_license_note: string | null;
  manual_locked_fields: string[];
  exclude_from_cloud_llm: boolean;
  metadata_verified: boolean;
  reading_status?: 'unread' | 'reading' | 'read' | string;
  is_starred?: boolean;
  collection_ids?: number[];
  tags: string[];
  pending_candidates: CandidateRow[];
}

export interface ManualMetadataEdit {
  document_id: number;
  work_type: string;
  title: string;
  authors: BibliographicAuthor[];
  journal_abbrev: string | null;
  journal_full: string | null;
  booktitle: string | null;
  series: string | null;
  volume: string | null;
  issue_number: string | null;
  year: number | null;
  pages: string | null;
  edition: string | null;
  publisher: string | null;
  address: string | null;
  isbn: string | null;
  issn: string | null;
  doi: string | null;
  arxiv_id: string | null;
  mr_number: string | null;
  mr_reviewer: string | null;
  zbl_number: string | null;
  url: string | null;
  msc_primary: string | null;
  msc_secondary: string[];
  abstract_text: string | null;
  citation_key_override?: string | null;
  exclude_from_cloud_llm: boolean;
  tags: string[];
  edited_fields: string[];
}

export interface ResolveOutcome {
  document_id: number;
  status: 'updated' | 'candidates_found' | 'offline_only' | 'unchanged' | string;
  source: string | null;
  candidates_count: number;
}

export interface SaveAndFetchResult {
  outcome: ResolveOutcome;
  metadata: DocumentMetadataView;
}

export interface BulkMetadataProgressEvent {
  current: number;
  total: number;
  updated: number;
  candidates: number;
  unchanged: number;
  failed: number;
  current_title: string;
  finished: boolean;
  cancelled: boolean;
}

export type WorkCategory = 'all' | 'paper' | 'book';
export type VerifiedFilter = 'all' | 'unverified' | 'verified';
export type ReadingStatus = 'unread' | 'reading' | 'read';
export type SortField = 'updated' | 'title' | 'author' | 'year' | 'last_opened';
export type SortDirection = 'asc' | 'desc';

export interface MetadataSearchFilter {
  text?: string | null;
  author?: string | null;
  title?: string | null;
  journal?: string | null;
  year_from?: number | null;
  year_to?: number | null;
  msc_prefix?: string | null;
  tag?: string | null;
  cluster_id?: number | null;
  collection_id?: number | null;
  root_id?: string | null;
  format?: string | null;
  work_category?: WorkCategory | string | null;
  verified_filter?: VerifiedFilter | string | null;
  reading_status?: ReadingStatus | string | null;
  starred_only?: boolean | null;
  recent_only?: boolean | null;
  sort_by?: SortField | null;
  sort_dir?: SortDirection | null;
}

/** Content search filter (propagates MSC2020, tags, clusters, collections, author/year, and statement subtypes). */
export interface ContentSearchFilter {
  root_ids?: string[] | null;
  msc_prefix?: string | null;
  tag?: string | null;
  cluster_id?: number | null;
  collection_id?: number | null;
  author?: string | null;
  year_from?: number | null;
  year_to?: number | null;
  work_category?: WorkCategory | string | null;
  reading_status?: ReadingStatus | string | null;
  starred_only?: boolean | null;
  statements_only?: boolean;
  statement_kinds?: string[] | null;
}

export interface ImportMetadataReport {
  total_entries: number;
  matched_documents: number;
  unmatched_entries: number;
}

export interface DuplicateFileMember {
  document_id: number;
  file_id: number;
  root_id: string;
  root_label: string;
  rel_path: string;
  format: string;
  title: string;
  year: number | null;
  edition: string | null;
  isbn: string | null;
  doi: string | null;
  arxiv_id: string | null;
  mr_number: string | null;
  zbl_number: string | null;
  page_count: number | null;
  size_bytes: number;
  is_available: boolean;
}

export interface DuplicateGroupView {
  group_id: number;
  group_level: number;
  max_similarity: number;
  reason_detail: string;
  status: string;
  members: DuplicateFileMember[];
}

export interface DuplicateReport {
  pairs_checked: number;
  pairs_skipped_cached: number;
  new_groups_created: number;
}

export interface ScanSummary {
  scan_id: number;
  status: string;
  cnt_unchanged: number;
  cnt_new: number;
  cnt_identical_copy: number;
  cnt_moved_renamed: number;
  cnt_modified: number;
  cnt_removed: number;
  cnt_errors: number;
  cnt_dup_groups_new: number;
}

export type ScanProgressEvent =
  | { stage: 'started' }
  | { stage: 'scan_done'; summary: ScanSummary }
  | {
      stage: 'indexing_done';
      report: {
        processed: number;
        succeeded: number;
        failed: number;
        passages_indexed: number;
      };
    }
  | { stage: 'finished' }
  | { stage: 'error'; message: string };

export interface MscTreeNode {
  code: string;
  title: string;
  level: number;
  confirmed_count: number;
  proposed_count: number;
  total_count: number;
  children: MscTreeNode[];
}

export interface KnnProposeReport {
  labeled_docs: number;
  unlabeled_docs: number;
  proposed_count: number;
}

export interface ClusteringReport {
  documents_clustered: number;
  clusters_created: number;
  selected_k: number;
  silhouette_score: number;
}

export interface TopicClusterView {
  id: number;
  model_profile: string;
  label: string;
  is_label_locked: boolean;
  keywords: string[];
  document_count: number;
  document_ids: number[];
}

export interface TagSummary {
  id: number;
  name: string;
  color_hex: string | null;
  document_count: number;
}

export type SettingsTabId =
  | 'general'
  | 'folders'
  | 'ml'
  | 'ocr'
  | 'metadata'
  | 'llm'
  | 'backup'
  | 'shortcuts';

export interface BackupReport {
  archive_path: string;
  files_count: number;
  total_bytes: number;
}

export interface RestoreReport {
  files_restored: number;
  total_bytes: number;
  restart_recommended: boolean;
}

export interface OcrEnvironmentStatus {
  available: boolean;
  tesseract_path: string;
  version: string | null;
  installed_langs: string[];
  selected_langs: string[];
  reason: string | null;
}

export interface OcrProgressEvent {
  file_id: number;
  page_ord: number;
  total_pages: number;
}

export interface OcrFileReport {
  file_id: number;
  document_id: number;
  pages_processed: number;
  passages_indexed: number;
  languages_used: string;
}

export type LlmProvider = 'disabled' | 'ollama' | 'anthropic' | 'openai' | 'gemini';

export interface LlmKeyStatus {
  anthropic_present: boolean;
  openai_present: boolean;
  gemini_present: boolean;
}

export interface LlmModelInfo {
  id: string;
  display_name: string;
}

export interface LlmUsage {
  input_tokens: number | null;
  output_tokens: number | null;
  total_tokens: number | null;
}

export interface LlmPromptPreview {
  provider: LlmProvider;
  model: string | null;
  requires_cloud_consent: boolean;
  estimated_input_tokens: number;
  max_input_tokens: number;
  max_passages: number;
  passages_total: number;
  passages_included: number;
  passages_excluded_cloud: number;
  passages_dropped_budget: number;
  excluded_doc_titles: string[];
}

export interface LlmCitation {
  marker: string;
  document_id: number;
  file_id: number;
  page_ord: number;
  printed_label: string | null;
  doc_title: string;
  bboxes: NormalizedRect[];
  snippet: string;
}

export interface LlmAnswerResult {
  provider: LlmProvider;
  model: string;
  answer_markdown: string;
  citations: LlmCitation[];
  refused: boolean;
  refusal_reason: string | null;
  usage: LlmUsage;
  estimated_input_tokens: number;
  included_passages: number;
  excluded_passages: number;
}

export interface LlmKeyVerifyResult {
  ok: boolean;
  provider: LlmProvider;
  models_count: number;
  models: LlmModelInfo[];
  message: string;
}

export interface EmbedProgressStatus {
  profile: string;
  profile_ready: boolean;
  total_files: number;
  embedded_files: number;
  pending_files: number;
  running: boolean;
  paused: boolean;
  user_paused: boolean;
}

export type DocLlmScope = 'selection' | 'page' | 'document';

export type LlmStreamEvent =
  | { event: 'delta'; text: string }
  | { event: 'done'; result: LlmAnswerResult }
  | { event: 'error'; message: string };

export interface SnipLatexResult {
  provider: LlmProvider;
  model: string;
  latex: string;
  usage: LlmUsage;
}

export interface FileAnnotation {
  id: number;
  blake3_hash: string;
  page_ord: number;
  kind: 'highlight' | 'bookmark' | 'note' | 'snip' | string;
  color: 'yellow' | 'green' | 'blue' | 'pink' | string;
  rects: NormalizedRect[];
  quoted_text: string;
  note_markdown: string;
  image_data_url?: string | null;
  created_at: number;
  updated_at: number;
}

export interface NewFileAnnotation {
  page_ord: number;
  kind: 'highlight' | 'bookmark' | 'note' | 'snip';
  color?: 'yellow' | 'green' | 'blue' | 'pink';
  rects?: NormalizedRect[];
  quoted_text?: string;
  note_markdown?: string;
  image_data_url?: string | null;
}

export interface UpdateFileAnnotation {
  color?: string | null;
  note_markdown?: string | null;
  quoted_text?: string | null;
}

export interface AnnotationSearchHit {
  annotation: FileAnnotation;
  document_id: number | null;
  file_id: number | null;
  doc_title: string;
}

export interface FileReadingState {
  blake3_hash: string;
  last_page_ord: number;
  last_opened_at: number | null;
  zoom_mode: 'fit-width' | 'fit-page' | 'custom' | string;
  custom_zoom: number;
  rotation_deg: number;
  updated_at: number;
}

export interface ReadingStatePatch {
  last_page_ord?: number | null;
  zoom_mode?: 'fit-width' | 'fit-page' | 'custom' | string | null;
  custom_zoom?: number | null;
  rotation_deg?: number | null;
  touch_opened?: boolean;
}

export interface CollectionView {
  id: number;
  name: string;
  color_hex: string | null;
  sort_order: number;
  bib_sync_path?: string | null;
  bib_sync_enabled?: boolean;
  document_count: number;
  document_ids: number[];
}

export interface SavedSearchView {
  id: number;
  name: string;
  search_mode: 'metadata' | 'content' | string;
  query_text: string;
  filter_json: string;
  sort_order: number;
  created_at: number;
}

export interface SimilarDocumentItem {
  document_id: number;
  file_id: number;
  title: string;
  authors_summary: string;
  year: number | null;
  work_type: string;
  similarity: number;
}

export interface CitationLinkItem {
  document_id: number;
  file_id: number;
  title: string;
  authors_summary: string;
  year: number | null;
  source: string;
}

export interface PreprintPublishedNotice {
  doi: string | null;
  mr_number: string | null;
  journal: string | null;
  year: number | null;
  title: string | null;
  source: string;
}

export interface DocumentRelationsView {
  similar: SimilarDocumentItem[];
  cites: CitationLinkItem[];
  cited_by: CitationLinkItem[];
  published_notice: PreprintPublishedNotice | null;
}

export interface SmartPasteApplyResult {
  kind: 'bibtex' | 'identifiers' | string;
  summary: string;
  outcome: ResolveOutcome | null;
  metadata: DocumentMetadataView;
}

export interface MissingClassicCiter {
  document_id: number;
  title: string;
  year: number | null;
}

export interface MissingClassicItem {
  title: string;
  authors: string;
  year: number | null;
  venue: string | null;
  work_category: 'paper' | 'book' | string;
  doi: string | null;
  arxiv_id: string | null;
  isbn: string | null;
  cited_by_count: number;
  citing_docs: MissingClassicCiter[];
}

export interface StorageHealthReport {
  data_dir: string;
  db_bytes: number;
  wal_bytes: number;
  tantivy_bytes: number;
  vector_bytes: number;
  djvu_cache_bytes: number;
  models_bytes: number;
  logs_bytes: number;
  total_bytes: number;
  documents_count: number;
  passages_count: number;
  annotations_count: number;
}

export interface CompactionReport {
  before_bytes: number;
  after_bytes: number;
  reclaimed_bytes: number;
}

export interface UpdateCheckResult {
  current_version: string;
  latest_version: string;
  update_available: boolean;
  release_url: string;
  release_notes: string | null;
  published_at: string | null;
}
