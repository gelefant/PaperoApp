use papero_core::backup::{create_backup_bundle_with_bm25, BackupReport};
use papero_core::config::{init_logging, AppConfig, AppPaths, WorkerGuard};
use papero_core::db::repos::roots::{list_source_roots, prune_orphaned_catalog_entities};
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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tokio::sync::Notify;

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
    pub clients: RwLock<Arc<WaterfallClients>>,
    pub scan_control: Mutex<Option<Arc<ScanControl>>>,
    pub metadata_batch_cancel: Arc<AtomicBool>,
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
        paths.ensure_dirs()?;

        let log_guard = init_logging(&paths.logs_dir)?;
        let config = AppConfig::load_or_default(&paths.config_path)?;
        let db = Arc::new(Database::open(&paths.db_path, 4)?);
        let bm25 = Arc::new(Bm25Index::open_or_create(&paths.tantivy_dir)?);
        {
            let w = db.writer();
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
            clients: RwLock::new(clients),
            scan_control: Mutex::new(None),
            metadata_batch_cancel: Arc::new(AtomicBool::new(false)),
            watcher: Mutex::new(None),
            watcher_task: Mutex::new(None),
            app_handle: Mutex::new(None),
            backup_mutex: Arc::new(tokio::sync::Mutex::new(())),
            auto_backup_dirty: Arc::new(AtomicBool::new(false)),
            auto_backup_notify: Arc::new(Notify::new()),
            _log_guard: Some(log_guard),
        })
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
                let manual_busy = st
                    .scan_control
                    .lock()
                    .map(|g| g.is_some())
                    .unwrap_or(false);
                if manual_busy {
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
            clients: RwLock::new(clients),
            scan_control: Mutex::new(None),
            metadata_batch_cancel: Arc::new(AtomicBool::new(false)),
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
