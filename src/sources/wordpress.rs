use reqwest::blocking::Client;
use url::Url;

use crate::domain::{Article, Feed, SourceType};
use crate::errors::{FeederError, FeederResult};
use crate::sources::traits::{FeedMetadata, FeedSource};
use crate::sources::rss_atom::RssAtomSource;

pub struct WordPressSource {
    client: Client,
    rss_source: RssAtomSource,
}

impl WordPressSource {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
            rss_source: RssAtomSource::new(),
        }
    }

    /// Check if a site has WordPress REST API
    fn is_wordpress(&self, url: &str) -> bool {
        let parsed = match Url::parse(url) {
            Ok(u) => u,
            Err(_) => return false,
        };

        let base_url = format!(
            "{}://{}",
            parsed.scheme(),
            parsed.host_str().unwrap_or("")
        );

        let wp_json_url = format!("{}/wp-json/", base_url);

        // Try HEAD request first (faster)
        if let Ok(response) = self.client.head(&wp_json_url).send() {
            if response.status().is_success() {
                return true;
            }
        }

        // Fall back to GET request
        if let Ok(response) = self.client.get(&wp_json_url).send() {
            return response.status().is_success();
        }

        false
    }

    /// Build the RSS feed URL for a WordPress site
    fn build_feed_url(&self, url: &str) -> FeederResult<String> {
        let parsed = Url::parse(url).map_err(|e| FeederError::InvalidUrl(e.to_string()))?;

        let base_url = format!(
            "{}://{}",
            parsed.scheme(),
            parsed.host_str().ok_or_else(|| FeederError::InvalidUrl("Missing host".to_string()))?
        );

        Ok(format!("{}/feed/", base_url))
    }
}

impl Default for WordPressSource {
    fn default() -> Self {
        Self::new()
    }
}

impl FeedSource for WordPressSource {
    fn source_type(&self) -> SourceType {
        SourceType::WordPress
    }

    fn can_handle(&self, url: &str) -> bool {
        // Check for explicit WordPress.com URLs or wp-json endpoint
        if url.contains("wordpress.com") {
            return true;
        }

        // Try to detect WordPress via REST API
        self.is_wordpress(url)
    }

    fn validate(&self, url: &str) -> FeederResult<FeedMetadata> {
        let feed_url = self.build_feed_url(url)?;

        // Use the RSS source to validate the feed
        let mut metadata = self.rss_source.validate(&feed_url)?;
        metadata.source_type = SourceType::WordPress;

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
    fn test_can_handle_wordpress_com_urls() {
        let source = WordPressSource::new();

        assert!(source.can_handle("https://example.wordpress.com"));
        assert!(source.can_handle("https://blog.wordpress.com/2023/01/post"));
    }

    #[test]
    fn test_build_feed_url() {
        let source = WordPressSource::new();

        let feed_url = source.build_feed_url("https://example.com/blog").unwrap();
        assert_eq!(feed_url, "https://example.com/feed/");

        let feed_url = source.build_feed_url("https://blog.wordpress.com").unwrap();
        assert_eq!(feed_url, "https://blog.wordpress.com/feed/");
    }

    #[test]
    fn test_source_type() {
        let source = WordPressSource::new();
        assert_eq!(source.source_type(), SourceType::WordPress);
    }
}
