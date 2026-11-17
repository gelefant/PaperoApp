use papero_core::backup::{
    apply_pending_restore_if_ready, create_backup_bundle_with_bm25, BackupReport,
};
use papero_core::config::{init_logging, AppConfig, AppPaths, WorkerGuard};
use papero_core::db::repos::docs::{get_file_stream_info, FileStreamInfo};
use papero_core::db::repos::roots::{
    list_source_roots, prune_orphaned_catalog_entities, reconcile_source_roots_with_local_map,
};
use papero_core::db::Database;
use papero_core::error::Result;
use papero_core::fs::scanner::{rescan_sources, ScanControl};
use papero_core::fs::watcher::{FsChangeBatch, RootWatcher};
use papero_core::index::bm25::Bm25Index;
use papero_core::index::vector::VectorIndex;
use papero_core::index::worker::run_extract_bm25_queue;
use papero_core::metadata::WaterfallClients;
use papero_core::ml::{LazyModelSlot, ModelProfile, OnnxEmbedder, OnnxReranker};
use papero_core::viewer::djvu_cache::DjvuCache;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tokio::sync::Notify;

const VIEWER_FILE_CACHE_CAPACITY: usize = 8;

#[derive(Clone)]
pub struct CachedViewerFile {
    pub file: Arc<std::fs::File>,
    pub len: u64,
}

#[derive(Debug)]
pub enum ViewerFileError {
    NotFound,
    Unreachable,
    Forbidden(String),
    Io(String),
}

/// Thread-safe positional file read (`pread` on Unix, `seek_read` on Windows) so concurrent
/// PDF.js range requests share one cached `Arc<File>` without reopening or locking SQLite.
pub fn read_file_range_at(
    file: &std::fs::File,
    start: u64,
    len: usize,
) -> std::io::Result<Vec<u8>> {
    if len == 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; len];
    let mut filled = 0usize;
    while filled < len {
        let offset = start + filled as u64;
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(file, &mut buf[filled..], offset)?;
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_read(file, &mut buf[filled..], offset)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF while reading PDF byte range",
            ));
        }
        filled += n;
    }
    Ok(buf)
}

/// RAII guard that pauses background vector embedding while an interactive scan or bulk metadata
/// synchronization is running, automatically resuming when the last foreground batch finishes.
pub struct EmbedPauseGuard(Arc<AppState>);

impl Drop for EmbedPauseGuard {
    fn drop(&mut self) {
        if self.0.embed_pause_depth.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.current_embed_control().resume();
        }
    }
}

/// RAII guard marking `metadata_batch_active` while a metadata batch loop runs.
pub struct MetadataBatchActiveGuard(Arc<AppState>);

impl Drop for MetadataBatchActiveGuard {
    fn drop(&mut self) {
        self.0.metadata_batch_active.store(false, Ordering::SeqCst);
    }
}

pub struct AppState {
    pub paths: AppPaths,
    pub config: RwLock<AppConfig>,
    pub db: Arc<Database>,
    pub bm25: Arc<Bm25Index>,
    pub vector_light: Arc<VectorIndex>,
    pub vector_quality: Arc<VectorIndex>,
    pub embedder_slot: Arc<LazyModelSlot<OnnxEmbedder>>,
    pub reranker_slot: Arc<LazyModelSlot<OnnxReranker>>,
    pub djvu_cache: Arc<DjvuCache>,
    pub viewer_file_cache: Mutex<VecDeque<(i64, CachedViewerFile)>>,
    pub clients: RwLock<Arc<WaterfallClients>>,
    pub scan_control: Mutex<Option<Arc<ScanControl>>>,
    pub metadata_run_mutex: Arc<tokio::sync::Mutex<()>>,
    pub metadata_batch_cancel: Arc<AtomicBool>,
    pub bg_metadata_cancel: Arc<AtomicBool>,
    pub metadata_cancel_notify: Arc<Notify>,
    pub metadata_batch_active: Arc<AtomicBool>,
    pub embed_control: Mutex<Arc<ScanControl>>,
    pub embed_pause_depth: AtomicUsize,
    pub embed_worker_running: Arc<AtomicBool>,
    pub embed_worker_reschedule: Arc<AtomicBool>,
    pub watcher: Mutex<Option<RootWatcher>>,
    pub watcher_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    pub app_handle: Mutex<Option<AppHandle>>,
    pub backup_mutex: Arc<tokio::sync::Mutex<()>>,
    pub auto_backup_dirty: Arc<AtomicBool>,
    pub auto_backup_notify: Arc<Notify>,
    _log_guard: Option<WorkerGuard>,
}

