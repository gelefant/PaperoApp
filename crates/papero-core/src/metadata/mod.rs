pub mod arxiv;
pub mod crossref;
pub mod datacite;
pub mod dblp;
pub mod europepmc;
pub mod extract;
pub mod googlebooks;
pub mod harvard;
pub mod inspirehep;
pub mod io;
pub mod journal_abbrev;
pub mod limiter;
pub mod loc;
pub mod models;
pub mod mrlookup;
pub mod openalex;
pub mod openlibrary;
pub mod relations;
pub mod resolver;
pub mod semanticscholar;
pub mod smart_paste;
pub mod zbmath;

pub use extract::{
    alternate_isbn_form, extract_identifiers_and_heuristics,
    extract_identifiers_and_heuristics_with_front_pages,
    extract_identifiers_and_heuristics_with_layout, normalize_isbn,
};
pub use io::{
    export_documents_bibtex, export_documents_csv, import_bibtex, import_zotero_csl_json,
    metadata_view_to_record, resolve_disambiguated_cite_key_for_doc,
    sanitize_citation_key_override, write_bibtex_file_atomically,
};
pub use models::{
    clean_author_surname, extract_arxiv_from_doi, normalize_arxiv_base_id, normalize_doi,
    normalize_mr_digits, normalize_pages, BibliographicAuthor, BibliographicRecord, CandidateMatch,
    CandidateRow, DocumentMetadataView, ExtractedIdentifiers, WorkType,
};
pub use relations::{
    find_missing_cited_classics, find_similar_documents, get_document_relations,
    refresh_local_citations_for_doc, resolve_citation_snippet_in_library, CitationLinkItem,
    DocumentRelationsView, MissingClassicItem, PreprintPublishedNotice, SimilarDocumentItem,
};
pub use resolver::{
    apply_manual_metadata_edit, apply_manual_metadata_edit_with_locks, confirm_metadata_candidate,
    confirm_metadata_candidate_with_options, dismiss_metadata_candidate, get_document_metadata,
    get_documents_metadata_for_export, is_document_metadata_verified, prepare_guided_search_edit,
    resolve_document_metadata, resolve_document_metadata_with_hints,
    run_pending_metadata_jobs_cancellable, search_documents_metadata,
    set_document_metadata_verified, GuidedSearchHints, ManualMetadataEdit, MetadataLookupScope,
    MetadataSearchFilter, ResolveOutcome, SaveAndFetchResult, WaterfallClients,
};
