use crate::errors::{FeederError, FeederResult};

#[derive(Debug, Clone)]
pub struct Config {
    pub notebrook_url: String,
    pub notebrook_token: String,
    pub notebrook_channel: String,
    pub db_path: String,
    /// GitHub token used for release tracking (private repos + higher rate limits)
    pub github_token: Option<String>,
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

        Ok(Self {
            notebrook_url,
            notebrook_token,
            notebrook_channel,
            db_path,
            github_token,
        })
    }

    /// Locate the database when `FEEDER_DB_PATH` is unset.
    ///
    /// Prefers an existing `feeder.db` in the current directory — that is what the
    /// docs promise and what the systemd unit's `WorkingDirectory` points at. Only
    /// falls back to one sitting next to the executable, and never *creates* one
    /// there: `current_exe()` resolves symlinks, so for a deployment that symlinks
    /// `feeder -> target/release/feeder` the exe directory is a build artifact
    /// directory. Creating the database there silently strands the real one and the
    /// run reports "No feeds configured" while still exiting 0.
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
