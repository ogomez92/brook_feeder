use serde::Deserialize;

use crate::domain::{parse_repo_input, TrackedRepo};
use crate::errors::{FeederError, FeederResult};
use crate::github::{GithubClient, OwnedRepo, RepoFetch};
use crate::storage::traits::{ReleaseCacheRepository, RepoRepository};

/// Result of importing repos from a Release Tracker JSON export.
pub struct ImportRepoResult {
    pub added: Vec<TrackedRepo>,
    pub duplicates: Vec<String>,
    pub invalid: Vec<(String, String)>, // (repo reference, reason)
}

/// Subset of the Release Tracker JSON export we care about: the tracked repos.
/// Releases/commits/pages in the file are ignored — this app tracks notified
/// state in its own database.
#[derive(Debug, Deserialize)]
struct ReleaseTrackerExport {
    #[serde(default)]
    repos: Vec<JsonRepo>,
}

#[derive(Debug, Deserialize)]
struct JsonRepo {
    owner: String,
    name: String,
    #[serde(default)]
    url: Option<String>,
}

pub struct ReleaseService<R: RepoRepository, C: ReleaseCacheRepository> {
    repo_repository: R,
    cache_repository: C,
    github: GithubClient,
}

impl<R: RepoRepository, C: ReleaseCacheRepository> ReleaseService<R, C> {
    pub fn new(repo_repository: R, cache_repository: C, github: GithubClient) -> Self {
        Self {
            repo_repository,
            cache_repository,
            github,
        }
    }

    /// Add a single repo from a URL or `owner/name` reference.
    pub fn add(&self, reference: &str) -> FeederResult<TrackedRepo> {
        let (owner, name) = parse_repo_input(reference).ok_or_else(|| {
            FeederError::InvalidInput(format!(
                "not a recognizable GitHub repo: '{}' (try owner/name or a github.com URL)",
                reference
            ))
        })?;

        if self.repo_repository.exists(&owner, &name)? {
            return Err(FeederError::FeedAlreadyExists(format!("{}/{}", owner, name)));
        }

        let repo = TrackedRepo::new(
            owner.clone(),
            name.clone(),
            format!("https://github.com/{}/{}", owner, name),
        );
        let id = self.repo_repository.add(&repo)?;

        Ok(TrackedRepo {
            id: Some(id),
            ..repo
        })
    }

    /// List every repo owned by `username` (a GitHub user or org) that we're not
    /// already tracking. A leading `@` is accepted and stripped. The returned
    /// repos keep GitHub's newest-pushed-first ordering.
    pub fn discover_untracked(&self, username: &str) -> FeederResult<Vec<OwnedRepo>> {
        let login = username.trim().trim_start_matches('@').trim();
        if login.is_empty() {
            return Err(FeederError::InvalidInput(
                "provide a GitHub username, e.g. `@torvalds`".to_string(),
            ));
        }

        let owned = self.github.list_owner_repos(login)?;

        let mut untracked = Vec::new();
        for repo in owned {
            if !self.repo_repository.exists(&repo.owner, &repo.name)? {
                untracked.push(repo);
            }
        }
        Ok(untracked)
    }

    /// Start tracking a repo discovered via [`discover_untracked`].
    pub fn add_owned(&self, repo: &OwnedRepo) -> FeederResult<TrackedRepo> {
        let tracked = TrackedRepo::new(repo.owner.clone(), repo.name.clone(), repo.url.clone());
        let id = self.repo_repository.add(&tracked)?;
        Ok(TrackedRepo {
            id: Some(id),
            ..tracked
        })
    }

