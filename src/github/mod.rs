//! Minimal GitHub GraphQL client for release tracking.
//!
//! Bulk-fetches the latest release and latest default-branch commit for many
//! repositories at once (batches of 30 aliased sub-queries), mirroring the
//! approach used by the Release Tracker desktop app. Using the GraphQL API with
//! a token means a single request covers up to 30 repos (1 rate-limit point per
//! request) and private repos are accessible.

use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::Value;

use crate::domain::{RepoCommit, RepoRelease, RepoUpdate, TrackedRepo};
use crate::errors::{FeederError, FeederResult};

const GRAPHQL_URL: &str = "https://api.github.com/graphql";
const BATCH_SIZE: usize = 30;
/// Per-request timeout so a hung connection can't stall a silent scheduled run.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
/// Retries per batch on transient failures (network blips, GitHub 502s).
const BATCH_ATTEMPTS: u32 = 3;

/// The result of fetching updates for a single repo.
pub struct RepoFetch {
    pub repo: TrackedRepo,
    pub update: RepoUpdate,
    /// Set when the repo could not be read (not found, renamed, no access...).
    pub error: Option<String>,
}

/// A repository owned by a user or organization, as returned when listing an
/// owner's repos (used to discover repos to start tracking).
pub struct OwnedRepo {
    pub owner: String,
    pub name: String,
    pub url: String,
    pub description: Option<String>,
    pub stargazers: u64,
    pub is_fork: bool,
    pub is_archived: bool,
}

impl OwnedRepo {
    /// `owner/name`, e.g. `sveltejs/kit`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// Lists every repository owned by a user or org, newest-pushed first. Uses
/// `repositoryOwner` so it works for both users and organizations, and
/// `ownerAffiliations: OWNER` so only their own repos come back (not ones they
/// merely contribute to). Paginated 100 at a time.
const OWNER_REPOS_QUERY: &str = r#"
query($login: String!, $cursor: String) {
  repositoryOwner(login: $login) {
    repositories(
      first: 100
      after: $cursor
      ownerAffiliations: OWNER
      orderBy: { field: PUSHED_AT, direction: DESC }
    ) {
      pageInfo { hasNextPage endCursor }
      nodes {
        name
        url
        description
        stargazerCount
        isFork
        isArchived
        owner { login }
      }
    }
  }
}
"#;

pub struct GithubClient {
    client: Client,
    token: Option<String>,
}

impl GithubClient {
    pub fn new(token: Option<String>) -> FeederResult<Self> {
        let client = Client::builder()
            .user_agent("feeder-release-tracker")
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self { client, token })
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    /// Fetch updates for every repo, batching requests to keep queries small.
    /// `progress` is called once per completed batch with (repos_done, total).
    pub fn fetch_updates(
        &self,
        repos: &[TrackedRepo],
        mut progress: impl FnMut(usize, usize),
    ) -> FeederResult<Vec<RepoFetch>> {
        let total = repos.len();
        let mut out = Vec::with_capacity(total);

        for batch in repos.chunks(BATCH_SIZE) {
            match self.fetch_batch_with_retry(batch) {
                Ok(fetched) => out.extend(fetched),
                Err(e) => {
                    // A whole-batch failure (network/HTTP/rate) must not abort a
                    // silent run: mark every repo in the batch as errored and
                    // keep going so the rest still get checked.
                    let msg = e.to_string();
                    for repo in batch {
                        out.push(RepoFetch {
                            repo: repo.clone(),
                            update: RepoUpdate::None,
                            error: Some(msg.clone()),
                        });
                    }
                }
            }
            progress(out.len().min(total), total);
        }

        Ok(out)
    }

    /// Fetch a batch, retrying a few times on transient failures.
    fn fetch_batch_with_retry(&self, batch: &[TrackedRepo]) -> FeederResult<Vec<RepoFetch>> {
        let mut last_err = None;
        for attempt in 1..=BATCH_ATTEMPTS {
            match self.fetch_batch(batch) {
                Ok(fetched) => return Ok(fetched),
                Err(e) => {
                    if attempt < BATCH_ATTEMPTS {
                        // Linear backoff (2s, 4s, ...) for blips and GitHub 502s.
                        std::thread::sleep(Duration::from_secs(2 * attempt as u64));
                    }
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| FeederError::Github("unknown error".to_string())))
    }

    /// List every repository owned by `login` (a GitHub user or organization),
    /// paginating until GitHub reports no more pages. Returns an error if the
    /// login doesn't exist.
    pub fn list_owner_repos(&self, login: &str) -> FeederResult<Vec<OwnedRepo>> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let value = self.send_graphql(serde_json::json!({
                "query": OWNER_REPOS_QUERY,
                "variables": { "login": login, "cursor": cursor },
            }))?;

            // A missing/null `repositoryOwner` means either the login doesn't
            // exist or the whole query errored — surface the GraphQL message if
            // there is one, otherwise report it as an unknown owner.
            let owner = value.get("data").and_then(|d| d.get("repositoryOwner"));
            let owner = match owner {
                Some(o) if !o.is_null() => o,
                _ => {
                    return Err(FeederError::Github(first_error_message(&value).unwrap_or_else(
                        || format!("no GitHub user or organization named '{}'", login),
                    )));
                }
            };

            let (page, next) = parse_owned_repos_page(owner);
            all.extend(page);
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }

