use feed_rs::parser;
use reqwest::blocking::Client;

use crate::domain::{Article, Feed, FeedType, SourceType};
use crate::errors::{FeederError, FeederResult};
use crate::sources::traits::{FeedMetadata, FeedSource};

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

    fn fetch_and_parse(&self, url: &str) -> FeederResult<feed_rs::model::Feed> {
        let response = self.client.get(url).send()?;
        let bytes = response.bytes()?;

        parser::parse(&bytes[..]).map_err(|e| FeederError::FeedParse(e.to_string()))
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
        let feed = self.fetch_and_parse(url)?;

        let feed_type = Self::determine_feed_type(&feed);

        let title = feed
            .title
            .map(|t| t.content)
            .unwrap_or_else(|| "Untitled Feed".to_string());

        let description = feed.description.map(|d| d.content);

        Ok(FeedMetadata {
            title,
            feed_type,
            feed_url: url.to_string(),
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

                let content = entry
                    .content
                    .and_then(|c| c.body)
                    .or_else(|| entry.summary.map(|s| s.content));

                let links: Vec<String> = entry.links.into_iter().map(|l| l.href).collect();

                let published = entry
                    .published
                    .or(entry.updated)
                    .map(|dt| dt.to_rfc3339());

                Article::new(id, title)
                    .with_content(content)
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
}