impl AppState {
    pub fn initialize() -> Result<Self> {
        let data_dir = AppPaths::default_app_dir();
        let paths = AppPaths::from_data_dir(data_dir);
        std::fs::create_dir_all(&paths.data_dir)?;
        let restored_on_boot = apply_pending_restore_if_ready(&paths)?;
        paths.ensure_dirs()?;

        let log_guard = init_logging(&paths.logs_dir)?;
        if restored_on_boot {
            tracing::info!("applied pending backup restore on startup");
        }
        let config = AppConfig::load_or_default(&paths.config_path)?;
        let db = Arc::new(Database::open(&paths.db_path, 8)?);
        let bm25 = Arc::new(Bm25Index::open_or_create(&paths.tantivy_dir)?);
        {
            let w = db.writer();
            if let Err(err) = reconcile_source_roots_with_local_map(&w, &paths.local_roots_path()) {
                tracing::warn!(error = %err, "failed reconciling source roots on startup");
            }
            if let Ok(orphan_docs) = prune_orphaned_catalog_entities(&w) {
                if !orphan_docs.is_empty() {
                    for doc_id in orphan_docs {
                        let _ = bm25.delete_document(doc_id as u64);
                    }
                    let _ = bm25.commit();
                }
            }
        }
        let vector_light = Arc::new(VectorIndex::open_for_profile(
            &paths.indices_dir,
            ModelProfile::Light,
        )?);
        let vector_quality = Arc::new(VectorIndex::open_for_profile(
            &paths.indices_dir,
            ModelProfile::Quality,
        )?);
        let djvu_cache = Arc::new(DjvuCache::new(&paths.cache_dir, config.cache_limit_mb)?);
        let clients = Arc::new(WaterfallClients::from_config(&config)?);

        Ok(Self {
            paths,
            config: RwLock::new(config),
            db,
            bm25,
            vector_light,
            vector_quality,
            embedder_slot: Arc::new(LazyModelSlot::new()),
            reranker_slot: Arc::new(LazyModelSlot::new()),
            djvu_cache,
            viewer_file_cache: Mutex::new(VecDeque::with_capacity(VIEWER_FILE_CACHE_CAPACITY)),
            clients: RwLock::new(clients),
            scan_control: Mutex::new(None),
            metadata_run_mutex: Arc::new(tokio::sync::Mutex::new(())),
            metadata_batch_cancel: Arc::new(AtomicBool::new(false)),
            bg_metadata_cancel: Arc::new(AtomicBool::new(false)),
            metadata_cancel_notify: Arc::new(Notify::new()),
            metadata_batch_active: Arc::new(AtomicBool::new(false)),
            embed_control: Mutex::new(Arc::new(ScanControl::new())),
            embed_pause_depth: AtomicUsize::new(0),
            embed_worker_running: Arc::new(AtomicBool::new(false)),
            embed_worker_reschedule: Arc::new(AtomicBool::new(false)),
            watcher: Mutex::new(None),
            watcher_task: Mutex::new(None),
            app_handle: Mutex::new(None),
            backup_mutex: Arc::new(tokio::sync::Mutex::new(())),
            auto_backup_dirty: Arc::new(AtomicBool::new(false)),
            auto_backup_notify: Arc::new(Notify::new()),
            _log_guard: Some(log_guard),
        })
    }

    pub fn current_embed_control(&self) -> Arc<ScanControl> {
        self.embed_control
            .lock()
            .expect("embed_control poisoned")
            .clone()
    }

    pub fn reset_embed_control(&self) -> Arc<ScanControl> {
        let next = Arc::new(ScanControl::new());
        if self.embed_pause_depth.load(Ordering::SeqCst) > 0 {
            next.pause();
        }
        let mut slot = self.embed_control.lock().expect("embed_control poisoned");
        *slot = Arc::clone(&next);
        next
    }

    pub fn cancel_embed_worker(&self) {
        self.embed_worker_reschedule.store(false, Ordering::SeqCst);
        let mut slot = self.embed_control.lock().expect("embed_control poisoned");
        slot.cancel();
        let next = Arc::new(ScanControl::new());
        if self.embed_pause_depth.load(Ordering::SeqCst) > 0 {
            next.pause();
        }
        *slot = next;
    }

