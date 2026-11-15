pub mod migrations;
pub mod repos;

use crate::error::Result;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

/// Thread-safe SQLite database handle using WAL mode:
/// - 1 serialized writer `Mutex<Connection>`
/// - A round-robin pool of `SQLITE_OPEN_READ_ONLY` connections so UI reads
///   never block behind background indexing writes.
pub struct Database {
    writer: Mutex<Connection>,
    readers: Vec<Mutex<Connection>>,
    next_reader: AtomicUsize,
}

impl Database {
    /// Opens or creates the SQLite database at `db_path` with `read_pool_size` read-only connections.
    pub fn open(db_path: &Path, read_pool_size: usize) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let writer = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        migrations::migrate(&writer)?;

        let pool_size = read_pool_size.max(1);
        let mut readers = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            let reader = Connection::open_with_flags(
                db_path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            migrations::apply_pragmas(&reader, true)?;
            readers.push(Mutex::new(reader));
        }

        Ok(Self {
            writer: Mutex::new(writer),
            readers,
            next_reader: AtomicUsize::new(0),
        })
    }

    /// Opens an isolated in-memory database (useful for unit tests).
    /// Uses a single shared-cache or single-connection reader fallback.
    pub fn in_memory() -> Result<Self> {
        let writer = Connection::open_in_memory()?;
        migrations::migrate(&writer)?;
        Ok(Self {
            writer: Mutex::new(writer),
            readers: Vec::new(),
            next_reader: AtomicUsize::new(0),
        })
    }

    /// Acquires the exclusive writer connection.
    pub fn writer(&self) -> MutexGuard<'_, Connection> {
        self.writer.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Acquires a read-only connection from the reader pool (falls back to writer for `:memory:`).
    pub fn reader(&self) -> MutexGuard<'_, Connection> {
        if self.readers.is_empty() {
            return self.writer();
        }
        let idx = self.next_reader.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        self.readers[idx]
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
}