    /// Import tracked repos from a Release Tracker JSON export string.
    /// Only the `repos` array is used; existing repos are reported as duplicates.
    pub fn import_json(&self, content: &str) -> FeederResult<ImportRepoResult> {
        let export: ReleaseTrackerExport = serde_json::from_str(content)
            .map_err(|e| FeederError::JsonParse(e.to_string()))?;

        let mut result = ImportRepoResult {
            added: Vec::new(),
            duplicates: Vec::new(),
            invalid: Vec::new(),
        };

        for entry in export.repos {
            let owner = entry.owner.trim().to_string();
            let name = entry.name.trim().to_string();
            let reference = format!("{}/{}", owner, name);

            if owner.is_empty() || name.is_empty() {
                result
                    .invalid
                    .push((reference, "missing owner or name".to_string()));
                continue;
            }

            match self.repo_repository.exists(&owner, &name) {
                Ok(true) => {
                    result.duplicates.push(reference);
                    continue;
                }
                Ok(false) => {}
                Err(e) => {
                    result.invalid.push((reference, e.to_string()));
                    continue;
                }
            }

            let url = entry
                .url
                .filter(|u| !u.trim().is_empty())
                .unwrap_or_else(|| format!("https://github.com/{}/{}", owner, name));

            let repo = TrackedRepo::new(owner, name, url);
            match self.repo_repository.add(&repo) {
                Ok(id) => result.added.push(TrackedRepo {
                    id: Some(id),
                    ..repo
                }),
                Err(FeederError::FeedAlreadyExists(_)) => result.duplicates.push(reference),
                Err(e) => result.invalid.push((reference, e.to_string())),
            }
        }

        Ok(result)
    }

    /// List all tracked repos.
    pub fn list(&self) -> FeederResult<Vec<TrackedRepo>> {
        self.repo_repository.get_all()
    }

    /// Remove a tracked repo by id.
    pub fn remove(&self, id: i64) -> FeederResult<()> {
        self.repo_repository.remove(id)
    }

    pub fn has_token(&self) -> bool {
        self.github.has_token()
    }

    /// Fetch the latest release/commit for every tracked repo (in bulk).
    pub fn fetch_all(
        &self,
        progress: impl FnMut(usize, usize),
    ) -> FeederResult<Vec<RepoFetch>> {
        let repos = self.repo_repository.get_all()?;
        self.github.fetch_updates(&repos, progress)
    }

    pub fn is_notified(&self, cache_key: &str) -> FeederResult<bool> {
        self.cache_repository.is_notified(cache_key)
    }

    pub fn mark_notified(&self, cache_key: &str, repo_id: i64, title: &str) -> FeederResult<()> {
        self.cache_repository.mark_notified(cache_key, repo_id, title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::{
        SqliteReleaseCacheRepository, SqliteRepoRepository, SqliteStorage,
    };

    fn setup() -> ReleaseService<SqliteRepoRepository, SqliteReleaseCacheRepository> {
        let storage = SqliteStorage::in_memory().unwrap();
        let repo_repo = SqliteRepoRepository::new(storage.clone());
        let cache = SqliteReleaseCacheRepository::new(storage);
        let github = GithubClient::new(None).unwrap();
        ReleaseService::new(repo_repo, cache, github)
    }

    #[test]
    fn test_add_individual() {
        let service = setup();
        let repo = service.add("https://github.com/sveltejs/kit").unwrap();
        assert_eq!(repo.full_name(), "sveltejs/kit");
        assert!(repo.id.is_some());

        // duplicate rejected
        let dup = service.add("sveltejs/kit");
        assert!(matches!(dup, Err(FeederError::FeedAlreadyExists(_))));
    }

    #[test]
    fn test_add_invalid() {
        let service = setup();
        let result = service.add("not a repo");
        assert!(matches!(result, Err(FeederError::InvalidInput(_))));
    }

    #[test]
    fn test_import_json() {
        let service = setup();
        let json = r#"{
            "repos": [
                {"owner": "sveltejs", "name": "kit", "url": "https://github.com/sveltejs/kit"},
                {"owner": "rust-lang", "name": "rust"},
                {"owner": "sveltejs", "name": "kit"}
            ],
            "releases": [{"tagName": "ignored"}],
            "pages": []
        }"#;

        let result = service.import_json(json).unwrap();
        assert_eq!(result.added.len(), 2);
        assert_eq!(result.duplicates.len(), 1); // second sveltejs/kit
        assert!(result.invalid.is_empty());

        // Re-importing is all duplicates
        let again = service.import_json(json).unwrap();
        assert_eq!(again.added.len(), 0);
        assert_eq!(again.duplicates.len(), 3);
    }

    #[test]
    fn test_import_json_missing_fields() {
        let service = setup();
        let json = r#"{"repos": [{"owner": "", "name": "x"}]}"#;
        let result = service.import_json(json).unwrap();
        assert_eq!(result.invalid.len(), 1);
    }
}