    pub fn pause_embed_for_batch(state: &Arc<Self>) -> EmbedPauseGuard {
        if state.embed_pause_depth.fetch_add(1, Ordering::SeqCst) == 0 {
            state.current_embed_control().pause();
        }
        EmbedPauseGuard(Arc::clone(state))
    }

    pub fn mark_metadata_batch_active(state: &Arc<Self>) -> MetadataBatchActiveGuard {
        state.metadata_batch_active.store(true, Ordering::SeqCst);
        MetadataBatchActiveGuard(Arc::clone(state))
    }

    pub fn is_foreground_batch_active(&self) -> bool {
        let scan_busy = self
            .scan_control
            .lock()
            .map(|g| g.is_some())
            .unwrap_or(false);
        scan_busy || self.metadata_batch_active.load(Ordering::SeqCst)
    }

    pub fn stop_background_activity(&self) {
        self.auto_backup_dirty.store(false, Ordering::SeqCst);
        self.metadata_batch_cancel.store(true, Ordering::SeqCst);
        self.bg_metadata_cancel.store(true, Ordering::SeqCst);
        self.metadata_cancel_notify.notify_waiters();
        self.cancel_embed_worker();
        if let Ok(g) = self.scan_control.lock() {
            if let Some(ctrl) = g.as_ref() {
                ctrl.cancel();
            }
        }
        if let Ok(mut t) = self.watcher_task.lock() {
            if let Some(prev) = t.take() {
                prev.abort();
            }
        }
        if let Ok(mut w) = self.watcher.lock() {
            *w = None;
        }
        self.invalidate_viewer_file_cache();
        self.embedder_slot.unload();
        self.reranker_slot.unload();
    }

    pub fn invalidate_viewer_file_cache(&self) {
        if let Ok(mut guard) = self.viewer_file_cache.lock() {
            guard.clear();
        }
    }

    pub fn get_viewer_stream_info(
        &self,
        file_id: i64,
    ) -> std::result::Result<FileStreamInfo, ViewerFileError> {
        let info = {
            let r = self.db.reader();
            match get_file_stream_info(&r, file_id) {
                Ok(Some(i)) => i,
                Ok(None) => return Err(ViewerFileError::NotFound),
                Err(e) => return Err(ViewerFileError::Forbidden(e.to_string())),
            }
        };
        if !info.is_available || !info.abs_path.exists() {
            return Err(ViewerFileError::Unreachable);
        }
        Ok(info)
    }

    pub fn get_or_open_viewer_file(
        &self,
        file_id: i64,
    ) -> std::result::Result<CachedViewerFile, ViewerFileError> {
        if let Ok(mut guard) = self.viewer_file_cache.lock() {
            if let Some(pos) = guard.iter().position(|(id, _)| *id == file_id) {
                let entry = if pos == 0 {
                    guard[0].1.clone()
                } else {
                    let item = guard.remove(pos).unwrap();
                    let cloned = item.1.clone();
                    guard.push_front(item);
                    cloned
                };
                return Ok(entry);
            }
        }

        let info = self.get_viewer_stream_info(file_id)?;
        let file = std::fs::File::open(&info.abs_path)
            .map_err(|e| ViewerFileError::Io(e.to_string()))?;
        let len = file
            .metadata()
            .map_err(|e| ViewerFileError::Io(e.to_string()))?
            .len();
        let entry = CachedViewerFile {
            file: Arc::new(file),
            len,
        };
        if let Ok(mut guard) = self.viewer_file_cache.lock() {
            if let Some(pos) = guard.iter().position(|(id, _)| *id == file_id) {
                guard.remove(pos);
            } else if guard.len() >= VIEWER_FILE_CACHE_CAPACITY {
                guard.pop_back();
            }
            guard.push_front((file_id, entry.clone()));
        }
        Ok(entry)
    }

