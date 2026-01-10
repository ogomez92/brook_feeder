use std::io::{self, Write};
use std::fs;

use clap::Parser;

use feeder::cli::{Cli, Commands};
use feeder::config::Config;
use feeder::errors::{FeederError, FeederResult};
use feeder::services::{FeedService, FetchService, ImportExportService, NotificationService};
use feeder::sources::SourceRegistry;
use feeder::storage::sqlite::{
    SqliteArticleCacheRepository, SqliteFeedRepository, SqliteStorage,
};

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run() -> FeederResult<()> {
    let cli = Cli::parse();

    // Load configuration
    let config = Config::from_env()?;

    // Initialize storage
    let storage = SqliteStorage::new(&config.db_path)?;
    let feed_repo = SqliteFeedRepository::new(storage.clone());
    let cache_repo = SqliteArticleCacheRepository::new(storage);

    // Initialize source registry
    let source_registry = SourceRegistry::new();

    match cli.command {
        Commands::Add { url } => cmd_add(&url, feed_repo, source_registry),
        Commands::Remove => cmd_remove(feed_repo),
        Commands::List => cmd_list(feed_repo),
        Commands::Import { path } => cmd_import(&path, feed_repo, source_registry),
        Commands::Export { output } => cmd_export(feed_repo, source_registry, output),
        Commands::Run { dry_run } => {
            cmd_run(feed_repo, cache_repo, source_registry, &config, dry_run)
        }
    }
}

fn cmd_add(
    url: &str,
    feed_repo: SqliteFeedRepository,
    source_registry: SourceRegistry,
) -> FeederResult<()> {
    let service = FeedService::new(feed_repo, source_registry);

    println!("Validating feed: {}", url);

    match service.add(url) {
        Ok(feed) => {
            println!("Feed added successfully!");
            println!("  Title: {}", feed.title);
            println!("  Type: {:?}", feed.feed_type);
            println!("  Source: {}", feed.source_type);
            Ok(())
        }
        Err(FeederError::FeedAlreadyExists(_)) => {
            println!("Feed already exists: {}", url);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn cmd_remove(feed_repo: SqliteFeedRepository) -> FeederResult<()> {
    let service = FeedService::new(feed_repo, SourceRegistry::new());
    let feeds = service.list()?;

    if feeds.is_empty() {
        println!("No feeds to remove.");
        return Ok(());
    }

    // Display numbered list
    println!("Select a feed to remove:\n");
    for (i, feed) in feeds.iter().enumerate() {
        println!(
            "  {}. {} [{}] ({})",
            i + 1,
            feed.title,
            feed.source_type,
            feed.url
        );
    }
    println!();

    // Read user input
    print!("Enter number (or 'q' to cancel): ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.eq_ignore_ascii_case("q") {
        println!("Cancelled.");
        return Ok(());
    }

    let index: usize = input
        .parse()
        .map_err(|_| FeederError::InvalidInput("Invalid number".to_string()))?;

    if index == 0 || index > feeds.len() {
        return Err(FeederError::InvalidInput(
            "Number out of range".to_string(),
        ));
    }

    let feed = &feeds[index - 1];
    let feed_id = feed.id.ok_or_else(|| {
        FeederError::FeedNotFound("Feed has no ID".to_string())
    })?;

    service.remove(feed_id)?;
    println!("Removed: {}", feed.title);

    Ok(())
}

fn cmd_list(feed_repo: SqliteFeedRepository) -> FeederResult<()> {
    let service = FeedService::new(feed_repo, SourceRegistry::new());
    let feeds = service.list()?;

    if feeds.is_empty() {
        println!("No feeds configured.");
        return Ok(());
    }

    println!("Configured feeds:\n");
    for feed in feeds {
        println!("  {} [{}]", feed.title, feed.source_type);
        println!("    URL: {}", feed.url);
        if feed.url != feed.feed_url {
            println!("    Feed: {}", feed.feed_url);
        }
        println!();
    }

    Ok(())
}

fn cmd_import(
    path: &str,
    feed_repo: SqliteFeedRepository,
    source_registry: SourceRegistry,
) -> FeederResult<()> {
    let content = fs::read_to_string(path)?;
    let service = ImportExportService::new(feed_repo, source_registry);

    println!("Importing feeds from {}...\n", path);

    let result = service.import_opml(&content)?;

    if !result.added.is_empty() {
        println!("Added {} feeds:", result.added.len());
        for feed in &result.added {
            println!("  + {} [{}]", feed.title, feed.source_type);
        }
        println!();
    }

    if !result.duplicates.is_empty() {
        println!("Skipped {} duplicates:", result.duplicates.len());
        for url in &result.duplicates {
            println!("  - {}", url);
        }
        println!();
    }

    if !result.invalid.is_empty() {
        println!("Failed {} feeds:", result.invalid.len());
        for (url, error) in &result.invalid {
            println!("  ! {}: {}", url, error);
        }
        println!();
    }

    println!(
        "Import complete: {} added, {} duplicates, {} failed",
        result.added.len(),
        result.duplicates.len(),
        result.invalid.len()
    );

    Ok(())
}

fn cmd_export(
    feed_repo: SqliteFeedRepository,
    source_registry: SourceRegistry,
    output: Option<String>,
) -> FeederResult<()> {
    let service = ImportExportService::new(feed_repo, source_registry);
    let opml = service.export_opml()?;

    match output {
        Some(path) => {
            fs::write(&path, &opml)?;
            println!("Exported feeds to {}", path);
        }
        None => {
            println!("{}", opml);
        }
    }

    Ok(())
}

fn cmd_run(
    feed_repo: SqliteFeedRepository,
    cache_repo: SqliteArticleCacheRepository,
    source_registry: SourceRegistry,
    config: &Config,
    dry_run: bool,
) -> FeederResult<()> {
    let fetch_service = FetchService::new(feed_repo, cache_repo, source_registry);

    println!("Fetching feeds...\n");

    let results = fetch_service.fetch_all_unnotified()?;

    if results.is_empty() {
        println!("No new articles to notify.");
        return Ok(());
    }

    let notification_service = if !dry_run {
        Some(NotificationService::new(config)?)
    } else {
        None
    };

    let mut total_notified = 0;

    for (feed, articles) in &results {
        println!("{} ({} new articles):", feed.title, articles.len());

        // Track which articles were successfully notified
        let mut notified_articles = Vec::new();

        for article in articles {
            let notification = feeder::domain::Notification::from_article(feed, article);

            if dry_run {
                println!("  [DRY RUN] {}", notification.format());
            } else {
                print!("  Sending: {}... ", notification.article_title);
                io::stdout().flush()?;

                match notification_service.as_ref().unwrap().send(&notification) {
                    Ok(()) => {
                        println!("OK");
                        total_notified += 1;
                        notified_articles.push(article.clone());
                    }
                    Err(e) => {
                        println!("FAILED: {}", e);
                        // Don't add to notified_articles - will retry next run
                    }
                }
            }
        }

        // Only mark successfully notified articles
        if !dry_run && !notified_articles.is_empty() {
            fetch_service.mark_notified(feed, &notified_articles)?;
        }

        println!();
    }

    if dry_run {
        println!(
            "Dry run complete. Would notify {} articles.",
            results.iter().map(|(_, a)| a.len()).sum::<usize>()
        );
    } else {
        println!("Notified {} articles.", total_notified);
    }

    Ok(())
}
