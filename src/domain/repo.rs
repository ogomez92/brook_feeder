/// A GitHub repository we track for new releases / commits.
#[derive(Debug, Clone)]
pub struct TrackedRepo {
    pub id: Option<i64>,
    pub owner: String,
    pub name: String,
    pub url: String,
    pub added_at: Option<String>,
}

impl TrackedRepo {
    pub fn new(owner: String, name: String, url: String) -> Self {
        Self {
            id: None,
            owner,
            name,
            url,
            added_at: None,
        }
    }

    /// `owner/name`, e.g. `sveltejs/kit`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// The latest published release of a repository.
#[derive(Debug, Clone)]
pub struct RepoRelease {
    pub tag_name: String,
    pub name: String,
    pub published_at: Option<String>,
    pub html_url: String,
    pub body: String,
}

/// The latest commit on a repository's default branch.
#[derive(Debug, Clone)]
pub struct RepoCommit {
    pub sha: String,
    pub message: String,
    pub date: Option<String>,
    pub author: String,
    pub html_url: String,
}

/// What we found for a repo: a release if it has one, otherwise its latest
/// commit, otherwise nothing.
#[derive(Debug, Clone)]
pub enum RepoUpdate {
    Release(RepoRelease),
    Commit(RepoCommit),
    None,
}

impl RepoUpdate {
    /// Stable dedup key for the update, scoped to the repo.
    /// A release is keyed by its tag, a commit by its sha, so a notification is
    /// only sent again when the tag or the head commit actually changes.
    pub fn cache_key(&self, repo: &TrackedRepo) -> Option<String> {
        match self {
            RepoUpdate::Release(r) => Some(format!("{}:release:{}", repo.full_name(), r.tag_name)),
            RepoUpdate::Commit(c) => Some(format!("{}:commit:{}", repo.full_name(), c.sha)),
            RepoUpdate::None => None,
        }
    }
}

/// Parse a user-supplied repository reference into `(owner, name)`.
///
/// Accepts the shapes a GitHub repo can be pasted in:
///   owner/repo
///   https://github.com/owner/repo(/anything)(.git)
///   github.com/owner/repo
///   git@github.com:owner/repo.git
pub fn parse_repo_input(raw: &str) -> Option<(String, String)> {
    let input = raw.trim();
    if input.is_empty() {
        return None;
    }

    // SSH / scp-like form: git@github.com:owner/repo(.git)
    if let Some(rest) = input.strip_prefix("git@github.com:") {
        let mut parts = rest.split('/');
        let owner = parts.next()?;
        let name = parts.next()?;
        return clean(owner, name);
    }

    // Anything referencing github.com -> read the first two path segments.
    let lower = input.to_lowercase();
    if lower.contains("github.com") {
        let after = input
            .split_once("github.com")
            .map(|(_, rest)| rest)
            .unwrap_or(input);
        // Strip a leading separator (":" for scp form already handled, "/" for URLs)
        let after = after.trim_start_matches(['/', ':']);
        let segments: Vec<&str> = after.split('/').filter(|s| !s.is_empty()).collect();
        if segments.len() >= 2 {
            return clean(segments[0], segments[1]);
        }
        return None;
    }

    // Plain "owner/repo" (ignore any trailing path the user pasted).
    let trimmed = input.trim_matches('/');
    let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    if segments.len() >= 2 {
        return clean(segments[0], segments[1]);
    }

    None
}

fn clean(owner: &str, name: &str) -> Option<(String, String)> {
    let owner = owner.trim();
    let name = name.trim().trim_end_matches(".git");

    if owner.is_empty() || name.is_empty() {
        return None;
    }

    let valid = |s: &str| {
        s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    };
    if !valid(owner) || !valid(name) {
        return None;
    }

    Some((owner.to_string(), name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_plain() {
        assert_eq!(
            parse_repo_input("sveltejs/kit"),
            Some(("sveltejs".to_string(), "kit".to_string()))
        );
    }

    #[test]
    fn test_parse_https_url() {
        assert_eq!(
            parse_repo_input("https://github.com/sveltejs/kit"),
            Some(("sveltejs".to_string(), "kit".to_string()))
        );
    }

    #[test]
    fn test_parse_url_with_extra_path() {
        assert_eq!(
            parse_repo_input("https://github.com/sveltejs/kit/releases/tag/v1.2.3"),
            Some(("sveltejs".to_string(), "kit".to_string()))
        );
    }

    #[test]
    fn test_parse_url_dot_git() {
        assert_eq!(
            parse_repo_input("https://github.com/sveltejs/kit.git"),
            Some(("sveltejs".to_string(), "kit".to_string()))
        );
    }

    #[test]
    fn test_parse_ssh() {
        assert_eq!(
            parse_repo_input("git@github.com:sveltejs/kit.git"),
            Some(("sveltejs".to_string(), "kit".to_string()))
        );
    }

    #[test]
    fn test_parse_bare_host() {
        assert_eq!(
            parse_repo_input("github.com/owner/repo"),
            Some(("owner".to_string(), "repo".to_string()))
        );
    }

    #[test]
    fn test_parse_invalid() {
        assert_eq!(parse_repo_input(""), None);
        assert_eq!(parse_repo_input("justonesegment"), None);
        assert_eq!(parse_repo_input("https://github.com/onlyowner"), None);
    }

    #[test]
    fn test_cache_key() {
        let repo = TrackedRepo::new("a".into(), "b".into(), "https://github.com/a/b".into());
        let rel = RepoUpdate::Release(RepoRelease {
            tag_name: "v1.0".into(),
            name: "v1.0".into(),
            published_at: None,
            html_url: "u".into(),
            body: String::new(),
        });
        assert_eq!(rel.cache_key(&repo).unwrap(), "a/b:release:v1.0");

        let commit = RepoUpdate::Commit(RepoCommit {
            sha: "abc123".into(),
            message: "m".into(),
            date: None,
            author: "x".into(),
            html_url: "u".into(),
        });
        assert_eq!(commit.cache_key(&repo).unwrap(), "a/b:commit:abc123");
    }
}
