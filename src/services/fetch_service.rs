use crate::domain::{Article, Feed, Notification};
use crate::errors::FeederResult;
use crate::sources::SourceRegistry;
use crate::storage::traits::{ArticleCacheRepository, FeedRepository};

pub struct FetchService<F: FeedRepository, C: ArticleCacheRepository> {
    feed_repository: F,
    cache_repository: C,
    source_registry: SourceRegistry,
}

impl<F: FeedRepository, C: ArticleCacheRepository> FetchService<F, C> {
    pub fn new(
        feed_repository: F,
        cache_repository: C,
        source_registry: SourceRegistry,
    ) -> Self {
        Self {
            feed_repository,
            cache_repository,
            source_registry,
        }
    }

    /// Fetch articles from a single feed and return unnotified ones
    pub fn fetch_unnotified(&self, feed: &Feed) -> FeederResult<Vec<Article>> {
        let articles = self.source_registry.fetch_articles(feed)?;

        // Generate cache keys for all articles
        let cache_keys: Vec<String> = articles
            .iter()
            .map(|a| a.cache_key(&feed.title))
            .collect();

        // Get unnotified cache keys
        let unnotified_keys = self.cache_repository.get_unnotified(&cache_keys)?;

        // Filter articles to only unnotified ones
        let unnotified_articles: Vec<Article> = articles
            .into_iter()
            .filter(|a| unnotified_keys.contains(&a.cache_key(&feed.title)))
            .collect();

        Ok(unnotified_articles)
    }

    /// Mark articles as notified
    pub fn mark_notified(&self, feed: &Feed, articles: &[Article]) -> FeederResult<()> {
        let feed_id = feed.id.ok_or_else(|| {
            crate::errors::FeederError::FeedNotFound("Feed has no ID".to_string())
        })?;

        for article in articles {
            let cache_key = article.cache_key(&feed.title);
            self.cache_repository
                .mark_notified(&cache_key, feed_id, &article.title)?;
        }

        Ok(())
    }

    /// Fetch all feeds and return unnotified articles with their feeds
    pub fn fetch_all_unnotified(&self) -> FeederResult<Vec<(Feed, Vec<Article>)>> {
        let feeds = self.feed_repository.get_all()?;
        let mut results = Vec::new();

        for feed in feeds {
            match self.fetch_unnotified(&feed) {
                Ok(articles) if !articles.is_empty() => {
                    results.push((feed, articles));
                }
                Ok(_) => {
                    // No new articles
                }
                Err(e) => {
                    // Log error but continue with other feeds
                    eprintln!("Error fetching {}: {}", feed.title, e);
                }
            }
        }

        Ok(results)
    }

    /// Create notifications from articles
    pub fn create_notifications(feed: &Feed, articles: &[Article]) -> Vec<Notification> {
        articles
            .iter()
            .map(|article| Notification::from_article(feed, article))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{FeedType, SourceType};
    use crate::storage::sqlite::{
        SqliteArticleCacheRepository, SqliteFeedRepository, SqliteStorage,
    };

    fn setup() -> FetchService<SqliteFeedRepository, SqliteArticleCacheRepository> {
        let storage = SqliteStorage::in_memory().unwrap();
        let feed_repo = SqliteFeedRepository::new(storage.clone());
        let cache_repo = SqliteArticleCacheRepository::new(storage);
        let registry = SourceRegistry::new();
        FetchService::new(feed_repo, cache_repo, registry)
    }

    #[test]
    fn test_fetch_all_empty() {
        let service = setup();
        let results = service.fetch_all_unnotified().unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_create_notifications() {
        let feed = Feed::new(
            "https://example.com/feed".to_string(),
            "https://example.com/feed".to_string(),
            "Test Feed".to_string(),
            FeedType::Rss,
            SourceType::RssAtom,
        );

        let articles = vec![
            Article::new("1".to_string(), "Article 1".to_string())
                .with_content(Some("Content 1".to_string())),
            Article::new("2".to_string(), "Article 2".to_string())
                .with_content(Some("Content 2".to_string())),
        ];

        let notifications = FetchService::<SqliteFeedRepository, SqliteArticleCacheRepository>::create_notifications(&feed, &articles);

        assert_eq!(notifications.len(), 2);
        assert_eq!(notifications[0].feed_title, "Test Feed");
        assert_eq!(notifications[0].article_title, "Article 1");
    }
}
