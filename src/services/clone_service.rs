//! Clone or update tracked repos onto local disk.
//!
//! Mirrors the Release Tracker desktop app's "Update Repos" flow, but instead of
//! prompting for a folder every repo is materialized under `<base>/owner/name`
//! (the base being `FEEDER_REPOS_DIR`, `./repos` by default). A repo that
//! isn't on disk yet is cloned; one that already exists is fast-forwarded. These
//! checkouts are an archive of what upstream publishes, not a workspace, so a
//! working copy with uncommitted changes is reset to HEAD and its untracked
//! files removed rather than skipped — local edits are never the thing worth
//! keeping here, and skipping left repos frozen at whatever commit they were on
//! when something first dirtied them.
//!
//! Upstream history is followed wherever it goes: a repo whose author force
//! pushed can't be fast-forwarded, and for an archive the rewritten history *is*
//! the truth, so the checkout is reset onto it rather than left behind at a
//! commit that no longer exists upstream.
//!
//! Network git commands are retried when the failure reads like the link giving
//! out (`RPC failed`, `early EOF`, a reset connection) rather than an answer
//! from the server. Cloning a few hundred repos at once over a slow mount hits
//! these constantly, and a single dropped transfer shouldn't leave a repo
//! missing from the archive until someone notices and reruns the command.
//!
//! The GitHub token (when present) is injected into the URL only for the git
//! command itself — after cloning, the persisted `origin` remote is reset to the
//! clean URL so the token is never written to `.git/config`, and it is scrubbed
//! from any output shown to the user.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use crate::domain::TrackedRepo;

/// What happened when syncing a single repo.
#[derive(Debug)]
pub enum CloneOutcome {
    /// Freshly cloned (wasn't on disk before).
    Cloned,
    /// Existing checkout moved onto new commits. `discarded` counts the local
    /// modifications thrown away first, 0 when the checkout was clean;
    /// `rewound` marks a checkout that couldn't be fast-forwarded because
    /// upstream rewrote its history, and was reset onto it instead.
    Updated { discarded: usize, rewound: bool },
    /// Existing checkout already at the latest commit; `discarded` as above.
    UpToDate { discarded: usize },
    /// Clone/update failed; carries a user-facing (token-scrubbed) message.
    Failed(String),
}

/// Failures that mean the transfer broke rather than the server answered.
/// Matched case-insensitively against git's stderr.
const TRANSIENT_MARKERS: &[&str] = &[
    "rpc failed",
    "early eof",
    "unexpected disconnect",
    "connection reset",
    "broken pipe",
    "remote end hung up",
    "index-pack",
    "timed out",
    "transfer closed",
    "empty reply from server",
    "not closed cleanly",
    "could not resolve host",
    "failed to connect",
    "gnutls",
    "ssl_read",
    "the requested url returned error: 5",
];

/// How many times a network git command is attempted before giving up.
const ATTEMPTS: usize = 3;

/// Base wait between attempts; multiplied by the attempt number.
const BACKOFF: Duration = Duration::from_secs(3);

/// Clones/updates tracked repos under `base_dir/owner/name`.
pub struct CloneService {
    base_dir: PathBuf,
    token: Option<String>,
    attempts: usize,
    backoff: Duration,
    /// Transient failures retried across every repo synced through this
    /// service, so a run can say the link was flaky instead of hiding it.
    retried: AtomicUsize,
}

impl CloneService {
    pub fn new(base_dir: PathBuf, token: Option<String>) -> Self {
        Self {
            base_dir,
            token,
            attempts: ATTEMPTS,
            backoff: BACKOFF,
            retried: AtomicUsize::new(0),
        }
    }

    /// Override the retry policy (tests, which can't wait out a real backoff).
    #[cfg(test)]
    fn with_retries(mut self, attempts: usize, backoff: Duration) -> Self {
        self.attempts = attempts;
        self.backoff = backoff;
        self
    }

    /// How many transient network failures were retried so far.
    pub fn retries(&self) -> usize {
        self.retried.load(Ordering::Relaxed)
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

        // A clone that dies mid-transfer normally removes its own directory,
        // but not reliably over a network mount — and a leftover partial
        // checkout would make the retry fail for a different reason. Nothing
        // was at this path before (`sync` only calls `clone` when it's free),
        // so there is nothing here worth keeping.
        let cleanup = || {
            let _ = std::fs::remove_dir_all(path);
        };

        let safe = Self::safe_directory(&path_str);
        match self.git_networked(
            &[
                "-c",
                safe.as_str(),
                "clone",
                auth_url.as_str(),
                path_str.as_str(),
            ],
            &cleanup,
        ) {
            Ok(_) => {
                // Reset the remote to the clean URL so the token isn't persisted
                // in the cloned repo's .git/config. Best-effort: a clone with no
                // token used the clean URL already.
                let _ = self.git_in(
                    &path_str,
                    &["remote", "set-url", "origin", repo.url.as_str()],
                );
                CloneOutcome::Cloned
            }
            Err(e) => CloneOutcome::Failed(self.scrub(&e)),
        }
    }

