use feed_rs::parser;
use reqwest::blocking::Client;
use url::Url;

use crate::domain::{Article, Feed, FeedType, SourceType};
use crate::errors::{FeederError, FeederResult};
use crate::sources::traits::{FeedMetadata, FeedSource};

/// Common feed URL patterns to try when direct URL fails
const FEED_PATTERNS: &[&str] = &[
    "/feed/",           // WordPress
    "/index.xml",       // Hugo
    "/atom.xml",        // Hugo/Jekyll Atom
    "/rss.xml",         // Generic RSS
    "/feed.xml",        // Generic feed
    "/rss",             // Some sites
    "/feed",            // Some sites
    "/feeds/posts/default", // Blogger
    "/.rss",            // Some static generators
];

pub struct RssAtomSource {
    client: Client,
}

impl RssAtomSource {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    /// Try to discover a valid feed URL by testing common patterns
    /// Returns the first URL that successfully parses as a feed
    fn discover_feed_url(&self, url: &str) -> FeederResult<(String, feed_rs::model::Feed)> {
        // First, try the URL as-is (might already be a feed URL)
        if let Ok(feed) = self.fetch_and_parse(url) {
            return Ok((url.to_string(), feed));
        }

        // Parse the base URL
        let parsed = Url::parse(url).map_err(|e| FeederError::InvalidUrl(e.to_string()))?;
        let base_url = format!(
            "{}://{}",
            parsed.scheme(),
            parsed.host_str().ok_or_else(|| FeederError::InvalidUrl("Missing host".to_string()))?
        );

        // Try each pattern
        let mut last_error = FeederError::FeedParse("No valid feed found".to_string());

        for pattern in FEED_PATTERNS {
            let feed_url = format!("{}{}", base_url, pattern);

            // Check if URL returns success before trying to parse
            match self.client.head(&feed_url).send() {
                Ok(response) if response.status().is_success() => {
                    // Try to parse as feed
                    match self.fetch_and_parse(&feed_url) {
                        Ok(feed) => return Ok((feed_url, feed)),
                        Err(e) => last_error = e,
                    }
                }
                Ok(_) => continue, // Non-success status, try next
                Err(_) => continue, // Request failed, try next
            }
        }

        Err(last_error)
    }

    fn fetch_and_parse(&self, url: &str) -> FeederResult<feed_rs::model::Feed> {
        let response = self.client.get(url).send()?;
        let bytes = response.bytes()?;

        Self::parse_bytes(&bytes)
    }

    fn parse_bytes(bytes: &[u8]) -> FeederResult<feed_rs::model::Feed> {
        parser::parse(bytes).map_err(|e| FeederError::FeedParse(e.to_string()))
    }

