use regex::Regex;
use url::Url;

use crate::domain::{Article, Feed, SourceType};
use crate::errors::{FeederError, FeederResult};
use crate::sources::traits::{FeedMetadata, FeedSource};
use crate::sources::rss_atom::RssAtomSource;

pub struct MastodonSource {
    rss_source: RssAtomSource,
}

impl MastodonSource {
    pub fn new() -> Self {
        Self {
            rss_source: RssAtomSource::new(),
        }
    }

    /// Extract instance and username from Mastodon URL
    /// e.g., https://mastodon.social/@username -> (mastodon.social, username)
    fn extract_user_info(&self, url: &str) -> FeederResult<(String, String)> {
        let parsed = Url::parse(url).map_err(|e| FeederError::InvalidUrl(e.to_string()))?;

        let host = parsed
            .host_str()
            .ok_or_else(|| FeederError::InvalidUrl("Missing host in URL".to_string()))?
            .to_string();

        let path = parsed.path();

        // Match /@username pattern
        let user_regex = Regex::new(r"^/@([^/]+)").unwrap();
        if let Some(caps) = user_regex.captures(path) {
            return Ok((host, caps[1].to_string()));
        }

        Err(FeederError::InvalidUrl(
            "Could not extract Mastodon username from URL".to_string(),
        ))
    }

    /// Build the RSS feed URL for a Mastodon user
    fn build_feed_url(&self, instance: &str, username: &str) -> String {
        format!("https://{}/users/{}.rss", instance, username)
    }
}

impl Default for MastodonSource {
    fn default() -> Self {
        Self::new()
    }
}

impl FeedSource for MastodonSource {
    fn source_type(&self) -> SourceType {
        SourceType::Mastodon
    }

    fn can_handle(&self, url: &str) -> bool {
        // Exclude YouTube URLs (they also have @ but are handled by YouTubeSource)
        if url.contains("youtube.com") || url.contains("youtu.be") {
            return false;
        }

        // Check if URL contains /@username pattern (Mastodon/Fediverse)
        let user_regex = Regex::new(r"https?://[^/]+/@[^/]+").unwrap();
        user_regex.is_match(url)
    }

    fn validate(&self, url: &str) -> FeederResult<FeedMetadata> {
        let (instance, username) = self.extract_user_info(url)?;
        let feed_url = self.build_feed_url(&instance, &username);

        // Use the RSS source to validate the feed
        let mut metadata = self.rss_source.validate(&feed_url)?;
        metadata.source_type = SourceType::Mastodon;

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
    fn test_can_handle_mastodon_urls() {
        let source = MastodonSource::new();

        assert!(source.can_handle("https://mastodon.social/@username"));
        assert!(source.can_handle("https://fosstodon.org/@user"));
        assert!(source.can_handle("https://hachyderm.io/@someone"));

        assert!(!source.can_handle("https://example.com/feed"));
        assert!(!source.can_handle("https://youtube.com/@channel"));
    }

    #[test]
    fn test_extract_user_info() {
        let source = MastodonSource::new();

        let (instance, username) = source
            .extract_user_info("https://mastodon.social/@testuser")
            .unwrap();

        assert_eq!(instance, "mastodon.social");
        assert_eq!(username, "testuser");
    }

    #[test]
    fn test_build_feed_url() {
        let source = MastodonSource::new();
        let feed_url = source.build_feed_url("mastodon.social", "testuser");
        assert_eq!(feed_url, "https://mastodon.social/users/testuser.rss");
    }

    #[test]
    fn test_source_type() {
        let source = MastodonSource::new();
        assert_eq!(source.source_type(), SourceType::Mastodon);
    }
}
