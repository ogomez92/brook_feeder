use crate::domain::{Article, Feed, FeedType, SourceType};
use crate::errors::FeederResult;

#[derive(Debug, Clone)]
pub struct FeedMetadata {
    pub title: String,
    pub feed_type: FeedType,
    pub feed_url: String,
    pub source_type: SourceType,
    pub description: Option<String>,
}

pub trait FeedSource: Send + Sync {
    /// Identifies this source type
    fn source_type(&self) -> SourceType;

    /// Check if this source can handle the given URL
    fn can_handle(&self, url: &str) -> bool;

    /// Validate that the URL points to a valid feed and return metadata
    fn validate(&self, url: &str) -> FeederResult<FeedMetadata>;

    /// Fetch articles from a feed
    fn fetch_articles(&self, feed: &Feed) -> FeederResult<Vec<Article>>;
}