    /// Parse articles from raw feed bytes (used for testing)
    #[cfg(test)]
    fn articles_from_bytes(bytes: &[u8]) -> FeederResult<Vec<Article>> {
        let parsed = Self::parse_bytes(bytes)?;

        let articles: Vec<Article> = parsed
            .entries
            .into_iter()
            .map(|entry| {
                let id = entry.id;
                let title = entry
                    .title
                    .map(|t| t.content)
                    .unwrap_or_else(|| "Untitled".to_string());

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

    fn determine_feed_type(feed: &feed_rs::model::Feed) -> FeedType {
        match feed.feed_type {
            feed_rs::model::FeedType::Atom => FeedType::Atom,
            feed_rs::model::FeedType::JSON => FeedType::Json,
            _ => FeedType::Rss,
        }
    }
}

impl Default for RssAtomSource {
    fn default() -> Self {
        Self::new()
    }
}

impl FeedSource for RssAtomSource {
    fn source_type(&self) -> SourceType {
        SourceType::RssAtom
    }

    fn can_handle(&self, _url: &str) -> bool {
        // RssAtomSource is the fallback, it can try to handle any URL
        true
    }

    fn validate(&self, url: &str) -> FeederResult<FeedMetadata> {
        // Use feed discovery to find a valid feed URL
        let (feed_url, feed) = self.discover_feed_url(url)?;

        let feed_type = Self::determine_feed_type(&feed);

        let title = feed
            .title
            .map(|t| t.content)
            .unwrap_or_else(|| "Untitled Feed".to_string());

        let description = feed.description.map(|d| d.content);

        Ok(FeedMetadata {
            title,
            feed_type,
            feed_url,
            source_type: SourceType::RssAtom,
            description,
        })
    }

    fn fetch_articles(&self, feed: &Feed) -> FeederResult<Vec<Article>> {
        let parsed = self.fetch_and_parse(&feed.feed_url)?;

        let articles: Vec<Article> = parsed
            .entries
            .into_iter()
            .map(|entry| {
                let id = entry.id;
                let title = entry
                    .title
                    .map(|t| t.content)
                    .unwrap_or_else(|| "Untitled".to_string());

                // Only extract title and links - skip content/summary to keep notifications concise
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
    fn test_can_handle_any_url() {
        let source = RssAtomSource::new();
        assert!(source.can_handle("https://example.com/feed.xml"));
        assert!(source.can_handle("https://blog.rust-lang.org/feed.xml"));
    }

    #[test]
    fn test_source_type() {
        let source = RssAtomSource::new();
        assert_eq!(source.source_type(), SourceType::RssAtom);
    }

    // Sample RSS feed (based on Rust Blog format)
    const SAMPLE_RSS: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Rust Blog</title>
    <link>https://blog.rust-lang.org/</link>
    <description>Empowering everyone to build reliable and efficient software.</description>
    <item>
      <title>Announcing Rust 1.75.0</title>
      <link>https://blog.rust-lang.org/2023/12/28/Rust-1.75.0.html</link>
      <description><![CDATA[<p>The Rust team is happy to announce a new version of Rust, 1.75.0. This release includes async fn in traits and many other improvements.</p>]]></description>
      <pubDate>Thu, 28 Dec 2023 00:00:00 +0000</pubDate>
      <guid>https://blog.rust-lang.org/2023/12/28/Rust-1.75.0.html</guid>
    </item>
    <item>
      <title>Rust 2024 Call for Testing</title>
      <link>https://blog.rust-lang.org/2024/01/10/Rust-2024-CFT.html</link>
      <description><![CDATA[<p>We're testing the next edition of Rust!</p>]]></description>
      <pubDate>Wed, 10 Jan 2024 00:00:00 +0000</pubDate>
      <guid>https://blog.rust-lang.org/2024/01/10/Rust-2024-CFT.html</guid>
    </item>
  </channel>
</rss>"#;

    // Sample Atom feed
    const SAMPLE_ATOM: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Example Tech Blog</title>
  <link href="https://example.com/"/>
  <id>https://example.com/feed.atom</id>
  <updated>2024-01-15T12:00:00Z</updated>
  <entry>
    <title>Understanding WebAssembly</title>
    <link href="https://example.com/posts/wasm-intro"/>
    <id>https://example.com/posts/wasm-intro</id>
    <updated>2024-01-15T12:00:00Z</updated>
    <summary type="html"><![CDATA[<p>WebAssembly (Wasm) is a binary instruction format for a stack-based virtual machine...</p>]]></summary>
    <content type="html"><![CDATA[<article><h1>Understanding WebAssembly</h1><p>WebAssembly (Wasm) is a binary instruction format...</p><p>More content here with <a href="https://example.com">links</a> and formatting.</p></article>]]></content>
  </entry>
</feed>"#;

    #[test]
    fn test_rss_articles_have_no_content() {
        let articles = RssAtomSource::articles_from_bytes(SAMPLE_RSS).unwrap();

        assert_eq!(articles.len(), 2);

        // First article
        assert_eq!(articles[0].title, "Announcing Rust 1.75.0");
        assert!(
            articles[0].content.is_none(),
            "RSS articles should not include content/description"
        );
        assert!(!articles[0].links.is_empty(), "Articles should have links");
        assert!(articles[0]
            .links
            .iter()
            .any(|l| l.contains("Rust-1.75.0.html")));

        // Second article
        assert_eq!(articles[1].title, "Rust 2024 Call for Testing");
        assert!(
            articles[1].content.is_none(),
            "RSS articles should not include content/description"
        );
    }

    #[test]
    fn test_atom_articles_have_no_content() {
        let articles = RssAtomSource::articles_from_bytes(SAMPLE_ATOM).unwrap();

        assert_eq!(articles.len(), 1);

        let article = &articles[0];
        assert_eq!(article.title, "Understanding WebAssembly");
        assert!(
            article.content.is_none(),
            "Atom articles should not include content/summary"
        );
        assert!(!article.links.is_empty(), "Articles should have links");
        assert!(article.links.iter().any(|l| l.contains("wasm-intro")));
    }

    #[test]
    fn test_rss_article_links_extracted() {
        let articles = RssAtomSource::articles_from_bytes(SAMPLE_RSS).unwrap();

        // Verify links are properly extracted
        let first = &articles[0];
        assert!(
            first
                .links
                .iter()
                .any(|l| l == "https://blog.rust-lang.org/2023/12/28/Rust-1.75.0.html"),
            "Should extract article link"
        );
    }

    #[test]
    fn test_feed_patterns_are_valid() {
        // Ensure all patterns start with /
        for pattern in FEED_PATTERNS {
            assert!(
                pattern.starts_with('/'),
                "Feed pattern '{}' should start with /",
                pattern
            );
        }

        // Ensure common patterns are included
        assert!(
            FEED_PATTERNS.contains(&"/feed/"),
            "WordPress pattern /feed/ should be included"
        );
        assert!(
            FEED_PATTERNS.contains(&"/index.xml"),
            "Hugo pattern /index.xml should be included"
        );
        assert!(
            FEED_PATTERNS.contains(&"/atom.xml"),
            "Atom pattern /atom.xml should be included"
        );
        assert!(
            FEED_PATTERNS.contains(&"/rss.xml"),
            "RSS pattern /rss.xml should be included"
        );
    }

    #[test]
    fn test_feed_patterns_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for pattern in FEED_PATTERNS {
            assert!(
                seen.insert(pattern),
                "Duplicate feed pattern found: {}",
                pattern
            );
        }
    }
}
