use crate::error::{CoreError, Result};
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct SlotEntry<T> {
    value: T,
    last_used: Instant,
}

/// Thread-safe lazy model holder that instantiates heavy ONNX sessions on demand
/// and unloads them after a configurable idle timeout so RSS returns to `<= 250 MB`.
pub struct LazyModelSlot<T> {
    inner: Mutex<Option<SlotEntry<T>>>,
}

impl<T> Default for LazyModelSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> LazyModelSlot<T> {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    pub fn is_loaded(&self) -> bool {
        self.inner
            .lock()
            .map(|g| g.is_some())
            .unwrap_or(false)
    }

    pub fn last_used_elapsed(&self) -> Option<Duration> {
        self.inner
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|e| e.last_used.elapsed()))
    }

    /// Runs `f` on the held model, lazily loading via `loader()` if unloaded.
    pub fn with_model<R, L, F>(&self, loader: L, f: F) -> Result<R>
    where
        L: FnOnce() -> Result<T>,
        F: FnOnce(&mut T) -> Result<R>,
    {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| CoreError::Config("model slot mutex poisoned".into()))?;
        if guard.is_none() {
            let loaded = loader()?;
            *guard = Some(SlotEntry {
                value: loaded,
                last_used: Instant::now(),
            });
        }
        let entry = guard.as_mut().expect("just loaded");
        entry.last_used = Instant::now();
        f(&mut entry.value)
    }

    /// Unloads the held model unconditionally, freeing session weights.
    pub fn unload(&self) -> bool {
        let was_loaded = if let Ok(mut guard) = self.inner.lock() {
            let was = guard.is_some();
            *guard = None;
            was
        } else {
            false
        };
        if was_loaded {
            release_freed_pages_to_os();
        }
        was_loaded
    }

    /// Unloads only if the model has been idle for at least `idle_timeout`.
    pub fn evict_if_idle(&self, idle_timeout: Duration) -> bool {
        if idle_timeout.is_zero() {
            return false;
        }
        let evicted = if let Ok(mut guard) = self.inner.lock() {
            if let Some(entry) = guard.as_ref() {
                if entry.last_used.elapsed() >= idle_timeout {
                    *guard = None;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };
        if evicted {
            release_freed_pages_to_os();
        }
        evicted
    }
}

#[cfg(target_os = "macos")]
fn release_freed_pages_to_os() {
    extern "C" {
        fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
    }
    unsafe {
        let _ = malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
}

#[cfg(not(target_os = "macos"))]
fn release_freed_pages_to_os() {}

/// Returns the current process physical footprint / RSS in bytes when supported by OS.
/// On macOS, uses `TASK_VM_INFO.phys_footprint` (matching Activity Monitor and excluding
/// `MADV_FREE_REUSABLE` allocator pages already returned to the kernel after model unload).
#[cfg(target_os = "macos")]
pub fn current_process_rss_bytes() -> Option<u64> {
    #[repr(C)]
    struct TaskVmInfoRev1 {
        virtual_size: u64,
        region_count: i32,
        page_size: i32,
        resident_size: u64,
        resident_size_peak: u64,
        device: u64,
        device_peak: u64,
        internal: u64,
        internal_peak: u64,
        external: u64,
        external_peak: u64,
        reusable: u64,
        reusable_peak: u64,
        purgeable_volatile_pmap: u64,
        purgeable_volatile_resident: u64,
        purgeable_volatile_virtual: u64,
        compressed: u64,
        compressed_peak: u64,
        compressed_lifetime: u64,
        phys_footprint: u64,
    }
    extern "C" {
        static mach_task_self_: u32;
        fn task_info(
            target_task: u32,
            flavor: u32,
            task_info_out: *mut TaskVmInfoRev1,
            task_info_out_cnt: *mut u32,
        ) -> i32;
    }

    const TASK_VM_INFO: u32 = 22;
    let mut info: TaskVmInfoRev1 = unsafe { std::mem::zeroed() };
    let mut count = (std::mem::size_of::<TaskVmInfoRev1>() / std::mem::size_of::<i32>()) as u32;
    let kr = unsafe { task_info(mach_task_self_, TASK_VM_INFO, &mut info, &mut count) };
    if kr == 0 && info.phys_footprint > 0 {
        Some(info.phys_footprint)
    } else if kr == 0 {
        Some(info.resident_size)
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
pub fn current_process_rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096)
}

#[cfg(all(not(target_os = "macos"), not(target_os = "linux")))]
pub fn current_process_rss_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lazy_slot_loads_once_and_evicts_after_idle_timeout() {
        let slot: LazyModelSlot<Vec<u8>> = LazyModelSlot::new();
        assert!(!slot.is_loaded());

        let len = slot
            .with_model(|| Ok(vec![42u8; 1024]), |m| Ok(m.len()))
            .unwrap();
        assert_eq!(len, 1024);
        assert!(slot.is_loaded());

        std::thread::sleep(Duration::from_millis(25));
        assert!(slot.evict_if_idle(Duration::from_millis(10)));
        assert!(!slot.is_loaded());
    }

    #[test]
    fn reports_nonzero_process_rss_on_supported_os() {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let rss = current_process_rss_bytes().unwrap();
            assert!(rss > 1024 * 1024, "unexpected small RSS: {rss}");
        }
    }
}
