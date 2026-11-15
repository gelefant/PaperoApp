use crate::config::{AppConfig, AppPaths};
use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::index::bm25::Bm25Index;
use rusqlite::{backup::Backup, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
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
    let p = Path::new(raw);
    if p.is_absolute() {
        return Err(CoreError::Config(format!(
            "backup contains absolute path: {raw}"
        )));
    }
    let mut clean = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Normal(seg) => clean.push(seg),
            _ => {
                return Err(CoreError::Config(format!(
                    "backup contains unsafe path component: {raw}"
                )));
            }
        }
    }
    let first = clean
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .unwrap_or("");
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

struct StagingCleanup(PathBuf);
impl Drop for StagingCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
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
    let _cleanup = StagingCleanup(tmp_dir.clone());

    let snap_db_path = tmp_dir.join("papero.db");
    {
        let src_conn = db.reader();
        let _ = src_conn.busy_timeout(Duration::from_secs(10));
        let mut dst_conn = Connection::open(&snap_db_path)?;
        let backup = Backup::new(&src_conn, &mut dst_conn)?;
        backup.run_to_completion(4096, Duration::from_millis(1), None)?;
    }

    let mut cfg = AppConfig::load_or_default(&paths.config_path)?;
    // Never export personal fallback email, secrets, or host-specific backup targets
    cfg.polite_email_fallback = None;
    cfg.backup_path = None;
    cfg.last_backup_at = None;
    cfg.last_backup_bytes = None;
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

/// Verifies integrity (magic header + per-file SHA-256 + path safety) and restores
/// `papero.db`, `config.json`, `tantivy/`, and `indices/` into `paths.data_dir`.
/// Supports both legacy uncompressed V1 and Zstd-compressed V2 `.papero-backup` archives.
pub fn restore_backup_bundle(paths: &AppPaths, src_file: &Path) -> Result<RestoreReport> {
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

    let staging = paths.data_dir.join(format!(".restore_staging_{}", uuid::Uuid::new_v4()));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    let _staging_guard = StagingCleanup(staging.clone());

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

    // Apply verified staging files into live data directory
    let staged_db = staging.join("papero.db");
    if staged_db.exists() {
        for sidecar in ["papero.db-wal", "papero.db-shm"] {
            let p = paths.data_dir.join(sidecar);
            if p.exists() {
                let _ = fs::remove_file(p);
            }
        }
        fs::copy(&staged_db, &paths.db_path)?;
    }
    let staged_cfg = staging.join("config.json");
    if staged_cfg.exists() {
        fs::copy(&staged_cfg, &paths.config_path)?;
    }
    for (sub, dst_dir) in [
        ("tantivy", &paths.tantivy_dir),
        ("indices", &paths.indices_dir),
    ] {
        let staged_sub = staging.join(sub);
        if dst_dir.exists() {
            fs::remove_dir_all(dst_dir)?;
        }
        fs::create_dir_all(dst_dir)?;
        if staged_sub.exists() {
            copy_dir_recursive(&staged_sub, dst_dir)?;
        }
    }

    let _ = fs::remove_dir_all(&staging);

    Ok(RestoreReport {
        files_restored: manifest.files.len(),
        total_bytes,
        restart_recommended: true,
    })
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
}
