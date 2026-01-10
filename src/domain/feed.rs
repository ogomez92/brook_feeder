use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedType {
    Rss,
    Atom,
    Json,
}

impl FeedType {
    pub fn as_str(&self) -> &'static str {
        match self {
            FeedType::Rss => "rss",
            FeedType::Atom => "atom",
            FeedType::Json => "json",
        }
    }
}

impl std::str::FromStr for FeedType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "rss" => Ok(FeedType::Rss),
            "atom" => Ok(FeedType::Atom),
            "json" => Ok(FeedType::Json),
            _ => Err(format!("Unknown feed type: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    RssAtom,
    YouTube,
    Mastodon,
    WordPress,
    Blogger,
}

impl SourceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceType::RssAtom => "rss_atom",
            SourceType::YouTube => "youtube",
            SourceType::Mastodon => "mastodon",
            SourceType::WordPress => "wordpress",
            SourceType::Blogger => "blogger",
        }
    }
}

impl std::str::FromStr for SourceType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "rss_atom" | "rss" | "atom" => Ok(SourceType::RssAtom),
            "youtube" => Ok(SourceType::YouTube),
            "mastodon" => Ok(SourceType::Mastodon),
            "wordpress" => Ok(SourceType::WordPress),
            "blogger" => Ok(SourceType::Blogger),
            _ => Err(format!("Unknown source type: {}", s)),
        }
    }
}

impl std::fmt::Display for SourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feed {
    pub id: Option<i64>,
    pub url: String,
    pub feed_url: String,
    pub title: String,
    pub feed_type: FeedType,
    pub source_type: SourceType,
    pub created_at: Option<String>,
}

impl Feed {
    pub fn new(
        url: String,
        feed_url: String,
        title: String,
        feed_type: FeedType,
        source_type: SourceType,
    ) -> Self {
        Self {
            id: None,
            url,
            feed_url,
            title,
            feed_type,
            source_type,
            created_at: None,
        }
    }
}
