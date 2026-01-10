use regex::Regex;
use reqwest::blocking::Client;
use scraper::{Html, Selector};

use crate::domain::{Article, Feed, SourceType};
use crate::errors::{FeederError, FeederResult};
use crate::sources::traits::{FeedMetadata, FeedSource};
use crate::sources::rss_atom::RssAtomSource;

pub struct YouTubeSource {
    client: Client,
    rss_source: RssAtomSource,
}

impl YouTubeSource {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
            rss_source: RssAtomSource::new(),
        }
    }

    /// Extract channel ID from various YouTube URL formats
    fn extract_channel_id(&self, url: &str) -> FeederResult<String> {
        // Pattern 1: /channel/UC... URLs
        let channel_regex = Regex::new(r"youtube\.com/channel/(UC[\w-]{22})").unwrap();
        if let Some(caps) = channel_regex.captures(url) {
            return Ok(caps[1].to_string());
        }

        // Pattern 2: /@username or /c/customname URLs - need to fetch page and extract
        if url.contains("/@") || url.contains("/c/") || url.contains("/user/") {
            return self.extract_channel_id_from_page(url);
        }

        Err(FeederError::InvalidUrl(
            "Could not extract YouTube channel ID from URL".to_string(),
        ))
    }

    /// Fetch YouTube page and extract channel ID from meta tags or page content
    fn extract_channel_id_from_page(&self, url: &str) -> FeederResult<String> {
        let response = self.client.get(url).send()?;
        let html = response.text()?;
        let document = Html::parse_document(&html);

        // Try to find channel ID in meta tags
        let meta_selector = Selector::parse("meta[itemprop='channelId']").unwrap();
        if let Some(element) = document.select(&meta_selector).next() {
            if let Some(channel_id) = element.value().attr("content") {
                return Ok(channel_id.to_string());
            }
        }

        // Try to find in canonical link
        let link_selector = Selector::parse("link[rel='canonical']").unwrap();
        if let Some(element) = document.select(&link_selector).next() {
            if let Some(href) = element.value().attr("href") {
                let channel_regex = Regex::new(r"youtube\.com/channel/(UC[\w-]{22})").unwrap();
                if let Some(caps) = channel_regex.captures(href) {
                    return Ok(caps[1].to_string());
                }
            }
        }

        // Try to find in page content using regex
        let channel_regex = Regex::new(r#""channelId":"(UC[\w-]{22})""#).unwrap();
        if let Some(caps) = channel_regex.captures(&html) {
            return Ok(caps[1].to_string());
        }

        // Alternative regex pattern
        let alt_regex = Regex::new(r#"channel/(UC[\w-]{22})"#).unwrap();
        if let Some(caps) = alt_regex.captures(&html) {
            return Ok(caps[1].to_string());
        }

        Err(FeederError::FeedValidation(
            "Could not find channel ID on YouTube page".to_string(),
        ))
    }

    /// Build the RSS feed URL from a channel ID
    fn build_feed_url(&self, channel_id: &str) -> String {
        format!(
            "https://www.youtube.com/feeds/videos.xml?channel_id={}",
            channel_id
        )
    }
}

impl Default for YouTubeSource {
    fn default() -> Self {
        Self::new()
    }
}

impl FeedSource for YouTubeSource {
    fn source_type(&self) -> SourceType {
        SourceType::YouTube
    }

    fn can_handle(&self, url: &str) -> bool {
        url.contains("youtube.com/channel/")
            || url.contains("youtube.com/@")
            || url.contains("youtube.com/c/")
            || url.contains("youtube.com/user/")
    }

    fn validate(&self, url: &str) -> FeederResult<FeedMetadata> {
        let channel_id = self.extract_channel_id(url)?;
        let feed_url = self.build_feed_url(&channel_id);

        // Use the RSS source to validate the feed
        let mut metadata = self.rss_source.validate(&feed_url)?;
        metadata.source_type = SourceType::YouTube;

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
    fn test_can_handle_youtube_urls() {
        let source = YouTubeSource::new();

        assert!(source.can_handle("https://www.youtube.com/channel/UCxxx"));
        assert!(source.can_handle("https://youtube.com/@username"));
        assert!(source.can_handle("https://www.youtube.com/c/channelname"));
        assert!(source.can_handle("https://www.youtube.com/user/username"));

        assert!(!source.can_handle("https://example.com/feed"));
        assert!(!source.can_handle("https://mastodon.social/@user"));
    }

    #[test]
    fn test_source_type() {
        let source = YouTubeSource::new();
        assert_eq!(source.source_type(), SourceType::YouTube);
    }

    #[test]
    fn test_build_feed_url() {
        let source = YouTubeSource::new();
        let feed_url = source.build_feed_url("UCxxxxxxxxxxxxxxxxxxxxxxx");
        assert_eq!(
            feed_url,
            "https://www.youtube.com/feeds/videos.xml?channel_id=UCxxxxxxxxxxxxxxxxxxxxxxx"
        );
    }
}