    fn update(&self, repo: &TrackedRepo, path: &Path) -> CloneOutcome {
        let path_str = path.to_string_lossy().to_string();

        // Throw away local changes so the update can always move cleanly. The
        // count is reported rather than the paths: it's the signal that a
        // checkout had drifted, without burying the run's output.
        let discarded = match self.git_in(&path_str, &["status", "--porcelain"]) {
            Ok(out) => {
                let dirty = out.lines().filter(|l| !l.trim().is_empty()).count();
                if dirty > 0 {
                    if let Err(e) = self.discard_local_changes(&path_str) {
                        return CloneOutcome::Failed(self.scrub(&e));
                    }
                }
                dirty
            }
            Err(e) => return CloneOutcome::Failed(self.scrub(&e)),
        };

        // Fetch from an authenticated URL passed on the command line so private
        // repos work without persisting the token in the remote config. Fetch
        // rather than pull: what to do with what came down depends on how the
        // remote moved, and `pull` decides that for us.
        let auth_url = self.auth_url(repo);
        if let Err(e) = self.git_in_networked(&path_str, &["fetch", auth_url.as_str()]) {
            return CloneOutcome::Failed(self.scrub(&e));
        }

        let remote = match self.git_in(&path_str, &["rev-parse", "FETCH_HEAD"]) {
            Ok(sha) => sha.trim().to_string(),
            Err(e) => return CloneOutcome::Failed(self.scrub(&e)),
        };

        // An unborn HEAD (a checkout of an empty repo that has since gained
        // commits) has nothing to compare or fast-forward — reset onto the
        // fetched history like any other move that isn't a fast-forward.
        let head = self
            .git_in(&path_str, &["rev-parse", "HEAD"])
            .map(|sha| sha.trim().to_string());

        if head.as_deref() == Ok(remote.as_str()) {
            return CloneOutcome::UpToDate { discarded };
        }

        // Fast-forward when upstream simply moved on. When it didn't — the
        // author force pushed, so the local commits no longer exist upstream —
        // reset onto the fetched history: the archive follows upstream, and the
        // alternative is a checkout frozen at a commit nobody else has.
        let fast_forward = head.is_ok()
            && self
                .git_in(&path_str, &["merge-base", "--is-ancestor", "HEAD", "FETCH_HEAD"])
                .is_ok();

        let moved = if fast_forward {
            self.git_in(&path_str, &["merge", "--ff-only", "FETCH_HEAD"])
        } else {
            self.git_in(&path_str, &["reset", "--hard", "FETCH_HEAD"])
        };

        match moved {
            // An unborn HEAD had no history to rewind, however it got here.
            Ok(_) => CloneOutcome::Updated {
                discarded,
                rewound: head.is_ok() && !fast_forward,
            },
            Err(e) => CloneOutcome::Failed(self.scrub(&e)),
        }
    }