        Ok(all)
    }

    /// POST a GraphQL request body, returning the parsed JSON response. Handles
    /// auth headers and maps non-2xx statuses to a `Github` error.
    fn send_graphql(&self, body: Value) -> FeederResult<Value> {
        let mut request = self.client.post(GRAPHQL_URL).json(&body);

        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("bearer {}", token));
        }

        let response = request.send()?;
        let status = response.status();

        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            let msg = match status.as_u16() {
                401 => "authentication failed (check GITHUB_TOKEN / gh login)".to_string(),
                403 => format!("rate limit or access denied: {}", body.trim()),
                _ => format!("HTTP {}: {}", status, body.trim()),
            };
            return Err(FeederError::Github(msg));
        }

        Ok(response.json()?)
    }

    fn fetch_batch(&self, batch: &[TrackedRepo]) -> FeederResult<Vec<RepoFetch>> {
        let query = build_query(batch);
        let value = self.send_graphql(serde_json::json!({ "query": query }))?;

        // GraphQL returns partial `data` even when some aliases error (e.g. a
        // repo was renamed/deleted). Only bail out if there's no data at all.
        let data = value.get("data");
        if data.is_none() || data == Some(&Value::Null) {
            let msg = first_error_message(&value).unwrap_or_else(|| "empty response".to_string());
            return Err(FeederError::Github(msg));
        }
        let data = data.unwrap();

        let mut result = Vec::with_capacity(batch.len());
        for (i, repo) in batch.iter().enumerate() {
            let repo_data = data.get(format!("repo{}", i));
            match repo_data {
                Some(rd) if !rd.is_null() => result.push(RepoFetch {
                    repo: repo.clone(),
                    update: parse_update(rd),
                    error: None,
                }),
                _ => result.push(RepoFetch {
                    repo: repo.clone(),
                    update: RepoUpdate::None,
                    error: Some("not found or inaccessible".to_string()),
                }),
            }
        }

        Ok(result)
    }
}

/// Build a single GraphQL document with one aliased `repository(...)` per repo.
fn build_query(batch: &[TrackedRepo]) -> String {
    let mut body = String::new();
    for (i, repo) in batch.iter().enumerate() {
        body.push_str(&format!(
            r#"
  repo{i}: repository(owner: {owner}, name: {name}) {{
    latestRelease {{ tagName name publishedAt url description }}
    defaultBranchRef {{
      target {{
        ... on Commit {{ oid message committedDate author {{ name }} url }}
      }}
    }}
  }}"#,
            i = i,
            owner = json_string(&repo.owner),
            name = json_string(&repo.name),
        ));
    }
    format!("query {{{}\n}}", body)
}

/// Encode a value as a JSON/GraphQL string literal (quoted, escaped).
fn json_string(s: &str) -> String {
    Value::String(s.to_string()).to_string()
}

/// Turn one repo's GraphQL payload into a `RepoUpdate`: prefer the latest
/// release, otherwise fall back to the latest commit.
fn parse_update(rd: &Value) -> RepoUpdate {
    if let Some(rel) = rd.get("latestRelease").filter(|v| !v.is_null()) {
        let tag = str_field(rel, "tagName");
        let name = {
            let n = str_field(rel, "name");
            if n.is_empty() {
                tag.clone()
            } else {
                n
            }
        };
        let url = str_field(rel, "url");
        if !tag.is_empty() || !url.is_empty() {
            return RepoUpdate::Release(RepoRelease {
                tag_name: tag,
                name,
                published_at: opt_str_field(rel, "publishedAt"),
                html_url: url,
                body: str_field(rel, "description"),
            });
        }
    }

    if let Some(target) = rd
        .get("defaultBranchRef")
        .and_then(|d| d.get("target"))
        .filter(|v| !v.is_null())
    {
        let sha = str_field(target, "oid");
        if !sha.is_empty() {
            let author = target
                .get("author")
                .map(|a| str_field(a, "name"))
                .unwrap_or_default();
            return RepoUpdate::Commit(RepoCommit {
                sha,
                message: str_field(target, "message"),
                date: opt_str_field(target, "committedDate"),
                author,
                html_url: str_field(target, "url"),
            });
        }
    }

    RepoUpdate::None
}

