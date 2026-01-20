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

    /// Normalize the channel URL by stripping tab paths like /videos, /shorts, /streams
    /// e.g., https://youtube.com/@user/videos -> https://youtube.com/@user
    fn normalize_channel_url(&self, url: &str) -> String {
        // YouTube tab paths that should be stripped
        let tab_paths = ["/videos", "/shorts", "/streams", "/playlists", "/community", "/channels", "/about", "/featured"];

        let mut normalized = url.to_string();
        for path in tab_paths {
            if normalized.ends_with(path) {
                normalized = normalized[..normalized.len() - path.len()].to_string();
                break;
            }
        }
        normalized
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
        // Normalize the URL by stripping tab paths like /videos, /shorts, /streams, etc.
        let normalized_url = self.normalize_channel_url(url);
        let channel_id = self.extract_channel_id(&normalized_url)?;
        let feed_url = self.build_feed_url(&channel_id);

        // Check if the feed URL returns a successful response before trying to parse
        let response = self.client.get(&feed_url).send()?;
        if !response.status().is_success() {
            return Err(FeederError::FeedValidation(format!(
                "YouTube RSS feed not available for this channel (HTTP {}). \
                Some channels may not have RSS feeds enabled.",
                response.status().as_u16()
            )));
        }

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

    #[test]
    fn test_normalize_channel_url_strips_videos() {
        let source = YouTubeSource::new();
        assert_eq!(
            source.normalize_channel_url("https://www.youtube.com/@username/videos"),
            "https://www.youtube.com/@username"
        );
    }

    #[test]
    fn test_normalize_channel_url_strips_shorts() {
        let source = YouTubeSource::new();
        assert_eq!(
            source.normalize_channel_url("https://www.youtube.com/@username/shorts"),
            "https://www.youtube.com/@username"
        );
    }

    #[test]
    fn test_normalize_channel_url_strips_streams() {
        let source = YouTubeSource::new();
        assert_eq!(
            source.normalize_channel_url("https://www.youtube.com/@username/streams"),
            "https://www.youtube.com/@username"
        );
    }

    #[test]
    fn test_normalize_channel_url_strips_playlists() {
        let source = YouTubeSource::new();
        assert_eq!(
            source.normalize_channel_url("https://www.youtube.com/@username/playlists"),
            "https://www.youtube.com/@username"
        );
    }

    #[test]
    fn test_normalize_channel_url_preserves_clean_url() {
        let source = YouTubeSource::new();
        assert_eq!(
            source.normalize_channel_url("https://www.youtube.com/@username"),
            "https://www.youtube.com/@username"
        );
    }

    #[test]
    fn test_normalize_channel_url_with_channel_id() {
        let source = YouTubeSource::new();
        assert_eq!(
            source.normalize_channel_url("https://www.youtube.com/channel/UCxxx/videos"),
            "https://www.youtube.com/channel/UCxxx"
        );
    }
}
