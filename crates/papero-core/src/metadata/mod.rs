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
pub mod resolver;
pub mod semanticscholar;
pub mod zbmath;

pub use extract::{
    alternate_isbn_form, extract_identifiers_and_heuristics,
    extract_identifiers_and_heuristics_with_front_pages,
    extract_identifiers_and_heuristics_with_layout, normalize_isbn,
};
pub use models::{
    clean_author_surname, extract_arxiv_from_doi, normalize_arxiv_base_id, normalize_doi,
    normalize_mr_digits, normalize_pages, BibliographicAuthor, BibliographicRecord, CandidateMatch,
    CandidateRow, DocumentMetadataView, ExtractedIdentifiers, WorkType,
};
pub use resolver::{
    apply_manual_metadata_edit, apply_manual_metadata_edit_with_locks, confirm_metadata_candidate,
    dismiss_metadata_candidate, get_document_metadata, get_documents_metadata_for_export,
    is_document_metadata_verified, resolve_document_metadata, run_pending_metadata_jobs_cancellable,
    search_documents_metadata, set_document_metadata_verified, ManualMetadataEdit,
    MetadataLookupScope, MetadataSearchFilter, ResolveOutcome, SaveAndFetchResult,
    WaterfallClients,
};