    /// Reset a dirty checkout back to HEAD: `reset --hard` for tracked edits,
    /// `clean -fd` for untracked files and directories. Ignored files are left
    /// alone (no `-x`) — they're what `.gitignore` says isn't part of the
    /// upstream tree, and they don't stand in the way of a fast-forward.
    fn discard_local_changes(&self, path: &str) -> Result<(), String> {
        self.git_in(path, &["reset", "--hard"])?;
        self.git_in(path, &["clean", "-fd"])?;
        Ok(())
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

    /// `-c safe.directory=<path>`, trusting exactly the checkout we manage.
    ///
    /// A checkout on a network mount — which `FEEDER_REPOS_DIR` may well point
    /// at — carries the remote account's uid, so git rejects every command in it
    /// as "dubious ownership" even though we put it there ourselves. Naming the
    /// one path keeps the check in force everywhere else on the machine.
    fn safe_directory(path: &str) -> String {
        format!("safe.directory={}", path)
    }

    /// Run a network `git` command inside the checkout at `path`, retrying a
    /// dropped transfer.
    fn git_in_networked(&self, path: &str, args: &[&str]) -> Result<String, String> {
        let full = self.in_repo_args(path, args);
        self.git_networked(&full, &|| {})
    }

    /// Run a network `git` command, retrying while the failure looks like the
    /// link giving out rather than an answer from the server. `on_retry` cleans
    /// up whatever the dead attempt left behind before the next one.
    fn git_networked<S: AsRef<OsStr>>(
        &self,
        args: &[S],
        on_retry: &dyn Fn(),
    ) -> Result<String, String> {
        for attempt in 1..=self.attempts {
            match self.git(args) {
                Ok(out) => return Ok(out),
                Err(e) if attempt < self.attempts && Self::is_transient(&e) => {
                    self.retried.fetch_add(1, Ordering::Relaxed);
                    // Back off further each time. With a dozen repos in flight
                    // these failures arrive in a clump, and retrying them all
                    // at once just recreates the pile-up that broke them.
                    thread::sleep(self.backoff * attempt as u32);
                    on_retry();
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!("the loop returns on the last attempt")
    }

    /// Whether a git failure is the transfer breaking (worth another go) rather
    /// than the server answering — a missing repo or a rejected token is the
    /// same answer however many times it's asked for.
    pub fn is_transient(err: &str) -> bool {
        let err = err.to_ascii_lowercase();
        TRANSIENT_MARKERS.iter().any(|marker| err.contains(marker))
    }

    /// `git` args that target the checkout at `path`, trusting it despite the
    /// ownership git sees on a network mount.
    fn in_repo_args(&self, path: &str, args: &[&str]) -> Vec<String> {
        let mut full = vec![
            "-c".to_string(),
            Self::safe_directory(path),
            "-C".to_string(),
            path.to_string(),
        ];
        full.extend(args.iter().map(|a| a.to_string()));
        full
    }

    /// Run `git` inside an existing checkout at `path`.
    fn git_in(&self, path: &str, args: &[&str]) -> Result<String, String> {
        let full = self.in_repo_args(path, args);
        self.git(&full)
    }

    /// Run `git` with the given args. Returns combined stdout+stderr on success,
    /// or a cleaned-up error message on failure. `GIT_TERMINAL_PROMPT=0` stops
    /// git from blocking on an interactive credential prompt.
    fn git<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<String, String> {
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
                format!(
                    "git {} failed",
                    args.first()
                        .map(|a| a.as_ref().to_string_lossy().into_owned())
                        .unwrap_or_default()
                )
            };
            Err(msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::TempDir;

    /// Run git in `dir` with an identity of its own, so the test doesn't depend
    /// on (or trip over) whatever is in the machine's global git config.
    fn git_at(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args([
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=test",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .output()
            .expect("git should be installed");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// An origin repo with one commit, plus a `CloneService` pointing at a
    /// fresh base directory.
    fn origin_and_service(tmp: &TempDir) -> (TrackedRepo, CloneService) {
        let origin = tmp.path().join("origin");
        fs::create_dir_all(&origin).unwrap();
        git_at(&origin, &["init"]);
        fs::write(origin.join("README.md"), "v1\n").unwrap();
        git_at(&origin, &["add", "."]);
        git_at(&origin, &["commit", "-m", "first"]);

        let repo = TrackedRepo::new(
            "owner".into(),
            "name".into(),
            format!("file://{}", origin.display()),
        );
        let service = CloneService::new(tmp.path().join("base"), None);
        (repo, service)
    }

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
    fn dirty_checkout_is_reset_instead_of_skipped() {
        let tmp = TempDir::new().unwrap();
        let (repo, service) = origin_and_service(&tmp);

        assert!(matches!(service.sync(&repo), CloneOutcome::Cloned));

        // Dirty the archive: an edited tracked file and an untracked stray.
        let checkout = service.repo_path(&repo);
        fs::write(checkout.join("README.md"), "local edit\n").unwrap();
        fs::write(checkout.join("stray.txt"), "junk\n").unwrap();

        match service.sync(&repo) {
            CloneOutcome::UpToDate { discarded } => assert_eq!(discarded, 2),
            other => panic!("expected an up-to-date sync, got {:?}", other),
        }

        assert_eq!(fs::read_to_string(checkout.join("README.md")).unwrap(), "v1\n");
        assert!(
            !checkout.join("stray.txt").exists(),
            "untracked files should be cleaned away"
        );
    }

    #[test]
    fn dirty_checkout_still_fast_forwards_to_new_commits() {
        let tmp = TempDir::new().unwrap();
        let (repo, service) = origin_and_service(&tmp);

        assert!(matches!(service.sync(&repo), CloneOutcome::Cloned));

        let checkout = service.repo_path(&repo);
        fs::write(checkout.join("README.md"), "local edit\n").unwrap();

        // Upstream moves on while the checkout is dirty — the whole reason
        // skipping was wrong: the archive would sit at the old commit forever.
        let origin = tmp.path().join("origin");
        fs::write(origin.join("README.md"), "v2\n").unwrap();
        git_at(&origin, &["commit", "-am", "second"]);

        match service.sync(&repo) {
            CloneOutcome::Updated { discarded, rewound } => {
                assert_eq!(discarded, 1);
                assert!(!rewound, "upstream moved on normally; nothing to rewind");
            }
            other => panic!("expected an updated sync, got {:?}", other),
        }
        assert_eq!(fs::read_to_string(checkout.join("README.md")).unwrap(), "v2\n");
    }

    #[test]
    fn clean_checkout_reports_nothing_discarded() {
        let tmp = TempDir::new().unwrap();
        let (repo, service) = origin_and_service(&tmp);

        assert!(matches!(service.sync(&repo), CloneOutcome::Cloned));
        match service.sync(&repo) {
            CloneOutcome::UpToDate { discarded } => assert_eq!(discarded, 0),
            other => panic!("expected an up-to-date sync, got {:?}", other),
        }
    }

    #[test]
    fn force_pushed_upstream_rewinds_the_checkout() {
        let tmp = TempDir::new().unwrap();
        let (repo, service) = origin_and_service(&tmp);

        assert!(matches!(service.sync(&repo), CloneOutcome::Cloned));
        let checkout = service.repo_path(&repo);
        let cloned_head = git_at(&checkout, &["rev-parse", "HEAD"]);

        // The author rewrites history: the commit we archived no longer exists
        // upstream, so there is no fast-forward to be had.
        let origin = tmp.path().join("origin");
        fs::write(origin.join("README.md"), "rewritten\n").unwrap();
        git_at(&origin, &["commit", "-a", "--amend", "-m", "rewritten history"]);

        match service.sync(&repo) {
            CloneOutcome::Updated { discarded, rewound } => {
                assert_eq!(discarded, 0);
                assert!(rewound, "a force push should be reported as a rewind");
            }
            other => panic!("expected a rewound sync, got {:?}", other),
        }

        assert_eq!(
            fs::read_to_string(checkout.join("README.md")).unwrap(),
            "rewritten\n"
        );
        assert_ne!(
            git_at(&checkout, &["rev-parse", "HEAD"]),
            cloned_head,
            "the checkout should have moved onto the rewritten history"
        );
    }

    #[test]
    fn dropped_transfers_are_retried_but_answers_are_not() {
        assert!(CloneService::is_transient(
            "error: RPC failed; curl 56 Recv failure: Connection reset by peer\n             fatal: fetch-pack: invalid index-pack output"
        ));
        assert!(CloneService::is_transient(
            "error: RPC failed; curl 92 HTTP/2 stream 5 was not closed cleanly: CANCEL (err 8)"
        ));
        assert!(CloneService::is_transient("fatal: early EOF"));

        // A deleted repo, a bad token or a wrong path answer the same way every
        // time — retrying only makes the run slower.
        assert!(!CloneService::is_transient(
            "remote: Repository not found.\nfatal: repository 'https://github.com/o/n/' not found"
        ));
        assert!(!CloneService::is_transient(
            "fatal: Authentication failed for 'https://github.com/o/n/'"
        ));
    }

    #[test]
    fn a_transient_clone_failure_is_retried_and_leaves_nothing_behind() {
        let tmp = TempDir::new().unwrap();
        // Port 1 refuses instantly, so the retries cost nothing but the loop.
        let repo = TrackedRepo::new(
            "owner".into(),
            "name".into(),
            "http://127.0.0.1:1/unreachable.git".into(),
        );
        let service = CloneService::new(tmp.path().join("base"), None)
            .with_retries(3, Duration::from_millis(0));

        match service.sync(&repo) {
            CloneOutcome::Failed(err) => assert!(
                err.contains("Failed to connect"),
                "unexpected error: {}",
                err
            ),
            other => panic!("expected a failure, got {:?}", other),
        }

        assert_eq!(service.retries(), 2, "3 attempts means 2 retries");
        assert!(
            !service.repo_path(&repo).exists(),
            "a failed clone should leave no partial checkout behind"
        );
    }

    #[test]
    fn an_answered_failure_is_not_retried() {
        let tmp = TempDir::new().unwrap();
        let repo = TrackedRepo::new(
            "owner".into(),
            "name".into(),
            format!("file://{}", tmp.path().join("nothing-here").display()),
        );
        let service = CloneService::new(tmp.path().join("base"), None)
            .with_retries(3, Duration::from_secs(30));

        assert!(matches!(service.sync(&repo), CloneOutcome::Failed(_)));
        assert_eq!(service.retries(), 0);
    }

    #[test]
    fn safe_directory_names_only_the_managed_checkout() {
        assert_eq!(
            CloneService::safe_directory("/mnt/storagebox/repos/sveltejs/kit"),
            "safe.directory=/mnt/storagebox/repos/sveltejs/kit"
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
