use crate::error::{CoreError, Result};
use crate::ml::ModelProfile;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use usearch::{Index, IndexOptions, MetricKind, ScalarKind};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VectorHit {
    pub passage_id: u64,
    /// Cosine similarity score in `[-1.0, 1.0]` (`1.0 - cosine_distance`).
    pub score: f32,
}

struct VectorState {
    index: Index,
    is_view: bool,
}

/// Disk-backed HNSW vector index using `usearch` (`ScalarKind::F16`, `MetricKind::Cos`).
/// Keeps reads OS-mmapped (`view()`) and switches to writable only during batch updates,
/// writing atomically via a temporary file before reopening as mmap.
pub struct VectorIndex {
    path: PathBuf,
    dimensions: usize,
    state: RwLock<VectorState>,
}

impl VectorIndex {
    pub fn filename_for_profile(profile: ModelProfile) -> &'static str {
        match profile {
            ModelProfile::Light => "passages_light_384.usearch",
            ModelProfile::Quality => "passages_quality_1024.usearch",
        }
    }

    pub fn dimensions_for_profile(profile: ModelProfile) -> usize {
        match profile {
            ModelProfile::Light => 384,
            ModelProfile::Quality => 1024,
        }
    }

    pub fn open_for_profile(indices_dir: &Path, profile: ModelProfile) -> Result<Self> {
        let path = indices_dir.join(Self::filename_for_profile(profile));
        let dim = Self::dimensions_for_profile(profile);
        Self::open(&path, dim)
    }

    pub fn open(path: &Path, dimensions: usize) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let opts = index_options(dimensions);
        let index = Index::new(&opts)
            .map_err(|e| CoreError::Config(format!("failed creating usearch index: {e}")))?;

        let is_view = if path.exists() {
            let path_str = path_to_str(path)?;
            index
                .view(path_str)
                .map_err(|e| CoreError::Config(format!("failed mmapping usearch index: {e}")))?;
            true
        } else {
            false
        };

        Ok(Self {
            path: path.to_path_buf(),
            dimensions,
            state: RwLock::new(VectorState { index, is_view }),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    pub fn len(&self) -> usize {
        self.state.read().map(|s| s.index.size()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, passage_id: u64) -> bool {
        self.state
            .read()
            .map(|s| s.index.contains(passage_id))
            .unwrap_or(false)
    }

    /// Retrieves a stored vector by `passage_id` (if present in the index).
    pub fn get_vector(&self, passage_id: u64) -> Option<Vec<f32>> {
        let guard = self.state.read().ok()?;
        if !guard.index.contains(passage_id) {
            return None;
        }
        let mut buf = vec![0.0_f32; self.dimensions];
        match guard.index.get(passage_id, &mut buf) {
            Ok(n) if n > 0 => Some(buf),
            _ => None,
        }
    }

    /// Upserts `(passage_id, embedding)` pairs, saves atomically, and restores OS-mmap view.
    pub fn upsert_batch(&self, items: &[(u64, Vec<f32>)]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        for (_, vec) in items {
            if vec.len() != self.dimensions {
                return Err(CoreError::Config(format!(
                    "vector dimension mismatch: expected {}, got {}",
                    self.dimensions,
                    vec.len()
                )));
            }
        }

        let mut guard = self
            .state
            .write()
            .map_err(|_| CoreError::Config("vector index lock poisoned".into()))?;
        self.ensure_writable_locked(&mut guard)?;

        let needed = guard.index.size() + items.len() + 64;
        if guard.index.capacity() < needed {
            guard
                .index
                .reserve(needed.max(guard.index.capacity() * 2))
                .map_err(|e| CoreError::Config(format!("usearch reserve failed: {e}")))?;
        }

        for (pid, vec) in items {
            if guard.index.contains(*pid) {
                let _ = guard.index.remove(*pid);
            }
            guard
                .index
                .add(*pid, vec)
                .map_err(|e| CoreError::Config(format!("usearch add key {pid} failed: {e}")))?;
        }

        self.save_and_remap_locked(&mut guard)
    }

    /// Removes passage keys from the index, persisting atomically if any were present.
    pub fn remove_keys(&self, passage_ids: &[u64]) -> Result<usize> {
        if passage_ids.is_empty() || !self.path.exists() {
            return Ok(0);
        }
        let mut guard = self
            .state
            .write()
            .map_err(|_| CoreError::Config("vector index lock poisoned".into()))?;
        let any_present = passage_ids.iter().any(|k| guard.index.contains(*k));
        if !any_present {
            return Ok(0);
        }
        self.ensure_writable_locked(&mut guard)?;

        let mut removed = 0usize;
        for &pid in passage_ids {
            if guard.index.contains(pid) && guard.index.remove(pid).is_ok() {
                removed += 1;
            }
        }
        if removed > 0 {
            self.save_and_remap_locked(&mut guard)?;
        }
        Ok(removed)
    }

    /// Searches nearest neighbors by cosine similarity.
    pub fn search(&self, query_vec: &[f32], limit: usize) -> Result<Vec<VectorHit>> {
        if limit == 0 || query_vec.len() != self.dimensions {
            return Ok(Vec::new());
        }
        let guard = self
            .state
            .read()
            .map_err(|_| CoreError::Config("vector index lock poisoned".into()))?;
        if guard.index.size() == 0 {
            return Ok(Vec::new());
        }
        let k = limit.min(guard.index.size());
        let matches = guard
            .index
            .search(query_vec, k)
            .map_err(|e| CoreError::Config(format!("usearch search failed: {e}")))?;
        Ok(matches
            .keys
            .into_iter()
            .zip(matches.distances)
            .map(|(passage_id, dist)| VectorHit {
                passage_id,
                score: (1.0 - dist).clamp(-1.0, 1.0),
            })
            .collect())
    }

    /// Searches nearest neighbors filtered by a caller predicate (e.g. active root_ids).
    pub fn filtered_search<F>(
        &self,
        query_vec: &[f32],
        limit: usize,
        predicate: F,
    ) -> Result<Vec<VectorHit>>
    where
        F: Fn(u64) -> bool,
    {
        if limit == 0 || query_vec.len() != self.dimensions {
            return Ok(Vec::new());
        }
        let guard = self
            .state
            .read()
            .map_err(|_| CoreError::Config("vector index lock poisoned".into()))?;
        if guard.index.size() == 0 {
            return Ok(Vec::new());
        }
        let k = limit.min(guard.index.size());
        let matches = guard
            .index
            .filtered_search(query_vec, k, predicate)
            .map_err(|e| CoreError::Config(format!("usearch filtered_search failed: {e}")))?;
        Ok(matches
            .keys
            .into_iter()
            .zip(matches.distances)
            .map(|(passage_id, dist)| VectorHit {
                passage_id,
                score: (1.0 - dist).clamp(-1.0, 1.0),
            })
            .collect())
    }

    fn ensure_writable_locked(&self, guard: &mut VectorState) -> Result<()> {
        if !guard.is_view {
            return Ok(());
        }
        let writable = Index::new(&index_options(self.dimensions))
            .map_err(|e| CoreError::Config(format!("failed allocating writable usearch: {e}")))?;
        if self.path.exists() {
            let path_str = path_to_str(&self.path)?;
            writable
                .load(path_str)
                .map_err(|e| CoreError::Config(format!("failed loading writable usearch: {e}")))?;
        }
        guard.index = writable;
        guard.is_view = false;
        Ok(())
    }

    fn save_and_remap_locked(&self, guard: &mut VectorState) -> Result<()> {
        let tmp_path = self.path.with_extension("usearch.tmp");
        let tmp_str = path_to_str(&tmp_path)?;
        guard
            .index
            .save(tmp_str)
            .map_err(|e| CoreError::Config(format!("failed saving usearch temp file: {e}")))?;
        fs::rename(&tmp_path, &self.path)?;

        let mapped = Index::new(&index_options(self.dimensions))
            .map_err(|e| CoreError::Config(format!("failed opening usearch mmap view: {e}")))?;
        let path_str = path_to_str(&self.path)?;
        mapped
            .view(path_str)
            .map_err(|e| CoreError::Config(format!("failed re-mmapping usearch index: {e}")))?;
        guard.index = mapped;
        guard.is_view = true;
        Ok(())
    }
}

fn index_options(dimensions: usize) -> IndexOptions {
    IndexOptions {
        dimensions,
        metric: MetricKind::Cos,
        quantization: ScalarKind::F16,
        connectivity: 16,
        expansion_add: 128,
        expansion_search: 64,
        multi: false,
    }
}

fn path_to_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| CoreError::InvalidPath(path.to_path_buf(), "non-UTF-8 path".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn upsert_search_filter_and_remove_persist_across_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("passages_test.usearch");

        {
            let idx = VectorIndex::open(&path, 4).unwrap();
            idx.upsert_batch(&[
                (10, vec![1.0, 0.0, 0.0, 0.0]),
                (20, vec![0.0, 1.0, 0.0, 0.0]),
                (30, vec![0.8, 0.6, 0.0, 0.0]),
            ])
            .unwrap();
            assert_eq!(idx.len(), 3);
        }

        let reopened = VectorIndex::open(&path, 4).unwrap();
        assert_eq!(reopened.len(), 3);

        let hits = reopened.search(&[1.0, 0.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(hits[0].passage_id, 10);
        assert!(hits[0].score > 0.99);
        assert_eq!(hits[1].passage_id, 30);

        let filtered = reopened
            .filtered_search(&[1.0, 0.0, 0.0, 0.0], 2, |k| k != 10)
            .unwrap();
        assert_eq!(filtered[0].passage_id, 30);

        assert_eq!(reopened.remove_keys(&[10]).unwrap(), 1);
        assert_eq!(reopened.len(), 2);
        assert!(!reopened.contains(10));
    }
}
