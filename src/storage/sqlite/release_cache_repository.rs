use crate::errors::FeederResult;
use crate::storage::sqlite::SqliteStorage;
use crate::storage::traits::ReleaseCacheRepository;

pub struct SqliteReleaseCacheRepository {
    storage: SqliteStorage,
}

impl SqliteReleaseCacheRepository {
    pub fn new(storage: SqliteStorage) -> Self {
        Self { storage }
    }
}

impl ReleaseCacheRepository for SqliteReleaseCacheRepository {
    fn is_notified(&self, cache_key: &str) -> FeederResult<bool> {
        let conn = self.storage.connection()?;
        let mut stmt =
            conn.prepare("SELECT EXISTS(SELECT 1 FROM notified_releases WHERE cache_key = ?1)")?;
        let exists: bool = stmt.query_row([cache_key], |row| row.get(0))?;
        Ok(exists)
    }

    fn mark_notified(&self, cache_key: &str, repo_id: i64, title: &str) -> FeederResult<()> {
        let conn = self.storage.connection()?;
        conn.execute(
            "INSERT OR IGNORE INTO notified_releases (cache_key, repo_id, title) VALUES (?1, ?2, ?3)",
            (cache_key, repo_id, title),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TrackedRepo;
    use crate::storage::sqlite::{SqliteRepoRepository, SqliteStorage};
    use crate::storage::traits::RepoRepository;

    #[test]
    fn test_mark_and_check() {
        let storage = SqliteStorage::in_memory().unwrap();
        let repo_repo = SqliteRepoRepository::new(storage.clone());
        let cache = SqliteReleaseCacheRepository::new(storage);

        let repo_id = repo_repo
            .add(&TrackedRepo::new(
                "a".into(),
                "b".into(),
                "https://github.com/a/b".into(),
            ))
            .unwrap();

        let key = "a/b:release:v1.0";
        assert!(!cache.is_notified(key).unwrap());
        cache.mark_notified(key, repo_id, "v1.0").unwrap();
        assert!(cache.is_notified(key).unwrap());

        // idempotent
        cache.mark_notified(key, repo_id, "v1.0").unwrap();
        assert!(cache.is_notified(key).unwrap());
    }
}
