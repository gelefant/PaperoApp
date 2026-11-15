use crate::error::Result;
use crate::fs::roots::detect_doc_format;
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct FsChangeBatch {
    pub paths: Vec<PathBuf>,
}

/// Watches enabled source roots with a 1-second debounce window, emitting batches
/// whenever `.pdf`, `.djvu`, or `.djv` files are created, modified, renamed, or deleted.
pub struct RootWatcher {
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
}

impl RootWatcher {
    pub fn start(
        roots: &[PathBuf],
        tx: mpsc::UnboundedSender<FsChangeBatch>,
    ) -> Result<Self> {
        let mut debouncer = new_debouncer(
            Duration::from_millis(800),
            None,
            move |res: DebounceEventResult| {
                if let Ok(events) = res {
                    let mut paths = Vec::new();
                    for ev in events {
                        for p in &ev.paths {
                            if detect_doc_format(p).is_some() {
                                paths.push(p.clone());
                            }
                        }
                    }
                    if !paths.is_empty() {
                        let _ = tx.send(FsChangeBatch { paths });
                    }
                }
            },
        )?;

        for r in roots {
            if r.is_dir() {
                debouncer.watch(r, RecursiveMode::Recursive)?;
            }
        }

        Ok(Self {
            _debouncer: debouncer,
        })
    }
}
