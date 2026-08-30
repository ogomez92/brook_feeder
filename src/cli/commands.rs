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

    /// Remove a feed by name match (with confirmation), or interactively if no query
    Remove {
        /// Part of the feed title or URL to match (omit to pick from the full list)
        query: Option<String>,
    },

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

    /// Track GitHub repository releases and notify new ones
    Releases {
        #[command(subcommand)]
        command: ReleaseCommands,
    },

    /// Mirror the Accessibility Mod Manager registry and its release artifacts
    Mods {
        #[command(subcommand)]
        command: ModCommands,
    },

    /// Clone (or update) every tracked repo into $FEEDER_REPOS_DIR/owner/name
    #[command(name = "getrepos", visible_alias = "get-repos")]
    GetRepos {
        /// Repos to sync at once (default: 16, or $FEEDER_REPOS_JOBS)
        #[arg(short, long)]
        jobs: Option<usize>,
    },
}

#[derive(Subcommand)]
pub enum ModCommands {
    /// Pull the registry, notify new artifacts, and mirror them to disk
    Run {
        /// Dry run - don't notify, download, or record anything
        #[arg(long)]
        dry_run: bool,

        /// Skip notifications but still mirror and record artifacts
        #[arg(long)]
        skip_notify: bool,

        /// Record and notify new artifacts without downloading them
        #[arg(long)]
        no_download: bool,

        /// Mirror directory (default: ./modmirror, or $FEEDER_MOD_MIRROR)
        #[arg(long)]
        dir: Option<String>,
    },

    /// List every artifact seen so far and where it landed
    List,
}

#[derive(Subcommand)]
pub enum ReleaseCommands {
    /// Add a repo to track (URL/owner/name), or `@username` to pick from their repos
    Add {
        /// `sveltejs/kit`, a github.com URL, or `@username` to list a user's/org's repos
        repo: String,
    },

    /// Remove a tracked repo by name match (with confirmation), or interactively if no query
    Remove {
        /// Part of the repo name (owner/name) to match (omit to pick from the full list)
        query: Option<String>,
    },

    /// List all tracked repos
    List,

    /// Import tracked repos from a Release Tracker JSON export
    Import {
        /// Path to the JSON file (e.g. releases.json)
        path: String,
    },

    /// Fetch latest releases/commits and notify new ones
    Run {
        /// Dry run - don't send notifications, just show what would be sent
        #[arg(long)]
        dry_run: bool,

        /// Skip notifications but still mark releases as seen in the database
        #[arg(long)]
        skip_notify: bool,
    },
}