/// Parse one page of an owner's `repositories` connection into `OwnedRepo`s,
/// returning them plus the cursor for the next page (`None` when there is none).
fn parse_owned_repos_page(owner: &Value) -> (Vec<OwnedRepo>, Option<String>) {
    let connection = match owner.get("repositories") {
        Some(c) => c,
        None => return (Vec::new(), None),
    };

    let mut repos = Vec::new();
    if let Some(nodes) = connection.get("nodes").and_then(|n| n.as_array()) {
        for node in nodes {
            let name = str_field(node, "name");
            let owner_login = node
                .get("owner")
                .map(|o| str_field(o, "login"))
                .unwrap_or_default();
            if name.is_empty() || owner_login.is_empty() {
                continue;
            }
            repos.push(OwnedRepo {
                owner: owner_login,
                name,
                url: str_field(node, "url"),
                description: opt_str_field(node, "description").filter(|s| !s.is_empty()),
                stargazers: node
                    .get("stargazerCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                is_fork: node.get("isFork").and_then(|v| v.as_bool()).unwrap_or(false),
                is_archived: node
                    .get("isArchived")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            });
        }
    }

    let page_info = connection.get("pageInfo");
    let has_next = page_info
        .and_then(|p| p.get("hasNextPage"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let next = if has_next {
        page_info
            .and_then(|p| p.get("endCursor"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    };

    (repos, next)
}

/// First `errors[].message` from a GraphQL response, if any.
fn first_error_message(value: &Value) -> Option<String> {
    value
        .get("errors")
        .and_then(|e| e.as_array())
        .and_then(|a| a.first())
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn opt_str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_query_escapes_and_aliases() {
        let repos = vec![
            TrackedRepo::new("a".into(), "b".into(), "u".into()),
            TrackedRepo::new("c".into(), "d".into(), "u".into()),
        ];
        let q = build_query(&repos);
        assert!(q.contains("repo0: repository(owner: \"a\", name: \"b\")"));
        assert!(q.contains("repo1: repository(owner: \"c\", name: \"d\")"));
        assert!(q.contains("latestRelease"));
        assert!(q.contains("defaultBranchRef"));
    }

    #[test]
    fn test_parse_release() {
        let rd: Value = serde_json::json!({
            "latestRelease": {
                "tagName": "v1.2.3",
                "name": "Version 1.2.3",
                "publishedAt": "2024-01-01T00:00:00Z",
                "url": "https://github.com/a/b/releases/tag/v1.2.3",
                "description": "notes"
            },
            "defaultBranchRef": { "target": { "oid": "deadbeef" } }
        });
        match parse_update(&rd) {
            RepoUpdate::Release(r) => {
                assert_eq!(r.tag_name, "v1.2.3");
                assert_eq!(r.name, "Version 1.2.3");
                assert_eq!(r.html_url, "https://github.com/a/b/releases/tag/v1.2.3");
            }
            _ => panic!("expected release"),
        }
    }

    #[test]
    fn test_parse_commit_fallback() {
        let rd: Value = serde_json::json!({
            "latestRelease": null,
            "defaultBranchRef": {
                "target": {
                    "oid": "abc123",
                    "message": "fix things\n\nbody",
                    "committedDate": "2024-02-02T00:00:00Z",
                    "author": { "name": "Jane" },
                    "url": "https://github.com/a/b/commit/abc123"
                }
            }
        });
        match parse_update(&rd) {
            RepoUpdate::Commit(c) => {
                assert_eq!(c.sha, "abc123");
                assert_eq!(c.author, "Jane");
                assert_eq!(c.html_url, "https://github.com/a/b/commit/abc123");
            }
            _ => panic!("expected commit"),
        }
    }

    #[test]
    fn test_parse_none() {
        let rd: Value = serde_json::json!({ "latestRelease": null, "defaultBranchRef": null });
        assert!(matches!(parse_update(&rd), RepoUpdate::None));
    }

    #[test]
    fn test_parse_owned_repos_page() {
        let owner: Value = serde_json::json!({
            "repositories": {
                "pageInfo": { "hasNextPage": true, "endCursor": "CUR2" },
                "nodes": [
                    {
                        "name": "kit",
                        "url": "https://github.com/sveltejs/kit",
                        "description": "web framework",
                        "stargazerCount": 100,
                        "isFork": false,
                        "isArchived": false,
                        "owner": { "login": "sveltejs" }
                    },
                    {
                        "name": "svelte",
                        "url": "https://github.com/sveltejs/svelte",
                        "description": null,
                        "stargazerCount": 200,
                        "isFork": false,
                        "isArchived": true,
                        "owner": { "login": "sveltejs" }
                    }
                ]
            }
        });

        let (repos, next) = parse_owned_repos_page(&owner);
        assert_eq!(next.as_deref(), Some("CUR2"));
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].full_name(), "sveltejs/kit");
        assert_eq!(repos[0].description.as_deref(), Some("web framework"));
        assert_eq!(repos[0].stargazers, 100);
        assert_eq!(repos[1].description, None);
        assert!(repos[1].is_archived);
    }

    #[test]
    fn test_parse_owned_repos_page_last() {
        let owner: Value = serde_json::json!({
            "repositories": {
                "pageInfo": { "hasNextPage": false, "endCursor": "CUR" },
                "nodes": []
            }
        });
        let (repos, next) = parse_owned_repos_page(&owner);
        assert!(repos.is_empty());
        assert_eq!(next, None);
    }

    #[test]
    fn test_release_name_falls_back_to_tag() {
        let rd: Value = serde_json::json!({
            "latestRelease": { "tagName": "v9", "name": "", "url": "u" },
            "defaultBranchRef": null
        });
        match parse_update(&rd) {
            RepoUpdate::Release(r) => assert_eq!(r.name, "v9"),
            _ => panic!("expected release"),
        }
    }
}
