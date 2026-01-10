use crate::domain::Feed;
use crate::errors::{FeederError, FeederResult};
use crate::sources::SourceRegistry;
use crate::storage::traits::FeedRepository;

pub struct FeedService<R: FeedRepository> {
    repository: R,
    source_registry: SourceRegistry,
}

impl<R: FeedRepository> FeedService<R> {
    pub fn new(repository: R, source_registry: SourceRegistry) -> Self {
        Self {
            repository,
            source_registry,
        }
    }

    /// Add a new feed by URL
    /// Validates the feed and stores it in the database
    pub fn add(&self, url: &str) -> FeederResult<Feed> {
        // Check if already exists
        if self.repository.exists(url)? {
            return Err(FeederError::FeedAlreadyExists(url.to_string()));
        }

        // Validate and get metadata
        let metadata = self.source_registry.validate(url)?;

        // Create feed entity
        let feed = Feed::new(
            url.to_string(),
            metadata.feed_url,
            metadata.title,
            metadata.feed_type,
            metadata.source_type,
        );

        // Store in database
        let id = self.repository.add(&feed)?;

        Ok(Feed {
            id: Some(id),
            ..feed
        })
    }

    /// Remove a feed by ID
    pub fn remove(&self, id: i64) -> FeederResult<()> {
        self.repository.remove(id)
    }

    /// List all feeds
    pub fn list(&self) -> FeederResult<Vec<Feed>> {
        self.repository.get_all()
    }

    /// Get a feed by ID
    pub fn get(&self, id: i64) -> FeederResult<Option<Feed>> {
        self.repository.get_by_id(id)
    }

    /// Check if a feed URL already exists
    pub fn exists(&self, url: &str) -> FeederResult<bool> {
        self.repository.exists(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{FeedType, SourceType};
    use crate::storage::sqlite::{SqliteFeedRepository, SqliteStorage};

    fn setup() -> FeedService<SqliteFeedRepository> {
        let storage = SqliteStorage::in_memory().unwrap();
        let repo = SqliteFeedRepository::new(storage);
        let registry = SourceRegistry::new();
        FeedService::new(repo, registry)
    }

    #[test]
    fn test_list_empty() {
        let service = setup();
        let feeds = service.list().unwrap();
        assert!(feeds.is_empty());
    }

    #[test]
    fn test_exists_false() {
        let service = setup();
        assert!(!service.exists("https://example.com/feed").unwrap());
    }
}
