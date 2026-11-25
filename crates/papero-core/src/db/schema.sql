-- Papero SQLite Schema (v1)

-- 1. Source folders (physical locations; files inside are NEVER touched)
CREATE TABLE IF NOT EXISTS source_roots (
    id                      TEXT PRIMARY KEY, -- UUID v4
    label                   TEXT NOT NULL,
    root_path               TEXT NOT NULL,    -- Native OS canonical path to root
    is_enabled              INTEGER NOT NULL DEFAULT 1,
    is_network_or_cloud     INTEGER NOT NULL DEFAULT 0, -- Default auto-scan OFF if 1
    is_reachable            INTEGER NOT NULL DEFAULT 1, -- Updated before every scan
    auto_rescan_on_startup  INTEGER NOT NULL DEFAULT 0,
    auto_rescan_interval_s  INTEGER NOT NULL DEFAULT 0, -- 0 = disabled
    exclude_from_cloud_llm  INTEGER NOT NULL DEFAULT 0,
    last_scan_at            INTEGER,          -- Unix epoch seconds
    created_at              INTEGER NOT NULL
);

-- 2. Logical bibliographic records (1 document -> 1..N physical files)
CREATE TABLE IF NOT EXISTS documents (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    work_type               TEXT NOT NULL DEFAULT 'article',
                            -- 'article','book','incollection','inproceedings','phdthesis','misc'
    title                   TEXT NOT NULL,
    title_norm              TEXT NOT NULL,    -- Lowercased, diacritic+math-translit stripped
    authors_norm_summary    TEXT NOT NULL DEFAULT '', -- Fast search string: "erdos paul; renyi alfred"
    journal_abbrev          TEXT,
    journal_full            TEXT,
    booktitle               TEXT,
    series                  TEXT,
    volume                  TEXT,
    issue_number            TEXT,
    year                    INTEGER,
    pages                   TEXT,             -- Stored normalized with '--' when applicable
    edition                 TEXT,
    publisher               TEXT,
    address                 TEXT,
    isbn                    TEXT,             -- Normalized digits/X
    issn                    TEXT,
    doi                     TEXT,             -- Lowercase normalized DOI
    arxiv_id                TEXT,             -- Base ID without version e.g. '2301.12345', plus version col
    arxiv_version           TEXT,
    mr_number               TEXT,             -- Normalized digits e.g. '0237510' or '997568'
    mr_number_display       TEXT,             -- Verbatim e.g. '997568 (90g:49033)'
    mr_reviewer             TEXT,
    zbl_number              TEXT,
    url                     TEXT,
    msc_primary             TEXT,             -- e.g. '49F20' or '49-XX'
    msc_primary_status      TEXT NOT NULL DEFAULT 'none',
                            -- 'none' | 'metadata' | 'proposed' | 'confirmed'
    abstract_text           TEXT,
    raw_mrlookup_bibtex     TEXT,             -- Verbatim MRLookup BibTeX if fetched
    metadata_source         TEXT,             -- 'manual'|'mrlookup'|'zbmath'|'crossref'|'arxiv'|'openlibrary'|'heuristic'
    metadata_license_note   TEXT,             -- e.g. 'CC-BY-SA 4.0 (zbMATH Open / FIZ Karlsruhe)'
    manual_locked_fields    TEXT NOT NULL DEFAULT '[]',
                            -- JSON array of field names edited by hand; NEVER overwritten by auto-fetch!
    metadata_verified       INTEGER NOT NULL DEFAULT 0,
                            -- 1 when user explicitly checked and confirmed metadata are accurate
    exclude_from_cloud_llm  INTEGER NOT NULL DEFAULT 0,
    reading_status          TEXT NOT NULL DEFAULT 'unread',
                            -- 'unread' | 'reading' | 'read'
    is_starred              INTEGER NOT NULL DEFAULT 0,
    published_notice_json   TEXT,
                            -- Cached JSON notice when an arXiv preprint has a published journal version
    minhash_sig             BLOB,             -- 128 x u64 (1024 bytes) MinHash signature of extracted text
    content_modified_notice INTEGER NOT NULL DEFAULT 0, -- Flag set if underlying file was replaced on disk
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_docs_title_norm ON documents(title_norm);
CREATE INDEX IF NOT EXISTS idx_docs_authors_norm ON documents(authors_norm_summary);
CREATE INDEX IF NOT EXISTS idx_docs_year ON documents(year);
CREATE INDEX IF NOT EXISTS idx_docs_updated_at ON documents(updated_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_docs_msc_primary ON documents(msc_primary);
CREATE INDEX IF NOT EXISTS idx_docs_doi ON documents(doi) WHERE doi IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_docs_arxiv ON documents(arxiv_id) WHERE arxiv_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_docs_mr ON documents(mr_number) WHERE mr_number IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_docs_zbl ON documents(zbl_number) WHERE zbl_number IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_docs_isbn ON documents(isbn) WHERE isbn IS NOT NULL;

-- 3. Normalized authors (preserves exact order)
CREATE TABLE IF NOT EXISTS document_authors (
    document_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    position        INTEGER NOT NULL, -- 0-based order
    family_name     TEXT NOT NULL,
    given_name      TEXT NOT NULL DEFAULT '',
    name_norm       TEXT NOT NULL,    -- Transliterated lowercase for tolerant search
    role            TEXT NOT NULL DEFAULT 'author', -- 'author' | 'editor'
    PRIMARY KEY (document_id, role, position)
);
CREATE INDEX IF NOT EXISTS idx_doc_authors_norm ON document_authors(name_norm);

-- 4. Secondary MSC2020 classifications
CREATE TABLE IF NOT EXISTS document_msc_secondary (
    document_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    msc_code        TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'metadata', -- 'metadata'|'proposed'|'confirmed'
    PRIMARY KEY (document_id, msc_code)
);
CREATE INDEX IF NOT EXISTS idx_doc_msc_sec ON document_msc_secondary(msc_code);

-- 5. Physical files (stored as root_id + POSIX relative path with '/')
CREATE TABLE IF NOT EXISTS files (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    document_id         INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    root_id             TEXT NOT NULL REFERENCES source_roots(id) ON DELETE CASCADE,
    rel_path            TEXT NOT NULL,        -- Always '/' separated relative to root_path!
    format              TEXT NOT NULL,        -- 'pdf' | 'djvu'
    size_bytes          INTEGER NOT NULL,
    mtime_ns            INTEGER NOT NULL,     -- Last-modified timestamp in nanoseconds
    blake3_hash         TEXT NOT NULL,        -- 64-char hex BLAKE3 digest
    is_available        INTEGER NOT NULL DEFAULT 1, -- 0 when source_root is unreachable
    page_count          INTEGER,
    has_text_layer      INTEGER NOT NULL DEFAULT 0,
    ocr_status          TEXT NOT NULL DEFAULT 'none', -- 'none'|'queued'|'running'|'done'|'failed'
    alt_text_file_path  TEXT,                 -- Optional path in AppData to Nougat/Marker output
    index_state         TEXT NOT NULL DEFAULT 'pending_extract',
                        -- 'pending_extract'|'bm25_done'|'ready'|'corrupt'|'encrypted'|'io_error'
    embedded_light      INTEGER NOT NULL DEFAULT 0,
    embedded_quality    INTEGER NOT NULL DEFAULT 0,
    error_message       TEXT,
    outline_json        TEXT,                 -- Cached PDF/DjVu bookmarks tree
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    UNIQUE(root_id, rel_path)
);
CREATE INDEX IF NOT EXISTS idx_files_hash ON files(blake3_hash);
CREATE INDEX IF NOT EXISTS idx_files_doc ON files(document_id);
CREATE INDEX IF NOT EXISTS idx_files_root_stat ON files(root_id, rel_path, size_bytes, mtime_ns);

-- 6. Archive of manual metadata & tags keyed by BLAKE3 hash (survives file removal!)
CREATE TABLE IF NOT EXISTS manual_metadata_archive (
    blake3_hash         TEXT PRIMARY KEY,
    manual_fields_json  TEXT NOT NULL, -- Snapshot of manually edited fields + MSC confirmations
    tags_json           TEXT NOT NULL, -- JSON array of tag names
    archived_at         INTEGER NOT NULL
);

-- 7. Extracted page metadata (ordinal 1-based page -> printed page label & dimensions)
CREATE TABLE IF NOT EXISTS file_pages (
    file_id         INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    page_ord        INTEGER NOT NULL, -- 1-based ordinal page number
    printed_label   TEXT,             -- e.g. 'xiv', '142' (from PDF PageLabels / DjVu title)
    width_pts       REAL NOT NULL,
    height_pts      REAL NOT NULL,
    text_source     TEXT NOT NULL,    -- 'native' | 'ocr' | 'alt'
    PRIMARY KEY (file_id, page_ord)
);

-- 8. Indexed passages (~300-500 tokens with overlap & normalized bounding boxes)
-- NOTE: `id` is used directly as `passage_id` (u64) in BOTH Tantivy and USearch!
CREATE TABLE IF NOT EXISTS passages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    document_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    file_id         INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    page_ord        INTEGER NOT NULL, -- 1-based ordinal page
    printed_label   TEXT,
    chunk_index     INTEGER NOT NULL, -- Order within page/file
    is_alt_formula  INTEGER NOT NULL DEFAULT 0, -- 1 if from Nougat/Marker alternative text
    token_count     INTEGER NOT NULL,
    text_content    TEXT NOT NULL,
    bboxes_blob     BLOB NOT NULL     -- Little-endian packed array of `[f32; 4]` = (x, y, w, h)
                                      -- normalized in [0.0, 1.0], origin TOP-LEFT!
);
CREATE INDEX IF NOT EXISTS idx_passages_file_page ON passages(file_id, page_ord);
CREATE INDEX IF NOT EXISTS idx_passages_doc ON passages(document_id);
CREATE INDEX IF NOT EXISTS idx_passages_doc_page ON passages(document_id, page_ord, chunk_index);

-- 9. User free tags & Unsupervised topic clusters
CREATE TABLE IF NOT EXISTS tags (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL UNIQUE COLLATE NOCASE,
    color_hex   TEXT
);
CREATE TABLE IF NOT EXISTS document_tags (
    document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    tag_id      INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (document_id, tag_id)
);

CREATE TABLE IF NOT EXISTS topic_clusters (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    model_profile   TEXT NOT NULL, -- 'light' | 'quality'
    label           TEXT NOT NULL, -- Auto keyword label, editable by user
    is_label_locked INTEGER NOT NULL DEFAULT 0,
    keywords_json   TEXT NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS document_clusters (
    document_id         INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    cluster_id          INTEGER NOT NULL REFERENCES topic_clusters(id) ON DELETE CASCADE,
    is_user_assigned    INTEGER NOT NULL DEFAULT 0, -- Preserved across re-clustering if 1
    score               REAL NOT NULL DEFAULT 1.0,
    PRIMARY KEY (document_id, cluster_id)
);

-- 10. Duplicate / Version / Edition detection groups & evaluated pair memory
CREATE TABLE IF NOT EXISTS duplicate_groups (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    group_level     INTEGER NOT NULL,
                    -- 1 = identical_copies (same BLAKE3; auto-merged in `files`, shown as info)
                    -- 2 = same_work_id (shared DOI / MR / Zbl / arXiv / ISBN across distinct files)
                    -- 3 = probable_duplicate (fuzzy title+author or MinHash text similarity)
                    -- 4 = versions (arXiv preprint <-> published DOI/MR version)
                    -- 5 = book_editions (same book title/authors, different edition/year/ISBN)
    max_similarity  REAL NOT NULL DEFAULT 1.0,
    reason_detail   TEXT NOT NULL,    -- e.g. 'Shared MR0997568' or 'MinHash Jaccard 0.91'
    status          TEXT NOT NULL DEFAULT 'pending',
                    -- 'info_merged' (Level 1) | 'pending' | 'merged' | 'distinct' | 'versions' | 'editions'
    created_at      INTEGER NOT NULL,
    resolved_at     INTEGER
);
CREATE TABLE IF NOT EXISTS duplicate_group_members (
    group_id        INTEGER NOT NULL REFERENCES duplicate_groups(id) ON DELETE CASCADE,
    document_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    file_id         INTEGER REFERENCES files(id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, document_id, file_id)
);

-- Prevents ever recomputing or re-prompting already evaluated document pairs!
CREATE TABLE IF NOT EXISTS duplicate_evaluated_pairs (
    doc_id_min      INTEGER NOT NULL,
    doc_id_max      INTEGER NOT NULL,
    hash_sig_min    TEXT NOT NULL,    -- Snapshot of title_norm + primary file blake3
    hash_sig_max    TEXT NOT NULL,
    decision        TEXT NOT NULL,    -- 'pending' | 'distinct' | 'versions' | 'editions' | 'merged'
    evaluated_at    INTEGER NOT NULL,
    PRIMARY KEY (doc_id_min, doc_id_max)
);

-- 11. Ambiguous metadata lookup candidates awaiting user confirmation
CREATE TABLE IF NOT EXISTS metadata_candidates (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    document_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    source          TEXT NOT NULL,
    match_score     REAL NOT NULL,    -- [0.0, 1.0]
    candidate_json  TEXT NOT NULL,    -- Full BibliographicRecord JSON
    created_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_metadata_candidates_doc ON metadata_candidates(document_id);

-- 12. Permanent HTTP cache for bibliographic lookups (CRITICAL for Open Library ToS!)
CREATE TABLE IF NOT EXISTS metadata_http_cache (
    source          TEXT NOT NULL,    -- 'mrlookup'|'zbmath'|'crossref'|'arxiv'|'openlibrary'
    lookup_key      TEXT NOT NULL,    -- e.g. 'isbn:9780387947051' or 'title_author:<norm>'
    response_body   TEXT NOT NULL,
    status_code     INTEGER NOT NULL,
    fetched_at      INTEGER NOT NULL,
    PRIMARY KEY (source, lookup_key)
);

-- 13. Rescan history & category drill-down lists
CREATE TABLE IF NOT EXISTS scan_runs (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at          INTEGER NOT NULL,
    finished_at         INTEGER,
    status              TEXT NOT NULL, -- 'running' | 'paused' | 'cancelled' | 'completed'
    cnt_unchanged       INTEGER NOT NULL DEFAULT 0,
    cnt_new             INTEGER NOT NULL DEFAULT 0,
    cnt_identical_copy  INTEGER NOT NULL DEFAULT 0,
    cnt_moved_renamed   INTEGER NOT NULL DEFAULT 0,
    cnt_modified        INTEGER NOT NULL DEFAULT 0,
    cnt_removed         INTEGER NOT NULL DEFAULT 0,
    cnt_errors          INTEGER NOT NULL DEFAULT 0,
    cnt_dup_groups_new  INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS scan_run_items (
    scan_id     INTEGER NOT NULL REFERENCES scan_runs(id) ON DELETE CASCADE,
    category    TEXT NOT NULL, -- 'new'|'copy'|'moved'|'modified'|'removed'|'error'|'duplicate'
    root_id     TEXT,
    rel_path    TEXT NOT NULL,
    detail      TEXT
);

-- 14. Background indexing job queue (resumable across app restarts!)
CREATE TABLE IF NOT EXISTS index_jobs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id     INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    stage       TEXT NOT NULL, -- 'extract_bm25' | 'embed_light' | 'embed_quality' | 'metadata' | 'ocr'
    priority    INTEGER NOT NULL DEFAULT 10,
    status      TEXT NOT NULL DEFAULT 'queued', -- 'queued' | 'running' | 'failed'
    attempts    INTEGER NOT NULL DEFAULT 0,
    last_error  TEXT,
    UNIQUE(file_id, stage)
);
CREATE INDEX IF NOT EXISTS idx_index_jobs_stage_status ON index_jobs(stage, status, priority, id);

-- 15. Non-destructive annotations, highlights, page bookmarks & Markdown/LaTeX notes (keyed by blake3_hash)
CREATE TABLE IF NOT EXISTS file_annotations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    blake3_hash     TEXT NOT NULL,
    page_ord        INTEGER NOT NULL,
    kind            TEXT NOT NULL DEFAULT 'highlight', -- 'highlight' | 'bookmark' | 'note'
    color           TEXT NOT NULL DEFAULT 'yellow',    -- 'yellow' | 'green' | 'blue' | 'pink'
    rects_json      TEXT NOT NULL DEFAULT '[]',        -- JSON array of NormalizedRect
    quoted_text     TEXT NOT NULL DEFAULT '',
    note_markdown   TEXT NOT NULL DEFAULT '',
    search_norm     TEXT NOT NULL DEFAULT '',          -- Math-diacritic-folded text for fast search
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_file_annotations_hash_page ON file_annotations(blake3_hash, page_ord, id);
CREATE INDEX IF NOT EXISTS idx_file_annotations_hash_kind ON file_annotations(blake3_hash, kind);

-- 16. Persistent reading position, zoom mode, rotation & last opened timestamp (keyed by blake3_hash)
CREATE TABLE IF NOT EXISTS file_reading_state (
    blake3_hash     TEXT PRIMARY KEY,
    last_page_ord   INTEGER NOT NULL DEFAULT 1,
    last_opened_at  INTEGER,
    zoom_mode       TEXT NOT NULL DEFAULT 'fit-width', -- 'fit-width' | 'fit-page' | 'custom'
    custom_zoom     REAL NOT NULL DEFAULT 1.0,
    rotation_deg    INTEGER NOT NULL DEFAULT 0,        -- 0 | 90 | 180 | 270
    updated_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_file_reading_state_opened ON file_reading_state(last_opened_at DESC);

-- 17. User collections ("Projects / Reading Lists")
CREATE TABLE IF NOT EXISTS collections (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    name             TEXT NOT NULL,
    color_hex        TEXT,
    sort_order       INTEGER NOT NULL DEFAULT 0,
    bib_sync_path    TEXT,
    bib_sync_enabled INTEGER NOT NULL DEFAULT 0,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS collection_documents (
    collection_id   INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    document_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    position        INTEGER NOT NULL DEFAULT 0,
    added_at        INTEGER NOT NULL,
    PRIMARY KEY (collection_id, document_id)
);
CREATE INDEX IF NOT EXISTS idx_collection_docs_doc ON collection_documents(document_id);

-- 18. Saved quick views / searches
CREATE TABLE IF NOT EXISTS saved_searches (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    search_mode TEXT NOT NULL DEFAULT 'metadata', -- 'content' | 'metadata'
    query_text  TEXT NOT NULL DEFAULT '',
    filter_json TEXT NOT NULL DEFAULT '{}',
    sort_order  INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL
);

-- 19. Local citation graph between documents in the user's library
CREATE TABLE IF NOT EXISTS document_citations (
    citing_doc_id   INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    cited_doc_id    INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    source          TEXT NOT NULL, -- 'extracted_ref' | 'openalex' | 'semanticscholar' | 'crossref'
    confidence      REAL NOT NULL DEFAULT 1.0,
    PRIMARY KEY (citing_doc_id, cited_doc_id)
);
CREATE INDEX IF NOT EXISTS idx_doc_citations_cited ON document_citations(cited_doc_id);
