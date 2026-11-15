use crate::error::{CoreError, Result};
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

pub fn new_root_id() -> String {
    Uuid::new_v4().to_string()
}

/// Detects supported document extensions (`.pdf`, `.djvu`, `.djv`) case-insensitively.
/// Returns `Some("pdf")` or `Some("djvu")`.
pub fn detect_doc_format(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => Some("pdf"),
        "djvu" | "djv" => Some("djvu"),
        _ => None,
    }
}

/// Converts a path inside `root_path` into a canonical POSIX `'/'`-separated relative path.
pub fn to_posix_rel_path(root_path: &Path, full_path: &Path) -> Result<String> {
    let rel = full_path.strip_prefix(root_path).map_err(|_| {
        CoreError::InvalidPath(
            full_path.to_path_buf(),
            format!("not inside root {}", root_path.display()),
        )
    })?;

    let mut parts = Vec::new();
    for comp in rel.components() {
        match comp {
            Component::Normal(os) => {
                let s = os.to_str().ok_or_else(|| {
                    CoreError::InvalidPath(full_path.to_path_buf(), "non-UTF-8 path".into())
                })?;
                parts.push(s);
            }
            Component::CurDir => {}
            _ => {
                return Err(CoreError::InvalidPath(
                    full_path.to_path_buf(),
                    "unexpected relative path component".into(),
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err(CoreError::InvalidPath(
            full_path.to_path_buf(),
            "empty relative path".into(),
        ));
    }
    Ok(parts.join("/"))
}

/// Safely resolves a stored POSIX `'/'`-separated `rel_path` under `root_path`,
/// rejecting any path traversal (`..` or absolute components).
pub fn resolve_rel_path(root_path: &Path, rel_path: &str) -> Result<PathBuf> {
    let mut full = root_path.to_path_buf();
    for seg in rel_path.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.contains('\\') {
            return Err(CoreError::InvalidPath(
                PathBuf::from(rel_path),
                "invalid relative path segment".into(),
            ));
        }
        full.push(seg);
    }
    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_rel_path_roundtrip_and_traversal_guard() {
        let root = Path::new("/tmp/library");
        let full = Path::new("/tmp/library/Algebra/Serre/LocalFields.pdf");
        let rel = to_posix_rel_path(root, full).unwrap();
        assert_eq!(rel, "Algebra/Serre/LocalFields.pdf");
        assert_eq!(resolve_rel_path(root, &rel).unwrap(), full);

        assert!(resolve_rel_path(root, "../etc/passwd").is_err());
        assert!(resolve_rel_path(root, "a/../../b.pdf").is_err());
    }

    #[test]
    fn detects_pdf_and_djvu_case_insensitively() {
        assert_eq!(detect_doc_format(Path::new("book.PDF")), Some("pdf"));
        assert_eq!(detect_doc_format(Path::new("book.DjVu")), Some("djvu"));
        assert_eq!(detect_doc_format(Path::new("book.djv")), Some("djvu"));
        assert_eq!(detect_doc_format(Path::new("notes.txt")), None);
    }
}
