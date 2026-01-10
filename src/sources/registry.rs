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

    #[test]
    fn test_youtube_urls_not_handled_by_rss() {
        let registry = SourceRegistry::new();

        // Various YouTube URL formats should all be detected as YouTube, not RssAtom
        let youtube_urls = [
            "https://www.youtube.com/@channel",
            "https://youtube.com/@someuser",
            "https://www.youtube.com/channel/UCxxxx",
            "https://www.youtube.com/c/channelname",
        ];

        for url in youtube_urls {
            let source = registry.find_source(url).unwrap();
            assert_eq!(
                source.source_type(),
                SourceType::YouTube,
                "URL {} should be detected as YouTube, not {:?}",
                url,
                source.source_type()
            );
        }
    }

    #[test]
    fn test_mastodon_urls_not_handled_by_rss() {
        let registry = SourceRegistry::new();

        // Various Mastodon URL formats should all be detected as Mastodon, not RssAtom
        let mastodon_urls = [
            "https://mastodon.social/@user",
            "https://fosstodon.org/@someone",
            "https://hachyderm.io/@developer",
        ];

        for url in mastodon_urls {
            let source = registry.find_source(url).unwrap();
            assert_eq!(
                source.source_type(),
                SourceType::Mastodon,
                "URL {} should be detected as Mastodon, not {:?}",
                url,
                source.source_type()
            );
        }
    }

    #[test]
    fn test_blogger_urls_not_handled_by_rss() {
        let registry = SourceRegistry::new();

        let blogger_urls = [
            "https://example.blogspot.com",
            "https://myblog.blogspot.com/2024/01/post.html",
        ];

        for url in blogger_urls {
            let source = registry.find_source(url).unwrap();
            assert_eq!(
                source.source_type(),
                SourceType::Blogger,
                "URL {} should be detected as Blogger, not {:?}",
                url,
                source.source_type()
            );
        }
    }

    #[test]
    fn test_generic_urls_fallback_to_rss() {
        let registry = SourceRegistry::new();

        // Generic URLs that don't match specific sources should fall back to RssAtom
        let generic_urls = [
            "https://example.com",
            "https://blog.example.org/posts",
            "https://somesite.net/articles",
        ];

        for url in generic_urls {
            let source = registry.find_source(url).unwrap();
            assert_eq!(
                source.source_type(),
                SourceType::RssAtom,
                "URL {} should fall back to RssAtom, not {:?}",
                url,
                source.source_type()
            );
        }
    }
}
