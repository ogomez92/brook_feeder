use thiserror::Error;

#[derive(Error, Debug)]
pub enum FeederError {
    // Configuration errors
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Missing environment variable: {0}")]
    MissingEnvVar(String),

    // Feed errors
    #[error("Invalid feed URL: {0}")]
    InvalidUrl(String),

    #[error("Feed validation failed: {0}")]
    FeedValidation(String),

    #[error("Feed not found: {0}")]
    FeedNotFound(String),

    #[error("Feed already exists: {0}")]
    FeedAlreadyExists(String),

    #[error("Unsupported feed source: {0}")]
    UnsupportedSource(String),

    // Network errors
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    // Parsing errors
    #[error("Feed parsing failed: {0}")]
    FeedParse(String),

    #[error("OPML parsing failed: {0}")]
    OpmlParse(String),

    // Storage errors
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    // Notification errors
    #[error("Notification failed: {0}")]
    Notification(String),

    // IO errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    // User input errors
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    // Channel errors from notebrook library
    #[error("Channel error: {0}")]
    Channel(String),
}

impl From<channels::ChannelError> for FeederError {
    fn from(err: channels::ChannelError) -> Self {
        FeederError::Channel(err.to_string())
    }
}

pub type FeederResult<T> = Result<T, FeederError>;
