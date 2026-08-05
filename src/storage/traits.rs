use crate::domain::{ArtifactStatus, Feed, ModArtifact, ModArtifactRecord, TrackedRepo};
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

#[cfg_attr(test, mockall::automock)]
pub trait RepoRepository: Send + Sync {
    fn add(&self, repo: &TrackedRepo) -> FeederResult<i64>;
    fn remove(&self, id: i64) -> FeederResult<()>;
    fn get_all(&self) -> FeederResult<Vec<TrackedRepo>>;
    fn exists(&self, owner: &str, name: &str) -> FeederResult<bool>;
}

/// Dedup cache for repo releases/commits already notified.
#[cfg_attr(test, mockall::automock)]
pub trait ReleaseCacheRepository: Send + Sync {
    fn is_notified(&self, cache_key: &str) -> FeederResult<bool>;
    fn mark_notified(&self, cache_key: &str, repo_id: i64, title: &str) -> FeederResult<()>;
}

/// Inventory of mod-registry artifacts: doubles as the dedup cache (a known
/// `cache_key` is never announced twice) and as the record of what is mirrored
/// on disk.
///
/// Not `automock`ed: the borrowed `local_path` needs a named lifetime that
/// mockall can't infer, and the tests here run against real in-memory SQLite.
pub trait ModArtifactRepository: Send + Sync {
    fn get(&self, cache_key: &str) -> FeederResult<Option<ModArtifactRecord>>;
    fn record(
        &self,
        artifact: &ModArtifact,
        status: ArtifactStatus,
        local_path: Option<&str>,
    ) -> FeederResult<()>;
    fn get_all(&self) -> FeederResult<Vec<ModArtifactRecord>>;
}