    pub fn refresh_watcher(state: &Arc<Self>, handle: Option<AppHandle>) {
        if let Some(h) = handle {
            if let Ok(mut g) = state.app_handle.lock() {
                *g = Some(h);
            }
        }
        if let Ok(mut t) = state.watcher_task.lock() {
            if let Some(prev) = t.take() {
                prev.abort();
            }
        }
        if let Ok(mut w) = state.watcher.lock() {
            *w = None;
        }

        let enabled = state
            .config
            .read()
            .map(|c| c.fs_watcher_enabled)
            .unwrap_or(true);
        if !enabled {
            return;
        }

        let roots: Vec<PathBuf> = {
            let r = state.db.reader();
            list_source_roots(&r)
                .unwrap_or_default()
                .into_iter()
                .filter(|sr| sr.is_enabled && sr.is_reachable)
                .map(|sr| PathBuf::from(sr.root_path))
                .filter(|p| p.is_dir())
                .collect()
        };
        if roots.is_empty() {
            return;
        }

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<FsChangeBatch>();
        let Ok(watcher) = RootWatcher::start(&roots, tx) else {
            return;
        };
        if let Ok(mut w) = state.watcher.lock() {
            *w = Some(watcher);
        }

        let st = Arc::clone(state);
        let task = tauri::async_runtime::spawn(async move {
            while rx.recv().await.is_some() {
                tokio::time::sleep(Duration::from_millis(350)).await;
                while rx.try_recv().is_ok() {}
                if st.is_foreground_batch_active() {
                    continue;
                }
                let db = Arc::clone(&st.db);
                let bm25 = Arc::clone(&st.bm25);
                let cfg = st.config.read().expect("config poisoned").clone();
                let has_changes = tokio::task::spawn_blocking(move || -> bool {
                    let Ok(summary) = rescan_sources(&db, Some(&bm25), None, None) else {
                        return false;
                    };
                    let _ = run_extract_bm25_queue(&db, &bm25, &cfg, None);
                    summary.cnt_new + summary.cnt_modified + summary.cnt_moved_renamed + summary.cnt_removed > 0
                })
                .await
                .unwrap_or(false);
                if has_changes {
                    st.invalidate_viewer_file_cache();
                    st.schedule_auto_backup();
                    if let Ok(g) = st.app_handle.lock() {
                        if let Some(app) = g.as_ref() {
                            let _ = app.emit("library-fs-updated", ());
                        }
                    }
                }
            }
        });
        if let Ok(mut t) = state.watcher_task.lock() {
            *t = Some(task);
        }
    }

    /// Marks library state as dirty for debounced automatic backup refresh
    /// if a primary backup file has been configured and auto-backup is enabled.
    pub fn schedule_auto_backup(&self) {
        let should_schedule = self
            .config
            .read()
            .map(|c| {
                c.auto_backup_enabled
                    && c.backup_path
                        .as_deref()
                        .map(str::trim)
                        .is_some_and(|p| !p.is_empty())
            })
            .unwrap_or(false);
        if should_schedule {
            self.auto_backup_dirty.store(true, Ordering::SeqCst);
            self.auto_backup_notify.notify_one();
        }
    }

