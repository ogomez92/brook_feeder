use feed_rs::parser;
use regex::Regex;
use reqwest::blocking::Client;
use scraper::Html;
use url::Url;

use crate::domain::{Article, Feed, SourceType};
use crate::errors::{FeederError, FeederResult};
use crate::sources::traits::{FeedMetadata, FeedSource};
use crate::sources::rss_atom::RssAtomSource;

pub struct MastodonSource {
    client: Client,
    rss_source: RssAtomSource,
}

impl MastodonSource {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
            rss_source: RssAtomSource::new(),
        }
    }

    /// Extract plain text from HTML content, preserving some structure
    fn html_to_text(html: &str) -> String {
        let document = Html::parse_fragment(html);
        let mut text = String::new();

        for node in document.root_element().descendants() {
            if let Some(text_node) = node.value().as_text() {
                text.push_str(text_node);
            }
            // Add space after block elements to preserve word boundaries
            if let Some(element) = node.value().as_element() {
                match element.name() {
                    "p" | "br" | "div" => text.push(' '),
                    _ => {}
                }
            }
        }

        // Collapse whitespace and trim
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Truncate text to a reasonable length for a title
    fn truncate_for_title(text: &str, max_len: usize) -> String {
        if text.len() <= max_len {
            return text.to_string();
        }

        // Try to break at a word boundary
        if let Some(pos) = text[..max_len].rfind(' ') {
            format!("{}...", &text[..pos])
        } else {
            format!("{}...", &text[..max_len])
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
        // Fetch and parse the feed ourselves to handle Mastodon's title-less posts
        let response = self.client.get(&feed.feed_url).send()?;
        let bytes = response.bytes()?;
        let parsed = parser::parse(&bytes[..])
            .map_err(|e| FeederError::FeedParse(e.to_string()))?;

        let articles: Vec<Article> = parsed
            .entries
            .into_iter()
            .map(|entry| {
                let id = entry.id;

                // Mastodon posts typically don't have titles, so use the content/summary
                let title = entry
                    .title
                    .map(|t| t.content)
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| {
                        // Try to extract text from content or summary
                        let html_content = entry
                            .content
                            .and_then(|c| c.body)
                            .or_else(|| entry.summary.map(|s| s.content))
                            .unwrap_or_default();

                        let text = Self::html_to_text(&html_content);
                        if text.is_empty() {
                            "Untitled".to_string()
                        } else {
                            // Truncate to reasonable length for a title (200 chars)
                            Self::truncate_for_title(&text, 200)
                        }
                    });

                let links: Vec<String> = entry.links.into_iter().map(|l| l.href).collect();

                let published = entry
                    .published
                    .or(entry.updated)
                    .map(|dt| dt.to_rfc3339());

                Article::new(id, title)
                    .with_links(links)
                    .with_published(published)
            })
            .collect();

        Ok(articles)
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

    #[test]
    fn test_html_to_text_simple() {
        let html = "<p>Hello world</p>";
        let text = MastodonSource::html_to_text(html);
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn test_html_to_text_with_links() {
        let html = r#"<p>Check out <a href="https://example.com">this link</a>!</p>"#;
        let text = MastodonSource::html_to_text(html);
        assert_eq!(text, "Check out this link!");
    }

    #[test]
    fn test_html_to_text_multiple_paragraphs() {
        let html = "<p>First paragraph</p><p>Second paragraph</p>";
        let text = MastodonSource::html_to_text(html);
        assert_eq!(text, "First paragraph Second paragraph");
    }

    #[test]
    fn test_html_to_text_with_hashtags() {
        let html = r#"<p>Post content <a href="https://mastodon.social/tags/test" class="mention hashtag">#<span>test</span></a></p>"#;
        let text = MastodonSource::html_to_text(html);
        assert_eq!(text, "Post content #test");
    }

    #[test]
    fn test_html_to_text_strips_extra_whitespace() {
        let html = "<p>  Multiple   spaces   here  </p>";
        let text = MastodonSource::html_to_text(html);
        assert_eq!(text, "Multiple spaces here");
    }

    #[test]
    fn test_html_to_text_empty() {
        let html = "";
        let text = MastodonSource::html_to_text(html);
        assert_eq!(text, "");
    }

    #[test]
    fn test_truncate_for_title_short_text() {
        let text = "Short text";
        let truncated = MastodonSource::truncate_for_title(text, 50);
        assert_eq!(truncated, "Short text");
    }

    #[test]
    fn test_truncate_for_title_long_text() {
        let text = "This is a very long text that should be truncated at a word boundary";
        let truncated = MastodonSource::truncate_for_title(text, 30);
        assert_eq!(truncated, "This is a very long text that...");
    }

    #[test]
    fn test_truncate_for_title_exact_length() {
        let text = "Exactly twenty chars";
        let truncated = MastodonSource::truncate_for_title(text, 20);
        assert_eq!(truncated, "Exactly twenty chars");
    }

    #[test]
    fn test_truncate_for_title_no_word_boundary() {
        let text = "Verylongwordwithoutspaces";
        let truncated = MastodonSource::truncate_for_title(text, 10);
        assert_eq!(truncated, "Verylongwo...");
    }

    #[test]
    fn test_html_to_text_real_mastodon_post() {
        // Real example from Humble Bundle bot
        let html = r#"<p>Design Unlimited Bundle Encore</p><p>Get CorelDRAW Standard 2024!</p><p><a href="https://www.humblebundle.com/software/design-unlimited-bundle-encore-software" target="_blank" rel="nofollow noopener" translate="no"><span class="invisible">https://www.</span><span class="ellipsis">humblebundle.com/software/desi</span><span class="invisible">gn-unlimited-bundle-encore-software</span></a></p><p><a href="https://tech.lgbt/tags/humblebundle" class="mention hashtag" rel="tag">#<span>humblebundle</span></a></p>"#;
        let text = MastodonSource::html_to_text(html);
        assert!(text.starts_with("Design Unlimited Bundle Encore"));
        assert!(text.contains("CorelDRAW"));
    }
}
