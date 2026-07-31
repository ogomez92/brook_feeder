//! Clone or update tracked repos onto local disk.
//!
//! Mirrors the Release Tracker desktop app's "Update Repos" flow, but instead of
//! prompting for a folder every repo is materialized under `<base>/owner/name`
//! (the base being `./repos` relative to the current directory). A repo that
//! isn't on disk yet is cloned; one that already exists is fast-forwarded, and a
//! working copy with uncommitted changes is left untouched.
//!
//! The GitHub token (when present) is injected into the URL only for the git
//! command itself — after cloning, the persisted `origin` remote is reset to the
//! clean URL so the token is never written to `.git/config`, and it is scrubbed
//! from any output shown to the user.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::domain::TrackedRepo;

/// What happened when syncing a single repo.
pub enum CloneOutcome {
    /// Freshly cloned (wasn't on disk before).
    Cloned,
    /// Existing checkout fast-forwarded to new commits.
    Updated,
    /// Existing checkout already at the latest commit.
    UpToDate,
    /// Left alone because the working copy has uncommitted changes.
    SkippedDirty,
    /// Clone/update failed; carries a user-facing (token-scrubbed) message.
    Failed(String),
}

/// Clones/updates tracked repos under `base_dir/owner/name`.
pub struct CloneService {
    base_dir: PathBuf,
    token: Option<String>,
}

impl CloneService {
    pub fn new(base_dir: PathBuf, token: Option<String>) -> Self {
        Self { base_dir, token }
    }

    /// Where a repo lives on disk: `<base_dir>/owner/name`.
    pub fn repo_path(&self, repo: &TrackedRepo) -> PathBuf {
        self.base_dir.join(&repo.owner).join(&repo.name)
    }

    /// Clone the repo if it isn't on disk, otherwise pull the latest changes.
    pub fn sync(&self, repo: &TrackedRepo) -> CloneOutcome {
        let path = self.repo_path(repo);

        if path.join(".git").is_dir() {
            self.update(repo, &path)
        } else if path.exists() {
            // Something is already there but it isn't a git checkout — refuse to
            // touch it rather than clobber whatever the user has in that folder.
            CloneOutcome::Failed(format!(
                "{} already exists and is not a git repository",
                path.display()
            ))
        } else {
            self.clone(repo, &path)
        }
    }

    fn clone(&self, repo: &TrackedRepo, path: &Path) -> CloneOutcome {
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return CloneOutcome::Failed(format!(
                    "could not create {}: {}",
                    parent.display(),
                    e
                ));
            }
        }

        let auth_url = self.auth_url(repo);
        let path_str = path.to_string_lossy().to_string();

        match self.git(&["clone", auth_url.as_str(), path_str.as_str()]) {
            Ok(_) => {
                // Reset the remote to the clean URL so the token isn't persisted
                // in the cloned repo's .git/config. Best-effort: a clone with no
                // token used the clean URL already.
                let _ = self.git(&[
                    "-C",
                    path_str.as_str(),
                    "remote",
                    "set-url",
                    "origin",
                    repo.url.as_str(),
                ]);
                CloneOutcome::Cloned
            }
            Err(e) => CloneOutcome::Failed(self.scrub(&e)),
        }
    }

    fn update(&self, repo: &TrackedRepo, path: &Path) -> CloneOutcome {
        let path_str = path.to_string_lossy().to_string();

        // Skip repos with uncommitted changes rather than risk losing work.
        match self.git(&["-C", path_str.as_str(), "status", "--porcelain"]) {
            Ok(out) if !out.trim().is_empty() => return CloneOutcome::SkippedDirty,
            Ok(_) => {}
            Err(e) => return CloneOutcome::Failed(self.scrub(&e)),
        }

        // Pull from an authenticated URL passed on the command line so private
        // repos work without persisting the token in the remote config.
        // --ff-only keeps us from creating merge commits: if the local branch
        // has diverged the pull fails loudly instead of making a mess.
        let auth_url = self.auth_url(repo);
        match self.git(&["-C", path_str.as_str(), "pull", "--ff-only", auth_url.as_str()]) {
            Ok(out) => {
                if out.contains("Already up to date") || out.contains("Already up-to-date") {
                    CloneOutcome::UpToDate
                } else {
                    CloneOutcome::Updated
                }
            }
            Err(e) => CloneOutcome::Failed(self.scrub(&e)),
        }
    }

    /// Inject the token into an `https://github.com` URL for authenticated access
    /// (needed for private repos). Non-github or tokenless URLs are used as-is.
    fn auth_url(&self, repo: &TrackedRepo) -> String {
        match &self.token {
            Some(token) if repo.url.starts_with("https://github.com") => repo.url.replacen(
                "https://",
                &format!("https://x-access-token:{}@", token),
                1,
            ),
            _ => repo.url.clone(),
        }
    }

    /// Remove the token from text before it's shown to the user.
    fn scrub(&self, text: &str) -> String {
        match &self.token {
            Some(token) if !token.is_empty() => text.replace(token.as_str(), "***"),
            _ => text.to_string(),
        }
    }

    /// Run `git` with the given args. Returns combined stdout+stderr on success,
    /// or a cleaned-up error message on failure. `GIT_TERMINAL_PROMPT=0` stops
    /// git from blocking on an interactive credential prompt.
    fn git(&self, args: &[&str]) -> Result<String, String> {
        let output = Command::new("git")
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map_err(|e| format!("failed to run git (is it installed?): {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            Ok(format!("{}{}", stdout, stderr))
        } else {
            let msg = if !stderr.trim().is_empty() {
                stderr.trim().to_string()
            } else if !stdout.trim().is_empty() {
                stdout.trim().to_string()
            } else {
                format!("git {} failed", args.first().copied().unwrap_or_default())
            };
            Err(msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> TrackedRepo {
        TrackedRepo::new(
            "sveltejs".into(),
            "kit".into(),
            "https://github.com/sveltejs/kit".into(),
        )
    }

    #[test]
    fn repo_path_is_base_owner_name() {
        let svc = CloneService::new(PathBuf::from("/tmp/repos"), None);
        assert_eq!(
            svc.repo_path(&repo()),
            PathBuf::from("/tmp/repos/sveltejs/kit")
        );
    }

    #[test]
    fn auth_url_injects_token_for_github() {
        let svc = CloneService::new(PathBuf::from("."), Some("ghp_secret".into()));
        assert_eq!(
            svc.auth_url(&repo()),
            "https://x-access-token:ghp_secret@github.com/sveltejs/kit"
        );
    }

    #[test]
    fn auth_url_untouched_without_token() {
        let svc = CloneService::new(PathBuf::from("."), None);
        assert_eq!(svc.auth_url(&repo()), "https://github.com/sveltejs/kit");
    }

    #[test]
    fn auth_url_ignores_non_github_hosts() {
        let svc = CloneService::new(PathBuf::from("."), Some("ghp_secret".into()));
        let gitlab = TrackedRepo::new(
            "group".into(),
            "proj".into(),
            "https://gitlab.com/group/proj".into(),
        );
        assert_eq!(svc.auth_url(&gitlab), "https://gitlab.com/group/proj");
    }

    #[test]
    fn scrub_redacts_token() {
        let svc = CloneService::new(PathBuf::from("."), Some("ghp_secret".into()));
        assert_eq!(
            svc.scrub("fatal: could not read from https://x-access-token:ghp_secret@github.com"),
            "fatal: could not read from https://x-access-token:***@github.com"
        );
    }
}
