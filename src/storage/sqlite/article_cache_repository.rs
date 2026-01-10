use crate::errors::FeederResult;
use crate::storage::traits::ArticleCacheRepository;
use crate::storage::sqlite::SqliteStorage;

pub struct SqliteArticleCacheRepository {
    storage: SqliteStorage,
}

impl SqliteArticleCacheRepository {
    pub fn new(storage: SqliteStorage) -> Self {
        Self { storage }
    }
}

impl ArticleCacheRepository for SqliteArticleCacheRepository {
    fn is_notified(&self, cache_key: &str) -> FeederResult<bool> {
        let conn = self.storage.connection()?;
        let mut stmt = conn.prepare(
            "SELECT EXISTS(SELECT 1 FROM notified_articles WHERE cache_key = ?1)"
        )?;
        let exists: bool = stmt.query_row([cache_key], |row| row.get(0))?;
        Ok(exists)
    }

    fn mark_notified(&self, cache_key: &str, feed_id: i64, title: &str) -> FeederResult<()> {
        let conn = self.storage.connection()?;
        conn.execute(
            "INSERT OR IGNORE INTO notified_articles (cache_key, feed_id, article_title) VALUES (?1, ?2, ?3)",
            (cache_key, feed_id, title),
        )?;
        Ok(())
    }

    fn get_unnotified(&self, cache_keys: &[String]) -> FeederResult<Vec<String>> {
        if cache_keys.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.storage.connection()?;

        // Build placeholders for IN clause
        let placeholders: Vec<String> = (0..cache_keys.len()).map(|i| format!("?{}", i + 1)).collect();
        let query = format!(
            "SELECT cache_key FROM notified_articles WHERE cache_key IN ({})",
            placeholders.join(", ")
        );

        let mut stmt = conn.prepare(&query)?;

        // Get notified keys
        let notified: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(cache_keys.iter()), |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        // Return keys that are not in notified
        Ok(cache_keys
            .iter()
            .filter(|k| !notified.contains(k))
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::SqliteFeedRepository;
    use crate::storage::traits::FeedRepository;
    use crate::domain::{Feed, FeedType, SourceType};

    fn setup() -> (SqliteStorage, SqliteFeedRepository, SqliteArticleCacheRepository) {
        let storage = SqliteStorage::in_memory().unwrap();
        let feed_repo = SqliteFeedRepository::new(storage.clone());
        let cache_repo = SqliteArticleCacheRepository::new(storage.clone());
        (storage, feed_repo, cache_repo)
    }

    #[test]
    fn test_mark_and_check_notified() {
        let (_, feed_repo, cache_repo) = setup();

        let feed = Feed::new(
            "https://example.com/feed".to_string(),
            "https://example.com/feed".to_string(),
            "Example Feed".to_string(),
            FeedType::Rss,
            SourceType::RssAtom,
        );
        let feed_id = feed_repo.add(&feed).unwrap();

        let cache_key = "Example Feed:article-123";

        assert!(!cache_repo.is_notified(cache_key).unwrap());
        cache_repo.mark_notified(cache_key, feed_id, "Test Article").unwrap();
        assert!(cache_repo.is_notified(cache_key).unwrap());
    }

    #[test]
    fn test_get_unnotified() {
        let (_, feed_repo, cache_repo) = setup();

        let feed = Feed::new(
            "https://example.com/feed".to_string(),
            "https://example.com/feed".to_string(),
            "Example Feed".to_string(),
            FeedType::Rss,
            SourceType::RssAtom,
        );
        let feed_id = feed_repo.add(&feed).unwrap();

        // Mark one as notified
        cache_repo.mark_notified("key1", feed_id, "Article 1").unwrap();

        let keys = vec!["key1".to_string(), "key2".to_string(), "key3".to_string()];
        let unnotified = cache_repo.get_unnotified(&keys).unwrap();

        assert_eq!(unnotified.len(), 2);
        assert!(unnotified.contains(&"key2".to_string()));
        assert!(unnotified.contains(&"key3".to_string()));
        assert!(!unnotified.contains(&"key1".to_string()));
    }

    #[test]
    fn test_get_unnotified_empty() {
        let (_, _, cache_repo) = setup();
        let keys: Vec<String> = vec![];
        let unnotified = cache_repo.get_unnotified(&keys).unwrap();
        assert!(unnotified.is_empty());
    }
}
