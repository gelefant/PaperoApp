use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reachability {
    Reachable,
    Unreachable { reason: String },
}

/// Checks whether a source root directory is currently mounted and readable.
/// Never writes any marker file into `root_path`.
///
/// Safeguard for unmounted volume mountpoints (e.g., `/Volumes/ExtDrive` or `/mnt/books`
/// appearing as an empty stub directory): if the root previously contained `> 0` files
/// and `read_dir` yields `0` entries, treat it as unreachable rather than wiping the catalog.
pub fn check_root_reachability(root_path: &Path, previous_file_count: i64) -> Reachability {
    if !root_path.exists() {
        return Reachability::Unreachable {
            reason: format!("path does not exist: {}", root_path.display()),
        };
    }
    if !root_path.is_dir() {
        return Reachability::Unreachable {
            reason: format!("path is not a directory: {}", root_path.display()),
        };
    }
    let mut entries = match fs::read_dir(root_path) {
        Ok(rd) => rd,
        Err(e) => {
            return Reachability::Unreachable {
                reason: format!("cannot read directory {}: {e}", root_path.display()),
            };
        }
    };

    if previous_file_count > 0 && entries.next().is_none() {
        return Reachability::Unreachable {
            reason: format!(
                "directory {} is empty (possible unmounted drive)",
                root_path.display()
            ),
        };
    }

    Reachability::Reachable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stub_directory_with_previous_files_is_unreachable() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(check_root_reachability(dir.path(), 0), Reachability::Reachable);
        assert!(matches!(
            check_root_reachability(dir.path(), 10),
            Reachability::Unreachable { .. }
        ));
    }
}
