use crate::errors::{FeederError, FeederResult};

#[derive(Debug, Clone)]
pub struct Config {
    pub notebrook_url: String,
    pub notebrook_token: String,
    pub notebrook_channel: String,
    pub db_path: String,
    /// GitHub token used for release tracking (private repos + higher rate limits)
    pub github_token: Option<String>,
    /// Where `mods run` mirrors registry metadata and downloaded artifacts
    pub mod_mirror_dir: String,
    /// Where `getrepos` clones tracked repositories (`owner/name` beneath it)
    pub repos_dir: String,
    /// How many repos `getrepos` syncs at once (`--jobs` overrides it)
    pub repos_jobs: usize,
    /// Whether `feeder run` also mirrors the mod registry. Feeds and releases
    /// are driven by database contents, so an empty database makes them
    /// no-ops; the registry is a fixed remote, so without this switch a bare
    /// `run` always reaches the network and downloads. Set `FEEDER_MODS=0` to
    /// turn it off (`feeder mods run` still works — it was asked for
    /// explicitly).
    pub mods_enabled: bool,
}

impl Config {
    /// Get the directory where the executable is located
    fn exe_dir() -> Option<std::path::PathBuf> {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
    }

    pub fn from_env() -> FeederResult<Self> {
        let exe_dir = Self::exe_dir();

        // Try to load .env from executable's directory first
        if let Some(ref dir) = exe_dir {
            let env_path = dir.join(".env");
            if env_path.exists() {
                dotenvy::from_path(&env_path).ok();
            }
        }
        // Fall back to current directory
        dotenvy::dotenv().ok();

        let notebrook_url = std::env::var("NOTEBROOK_URL")
            .map_err(|_| FeederError::MissingEnvVar("NOTEBROOK_URL".to_string()))?;

        let notebrook_token = std::env::var("NOTEBROOK_TOKEN")
            .map_err(|_| FeederError::MissingEnvVar("NOTEBROOK_TOKEN".to_string()))?;

        let notebrook_channel = std::env::var("NOTEBROOK_CHANNEL")
            .unwrap_or_else(|_| "feeds".to_string());

        let db_path = std::env::var("FEEDER_DB_PATH")
            .unwrap_or_else(|_| Self::default_db_path(exe_dir.as_deref()));

        let github_token = Self::resolve_github_token();

        // Relative by default, like the database and `getrepos`, so it lands in
        // the systemd unit's WorkingDirectory rather than next to the binary.
        let mod_mirror_dir = std::env::var("FEEDER_MOD_MIRROR")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "./modmirror".to_string());

        // Same shape as the mirror directory: relative by default so it lands in
        // the systemd unit's WorkingDirectory, absolute when pointed elsewhere
        // (e.g. a mounted storage box).
        let repos_dir = std::env::var("FEEDER_REPOS_DIR")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "./repos".to_string());

        let repos_jobs = Self::parse_jobs(std::env::var("FEEDER_REPOS_JOBS").ok().as_deref());

        let mods_enabled = std::env::var("FEEDER_MODS")
            .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
            .unwrap_or(true);

        Ok(Self {
            notebrook_url,
            notebrook_token,
            notebrook_channel,
            db_path,
            github_token,
            mod_mirror_dir,
            repos_dir,
            repos_jobs,
            mods_enabled,
        })
    }

    /// How many repos `getrepos` syncs concurrently.
    ///
    /// Syncing is latency-bound, not CPU- or bandwidth-bound: a `git status`
    /// against a checkout on a network mount spends its time waiting on
    /// per-file round trips, so overlapping repos is what makes a full run
    /// finish in minutes instead of hours. 16 is a safe default even for a local
    /// directory, where the work is dominated by GitHub round trips anyway.
    /// Garbage and 0 fall back to the default rather than serializing the run
    /// or spawning nothing.
    fn parse_jobs(raw: Option<&str>) -> usize {
        const DEFAULT_JOBS: usize = 16;

        raw.map(str::trim)
            .filter(|v| !v.is_empty())
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_JOBS)
    }

    /// Locate the database when `FEEDER_DB_PATH` is unset.
    ///
    /// Prefers an existing `feeder.db` in the current directory — that is what the
    /// docs promise and what the systemd unit's `WorkingDirectory` points at. Only
    /// falls back to one sitting next to the executable, and never *creates* one
    /// there: `current_exe()` resolves symlinks, so whenever the binary is reached
    /// through one the exe directory is a build artifact directory. Creating the
    /// database there silently strands the real one and the run reports "No feeds
    /// configured" while still exiting 0.
    fn default_db_path(exe_dir: Option<&std::path::Path>) -> String {
        let cwd_db = std::path::Path::new("feeder.db");
        if cwd_db.exists() {
            return "./feeder.db".to_string();
        }

        if let Some(exe_db) = exe_dir.map(|d| d.join("feeder.db")) {
            if exe_db.exists() {
                return exe_db.to_string_lossy().into_owned();
            }
        }

        "./feeder.db".to_string()
    }

    /// Resolve the GitHub token: `GITHUB_TOKEN` env var first, then fall back to
    /// the `gh` CLI (`gh auth token`) so it works out of the box on a machine
    /// already logged in with the GitHub CLI.
    fn resolve_github_token() -> Option<String> {
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            let token = token.trim().to_string();
            if !token.is_empty() {
                return Some(token);
            }
        }

        std::process::Command::new("gh")
            .args(["auth", "token"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| {
                let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if token.is_empty() {
                    None
                } else {
                    Some(token)
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_default_when_unset() {
        assert_eq!(Config::parse_jobs(None), 16);
    }

    #[test]
    fn jobs_honours_an_explicit_count() {
        assert_eq!(Config::parse_jobs(Some("16")), 16);
        assert_eq!(Config::parse_jobs(Some(" 3 ")), 3);
    }

    #[test]
    fn jobs_falls_back_on_junk_or_zero() {
        assert_eq!(Config::parse_jobs(Some("")), 16);
        assert_eq!(Config::parse_jobs(Some("lots")), 16);
        assert_eq!(Config::parse_jobs(Some("0")), 16);
        assert_eq!(Config::parse_jobs(Some("-2")), 16);
    }
}