    /// Runs a serialized streamed backup to `dest_path`, records `backup_path`,
    /// `last_backup_at`, and `last_backup_bytes` in `config.json`, and notifies the UI.
    pub async fn run_backup_and_record(
        state: &Arc<Self>,
        dest_path: &Path,
    ) -> std::result::Result<BackupReport, String> {
        let _guard = state.backup_mutex.lock().await;
        // Clear pending auto-backup flag since this run captures latest committed state
        state.auto_backup_dirty.store(false, Ordering::SeqCst);

        let st = Arc::clone(state);
        let dest = dest_path.to_path_buf();
        let report = tokio::task::spawn_blocking(move || {
            create_backup_bundle_with_bm25(&st.paths, &st.db, Some(&st.bm25), &dest)
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        {
            let mut cfg = state.config.read().expect("config poisoned").clone();
            cfg.backup_path = Some(report.archive_path.clone());
            cfg.last_backup_at = Some(now);
            cfg.last_backup_bytes = Some(report.total_bytes);
            cfg.save(&state.paths.config_path).map_err(|e| e.to_string())?;
            *state.config.write().expect("config poisoned") = cfg;
        }
        if let Ok(g) = state.app_handle.lock() {
            if let Some(app) = g.as_ref() {
                let _ = app.emit("papero://backup-updated", &report);
            }
        }
        Ok(report)
    }

    pub fn spawn_auto_backup_loop(state: Arc<Self>) {
        tauri::async_runtime::spawn(async move {
            loop {
                if !state.auto_backup_dirty.load(Ordering::SeqCst) {
                    state.auto_backup_notify.notified().await;
                }
                // Debounce rapid mutations (scans, bulk metadata edits) up to 45s max wait
                let start = tokio::time::Instant::now();
                loop {
                    let elapsed = start.elapsed();
                    if elapsed >= Duration::from_secs(45) {
                        break;
                    }
                    let wait = Duration::from_secs(12).min(Duration::from_secs(45) - elapsed);
                    tokio::select! {
                        _ = state.auto_backup_notify.notified() => {},
                        _ = tokio::time::sleep(wait) => break,
                    }
                }

                // Defer automatic backup compression until active rescans or bulk metadata runs settle
                // so a 300-book import never re-compresses ~900 MB repeatedly mid-sync.
                let defer_start = tokio::time::Instant::now();
                while state.is_foreground_batch_active()
                    && defer_start.elapsed() < Duration::from_secs(3600)
                {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }

                if !state.auto_backup_dirty.swap(false, Ordering::SeqCst) {
                    continue;
                }

                let target: Option<PathBuf> = {
                    let cfg = state.config.read().expect("config poisoned");
                    if cfg.auto_backup_enabled {
                        cfg.backup_path
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(PathBuf::from)
                    } else {
                        None
                    }
                };
                let Some(dest) = target else {
                    continue;
                };
                if let Some(parent) = dest.parent() {
                    if !parent.as_os_str().is_empty() && !parent.exists() {
                        tracing::warn!(
                            path = %dest.display(),
                            "skipping automatic backup update: destination folder does not exist (unmounted volume?)"
                        );
                        continue;
                    }
                }
                if let Err(err) = Self::run_backup_and_record(&state, &dest).await {
                    tracing::warn!(error = %err, path = %dest.display(), "automatic backup update failed");
                }
            }
        });
    }

    pub fn vector_index_for(&self, profile: ModelProfile) -> Arc<VectorIndex> {
        match profile {
            ModelProfile::Light => Arc::clone(&self.vector_light),
            ModelProfile::Quality => Arc::clone(&self.vector_quality),
        }
    }

    pub fn spawn_idle_reaper(state: Arc<Self>) {
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(15)).await;
                let timeout_secs = state
                    .config
                    .read()
                    .map(|c| c.model_idle_unload_secs)
                    .unwrap_or(180);
                if timeout_secs == 0 {
                    continue;
                }
                let timeout = Duration::from_secs(timeout_secs);
                state.embedder_slot.evict_if_idle(timeout);
                state.reranker_slot.evict_if_idle(timeout);
            }
        });
    }

    #[cfg(test)]
    pub fn from_paths_without_logging(paths: AppPaths) -> Result<Self> {
        paths.ensure_dirs()?;
        let config = AppConfig::default();
        let db = Arc::new(Database::open(&paths.db_path, 2)?);
        let bm25 = Arc::new(Bm25Index::open_or_create(&paths.tantivy_dir)?);
        let vector_light = Arc::new(VectorIndex::open_for_profile(
            &paths.indices_dir,
            ModelProfile::Light,
        )?);
        let vector_quality = Arc::new(VectorIndex::open_for_profile(
            &paths.indices_dir,
            ModelProfile::Quality,
        )?);
        let djvu_cache = Arc::new(DjvuCache::new(&paths.cache_dir, config.cache_limit_mb)?);
        let clients = Arc::new(WaterfallClients::from_config(&config)?);
        Ok(Self {
            paths,
            config: RwLock::new(config),
            db,
            bm25,
            vector_light,
            vector_quality,
            embedder_slot: Arc::new(LazyModelSlot::new()),
            reranker_slot: Arc::new(LazyModelSlot::new()),
            djvu_cache,
            viewer_file_cache: Mutex::new(VecDeque::with_capacity(VIEWER_FILE_CACHE_CAPACITY)),
            clients: RwLock::new(clients),
            scan_control: Mutex::new(None),
            metadata_run_mutex: Arc::new(tokio::sync::Mutex::new(())),
            metadata_batch_cancel: Arc::new(AtomicBool::new(false)),
            bg_metadata_cancel: Arc::new(AtomicBool::new(false)),
            metadata_cancel_notify: Arc::new(Notify::new()),
            metadata_batch_active: Arc::new(AtomicBool::new(false)),
            embed_control: Mutex::new(Arc::new(ScanControl::new())),
            embed_pause_depth: AtomicUsize::new(0),
            embed_worker_running: Arc::new(AtomicBool::new(false)),
            embed_worker_reschedule: Arc::new(AtomicBool::new(false)),
            watcher: Mutex::new(None),
            watcher_task: Mutex::new(None),
            app_handle: Mutex::new(None),
            backup_mutex: Arc::new(tokio::sync::Mutex::new(())),
            auto_backup_dirty: Arc::new(AtomicBool::new(false)),
            auto_backup_notify: Arc::new(Notify::new()),
            _log_guard: None,
        })
    }
}
