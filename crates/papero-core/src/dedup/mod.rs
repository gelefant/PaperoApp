pub mod detector;
pub mod minhash;

pub use detector::{
    list_duplicate_groups, merge_duplicate_group, resolve_duplicate_group, run_duplicate_detection,
    DuplicateFileMember, DuplicateGroupView, DuplicateReport,
};
pub use minhash::{
    compute_document_minhash_from_pages, compute_minhash, jaccard_similarity, pack_minhash,
    unpack_minhash,
};
