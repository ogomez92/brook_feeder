use crate::errors::{FeederError, FeederResult};

#[derive(Debug, Clone)]
pub struct Config {
    pub notebrook_url: String,
    pub notebrook_token: String,
    pub notebrook_channel: String,
    pub db_path: String,
}

impl Config {
    pub fn from_env() -> FeederResult<Self> {
        dotenvy::dotenv().ok();

        let notebrook_url = std::env::var("NOTEBROOK_URL")
            .map_err(|_| FeederError::MissingEnvVar("NOTEBROOK_URL".to_string()))?;

        let notebrook_token = std::env::var("NOTEBROOK_TOKEN")
            .map_err(|_| FeederError::MissingEnvVar("NOTEBROOK_TOKEN".to_string()))?;

        let notebrook_channel = std::env::var("NOTEBROOK_CHANNEL")
            .unwrap_or_else(|_| "feeds".to_string());

        let db_path = std::env::var("FEEDER_DB_PATH")
            .unwrap_or_else(|_| "./feeder.db".to_string());

        Ok(Self {
            notebrook_url,
            notebrook_token,
            notebrook_channel,
            db_path,
        })
    }
}
