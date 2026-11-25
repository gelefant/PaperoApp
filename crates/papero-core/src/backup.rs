use crate::config::{
    sanitize_config_for_backup, sanitize_config_for_cross_os_restore, AppConfig, AppPaths,
};
use crate::db::repos::roots::reconcile_source_roots_with_local_map;
use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::index::bm25::Bm25Index;
use rusqlite::{backup::Backup, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use walkdir::WalkDir;

const BACKUP_MAGIC_V1: &[u8; 14] = b"PAPERO_BKP_V1\0";
const BACKUP_MAGIC_V2: &[u8; 14] = b"PAPERO_BKP_V2\0";
const ZSTD_COMPRESSION_LEVEL: i32 = 3;
const STREAM_BUF_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackupFileEntry {
    rel_path: String,
    size: u64,
    sha256_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackupManifest {
    version: u32,
    files: Vec<BackupFileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupReport {
    pub archive_path: String,
    pub files_count: usize,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreReport {
    pub files_restored: usize,
    pub total_bytes: u64,
    pub restart_recommended: bool,
}

fn should_skip_transient_file(name: &str) -> bool {
    name.ends_with(".lock")
        || name.ends_with(".tmp")
        || name == ".DS_Store"
        || name == "Thumbs.db"
        || name == "desktop.ini"
        || name.starts_with(".tantivy-meta.")
        || name.starts_with(".tantivy-writer.")
}

fn stream_hash_file(path: &Path, buf: &mut [u8]) -> Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    loop {
        let n = file.read(buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total = total
            .checked_add(n as u64)
            .ok_or_else(|| CoreError::Config("backup file size overflow".into()))?;
    }
    Ok((total, hex::encode(hasher.finalize())))
}

fn stream_copy_and_hash<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    expected_len: u64,
    buf: &mut [u8],
) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut remaining = expected_len;
    while remaining > 0 {
        let chunk_max = remaining.min(buf.len() as u64) as usize;
        let n = reader.read(&mut buf[..chunk_max])?;
        if n == 0 {
            return Err(CoreError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "unexpected EOF while streaming backup payload",
            )));
        }
        hasher.update(&buf[..n]);
        writer.write_all(&buf[..n])?;
        remaining -= n as u64;
    }
    Ok(hex::encode(hasher.finalize()))
}

fn stream_copy_exact<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    expected_len: u64,
    buf: &mut [u8],
) -> Result<()> {
    let mut remaining = expected_len;
    while remaining > 0 {
        let chunk_max = remaining.min(buf.len() as u64) as usize;
        let n = reader.read(&mut buf[..chunk_max])?;
        if n == 0 {
            return Err(CoreError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "unexpected EOF while streaming backup payload",
            )));
        }
        writer.write_all(&buf[..n])?;
        remaining -= n as u64;
    }
    Ok(())
}

fn validate_safe_rel_path(raw: &str) -> Result<PathBuf> {
    let normalized = raw.replace('\\', "/");
    if normalized.starts_with('/')
        || normalized.contains('\0')
        || normalized.as_bytes().get(1) == Some(&b':')
    {
        return Err(CoreError::Config(format!(
            "backup contains absolute path: {raw}"
        )));
    }
    let mut clean = PathBuf::new();
    for seg in normalized.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.contains(':') {
            return Err(CoreError::Config(format!(
                "backup contains unsafe path component: {raw}"
            )));
        }
        clean.push(seg);
    }
    let first = normalized.split('/').next().unwrap_or("");
    if !matches!(first, "papero.db" | "config.json" | "tantivy" | "indices") {
        return Err(CoreError::Config(format!(
            "backup entry outside allowed scope: {raw}"
        )));
    }
    Ok(clean)
}

fn collect_dir_files(base_dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    if !base_dir.exists() {
        return Ok(());
    }
    for entry in WalkDir::new(base_dir) {
        let entry = entry.map_err(|e| CoreError::Config(format!("walkdir error: {e}")))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(base_dir)
            .map_err(|e| CoreError::Config(format!("strip_prefix error: {e}")))?;
        if rel
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(should_skip_transient_file)
        {
            continue;
        }
        let posix = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        out.push((format!("{prefix}/{posix}"), entry.path().to_path_buf()));
    }
    Ok(())
}

