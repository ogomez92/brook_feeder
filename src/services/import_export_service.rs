use opml::{Outline, OPML};
use regex::Regex;

use crate::domain::Feed;
use crate::errors::{FeederError, FeederResult};
use crate::sources::SourceRegistry;
use crate::storage::traits::FeedRepository;

pub struct ImportResult {
    pub added: Vec<Feed>,
    pub invalid: Vec<(String, String)>, // (url, error_message)
    pub duplicates: Vec<String>,
}

pub struct ImportExportService<R: FeedRepository> {
    repository: R,
    source_registry: SourceRegistry,
}

impl<R: FeedRepository> ImportExportService<R> {
    pub fn new(repository: R, source_registry: SourceRegistry) -> Self {
        Self {
            repository,
            source_registry,
        }
    }

    /// Import feeds from OPML content
    pub fn import_opml(&self, content: &str) -> FeederResult<ImportResult> {
        let opml = OPML::from_str(content)
            .map_err(|e| FeederError::OpmlParse(e.to_string()))?;

        let mut result = ImportResult {
            added: Vec::new(),
            invalid: Vec::new(),
            duplicates: Vec::new(),
        };

        // Extract all feed URLs from outlines
        let urls = self.extract_feed_urls(&opml.body.outlines);

        for url in urls {
            // Check for duplicate
            if self.repository.exists(&url)? {
                result.duplicates.push(url);
                continue;
            }

            // Validate feed
            match self.source_registry.validate(&url) {
                Ok(metadata) => {
                    let feed = Feed::new(
                        url.clone(),
                        metadata.feed_url,
                        metadata.title,
                        metadata.feed_type,
                        metadata.source_type,
                    );

                    match self.repository.add(&feed) {
                        Ok(id) => {
                            result.added.push(Feed {
                                id: Some(id),
                                ..feed
                            });
                        }
                        Err(e) => {
                            result.invalid.push((url, e.to_string()));
                        }
                    }
                }
                Err(e) => {
                    result.invalid.push((url, e.to_string()));
                }
            }
        }

        Ok(result)
    }

    /// Recursively extract feed URLs from OPML outlines
    fn extract_feed_urls(&self, outlines: &[Outline]) -> Vec<String> {
        let mut urls = Vec::new();

        for outline in outlines {
            // Check for xml_url attribute (RSS/Atom feed)
            if let Some(url) = &outline.xml_url {
                if !url.is_empty() {
                    // Handle Mastodon handle format: @user@instance -> https://instance/@user
                    let normalized = self.normalize_url(url);
                    urls.push(normalized);
                }
            }

            // Recursively process child outlines
            urls.extend(self.extract_feed_urls(&outline.outlines));
        }

        urls
    }

    /// Normalize URL, converting Mastodon handles to proper URLs
    fn normalize_url(&self, url: &str) -> String {
        // Check for Mastodon handle format: @user@instance
        let handle_regex = Regex::new(r"^@([^@]+)@(.+)$").unwrap();
        if let Some(caps) = handle_regex.captures(url) {
            let user = &caps[1];
            let instance = &caps[2];
            return format!("https://{}/@{}", instance, user);
        }

        url.to_string()
    }

    /// Export feeds to OPML format
    pub fn export_opml(&self) -> FeederResult<String> {
        let feeds = self.repository.get_all()?;

        let mut opml = OPML::default();
        opml.head = Some(opml::Head {
            title: Some("Feeder Subscriptions".to_string()),
            ..Default::default()
        });

        for feed in feeds {
            let outline = Outline {
                text: feed.title.clone(),
                r#type: Some("rss".to_string()),
                xml_url: Some(feed.feed_url.clone()),
                html_url: Some(feed.url.clone()),
                title: Some(feed.title),
                ..Default::default()
            };
            opml.body.outlines.push(outline);
        }

        opml.to_string()
            .map_err(|e| FeederError::OpmlParse(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{FeedType, SourceType};
    use crate::storage::sqlite::{SqliteFeedRepository, SqliteStorage};
    use crate::storage::traits::FeedRepository as _;

    fn setup() -> ImportExportService<SqliteFeedRepository> {
        let storage = SqliteStorage::in_memory().unwrap();
        let repo = SqliteFeedRepository::new(storage);
        let registry = SourceRegistry::new();
        ImportExportService::new(repo, registry)
    }

    #[test]
    fn test_export_empty() {
        let service = setup();
        let opml = service.export_opml().unwrap();

        assert!(opml.contains("Feeder Subscriptions"));
        assert!(opml.contains("<opml"));
    }

    #[test]
    fn test_extract_feed_urls() {
        let service = setup();

        let outlines = vec![
            Outline {
                text: "Feed 1".to_string(),
                xml_url: Some("https://example1.com/feed".to_string()),
                ..Default::default()
            },
            Outline {
                text: "Category".to_string(),
                outlines: vec![Outline {
                    text: "Feed 2".to_string(),
                    xml_url: Some("https://example2.com/feed".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            },
        ];

        let urls = service.extract_feed_urls(&outlines);

        assert_eq!(urls.len(), 2);
        assert!(urls.contains(&"https://example1.com/feed".to_string()));
        assert!(urls.contains(&"https://example2.com/feed".to_string()));
    }
}
