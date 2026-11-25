use crate::config::AppPaths;
use crate::db::Database;
use crate::error::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::time::Duration;
use walkdir::WalkDir;

/// Default GitHub repository slug (`owner/repo`) queried for discrete update notifications.
pub const DEFAULT_GITHUB_REPO_SLUG: &str = "gelefant/PaperoApp";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageHealthReport {
    pub data_dir: String,
    pub db_bytes: u64,
    pub wal_bytes: u64,
    pub tantivy_bytes: u64,
    pub vector_bytes: u64,
    pub djvu_cache_bytes: u64,
    pub models_bytes: u64,
    pub logs_bytes: u64,
    pub total_bytes: u64,
    pub documents_count: i64,
    pub passages_count: i64,
    pub annotations_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionReport {
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub reclaimed_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_url: String,
    pub release_notes: Option<String>,
    pub published_at: Option<String>,
}

fn file_size_or_zero(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn dir_size_bytes(dir: &Path) -> u64 {
    if !dir.exists() {
        return 0;
    }
    let mut total = 0u64;
    for entry in WalkDir::new(dir).into_iter().flatten() {
        if entry.file_type().is_file() {
            if let Ok(meta) = entry.metadata() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

/// Computes AppData storage breakdown and core SQLite table counts without touching `source_roots`.
pub fn compute_storage_health(paths: &AppPaths, db: &Database) -> Result<StorageHealthReport> {
    let db_bytes = file_size_or_zero(&paths.db_path);
    let wal_path = paths.data_dir.join("papero.db-wal");
    let shm_path = paths.data_dir.join("papero.db-shm");
    let wal_bytes = file_size_or_zero(&wal_path).saturating_add(file_size_or_zero(&shm_path));
    let tantivy_bytes = dir_size_bytes(&paths.tantivy_dir);
    let vector_bytes = dir_size_bytes(&paths.indices_dir);
    let djvu_cache_bytes = dir_size_bytes(&paths.cache_dir);
    let models_bytes = dir_size_bytes(&paths.models_dir);
    let logs_bytes = dir_size_bytes(&paths.logs_dir);

    let total_bytes = db_bytes
        .saturating_add(wal_bytes)
        .saturating_add(tantivy_bytes)
        .saturating_add(vector_bytes)
        .saturating_add(djvu_cache_bytes)
        .saturating_add(models_bytes)
        .saturating_add(logs_bytes);

    let (documents_count, passages_count, annotations_count) = {
        let r = db.reader();
        let docs: i64 = r
            .query_row("SELECT COUNT(*) FROM documents", [], |row| row.get(0))
            .unwrap_or(0);
        let pass: i64 = r
            .query_row("SELECT COUNT(*) FROM passages", [], |row| row.get(0))
            .unwrap_or(0);
        let anns: i64 = r
            .query_row("SELECT COUNT(*) FROM file_annotations", [], |row| row.get(0))
            .unwrap_or(0);
        (docs, pass, anns)
    };

    Ok(StorageHealthReport {
        data_dir: paths.data_dir.to_string_lossy().into_owned(),
        db_bytes,
        wal_bytes,
        tantivy_bytes,
        vector_bytes,
        djvu_cache_bytes,
        models_bytes,
        logs_bytes,
        total_bytes,
        documents_count,
        passages_count,
        annotations_count,
    })
}

/// Runs `PRAGMA wal_checkpoint(TRUNCATE)` and `VACUUM` on SQLite to reclaim deleted pages and WAL slack.
pub fn compact_database(paths: &AppPaths, db: &Database) -> Result<CompactionReport> {
    let wal_path = paths.data_dir.join("papero.db-wal");
    let before_bytes = file_size_or_zero(&paths.db_path).saturating_add(file_size_or_zero(&wal_path));

    {
        let w = db.writer();
        w.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
    }

    let after_bytes = file_size_or_zero(&paths.db_path).saturating_add(file_size_or_zero(&wal_path));
    Ok(CompactionReport {
        before_bytes,
        after_bytes,
        reclaimed_bytes: before_bytes.saturating_sub(after_bytes),
    })
}

/// Clears cached DjVu renders and temporary OCR images inside `<AppData>/cache/` only.
/// Strictly never touches `source_roots`, SQLite, Tantivy, vectors, or ML models.
pub fn clear_render_cache(paths: &AppPaths) -> Result<u64> {
    let before = dir_size_bytes(&paths.cache_dir);
    if paths.cache_dir.exists() {
        for entry in fs::read_dir(&paths.cache_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let _ = fs::remove_dir_all(&path);
            } else {
                let _ = fs::remove_file(&path);
            }
        }
    }
    fs::create_dir_all(&paths.cache_dir)?;
    let after = dir_size_bytes(&paths.cache_dir);
    Ok(before.saturating_sub(after))
}

fn parse_semver_triplet(raw: &str) -> Option<(u64, u64, u64)> {
    let cleaned = raw.trim().trim_start_matches(['v', 'V']);
    let core = cleaned.split(['-', '+']).next()?.trim();
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    Some((major, minor, patch))
}

/// Returns `true` when `latest` represents a strictly newer semantic version than `current`.
pub fn is_newer_semver(current: &str, latest: &str) -> bool {
    match (parse_semver_triplet(current), parse_semver_triplet(latest)) {
        (Some(cur), Some(lat)) => lat > cur,
        _ => false,
    }
}

/// Queries GitHub Releases API (`releases/latest`) for a discrete update check.
pub async fn check_github_latest_release(
    current_version: &str,
    repo_slug: &str,
) -> Result<UpdateCheckResult> {
    let slug = repo_slug.trim();
    if slug.is_empty() || !slug.contains('/') {
        return Err(CoreError::Config("invalid GitHub repository slug".into()));
    }
    let url = format!("https://api.github.com/repos/{slug}/releases/latest");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(format!("Papero/{current_version}"))
        .build()
        .map_err(|e| CoreError::Config(format!("failed building HTTP client: {e}")))?;

    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| CoreError::Config(format!("update check failed: {e}")))?;

    if !resp.status().is_success() {
        return Err(CoreError::Config(format!(
            "GitHub release check returned HTTP {}",
            resp.status().as_u16()
        )));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| CoreError::Config(format!("invalid release JSON: {e}")))?;

    let tag_name = body
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim();
    let latest_version = tag_name.trim_start_matches(['v', 'V']).to_string();
    let release_url = body
        .get("html_url")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("https://github.com/{slug}/releases"));
    let release_notes = body
        .get("body")
        .and_then(serde_json::Value::as_str)
        .map(|s| s.trim().chars().take(1200).collect::<String>())
        .filter(|s| !s.is_empty());
    let published_at = body
        .get("published_at")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);

    let update_available = is_newer_semver(current_version, &latest_version);

    Ok(UpdateCheckResult {
        current_version: current_version.trim_start_matches(['v', 'V']).to_string(),
        latest_version,
        update_available,
        release_url,
        release_notes,
        published_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repos::docs::create_heuristic_document;

    #[test]
    fn semver_comparison_handles_v_prefix_and_prereleases() {
        assert!(is_newer_semver("0.5.1", "0.5.2"));
        assert!(is_newer_semver("0.5.1", "v0.6.0"));
        assert!(is_newer_semver("0.5.1", "1.0.0"));
        assert!(!is_newer_semver("0.5.1", "0.5.1"));
        assert!(!is_newer_semver("0.5.1", "v0.5.0"));
        assert!(!is_newer_semver("0.5.1", "0.4.9"));
    }

    #[test]
    fn storage_health_compaction_and_cache_clear_work_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_data_dir(tmp.path().to_path_buf());
        paths.ensure_dirs().unwrap();
        let db = Database::open(&paths.db_path, 2).unwrap();
        {
            let w = db.writer();
            create_heuristic_document(&w, "Spectral Analysis").unwrap();
        }
        fs::write(paths.cache_dir.join("page1.png"), vec![7u8; 4096]).unwrap();

        let health = compute_storage_health(&paths, &db).unwrap();
        assert_eq!(health.documents_count, 1);
        assert!(health.db_bytes > 0);
        assert!(health.djvu_cache_bytes >= 4096);

        let cleared = clear_render_cache(&paths).unwrap();
        assert!(cleared >= 4096);
        assert_eq!(dir_size_bytes(&paths.cache_dir), 0);

        let comp = compact_database(&paths, &db).unwrap();
        assert!(comp.after_bytes > 0);
    }
}