/// Copies a directory tree while skipping transient lock/temp files.
/// Retries briefly if an external segment/temp file is removed concurrently.
pub(crate) fn copy_dir_excluding_locks(src_dir: &Path, dst_dir: &Path) -> Result<()> {
    if !src_dir.exists() {
        return Ok(());
    }
    for attempt in 0..3 {
        if dst_dir.exists() {
            let _ = fs::remove_dir_all(dst_dir);
        }
        fs::create_dir_all(dst_dir)?;
        let mut files = Vec::new();
        collect_dir_files(src_dir, "", &mut files)?;
        let mut vanished_transient = false;
        for (rel, abs) in files {
            let rel_clean = rel.trim_start_matches('/');
            let target = dst_dir.join(rel_clean);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            match fs::copy(&abs, &target) {
                Ok(_) => {}
                Err(e)
                    if e.kind() == io::ErrorKind::NotFound
                        && attempt < 2
                        && rel_clean != "meta.json" =>
                {
                    vanished_transient = true;
                    break;
                }
                Err(e) => return Err(CoreError::Io(e)),
            }
        }
        if !vanished_transient {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

struct StagingCleanup(Option<PathBuf>);
impl Drop for StagingCleanup {
    fn drop(&mut self) {
        if let Some(ref p) = self.0 {
            let _ = fs::remove_dir_all(p);
        }
    }
}

struct TempFileCleanup(Option<PathBuf>);
impl Drop for TempFileCleanup {
    fn drop(&mut self) {
        if let Some(ref p) = self.0 {
            let _ = fs::remove_file(p);
        }
    }
}

fn temp_archive_path(dest_file: &Path) -> PathBuf {
    let base = dest_file
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("papero.papero-backup");
    dest_file.with_file_name(format!(".{base}.{}.tmp", uuid::Uuid::new_v4()))
}

fn replace_file_atomically(tmp_out: &Path, dest_file: &Path) -> Result<()> {
    match fs::rename(tmp_out, dest_file) {
        Ok(()) => Ok(()),
        Err(first_err) => {
            #[cfg(target_os = "windows")]
            if dest_file.exists() {
                let _ = fs::remove_file(dest_file);
                return fs::rename(tmp_out, dest_file).map_err(CoreError::Io);
            }
            Err(CoreError::Io(first_err))
        }
    }
}

/// Creates a verified single-file `.papero-backup` snapshot containing:
/// - Online hot SQLite backup of `papero.db`
/// - `tantivy/` full-text index & `indices/` vector indices
/// - Sanitized `config.json` (never including keychain secrets, email fallback, or local backup paths)
pub fn create_backup_bundle(
    paths: &AppPaths,
    db: &Database,
    dest_file: &Path,
) -> Result<BackupReport> {
    create_backup_bundle_with_bm25(paths, db, None, dest_file)
}

/// Same as `create_backup_bundle`, additionally committing and snapshotting `Bm25Index`
/// under its writer lock when available, and streaming payloads with bounded memory.
pub fn create_backup_bundle_with_bm25(
    paths: &AppPaths,
    db: &Database,
    bm25: Option<&Bm25Index>,
    dest_file: &Path,
) -> Result<BackupReport> {
    let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
    if let Some(parent) = dest_file.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let tmp_dir = paths.data_dir.join(format!(
        ".backup_staging_{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&tmp_dir)?;
    let _cleanup = StagingCleanup(Some(tmp_dir.clone()));

    let snap_db_path = tmp_dir.join("papero.db");
    if paths.db_path.exists() {
        // Open a dedicated read-only connection so copying a large DB never holds a UI reader-pool mutex.
        let src_conn = Connection::open_with_flags(
            &paths.db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let _ = src_conn.busy_timeout(Duration::from_secs(10));
        let mut dst_conn = Connection::open(&snap_db_path)?;
        let backup = Backup::new(&src_conn, &mut dst_conn)?;
        backup.run_to_completion(4096, Duration::ZERO, None)?;
    } else {
        let src_conn = db.reader();
        let _ = src_conn.busy_timeout(Duration::from_secs(10));
        let mut dst_conn = Connection::open(&snap_db_path)?;
        let backup = Backup::new(&src_conn, &mut dst_conn)?;
        backup.run_to_completion(4096, Duration::ZERO, None)?;
    }

    let mut cfg = AppConfig::load_or_default(&paths.config_path)?;
    sanitize_config_for_backup(&mut cfg);
    let snap_cfg_path = tmp_dir.join("config.json");
    cfg.save(&snap_cfg_path)?;

    let snap_tantivy_dir = tmp_dir.join("tantivy");
    if let Some(idx) = bm25 {
        idx.commit_and_copy_dir(&paths.tantivy_dir, &snap_tantivy_dir)?;
    } else {
        copy_dir_excluding_locks(&paths.tantivy_dir, &snap_tantivy_dir)?;
    }

    let snap_indices_dir = tmp_dir.join("indices");
    copy_dir_excluding_locks(&paths.indices_dir, &snap_indices_dir)?;

    let mut sources: Vec<(String, PathBuf)> = vec![
        ("papero.db".to_string(), snap_db_path),
        ("config.json".to_string(), snap_cfg_path),
    ];
    collect_dir_files(&snap_tantivy_dir, "tantivy", &mut sources)?;
    collect_dir_files(&snap_indices_dir, "indices", &mut sources)?;

    let mut buf = vec![0u8; STREAM_BUF_SIZE];
    let mut manifest_entries = Vec::with_capacity(sources.len());

    for (rel, abs) in &sources {
        let (size, sha256_hex) = stream_hash_file(abs, &mut buf)?;
        manifest_entries.push(BackupFileEntry {
            rel_path: rel.clone(),
            size,
            sha256_hex,
        });
    }

    let manifest = BackupManifest {
        version: 2,
        files: manifest_entries,
    };
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    let manifest_len = u32::try_from(manifest_bytes.len())
        .map_err(|_| CoreError::Config("backup manifest exceeds 4 GB".into()))?;

    let tmp_out = temp_archive_path(dest_file);
    let mut tmp_guard = TempFileCleanup(Some(tmp_out.clone()));
    let archive_bytes = {
        let out_file = File::create(&tmp_out)?;
        let mut w = BufWriter::with_capacity(STREAM_BUF_SIZE, out_file);
        w.write_all(BACKUP_MAGIC_V2)?;
        w.write_all(&manifest_len.to_le_bytes())?;
        w.write_all(&manifest_bytes)?;
        let mut encoder = zstd::stream::write::Encoder::new(w, ZSTD_COMPRESSION_LEVEL)?;
        for ((_, abs), entry) in sources.iter().zip(manifest.files.iter()) {
            let mut src_file = File::open(abs)?;
            stream_copy_exact(&mut src_file, &mut encoder, entry.size, &mut buf)?;
        }
        let mut w = encoder.finish()?;
        w.flush()?;
        w.get_ref().sync_all()?;
        w.get_ref().metadata()?.len()
    };
    replace_file_atomically(&tmp_out, dest_file)?;
    tmp_guard.0 = None;

    Ok(BackupReport {
        archive_path: dest_file.to_string_lossy().into_owned(),
        files_count: sources.len(),
        total_bytes: archive_bytes,
    })
}

const PENDING_READY_MARKER: &str = ".ready";

fn is_transient_sharing_error(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::PermissionDenied
        || (cfg!(target_os = "windows")
            && matches!(err.raw_os_error(), Some(5) | Some(32) | Some(33)))
}

fn retry_transient_fs_op<T, F>(mut op: F) -> io::Result<T>
where
    F: FnMut() -> io::Result<T>,
{
    let mut last_err = None;
    for attempt in 0..60 {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) if attempt < 59 && is_transient_sharing_error(&e) => {
                last_err = Some(e);
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::other("filesystem retry exhausted")))
}

fn unpack_and_verify_archive(src_file: &Path, staging: &Path) -> Result<(usize, u64)> {
    let mut r = BufReader::new(File::open(src_file)?);
    let mut magic = [0u8; 14];
    r.read_exact(&mut magic)?;
    let expected_version = if &magic == BACKUP_MAGIC_V1 {
        1
    } else if &magic == BACKUP_MAGIC_V2 {
        2
    } else {
        return Err(CoreError::Config(
            "Invalid backup archive header (not a .papero-backup file)".into(),
        ));
    };

    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let manifest_len = u32::from_le_bytes(len_buf) as usize;
    if manifest_len > 64 * 1024 * 1024 {
        return Err(CoreError::Config("Backup manifest unexpectedly large".into()));
    }
    let mut manifest_bytes = vec![0u8; manifest_len];
    r.read_exact(&mut manifest_bytes)?;
    let manifest: BackupManifest = serde_json::from_slice(&manifest_bytes)?;
    if manifest.version != expected_version {
        return Err(CoreError::Config(format!(
            "Unsupported or mismatched backup version: {}",
            manifest.version
        )));
    }

    let mut payload_reader: Box<dyn Read> = if expected_version == 2 {
        Box::new(BufReader::with_capacity(
            STREAM_BUF_SIZE,
            zstd::stream::read::Decoder::with_buffer(r)?,
        ))
    } else {
        Box::new(r)
    };

    let mut buf = vec![0u8; STREAM_BUF_SIZE];
    let mut total_bytes = 0u64;
    for entry in &manifest.files {
        let rel = validate_safe_rel_path(&entry.rel_path)?;
        let target = staging.join(&rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let actual_sha = {
            let mut out = BufWriter::with_capacity(STREAM_BUF_SIZE, File::create(&target)?);
            let sha = stream_copy_and_hash(&mut payload_reader, &mut out, entry.size, &mut buf)?;
            out.flush()?;
            sha
        };
        if actual_sha != entry.sha256_hex {
            return Err(CoreError::Config(format!(
                "Backup integrity check failed for {}",
                entry.rel_path
            )));
        }
        total_bytes = total_bytes
            .checked_add(entry.size)
            .ok_or_else(|| CoreError::Config("restore total size overflow".into()))?;
    }

    if expected_version == 2 {
        let mut extra = [0u8; 1];
        if payload_reader.read(&mut extra)? != 0 {
            return Err(CoreError::Config(
                "Backup archive contains unexpected trailing compressed data".into(),
            ));
        }
    }

    Ok((manifest.files.len(), total_bytes))
}

/// Validates staged DB integrity, sanitizes OS-specific settings in `config.json`, and
/// reconciles `source_roots` against `local_root_paths.json` for cross-OS portability.
fn prepare_staged_restore_artifacts(paths: &AppPaths, staging: &Path) -> Result<()> {
    let staged_cfg = staging.join("config.json");
    if staged_cfg.exists() {
        let mut cfg = AppConfig::load_or_default(&staged_cfg)?;
        sanitize_config_for_cross_os_restore(&mut cfg);
        cfg.save(&staged_cfg)?;
    }

    let staged_db = staging.join("papero.db");
    if staged_db.exists() {
        {
            let conn = Connection::open_with_flags(
                &staged_db,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            let _ = conn.execute_batch("PRAGMA journal_mode = DELETE;");
            let qc: String = conn.query_row("PRAGMA quick_check(1)", [], |r| r.get(0))?;
            if !qc.eq_ignore_ascii_case("ok") {
                return Err(CoreError::Config(format!(
                    "Staged backup SQLite integrity check failed: {qc}"
                )));
            }
            let has_roots_table: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='source_roots'",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(false);
            if has_roots_table {
                reconcile_source_roots_with_local_map(&conn, &paths.local_roots_path())?;
            }
        }
        for sidecar in ["papero.db-wal", "papero.db-shm"] {
            let p = staging.join(sidecar);
            if p.exists() {
                let _ = fs::remove_file(p);
            }
        }
    }

    Ok(())
}

fn apply_staged_dir_to_live(paths: &AppPaths, staging: &Path) -> Result<()> {
    fs::create_dir_all(&paths.data_dir)?;
    let staged_db = staging.join("papero.db");
    if staged_db.exists() {
        for sidecar in ["papero.db-wal", "papero.db-shm"] {
            let p = paths.data_dir.join(sidecar);
            if p.exists() {
                retry_transient_fs_op(|| {
                    if p.exists() {
                        fs::remove_file(&p)
                    } else {
                        Ok(())
                    }
                })?;
            }
        }
        retry_transient_fs_op(|| fs::copy(&staged_db, &paths.db_path).map(|_| ()))?;
    }
    let staged_cfg = staging.join("config.json");
    if staged_cfg.exists() {
        retry_transient_fs_op(|| fs::copy(&staged_cfg, &paths.config_path).map(|_| ()))?;
    }
    for (sub, dst_dir) in [
        ("tantivy", &paths.tantivy_dir),
        ("indices", &paths.indices_dir),
    ] {
        let staged_sub = staging.join(sub);
        if dst_dir.exists() {
            retry_transient_fs_op(|| {
                if dst_dir.exists() {
                    fs::remove_dir_all(dst_dir)
                } else {
                    Ok(())
                }
            })?;
        }
        fs::create_dir_all(dst_dir)?;
        if staged_sub.exists() {
            copy_dir_recursive(&staged_sub, dst_dir)?;
        }
    }
    Ok(())
}

/// Verifies integrity (magic header + per-file SHA-256 + path safety) and restores
/// `papero.db`, `config.json`, `tantivy/`, and `indices/` into `paths.data_dir`.
/// Supports both legacy uncompressed V1 and Zstd-compressed V2 `.papero-backup` archives.
pub fn restore_backup_bundle(paths: &AppPaths, src_file: &Path) -> Result<RestoreReport> {
    fs::create_dir_all(&paths.data_dir)?;
    let staging = paths
        .data_dir
        .join(format!(".restore_staging_{}", uuid::Uuid::new_v4()));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    let _staging_guard = StagingCleanup(Some(staging.clone()));

    let (files_restored, total_bytes) = unpack_and_verify_archive(src_file, &staging)?;
    prepare_staged_restore_artifacts(paths, &staging)?;
    apply_staged_dir_to_live(paths, &staging)?;

    Ok(RestoreReport {
        files_restored,
        total_bytes,
        restart_recommended: true,
    })
}

/// Verifies and stages a `.papero-backup` archive into `<data_dir>/.pending_restore`
/// without touching currently open SQLite, Tantivy, or memory-mapped USearch files.
/// Required on Windows (and clean on all OSes), where replacing open/mmapped files in-process
/// fails with OS sharing violations; applied on next startup by `apply_pending_restore_if_ready`.
pub fn stage_pending_restore(paths: &AppPaths, src_file: &Path) -> Result<RestoreReport> {
    fs::create_dir_all(&paths.data_dir)?;
    let tmp_staging = paths
        .data_dir
        .join(format!(".pending_restore_tmp_{}", uuid::Uuid::new_v4()));
    if tmp_staging.exists() {
        fs::remove_dir_all(&tmp_staging)?;
    }
    fs::create_dir_all(&tmp_staging)?;
    let mut staging_guard = StagingCleanup(Some(tmp_staging.clone()));

    let (files_restored, total_bytes) = unpack_and_verify_archive(src_file, &tmp_staging)?;
    prepare_staged_restore_artifacts(paths, &tmp_staging)?;

    let pending_dir = paths.pending_restore_dir();
    if pending_dir.exists() {
        fs::remove_dir_all(&pending_dir)?;
    }
    fs::rename(&tmp_staging, &pending_dir)?;
    staging_guard.0 = None;
    fs::write(pending_dir.join(PENDING_READY_MARKER), b"ready\n")?;

    Ok(RestoreReport {
        files_restored,
        total_bytes,
        restart_recommended: true,
    })
}

/// Applies a verified `.pending_restore` snapshot before any DB/Tantivy/USearch handles are opened.
/// Returns `Ok(true)` if a pending restore was applied.
pub fn apply_pending_restore_if_ready(paths: &AppPaths) -> Result<bool> {
    let pending_dir = paths.pending_restore_dir();
    let ready_marker = pending_dir.join(PENDING_READY_MARKER);
    if !ready_marker.exists() {
        return Ok(false);
    }
    apply_staged_dir_to_live(paths, &pending_dir)?;
    let _ = fs::remove_dir_all(&pending_dir);
    Ok(true)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else if ty.is_file() {
            fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repos::docs::create_heuristic_document;

    #[test]
    fn backup_and_restore_roundtrip_excludes_secrets_and_verifies_sha256() {
        let tmp = tempfile::tempdir().unwrap();
        let src_paths = AppPaths::from_data_dir(tmp.path().join("src_data"));
        src_paths.ensure_dirs().unwrap();
        let db = Database::open(&src_paths.db_path, 2).unwrap();
        {
            let w = db.writer();
            create_heuristic_document(&w, "Cohomology of Schemes").unwrap();
        }
        fs::write(src_paths.tantivy_dir.join("segment_0"), b"tantivy-mock").unwrap();
        fs::write(
            src_paths.indices_dir.join("passages_light_384.usearch"),
            b"hnsw-mock",
        )
        .unwrap();

        let cfg = AppConfig {
            polite_email_fallback: Some("secret-email@example.com".into()),
            ui_language: "en".into(),
            first_run_completed: true,
            ..AppConfig::default()
        };
        cfg.save(&src_paths.config_path).unwrap();

        let archive_path = tmp.path().join("out/my.papero-backup");
        let rep = create_backup_bundle(&src_paths, &db, &archive_path).unwrap();
        assert!(rep.files_count >= 4);
        assert!(rep.total_bytes > 0);

        let dst_paths = AppPaths::from_data_dir(tmp.path().join("dst_data"));
        dst_paths.ensure_dirs().unwrap();
        let res = restore_backup_bundle(&dst_paths, &archive_path).unwrap();
        assert_eq!(res.files_restored, rep.files_count);

        let restored_cfg = AppConfig::load_or_default(&dst_paths.config_path).unwrap();
        assert_eq!(restored_cfg.ui_language, "en");
        assert!(restored_cfg.first_run_completed);
        assert_eq!(restored_cfg.polite_email_fallback, None);

        let restored_db = Database::open(&dst_paths.db_path, 2).unwrap();
        let count: i64 = restored_db
            .reader()
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            fs::read(dst_paths.tantivy_dir.join("segment_0")).unwrap(),
            b"tantivy-mock"
        );
        assert_eq!(
            fs::read(dst_paths.indices_dir.join("passages_light_384.usearch")).unwrap(),
            b"hnsw-mock"
        );

        // Tamper detection
        let mut raw = fs::read(&archive_path).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xff;
        let bad_archive = tmp.path().join("out/bad.papero-backup");
        fs::write(&bad_archive, raw).unwrap();
        assert!(restore_backup_bundle(&dst_paths, &bad_archive).is_err());
    }

    #[test]
    fn backup_handles_live_bm25_managed_json_uncheckpointed_wal_and_overwrite() {
        use crate::index::bm25::Bm25Passage;

        let tmp = tempfile::tempdir().unwrap();
        let src_paths = AppPaths::from_data_dir(tmp.path().join("src"));
        src_paths.ensure_dirs().unwrap();

        let cfg = AppConfig {
            backup_path: Some("/tmp/local-only.papero-backup".into()),
            ..Default::default()
        };
        cfg.save(&src_paths.config_path).unwrap();

        let db = Database::open(&src_paths.db_path, 2).unwrap();
        {
            let w = db.writer();
            w.execute(
                "INSERT INTO documents (work_type, title, title_norm, created_at, updated_at)
                 VALUES ('article', 'Initial Doc', 'initial doc', 1, 1)",
                [],
            )
            .unwrap();
        }

        let bm25 = Bm25Index::open_or_create(&src_paths.tantivy_dir).unwrap();
        bm25.add_passages(&[Bm25Passage {
            passage_id: 1,
            document_id: 1,
            file_id: 1,
            root_id: "root-1".into(),
            text_content: "Cohomology of algebraic stacks".into(),
        }])
        .unwrap();

        let archive_path = tmp.path().join("backups/library.papero-backup");
        let rep1 = create_backup_bundle_with_bm25(&src_paths, &db, Some(&bm25), &archive_path).unwrap();
        assert!(archive_path.exists());
        assert!(rep1.files_count >= 3);

        // Mutate DB in WAL mode and overwrite existing archive (auto-update pattern)
        {
            let w = db.writer();
            w.execute(
                "INSERT INTO documents (work_type, title, title_norm, created_at, updated_at)
                 VALUES ('book', 'Second Doc', 'second doc', 2, 2)",
                [],
            )
            .unwrap();
        }
        let rep2 = create_backup_bundle_with_bm25(&src_paths, &db, Some(&bm25), &archive_path).unwrap();
        assert!(rep2.total_bytes > 0);
        assert_eq!(rep2.total_bytes, fs::metadata(&archive_path).unwrap().len());

        let dst_paths = AppPaths::from_data_dir(tmp.path().join("dst"));
        dst_paths.ensure_dirs().unwrap();
        restore_backup_bundle(&dst_paths, &archive_path).unwrap();

        // Portable backup clears host-specific backup_path
        let restored_cfg = AppConfig::load_or_default(&dst_paths.config_path).unwrap();
        assert!(restored_cfg.backup_path.is_none());

        let restored_db = Database::open(&dst_paths.db_path, 2).unwrap();
        let count: i64 = restored_db
            .reader()
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);

        // Restored Tantivy index must keep `.managed.json` and be searchable
        assert!(dst_paths.tantivy_dir.join(".managed.json").exists());
        let restored_bm25 = Bm25Index::open_or_create(&dst_paths.tantivy_dir).unwrap();
        assert_eq!(restored_bm25.search("cohomology", None, 5).unwrap().len(), 1);
    }

    #[test]
    fn backup_compresses_v2_archive_and_restores_legacy_v1_archives() {
        let tmp = tempfile::tempdir().unwrap();
        let src_paths = AppPaths::from_data_dir(tmp.path().join("src"));
        src_paths.ensure_dirs().unwrap();
        let db = Database::open(&src_paths.db_path, 2).unwrap();
        AppConfig::default().save(&src_paths.config_path).unwrap();

        // Write compressible synthetic index/data files (~512 KB total)
        let big_payload = "sheaf cohomology spectral sequence ".repeat(8000);
        let seg_path = src_paths.tantivy_dir.join("segment_big");
        fs::write(&seg_path, big_payload.as_bytes()).unwrap();
        let idx_payload = vec![0x2Au8; 256 * 1024];
        let idx_path = src_paths.indices_dir.join("passages_light_384.usearch");
        fs::write(&idx_path, &idx_payload).unwrap();

        let archive_v2 = tmp.path().join("v2.papero-backup");
        let rep = create_backup_bundle(&src_paths, &db, &archive_v2).unwrap();
        let raw_inputs = big_payload.len() as u64 + idx_payload.len() as u64;
        assert_eq!(rep.total_bytes, fs::metadata(&archive_v2).unwrap().len());
        assert!(
            rep.total_bytes < raw_inputs / 4,
            "expected V2 archive ({}) to compress far below raw inputs ({})",
            rep.total_bytes,
            raw_inputs
        );

        let dst_v2 = AppPaths::from_data_dir(tmp.path().join("dst_v2"));
        dst_v2.ensure_dirs().unwrap();
        let res_v2 = restore_backup_bundle(&dst_v2, &archive_v2).unwrap();
        assert!(res_v2.total_bytes >= raw_inputs);
        assert_eq!(fs::read(dst_v2.tantivy_dir.join("segment_big")).unwrap(), big_payload.as_bytes());

        // Truncated V2 stream must fail integrity validation cleanly
        let mut truncated = fs::read(&archive_v2).unwrap();
        truncated.pop();
        let bad_v2 = tmp.path().join("truncated_v2.papero-backup");
        fs::write(&bad_v2, truncated).unwrap();
        assert!(restore_backup_bundle(&dst_v2, &bad_v2).is_err());

        // Hand-craft a legacy uncompressed V1 archive and ensure backward-compatible restore
        let v1_cfg = br#"{"ui_language":"en","first_run_completed":true}"#;
        let v1_seg = b"legacy-v1-segment";
        let manifest = BackupManifest {
            version: 1,
            files: vec![
                BackupFileEntry {
                    rel_path: "config.json".into(),
                    size: v1_cfg.len() as u64,
                    sha256_hex: hex::encode(Sha256::digest(v1_cfg)),
                },
                BackupFileEntry {
                    rel_path: "tantivy/legacy_seg".into(),
                    size: v1_seg.len() as u64,
                    sha256_hex: hex::encode(Sha256::digest(v1_seg)),
                },
            ],
        };
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let archive_v1 = tmp.path().join("legacy_v1.papero-backup");
        {
            let mut f = File::create(&archive_v1).unwrap();
            f.write_all(BACKUP_MAGIC_V1).unwrap();
            f.write_all(&(manifest_bytes.len() as u32).to_le_bytes()).unwrap();
            f.write_all(&manifest_bytes).unwrap();
            f.write_all(v1_cfg).unwrap();
            f.write_all(v1_seg).unwrap();
        }
        let dst_v1 = AppPaths::from_data_dir(tmp.path().join("dst_v1"));
        dst_v1.ensure_dirs().unwrap();
        let res_v1 = restore_backup_bundle(&dst_v1, &archive_v1).unwrap();
        assert_eq!(res_v1.files_restored, 2);
        assert_eq!(fs::read(dst_v1.tantivy_dir.join("legacy_seg")).unwrap(), v1_seg);
    }

    #[test]
    fn staged_pending_restore_works_across_live_handles_and_remaps_cross_os_roots() {
        use crate::db::repos::docs::{insert_file_record, NewFileRecord};
        use crate::db::repos::roots::{
            get_source_root, insert_source_root, record_local_root_path, NewSourceRoot,
        };

        let tmp = tempfile::tempdir().unwrap();
        let src_paths = AppPaths::from_data_dir(tmp.path().join("src_macos"));
        src_paths.ensure_dirs().unwrap();

        let src_db = Database::open(&src_paths.db_path, 2).unwrap();
        {
            let w = src_db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "root-drive".into(),
                    label: "Drive Papers".into(),
                    root_path: "/Users/other-os/GoogleDrive/Papers".into(),
                    is_network_or_cloud: true,
                },
            )
            .unwrap();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "root-missing".into(),
                    label: "Unmapped Drive".into(),
                    root_path: "/Users/other-os/MissingFolder".into(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap();

            let d1 = create_heuristic_document(&w, "Portable Paper One").unwrap();
            insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: d1,
                    root_id: "root-drive",
                    rel_path: "sub/one.pdf",
                    format: "pdf",
                    size_bytes: 123,
                    mtime_ns: 456,
                    blake3_hash: "aaaa",
                    index_state: "ready",
                },
            )
            .unwrap();

            let d2 = create_heuristic_document(&w, "Offline Paper Two").unwrap();
            insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: d2,
                    root_id: "root-missing",
                    rel_path: "two.pdf",
                    format: "pdf",
                    size_bytes: 321,
                    mtime_ns: 654,
                    blake3_hash: "bbbb",
                    index_state: "ready",
                },
            )
            .unwrap();
        }

        let src_cfg = AppConfig {
            ui_language: "en".into(),
            first_run_completed: true,
            pdfium_lib_path: Some(PathBuf::from("/nonexistent/other-os/libpdfium.dylib")),
            tesseract_bin: Some("/nonexistent/other-os/tesseract".into()),
            ..Default::default()
        };
        src_cfg.save(&src_paths.config_path).unwrap();

        let archive = tmp.path().join("shared.papero-backup");
        create_backup_bundle(&src_paths, &src_db, &archive).unwrap();

        // Destination machine has a live open DB + Tantivy + a local path mapping for "root-drive"
        let dst_paths = AppPaths::from_data_dir(tmp.path().join("dst_win"));
        dst_paths.ensure_dirs().unwrap();
        let local_drive_dir = tmp.path().join("LocalDriveMount/Papers");
        fs::create_dir_all(&local_drive_dir).unwrap();
        fs::write(local_drive_dir.join("placeholder.pdf"), b"%PDF-1.4").unwrap();
        record_local_root_path(
            &dst_paths.local_roots_path(),
            "root-drive",
            &local_drive_dir.to_string_lossy(),
        )
        .unwrap();

        {
            let live_db = Database::open(&dst_paths.db_path, 2).unwrap();
            let _live_bm25 = Bm25Index::open_or_create(&dst_paths.tantivy_dir).unwrap();
            // Staging succeeds without touching live open DB/Tantivy files
            let rep = stage_pending_restore(&dst_paths, &archive).unwrap();
            assert!(rep.files_restored >= 2);
            assert!(dst_paths.pending_restore_dir().join(".ready").exists());
            let live_docs: i64 = live_db
                .reader()
                .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
                .unwrap();
            assert_eq!(live_docs, 0);
        }

        // Simulate next startup applying pending restore before opening DB/indices
        assert!(apply_pending_restore_if_ready(&dst_paths).unwrap());
        assert!(!dst_paths.pending_restore_dir().exists());
        assert!(!apply_pending_restore_if_ready(&dst_paths).unwrap());

        let restored_cfg = AppConfig::load_or_default(&dst_paths.config_path).unwrap();
        assert_eq!(restored_cfg.ui_language, "en");
        assert!(restored_cfg.first_run_completed);
        assert_eq!(restored_cfg.pdfium_lib_path, None);
        assert_eq!(restored_cfg.tesseract_bin, None);

        let restored_db = Database::open(&dst_paths.db_path, 2).unwrap();
        let r = restored_db.reader();
        let mapped = get_source_root(&r, "root-drive").unwrap().unwrap();
        assert_eq!(mapped.root_path, local_drive_dir.to_string_lossy());
        assert!(mapped.is_reachable);

        let unmapped = get_source_root(&r, "root-missing").unwrap().unwrap();
        assert!(!unmapped.is_reachable);

        let avail_one: i64 = r
            .query_row(
                "SELECT is_available FROM files WHERE rel_path = 'sub/one.pdf'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let avail_two: i64 = r
            .query_row(
                "SELECT is_available FROM files WHERE rel_path = 'two.pdf'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(avail_one, 1);
        assert_eq!(avail_two, 0);
    }

    #[test]
    fn validate_safe_rel_path_accepts_posix_and_backslash_and_rejects_traversal() {
        assert!(validate_safe_rel_path("papero.db").is_ok());
        assert!(validate_safe_rel_path("tantivy/meta.json").is_ok());
        assert!(validate_safe_rel_path("tantivy\\meta.json").is_ok());
        assert!(validate_safe_rel_path("../papero.db").is_err());
        assert!(validate_safe_rel_path("..\\papero.db").is_err());
        assert!(validate_safe_rel_path("C:\\papero.db").is_err());
        assert!(validate_safe_rel_path("/etc/passwd").is_err());
    }

    #[test]
    fn backup_v2_preserves_v7_annotations_reading_state_collections_and_saved_searches() {
        use crate::db::repos::annotations::{
            create_annotation_for_file, get_reading_state_for_file, list_annotations_for_file,
            set_document_reading_status, set_document_starred, upsert_reading_state_for_file,
            NewFileAnnotation, ReadingStatePatch,
        };
        use crate::db::repos::collections::{
            add_documents_to_collection, create_collection, create_saved_search, list_collections,
            list_saved_searches,
        };
        use crate::db::repos::docs::{insert_file_record, NewFileRecord};
        use crate::db::repos::roots::{insert_source_root, NewSourceRoot};
        use crate::extract::types::NormalizedRect;

        let tmp = tempfile::tempdir().unwrap();
        let src_paths = AppPaths::from_data_dir(tmp.path().join("src_v7"));
        src_paths.ensure_dirs().unwrap();
        let src_db = Database::open(&src_paths.db_path, 2).unwrap();

        let (doc_id, file_id) = {
            let w = src_db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "r1".into(),
                    label: "Library".into(),
                    root_path: tmp.path().to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap();
            let did = create_heuristic_document(&w, "Harmonic Analysis").unwrap();
            let fid = insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: did,
                    root_id: "r1",
                    rel_path: "harmonic.pdf",
                    format: "pdf",
                    size_bytes: 512,
                    mtime_ns: 1,
                    blake3_hash: "blake3_harmonic_v7",
                    index_state: "ready",
                },
            )
            .unwrap();
            create_annotation_for_file(
                &w,
                fid,
                &NewFileAnnotation {
                    page_ord: 9,
                    kind: "highlight".into(),
                    color: "green".into(),
                    rects: vec![NormalizedRect {
                        x: 0.1,
                        y: 0.2,
                        w: 0.5,
                        h: 0.03,
                    }],
                    quoted_text: "Calderón–Zygmund".into(),
                    note_markdown: "Key bound $\\|Tf\\|_p \\le C_p \\|f\\|_p$".into(),
                },
            )
            .unwrap();
            upsert_reading_state_for_file(
                &w,
                fid,
                &ReadingStatePatch {
                    last_page_ord: Some(45),
                    zoom_mode: Some("custom".into()),
                    custom_zoom: Some(1.25),
                    rotation_deg: Some(90),
                    touch_opened: true,
                },
            )
            .unwrap();
            set_document_reading_status(&w, did, "reading").unwrap();
            set_document_starred(&w, did, true).unwrap();
            let col = create_collection(&w, "Course Notes", Some("#10b981")).unwrap();
            add_documents_to_collection(&w, col.id, &[did]).unwrap();
            create_saved_search(&w, "Reading now", "metadata", "", r#"{"reading_status":"reading"}"#)
                .unwrap();
            (did, fid)
        };

        let archive = tmp.path().join("v7.papero-backup");
        create_backup_bundle(&src_paths, &src_db, &archive).unwrap();

        let dst_paths = AppPaths::from_data_dir(tmp.path().join("dst_v7"));
        dst_paths.ensure_dirs().unwrap();
        restore_backup_bundle(&dst_paths, &archive).unwrap();

        let restored_db = Database::open(&dst_paths.db_path, 2).unwrap();
        let r = restored_db.reader();
        let anns = list_annotations_for_file(&r, file_id).unwrap();
        assert_eq!(anns.len(), 1);
        assert_eq!(anns[0].color, "green");
        let rs = get_reading_state_for_file(&r, file_id).unwrap().unwrap();
        assert_eq!(rs.last_page_ord, 45);
        assert_eq!(rs.rotation_deg, 90);
        let cols = list_collections(&r).unwrap();
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].document_ids, vec![doc_id]);
        let saved = list_saved_searches(&r).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "Reading now");
    }
}
