use url::Url;

use crate::domain::{Article, Feed, SourceType};
use crate::errors::{FeederError, FeederResult};
use crate::sources::traits::{FeedMetadata, FeedSource};
use crate::sources::rss_atom::RssAtomSource;

pub struct BloggerSource {
    rss_source: RssAtomSource,
}

impl BloggerSource {
    pub fn new() -> Self {
        Self {
            rss_source: RssAtomSource::new(),
        }
    }

    /// Build the Atom feed URL for a Blogger site
    fn build_feed_url(&self, url: &str) -> FeederResult<String> {
        let parsed = Url::parse(url).map_err(|e| FeederError::InvalidUrl(e.to_string()))?;

        let host = parsed
            .host_str()
            .ok_or_else(|| FeederError::InvalidUrl("Missing host".to_string()))?;

        Ok(format!("https://{}/feeds/posts/default", host))
    }
}

impl Default for BloggerSource {
    fn default() -> Self {
        Self::new()
    }
}

impl FeedSource for BloggerSource {
    fn source_type(&self) -> SourceType {
        SourceType::Blogger
    }

    fn can_handle(&self, url: &str) -> bool {
        url.contains(".blogspot.com")
    }

    fn validate(&self, url: &str) -> FeederResult<FeedMetadata> {
        let feed_url = self.build_feed_url(url)?;

        // Use the RSS source to validate the feed
        let mut metadata = self.rss_source.validate(&feed_url)?;
        metadata.source_type = SourceType::Blogger;

        Ok(metadata)
    }

    fn fetch_articles(&self, feed: &Feed) -> FeederResult<Vec<Article>> {
        self.rss_source.fetch_articles(feed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_handle_blogger_urls() {
        let source = BloggerSource::new();

        assert!(source.can_handle("https://example.blogspot.com"));
        assert!(source.can_handle("https://myblog.blogspot.com/2023/01/post.html"));

        assert!(!source.can_handle("https://example.com"));
        assert!(!source.can_handle("https://wordpress.com"));
    }

    #[test]
    fn test_build_feed_url() {
        let source = BloggerSource::new();

        let feed_url = source
            .build_feed_url("https://example.blogspot.com")
            .unwrap();
        assert_eq!(feed_url, "https://example.blogspot.com/feeds/posts/default");

        let feed_url = source
            .build_feed_url("https://myblog.blogspot.com/2023/post")
            .unwrap();
        assert_eq!(feed_url, "https://myblog.blogspot.com/feeds/posts/default");
    }

    #[test]
    fn test_source_type() {
        let source = BloggerSource::new();
        assert_eq!(source.source_type(), SourceType::Blogger);
    }
}
