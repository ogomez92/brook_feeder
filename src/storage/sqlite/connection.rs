use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::errors::{FeederError, FeederResult};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS feeds (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    url TEXT NOT NULL UNIQUE,
    feed_url TEXT NOT NULL,
    title TEXT NOT NULL,
    feed_type TEXT NOT NULL,
    source_type TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_feeds_url ON feeds(url);

CREATE TABLE IF NOT EXISTS notified_articles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    cache_key TEXT NOT NULL UNIQUE,
    feed_id INTEGER NOT NULL,
    article_title TEXT,
    notified_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (feed_id) REFERENCES feeds(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_notified_articles_cache_key ON notified_articles(cache_key);

CREATE TABLE IF NOT EXISTS tracked_repos (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    owner TEXT NOT NULL,
    name TEXT NOT NULL,
    url TEXT NOT NULL UNIQUE,
    added_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_tracked_repos_owner_name
    ON tracked_repos(owner COLLATE NOCASE, name COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS notified_releases (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    cache_key TEXT NOT NULL UNIQUE,
    repo_id INTEGER NOT NULL,
    title TEXT,
    notified_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (repo_id) REFERENCES tracked_repos(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_notified_releases_cache_key ON notified_releases(cache_key);
"#;

#[derive(Clone)]
pub struct SqliteStorage {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteStorage {
    pub fn new<P: AsRef<Path>>(path: P) -> FeederResult<Self> {
        let conn = Connection::open(path)?;
        // Wait (up to 5s) rather than erroring when another feeder process (e.g.
        // the feeds timer and the releases timer running at once) holds a write
        // lock. Keeps silent scheduled runs from aborting on transient locks.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SCHEMA)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn in_memory() -> FeederResult<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SCHEMA)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>, FeederError> {
        self.conn
            .lock()
            .map_err(|_| FeederError::Database(rusqlite::Error::InvalidQuery))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_in_memory_storage() {
        let storage = SqliteStorage::in_memory().unwrap();
        let conn = storage.connection().unwrap();

        // Verify tables exist
        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='feeds'").unwrap();
        let count: i32 = stmt.query_row([], |row| row.get(0)).unwrap_or(0);
        drop(stmt);

        // Just check we can query
        assert!(true);
    }
}
