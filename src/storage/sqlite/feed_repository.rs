use crate::domain::{Feed, FeedType, SourceType};
use crate::errors::{FeederError, FeederResult};
use crate::storage::traits::FeedRepository;
use crate::storage::sqlite::SqliteStorage;

pub struct SqliteFeedRepository {
    storage: SqliteStorage,
}

impl SqliteFeedRepository {
    pub fn new(storage: SqliteStorage) -> Self {
        Self { storage }
    }
}

impl FeedRepository for SqliteFeedRepository {
    fn add(&self, feed: &Feed) -> FeederResult<i64> {
        let conn = self.storage.connection()?;

        // Check if already exists (within the same connection to avoid deadlock)
        let mut stmt = conn.prepare("SELECT EXISTS(SELECT 1 FROM feeds WHERE url = ?1)")?;
        let exists: bool = stmt.query_row([&feed.url], |row| row.get(0))?;
        drop(stmt);

        if exists {
            return Err(FeederError::FeedAlreadyExists(feed.url.clone()));
        }

        conn.execute(
            "INSERT INTO feeds (url, feed_url, title, feed_type, source_type) VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                &feed.url,
                &feed.feed_url,
                &feed.title,
                feed.feed_type.as_str(),
                feed.source_type.as_str(),
            ),
        )?;

        Ok(conn.last_insert_rowid())
    }

    fn remove(&self, id: i64) -> FeederResult<()> {
        let conn = self.storage.connection()?;
        conn.execute("DELETE FROM feeds WHERE id = ?1", [id])?;
        Ok(())
    }

    fn get_all(&self) -> FeederResult<Vec<Feed>> {
        let conn = self.storage.connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, url, feed_url, title, feed_type, source_type, created_at FROM feeds ORDER BY created_at DESC"
        )?;

        let feeds = stmt.query_map([], |row| {
            let feed_type_str: String = row.get(4)?;
            let source_type_str: String = row.get(5)?;

            Ok(Feed {
                id: Some(row.get(0)?),
                url: row.get(1)?,
                feed_url: row.get(2)?,
                title: row.get(3)?,
                feed_type: feed_type_str.parse().unwrap_or(FeedType::Rss),
                source_type: source_type_str.parse().unwrap_or(SourceType::RssAtom),
                created_at: row.get(6)?,
            })
        })?;

        feeds.collect::<Result<Vec<_>, _>>().map_err(FeederError::from)
    }

    fn get_by_id(&self, id: i64) -> FeederResult<Option<Feed>> {
        let conn = self.storage.connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, url, feed_url, title, feed_type, source_type, created_at FROM feeds WHERE id = ?1"
        )?;

        let feed = stmt.query_row([id], |row| {
            let feed_type_str: String = row.get(4)?;
            let source_type_str: String = row.get(5)?;

            Ok(Feed {
                id: Some(row.get(0)?),
                url: row.get(1)?,
                feed_url: row.get(2)?,
                title: row.get(3)?,
                feed_type: feed_type_str.parse().unwrap_or(FeedType::Rss),
                source_type: source_type_str.parse().unwrap_or(SourceType::RssAtom),
                created_at: row.get(6)?,
            })
        });

        match feed {
            Ok(f) => Ok(Some(f)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(FeederError::from(e)),
        }
    }

    fn get_by_url(&self, url: &str) -> FeederResult<Option<Feed>> {
        let conn = self.storage.connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, url, feed_url, title, feed_type, source_type, created_at FROM feeds WHERE url = ?1"
        )?;

        let feed = stmt.query_row([url], |row| {
            let feed_type_str: String = row.get(4)?;
            let source_type_str: String = row.get(5)?;

            Ok(Feed {
                id: Some(row.get(0)?),
                url: row.get(1)?,
                feed_url: row.get(2)?,
                title: row.get(3)?,
                feed_type: feed_type_str.parse().unwrap_or(FeedType::Rss),
                source_type: source_type_str.parse().unwrap_or(SourceType::RssAtom),
                created_at: row.get(6)?,
            })
        });

        match feed {
            Ok(f) => Ok(Some(f)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(FeederError::from(e)),
        }
    }

    fn exists(&self, url: &str) -> FeederResult<bool> {
        let conn = self.storage.connection()?;
        let mut stmt = conn.prepare("SELECT EXISTS(SELECT 1 FROM feeds WHERE url = ?1)")?;
        let exists: bool = stmt.query_row([url], |row| row.get(0))?;
        Ok(exists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_repo() -> SqliteFeedRepository {
        let storage = SqliteStorage::in_memory().unwrap();
        SqliteFeedRepository::new(storage)
    }

    #[test]
    fn test_add_and_get_feed() {
        let repo = setup_repo();
        let feed = Feed::new(
            "https://example.com/feed".to_string(),
            "https://example.com/feed".to_string(),
            "Example Feed".to_string(),
            FeedType::Rss,
            SourceType::RssAtom,
        );

        let id = repo.add(&feed).unwrap();
        assert!(id > 0);

        let retrieved = repo.get_by_id(id).unwrap().unwrap();
        assert_eq!(retrieved.title, "Example Feed");
        assert_eq!(retrieved.url, "https://example.com/feed");
    }

    #[test]
    fn test_duplicate_url_rejected() {
        let repo = setup_repo();
        let feed = Feed::new(
            "https://example.com/feed".to_string(),
            "https://example.com/feed".to_string(),
            "Example Feed".to_string(),
            FeedType::Rss,
            SourceType::RssAtom,
        );

        repo.add(&feed).unwrap();
        let result = repo.add(&feed);

        assert!(matches!(result, Err(FeederError::FeedAlreadyExists(_))));
    }

    #[test]
    fn test_remove_feed() {
        let repo = setup_repo();
        let feed = Feed::new(
            "https://example.com/feed".to_string(),
            "https://example.com/feed".to_string(),
            "Example Feed".to_string(),
            FeedType::Rss,
            SourceType::RssAtom,
        );

        let id = repo.add(&feed).unwrap();
        repo.remove(id).unwrap();

        let retrieved = repo.get_by_id(id).unwrap();
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_get_all_feeds() {
        let repo = setup_repo();

        let feed1 = Feed::new(
            "https://example1.com/feed".to_string(),
            "https://example1.com/feed".to_string(),
            "Feed 1".to_string(),
            FeedType::Rss,
            SourceType::RssAtom,
        );
        let feed2 = Feed::new(
            "https://example2.com/feed".to_string(),
            "https://example2.com/feed".to_string(),
            "Feed 2".to_string(),
            FeedType::Atom,
            SourceType::YouTube,
        );

        repo.add(&feed1).unwrap();
        repo.add(&feed2).unwrap();

        let all = repo.get_all().unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_exists() {
        let repo = setup_repo();
        let feed = Feed::new(
            "https://example.com/feed".to_string(),
            "https://example.com/feed".to_string(),
            "Example Feed".to_string(),
            FeedType::Rss,
            SourceType::RssAtom,
        );

        assert!(!repo.exists("https://example.com/feed").unwrap());
        repo.add(&feed).unwrap();
        assert!(repo.exists("https://example.com/feed").unwrap());
    }
}
