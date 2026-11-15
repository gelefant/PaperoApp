use crate::db::Database;
use crate::error::{CoreError, Result};
use rusqlite::{params, OptionalExtension};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct CachedHttpResponse {
    pub status_code: u16,
    pub body: String,
    pub fetched_at: i64,
}

/// Reads permanently cached response from `metadata_http_cache`.
pub fn load_http_cache(
    db: &Database,
    source: &str,
    lookup_key: &str,
) -> Result<Option<CachedHttpResponse>> {
    let conn = db.reader();
    let row = conn
        .query_row(
            "SELECT status_code, response_body, fetched_at
             FROM metadata_http_cache
             WHERE source = ?1 AND lookup_key = ?2",
            params![source, lookup_key],
            |r| {
                Ok(CachedHttpResponse {
                    status_code: r.get::<_, i64>(0)? as u16,
                    body: r.get(1)?,
                    fetched_at: r.get(2)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Persists a fetched HTTP response into `metadata_http_cache`.
pub fn store_http_cache(
    db: &Database,
    source: &str,
    lookup_key: &str,
    status_code: u16,
    body: &str,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let conn = db.writer();
    conn.execute(
        "INSERT INTO metadata_http_cache (source, lookup_key, response_body, status_code, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(source, lookup_key) DO UPDATE SET
            response_body = excluded.response_body,
            status_code = excluded.status_code,
            fetched_at = excluded.fetched_at",
        params![source, lookup_key, body, status_code as i64, now],
    )?;
    Ok(())
}

/// Strictly sequential, concurrency-1 rate limiter guaranteeing at least `min_interval`
/// between the start of consecutive network calls.
#[derive(Debug)]
pub struct SequentialRateLimiter {
    min_interval: Duration,
    last_call: Mutex<Option<Instant>>,
}

impl SequentialRateLimiter {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last_call: Mutex::new(None),
        }
    }

    /// Acquires permission sequentially (concurrency = 1) and sleeps if needed so consecutive
    /// dispatches are separated by at least `min_interval`.
    pub async fn acquire(&self) {
        let mut guard = self.last_call.lock().await;
        if let Some(prev) = *guard {
            let elapsed = prev.elapsed();
            if elapsed < self.min_interval {
                tokio::time::sleep(self.min_interval - elapsed).await;
            }
        }
        *guard = Some(Instant::now());
    }
}

/// Adaptive sequential rate limiter for Crossref (concurrency = 1).
/// Starts at conservative public/polite interval and adapts from HTTP response headers:
/// `x-rate-limit-limit`, `x-rate-limit-interval`, and `x-rate-limit-type`.
#[derive(Debug)]
pub struct CrossrefRateLimiter {
    inner: Mutex<CrossrefState>,
}

#[derive(Debug)]
struct CrossrefState {
    min_interval: Duration,
    last_call: Option<Instant>,
    last_limit_type: Option<String>,
}

impl CrossrefRateLimiter {
    pub fn new(has_polite_email: bool, is_list_query: bool) -> Self {
        let initial_ms = match (has_polite_email, is_list_query) {
            (true, false) => 100,   // polite single work: 10 req/s
            (true, true) => 340,    // polite list query: ~3 req/s
            (false, false) => 200,  // public single work: 5 req/s
            (false, true) => 1000,  // public list query: 1 req/s
        };
        Self {
            inner: Mutex::new(CrossrefState {
                min_interval: Duration::from_millis(initial_ms),
                last_call: None,
                last_limit_type: None,
            }),
        }
    }

    pub async fn acquire(&self) {
        let mut st = self.inner.lock().await;
        if let Some(prev) = st.last_call {
            let elapsed = prev.elapsed();
            if elapsed < st.min_interval {
                tokio::time::sleep(st.min_interval - elapsed).await;
            }
        }
        st.last_call = Some(Instant::now());
    }

    /// Parses Crossref `x-rate-limit-limit`, `x-rate-limit-interval`, `x-rate-limit-type` headers.
    pub async fn adapt_from_headers(
        &self,
        limit_hdr: Option<&str>,
        interval_hdr: Option<&str>,
        type_hdr: Option<&str>,
    ) {
        let limit = limit_hdr.and_then(|s| s.trim().parse::<u64>().ok()).filter(|&v| v > 0);
        let interval = interval_hdr.and_then(parse_interval_str);
        if let (Some(lim), Some(win)) = (limit, interval) {
            // Keep a small safety margin (+10%) above exact limit denominator
            let per_req_ms = ((win.as_millis() as f64 / lim as f64) * 1.1).ceil() as u64;
            let mut st = self.inner.lock().await;
            st.min_interval = Duration::from_millis(per_req_ms.max(80));
            st.last_limit_type = type_hdr.map(str::to_string);
        }
    }

    pub async fn current_interval(&self) -> Duration {
        self.inner.lock().await.min_interval
    }

    pub async fn last_limit_type(&self) -> Option<String> {
        self.inner.lock().await.last_limit_type.clone()
    }
}

fn parse_interval_str(s: &str) -> Option<Duration> {
    let t = s.trim();
    if let Some(ms) = t.strip_suffix("ms") {
        return ms.trim().parse::<u64>().ok().map(Duration::from_millis);
    }
    if let Some(sec) = t.strip_suffix('s') {
        return sec.trim().parse::<u64>().ok().map(Duration::from_secs);
    }
    if let Some(min) = t.strip_suffix('m') {
        return min.trim().parse::<u64>().ok().map(|v| Duration::from_secs(v * 60));
    }
    t.parse::<u64>().ok().map(Duration::from_secs)
}

/// Strict Open Library ToS limiter:
/// - Concurrency strictly `1` (mutex-guarded sequential dispatch)
/// - At most `1 request / second` (`min_interval = 1000 ms`)
/// - Permanent SQLite cache checked BEFORE network dispatch
/// - Exponential backoff on HTTP 429 (`Retry-After` or `2^k` seconds)
/// - Circuit breaker suspends source after 3 consecutive 429 / 5xx errors.
#[derive(Debug)]
pub struct OpenLibraryLimiter {
    base_interval: Duration,
    backoff_until: Mutex<(Option<Instant>, Duration)>,
    consecutive_errors: AtomicU32,
    suspended: AtomicBool,
    suspended_until: Mutex<Option<Instant>>,
    cooldown: Duration,
}

impl OpenLibraryLimiter {
    pub fn new(base_interval: Duration, cooldown: Duration) -> Self {
        Self {
            base_interval,
            backoff_until: Mutex::new((None, base_interval)),
            consecutive_errors: AtomicU32::new(0),
            suspended: AtomicBool::new(false),
            suspended_until: Mutex::new(None),
            cooldown,
        }
    }

    pub fn default_tos() -> Self {
        Self::new(Duration::from_secs(1), Duration::from_secs(600))
    }

    pub async fn check_circuit_open(&self) -> Result<()> {
        if self.suspended.load(Ordering::SeqCst) {
            let guard = self.suspended_until.lock().await;
            if let Some(until) = *guard {
                if Instant::now() < until {
                    return Err(CoreError::Config(
                        "Open Library source temporarily suspended due to repeated HTTP 429/5xx responses"
                            .to_string(),
                    ));
                }
            }
            drop(guard);
            self.suspended.store(false, Ordering::SeqCst);
            self.consecutive_errors.store(0, Ordering::SeqCst);
        }
        Ok(())
    }

    /// Acquires sequential slot respecting `>= 1 req/s` and any active 429 exponential backoff.
    pub async fn acquire(&self) -> Result<()> {
        self.check_circuit_open().await?;
        let mut st = self.backoff_until.lock().await;
        if let Some(last) = st.0 {
            let wait_dur = st.1.max(self.base_interval);
            let elapsed = last.elapsed();
            if elapsed < wait_dur {
                tokio::time::sleep(wait_dur - elapsed).await;
            }
        }
        st.0 = Some(Instant::now());
        Ok(())
    }

    pub async fn record_success(&self) {
        self.consecutive_errors.store(0, Ordering::SeqCst);
        self.suspended.store(false, Ordering::SeqCst);
        let mut st = self.backoff_until.lock().await;
        st.1 = self.base_interval;
    }

    pub async fn record_transient_failure(&self, status_code: u16) -> Duration {
        let count = self.consecutive_errors.fetch_add(1, Ordering::SeqCst) + 1;
        let next_backoff = {
            let mut st = self.backoff_until.lock().await;
            let mult = 1u32 << count.min(5);
            let grown = self
                .base_interval
                .max(Duration::from_millis(100))
                .saturating_mul(mult);
            st.1 = grown;
            st.0 = Some(Instant::now());
            grown
        };

        if (status_code == 429 || status_code >= 500) && count >= 3 {
            self.suspended.store(true, Ordering::SeqCst);
            *self.suspended_until.lock().await = Some(Instant::now() + self.cooldown);
        }
        next_backoff
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended.load(Ordering::SeqCst)
    }

    pub fn consecutive_error_count(&self) -> u32 {
        self.consecutive_errors.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn crossref_adapts_from_polite_headers() {
        let lim = CrossrefRateLimiter::new(true, false);
        assert_eq!(lim.current_interval().await, Duration::from_millis(100));
        lim.adapt_from_headers(Some("5"), Some("1s"), Some("public"))
            .await;
        let adapted = lim.current_interval().await;
        assert!(adapted >= Duration::from_millis(220) && adapted <= Duration::from_millis(225));
        assert_eq!(lim.last_limit_type().await.as_deref(), Some("public"));
    }

    #[tokio::test]
    async fn openlibrary_circuit_breaker_and_backoff() {
        let lim = OpenLibraryLimiter::new(Duration::from_millis(10), Duration::from_secs(60));
        let b1 = lim.record_transient_failure(429).await;
        let b2 = lim.record_transient_failure(429).await;
        assert!(b2 > b1);
        assert!(!lim.is_suspended());
        let _b3 = lim.record_transient_failure(429).await;
        assert!(lim.is_suspended());
        assert!(lim.acquire().await.is_err());
    }
}
