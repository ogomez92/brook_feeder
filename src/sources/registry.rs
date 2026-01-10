use crate::domain::{Article, Feed};
use crate::errors::{FeederError, FeederResult};
use crate::sources::traits::{FeedMetadata, FeedSource};
use crate::sources::{
    blogger::BloggerSource, mastodon::MastodonSource, rss_atom::RssAtomSource,
    wordpress::WordPressSource, youtube::YouTubeSource,
};

pub struct SourceRegistry {
    sources: Vec<Box<dyn FeedSource>>,
}

impl SourceRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            sources: Vec::new(),
        };

        // Register sources in order of specificity (most specific first)
        // The order matters for auto-detection
        registry.register(Box::new(YouTubeSource::new()));
        registry.register(Box::new(MastodonSource::new()));
        registry.register(Box::new(BloggerSource::new()));
        registry.register(Box::new(WordPressSource::new()));
        registry.register(Box::new(RssAtomSource::new())); // Fallback

        registry
    }

    pub fn register(&mut self, source: Box<dyn FeedSource>) {
        self.sources.push(source);
    }

    /// Find appropriate source for URL
    pub fn find_source(&self, url: &str) -> Option<&dyn FeedSource> {
        self.sources
            .iter()
            .find(|s| s.can_handle(url))
            .map(|s| s.as_ref())
    }

    /// Validate URL using appropriate source
    pub fn validate(&self, url: &str) -> FeederResult<FeedMetadata> {
        let source = self
            .find_source(url)
            .ok_or_else(|| FeederError::UnsupportedSource(url.to_string()))?;

        source.validate(url)
    }

    /// Fetch articles from a feed
    pub fn fetch_articles(&self, feed: &Feed) -> FeederResult<Vec<Article>> {
        // Find source by source_type stored in feed
        let source = self
            .sources
            .iter()
            .find(|s| s.source_type() == feed.source_type)
            .ok_or_else(|| FeederError::UnsupportedSource(feed.source_type.to_string()))?;

        source.fetch_articles(feed)
    }
}

impl Default for SourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::SourceType;

    #[test]
    fn test_youtube_detected_first() {
        let registry = SourceRegistry::new();

        let source = registry.find_source("https://www.youtube.com/@channel").unwrap();
        assert_eq!(source.source_type(), SourceType::YouTube);
    }

    #[test]
    fn test_mastodon_detected() {
        let registry = SourceRegistry::new();

        let source = registry.find_source("https://mastodon.social/@user").unwrap();
        assert_eq!(source.source_type(), SourceType::Mastodon);
    }

    #[test]
    fn test_blogger_detected() {
        let registry = SourceRegistry::new();

        let source = registry.find_source("https://example.blogspot.com").unwrap();
        assert_eq!(source.source_type(), SourceType::Blogger);
    }

    #[test]
    fn test_fallback_to_rss() {
        let registry = SourceRegistry::new();

        let source = registry.find_source("https://example.com/feed.xml").unwrap();
        assert_eq!(source.source_type(), SourceType::RssAtom);
    }
}
