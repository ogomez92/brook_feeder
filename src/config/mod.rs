use crate::errors::{FeederError, FeederResult};

#[derive(Debug, Clone)]
pub struct Config {
    pub notebrook_url: String,
    pub notebrook_token: String,
    pub notebrook_channel: String,
    pub db_path: String,
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

        // Default db_path is relative to executable directory
        let db_path = std::env::var("FEEDER_DB_PATH").unwrap_or_else(|_| {
            exe_dir
                .map(|d| d.join("feeder.db").to_string_lossy().into_owned())
                .unwrap_or_else(|| "./feeder.db".to_string())
        });

        Ok(Self {
            notebrook_url,
            notebrook_token,
            notebrook_channel,
            db_path,
        })
    }
}
