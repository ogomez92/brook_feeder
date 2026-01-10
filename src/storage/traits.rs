use crate::domain::Feed;
use crate::errors::FeederResult;

#[cfg_attr(test, mockall::automock)]
pub trait FeedRepository: Send + Sync {
    fn add(&self, feed: &Feed) -> FeederResult<i64>;
    fn remove(&self, id: i64) -> FeederResult<()>;
    fn get_all(&self) -> FeederResult<Vec<Feed>>;
    fn get_by_id(&self, id: i64) -> FeederResult<Option<Feed>>;
    fn get_by_url(&self, url: &str) -> FeederResult<Option<Feed>>;
    fn exists(&self, url: &str) -> FeederResult<bool>;
}

#[cfg_attr(test, mockall::automock)]
pub trait ArticleCacheRepository: Send + Sync {
    fn is_notified(&self, cache_key: &str) -> FeederResult<bool>;
    fn mark_notified(&self, cache_key: &str, feed_id: i64, title: &str) -> FeederResult<()>;
    fn get_unnotified(&self, cache_keys: &[String]) -> FeederResult<Vec<String>>;
}
