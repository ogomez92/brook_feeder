use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "feeder")]
#[command(about = "Multi-source feed aggregator with Notebrook notifications")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Add a new feed URL (RSS, YouTube, Mastodon, WordPress, Blogger)
    Add {
        /// Feed URL to add
        url: String,
    },

    /// Remove a feed (interactive selection)
    Remove,

    /// List all feeds
    List,

    /// Import feeds from OPML file
    Import {
        /// Path to OPML file
        path: String,
    },

    /// Export feeds to OPML format
    Export {
        /// Output file path (prints to stdout if not specified)
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Fetch all feeds and notify new articles
    Run {
        /// Dry run - don't send notifications, just show what would be sent
        #[arg(long)]
        dry_run: bool,

        /// Skip notifications but still mark articles as seen in the database
        #[arg(long)]
        skip_notify: bool,
    },
}
