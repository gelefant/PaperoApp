use crate::config::resolve_external_tool;
use crate::error::{CoreError, Result};
use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::SystemTime;

/// Disk-backed LRU cache for rendered DjVu page images (`ddjvu` -> PNG).
pub struct DjvuCache {
    cache_dir: PathBuf,
    max_bytes: u64,
    evict_lock: Mutex<()>,
}

impl DjvuCache {
    pub fn new(cache_root: &Path, max_mb: u64) -> Result<Self> {
        let cache_dir = cache_root.join("djvu_pages");
        fs::create_dir_all(&cache_dir)?;
        Ok(Self {
            cache_dir,
            max_bytes: max_mb.saturating_mul(1024 * 1024),
            evict_lock: Mutex::new(()),
        })
    }

    pub fn cache_path(&self, blake3_hash: &str, page_ord: u32, dpi: u32) -> PathBuf {
        let safe_dpi = dpi.clamp(40, 300);
        self.cache_dir
            .join(format!("{blake3_hash}_p{page_ord}_d{safe_dpi}.png"))
    }

    /// Returns PNG bytes for the requested DjVu page, reading from disk cache on hit
    /// or spawning `ddjvu -format=ppm -page=<N> -scale=<dpi>` on miss.
    pub fn render_page_png(
        &self,
        djvu_file: &Path,
        blake3_hash: &str,
        page_ord: u32,
        dpi: u32,
        ddjvu_bin: Option<&str>,
    ) -> Result<Vec<u8>> {
        let safe_dpi = dpi.clamp(40, 300);
        let target = self.cache_path(blake3_hash, page_ord, safe_dpi);

        if target.exists() {
            if let Ok(bytes) = fs::read(&target) {
                // Touch mtime for LRU ordering (best-effort)
                let _ = fs::OpenOptions::new()
                    .write(true)
                    .open(&target)
                    .and_then(|f| f.set_modified(SystemTime::now()));
                return Ok(bytes);
            }
        }

        let bin = resolve_external_tool(ddjvu_bin, "ddjvu");
        let output = Command::new(&bin)
            .arg("-format=ppm")
            .arg(format!("-page={page_ord}"))
            .arg(format!("-scale={safe_dpi}"))
            .arg(djvu_file)
            .arg("-")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    CoreError::MissingExternalTool {
                        tool: bin.clone(),
                        detail: format!("could not spawn '{bin}': {e}"),
                    }
                } else {
                    CoreError::Io(e)
                }
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(CoreError::CorruptFile(format!(
                "ddjvu failed ({}): {stderr}",
                output.status
            )));
        }

        let png_bytes = ppm_to_png(&output.stdout)?;

        // Atomic write via temp file in cache dir, then LRU eviction
        let tmp_path = self
            .cache_dir
            .join(format!(".tmp_{}_{}_{}.png", blake3_hash, page_ord, std::process::id()));
        if fs::write(&tmp_path, &png_bytes).is_ok() {
            let _ = fs::rename(&tmp_path, &target);
            self.evict_lru_if_needed();
        }

        Ok(png_bytes)
    }

    fn evict_lru_if_needed(&self) {
        if self.max_bytes == 0 {
            return;
        }
        let _guard = self.evict_lock.lock().expect("djvu_cache mutex poisoned");
        let Ok(rd) = fs::read_dir(&self.cache_dir) else {
            return;
        };

        let mut entries: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
        let mut total: u64 = 0;

        for ent in rd.flatten() {
            let path = ent.path();
            if path.extension().and_then(|e| e.to_str()) != Some("png") {
                continue;
            }
            if let Ok(meta) = ent.metadata() {
                let len = meta.len();
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                total = total.saturating_add(len);
                entries.push((path, len, mtime));
            }
        }

        if total <= self.max_bytes {
            return;
        }

        // Oldest modified first
        entries.sort_by_key(|(_, _, mtime)| *mtime);
        for (path, len, _) in entries {
            if total <= self.max_bytes {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
            }
        }
    }
}

/// Converts a binary P6 PPM buffer to compressed PNG bytes.
pub fn ppm_to_png(ppm: &[u8]) -> Result<Vec<u8>> {
    let (w, h, rgb) = parse_p6_ppm(ppm)?;
    let mut out = Vec::with_capacity((w * h) as usize);
    {
        let mut encoder = png::Encoder::new(BufWriter::new(&mut out), w, h);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder
            .write_header()
            .map_err(|e| CoreError::Extract(format!("PNG header error: {e}")))?;
        writer
            .write_image_data(rgb)
            .map_err(|e| CoreError::Extract(format!("PNG encode error: {e}")))?;
    }
    Ok(out)
}

fn parse_p6_ppm(data: &[u8]) -> Result<(u32, u32, &[u8])> {
    let mut pos = 0usize;
    let mut tokens: Vec<String> = Vec::with_capacity(4);

    while tokens.len() < 4 && pos < data.len() {
        // Skip whitespace
        while pos < data.len() && data[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= data.len() {
            break;
        }
        // Skip comments
        if data[pos] == b'#' {
            while pos < data.len() && data[pos] != b'\n' {
                pos += 1;
            }
            continue;
        }
        let start = pos;
        while pos < data.len() && !data[pos].is_ascii_whitespace() {
            pos += 1;
        }
        tokens.push(String::from_utf8_lossy(&data[start..pos]).into_owned());
    }

    if tokens.len() < 4 || tokens[0] != "P6" {
        return Err(CoreError::CorruptFile("invalid PPM P6 output from ddjvu".into()));
    }
    let width: u32 = tokens[1]
        .parse()
        .map_err(|_| CoreError::CorruptFile("invalid PPM width".into()))?;
    let height: u32 = tokens[2]
        .parse()
        .map_err(|_| CoreError::CorruptFile("invalid PPM height".into()))?;
    let maxval: u32 = tokens[3]
        .parse()
        .map_err(|_| CoreError::CorruptFile("invalid PPM maxval".into()))?;
    if maxval != 255 {
        return Err(CoreError::CorruptFile(format!(
            "unsupported PPM maxval {maxval}"
        )));
    }

    // Exactly one whitespace byte follows maxval before raw RGB pixels.
    if pos >= data.len() || !data[pos].is_ascii_whitespace() {
        return Err(CoreError::CorruptFile("truncated PPM header".into()));
    }
    pos += 1;

    let expected_bytes = (width as usize) * (height as usize) * 3;
    if data.len() < pos + expected_bytes {
        return Err(CoreError::CorruptFile("truncated PPM pixel payload".into()));
    }

    Ok((width, height, &data[pos..pos + expected_bytes]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_p6_ppm_to_valid_png() {
        let mut ppm = b"P6\n# comment\n2 2\n255\n".to_vec();
        ppm.extend_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
        let png = ppm_to_png(&ppm).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn lru_eviction_respects_max_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        // Set max_mb = 0? No, test eviction helper directly with tiny limit
        let cache = DjvuCache {
            cache_dir: tmp.path().to_path_buf(),
            max_bytes: 20,
            evict_lock: Mutex::new(()),
        };
        let old_file = tmp.path().join("old.png");
        let new_file = tmp.path().join("new.png");
        fs::write(&old_file, vec![1u8; 15]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(25));
        fs::write(&new_file, vec![2u8; 15]).unwrap();

        cache.evict_lru_if_needed();
        assert!(!old_file.exists(), "oldest cached page should be evicted");
        assert!(new_file.exists(), "newest cached page should remain");
    }
}
