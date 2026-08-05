use std::io::{self, Write};
use std::fs;

use clap::Parser;

use feeder::cli::{Cli, Commands, ModCommands, ReleaseCommands};
use feeder::config::Config;
use feeder::domain::{ArtifactStatus, Notification, RepoUpdate};
use feeder::errors::{FeederError, FeederResult};
use feeder::github::GithubClient;
use feeder::modregistry::RegistryClient;
use feeder::services::{
    CloneOutcome, CloneService, FeedService, FetchService, ImportExportService, MirrorOutcome,
    ModMirrorService, NotificationService, ReleaseService,
};
use feeder::sources::SourceRegistry;
use feeder::storage::sqlite::{
    SqliteArticleCacheRepository, SqliteFeedRepository, SqliteModArtifactRepository,
    SqliteReleaseCacheRepository, SqliteRepoRepository, SqliteStorage,
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
    let cache_repo = SqliteArticleCacheRepository::new(storage.clone());

    // Initialize source registry
    let source_registry = SourceRegistry::new();

    match cli.command {
        Commands::Add { url } => cmd_add(&url, feed_repo, source_registry),
        Commands::Remove { query } => cmd_remove(feed_repo, query),
        Commands::List => cmd_list(feed_repo),
        Commands::Import { path } => cmd_import(&path, feed_repo, source_registry),
        Commands::Export { output } => cmd_export(feed_repo, source_registry, output),
        Commands::Run { dry_run, skip_notify } => {
            // A single `run` covers feeds, GitHub releases, and the mod registry
            // so one timer handles everything. Run each independently: a failure
            // in one is reported but must not skip the others (this runs
            // silently).
            let feeds_result =
                cmd_run(feed_repo, cache_repo, source_registry, &config, dry_run, skip_notify);
            if let Err(e) = &feeds_result {
                eprintln!("Feeds run failed: {}", e);
            }

            println!("\n--- GitHub releases ---\n");

            let releases_result = build_release_service(storage.clone(), &config)
                .and_then(|service| cmd_release_run(&service, &config, dry_run, skip_notify));
            if let Err(e) = &releases_result {
                eprintln!("Releases run failed: {}", e);
            }

            // Unlike feeds and releases, the registry is a fixed remote rather
            // than a list in the database, so this step always reaches the
            // network. `FEEDER_MODS=0` opts out.
            let mods_result = if config.mods_enabled {
                println!("\n--- Mod registry ---\n");

                let result = build_mod_mirror_service(storage, &config, None).and_then(|service| {
                    cmd_mods_run(&service, &config, dry_run, skip_notify, false)
                });
                if let Err(e) = &result {
                    eprintln!("Mod registry run failed: {}", e);
                }
                result
            } else {
                println!("\n--- Mod registry: disabled (FEEDER_MODS=0) ---\n");
                Ok(())
            };

            // Surface a non-zero exit if any part failed (after running them all).
            feeds_result.and(releases_result).and(mods_result)
        }
        Commands::Releases { command } => {
            let service = build_release_service(storage, &config)?;

            match command {
                ReleaseCommands::Add { repo } => cmd_release_add(&service, &repo),
                ReleaseCommands::Remove { query } => cmd_release_remove(&service, query),
                ReleaseCommands::List => cmd_release_list(&service),
                ReleaseCommands::Import { path } => cmd_release_import(&service, &path),
                ReleaseCommands::Run {
                    dry_run,
                    skip_notify,
                } => cmd_release_run(&service, &config, dry_run, skip_notify),
            }
        }
        Commands::Mods { command } => match command {
            ModCommands::Run {
                dry_run,
                skip_notify,
                no_download,
                dir,
            } => {
                let service = build_mod_mirror_service(storage, &config, dir)?;
                cmd_mods_run(&service, &config, dry_run, skip_notify, no_download)
            }
            ModCommands::List => {
                let service = build_mod_mirror_service(storage, &config, None)?;
                cmd_mods_list(&service)
            }
        },

        Commands::GetRepos => {
            let service = build_release_service(storage, &config)?;
            cmd_get_repos(&service, &config)
        }
    }
}

/// Build the release-tracking service from shared storage + config.
fn build_release_service(
    storage: SqliteStorage,
    config: &Config,
) -> FeederResult<ReleaseService<SqliteRepoRepository, SqliteReleaseCacheRepository>> {
    let repo_repo = SqliteRepoRepository::new(storage.clone());
    let release_cache = SqliteReleaseCacheRepository::new(storage);
    let github = GithubClient::new(config.github_token.clone())?;
    Ok(ReleaseService::new(repo_repo, release_cache, github))
}

/// Build the mod-registry mirror service. `dir_override` comes from
/// `--dir`; otherwise the configured mirror directory is used, resolved
/// against the current directory the same way `getrepos` resolves `./repos`.
fn build_mod_mirror_service(
    storage: SqliteStorage,
    config: &Config,
    dir_override: Option<String>,
) -> FeederResult<ModMirrorService<SqliteModArtifactRepository>> {
    let artifact_repo = SqliteModArtifactRepository::new(storage);
    let client = RegistryClient::new(config.github_token.clone())?;

    let dir = dir_override
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| config.mod_mirror_dir.clone());

    // Drop a leading `./` so the resolved path prints as /home/feeder/modmirror
    // rather than /home/feeder/./modmirror.
    let path = std::path::PathBuf::from(dir.strip_prefix("./").unwrap_or(&dir));
    let mirror_dir = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };

    Ok(ModMirrorService::new(artifact_repo, client, mirror_dir))
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

fn cmd_remove(feed_repo: SqliteFeedRepository, query: Option<String>) -> FeederResult<()> {
    let service = FeedService::new(feed_repo, SourceRegistry::new());
    let feeds = service.list()?;

    if feeds.is_empty() {
        println!("No feeds to remove.");
        return Ok(());
    }

    // Narrow to feeds matching the query (title or URL, case-insensitive); with
    // no query, every feed is a candidate.
    let matches: Vec<_> = match &query {
        Some(q) => {
            let needle = q.to_lowercase();
            feeds
                .iter()
                .filter(|f| {
                    f.title.to_lowercase().contains(&needle)
                        || f.url.to_lowercase().contains(&needle)
                })
                .collect()
        }
        None => feeds.iter().collect(),
    };

    if matches.is_empty() {
        println!("No feeds match '{}'.", query.unwrap_or_default());
        return Ok(());
    }

    // One candidate is used directly; several are shown for a numbered pick.
    let feed = match select_one(&matches, "feeds", |f| {
        format!("{} [{}] ({})", f.title, f.source_type, f.url)
    })? {
        Some(feed) => feed,
        None => return Ok(()),
    };

    if !confirm(&format!("Remove \"{}\" ({})?", feed.title, feed.url))? {
        println!("Cancelled.");
        return Ok(());
    }

    let feed_id = feed
        .id
        .ok_or_else(|| FeederError::FeedNotFound("Feed has no ID".to_string()))?;

    service.remove(feed_id)?;
    println!("Removed: {}", feed.title);

    Ok(())
}

/// Pick the one item to act on from a candidate set: a single candidate is
/// returned directly; several are listed for a numbered pick. `label` renders
/// each item. Returns `Ok(None)` when the user cancels.
fn select_one<'a, T>(
    items: &[&'a T],
    noun: &str,
    label: impl Fn(&T) -> String,
) -> FeederResult<Option<&'a T>> {
    if let [only] = items {
        return Ok(Some(*only));
    }

    println!("Matching {}:\n", noun);
    for (i, item) in items.iter().enumerate() {
        println!("  {}. {}", i + 1, label(item));
    }
    println!();

    print!("Enter number (or 'q' to cancel): ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() || input.eq_ignore_ascii_case("q") {
        println!("Cancelled.");
        return Ok(None);
    }

    let index: usize = input
        .parse()
        .map_err(|_| FeederError::InvalidInput("Invalid number".to_string()))?;
    if index == 0 || index > items.len() {
        return Err(FeederError::InvalidInput("Number out of range".to_string()));
    }

    Ok(Some(items[index - 1]))
}

/// Ask a yes/no question, defaulting to no. Returns true only on an explicit yes.
fn confirm(question: &str) -> FeederResult<bool> {
    print!("{} [y/N]: ", question);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim();

    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
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
    skip_notify: bool,
) -> FeederResult<()> {
    let fetch_service = FetchService::new(feed_repo, cache_repo, source_registry);

    if skip_notify {
        println!("Fetching feeds (skip-notify mode)...\n");
    } else {
        println!("Fetching feeds...\n");
    }

    let results = fetch_service.fetch_all_unnotified()?;

    if results.is_empty() {
        println!("No feeds configured.");
        return Ok(());
    }

    // Display fetch results for each feed
    let mut error_count = 0;
    let mut total_new = 0;
    let mut feeds_with_new = 0;

    for result in &results {
        if result.is_error() {
            println!(
                "  {}: error: {}",
                result.feed.title,
                result.error.as_ref().unwrap()
            );
            error_count += 1;
        } else if result.has_new_articles() {
            println!(
                "  {}: fetched {} articles, {} new",
                result.feed.title,
                result.total_articles,
                result.new_articles.len()
            );
            total_new += result.new_articles.len();
            feeds_with_new += 1;
        } else {
            println!(
                "  {}: fetched {} articles, 0 new",
                result.feed.title, result.total_articles
            );
        }
    }

    println!();

    // Summary line
    if error_count > 0 {
        println!(
            "Found {} new articles from {} feeds ({} errors).\n",
            total_new, feeds_with_new, error_count
        );
    } else if total_new > 0 {
        println!("Found {} new articles from {} feeds.\n", total_new, feeds_with_new);
    } else {
        println!("No new articles to notify.");
        return Ok(());
    }

    // Process notifications
    let notification_service = if !dry_run && !skip_notify {
        Some(NotificationService::new(config)?)
    } else {
        None
    };

    let mut total_notified = 0;

    for result in &results {
        if !result.has_new_articles() {
            continue;
        }

        let feed = &result.feed;
        let articles = &result.new_articles;

        println!("{} ({} new articles):", feed.title, articles.len());

        // Track which articles were successfully notified
        let mut notified_articles = Vec::new();

        for article in articles {
            let notification = feeder::domain::Notification::from_article(feed, article);

            if dry_run {
                println!("  [DRY RUN] {}", notification.format());
            } else if skip_notify {
                println!("  [SKIP] {}", notification.article_title);
                total_notified += 1;
                notified_articles.push(article.clone());
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

        // Mark articles as notified (skip_notify marks without sending, normal marks after sending)
        if !dry_run && !notified_articles.is_empty() {
            fetch_service.mark_notified(feed, &notified_articles)?;
        }

        println!();
    }

    if dry_run {
        println!("Dry run complete. Would notify {} articles.", total_new);
    } else if skip_notify {
        println!("Marked {} articles as seen (notifications skipped).", total_notified);
    } else {
        println!("Notified {} articles.", total_notified);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Release tracking commands
// ---------------------------------------------------------------------------

type Releases = ReleaseService<SqliteRepoRepository, SqliteReleaseCacheRepository>;

fn cmd_release_add(service: &Releases, repo: &str) -> FeederResult<()> {
    // A leading `@` means "list this user/org's repos and pick some to track"
    // rather than adding a single `owner/name` or URL.
    if repo.trim().starts_with('@') {
        return cmd_release_add_user(service, repo.trim());
    }

    match service.add(repo) {
        Ok(tracked) => {
            println!("Now tracking {}", tracked.full_name());
            println!("  URL: {}", tracked.url);
            Ok(())
        }
        Err(FeederError::FeedAlreadyExists(name)) => {
            println!("Already tracking: {}", name);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Interactive flow for `releases add @username`: list the user's/org's
/// not-yet-tracked repos and let the user pick a comma-separated set to track.
fn cmd_release_add_user(service: &Releases, username: &str) -> FeederResult<()> {
    let login = username.trim_start_matches('@').trim();
    if login.is_empty() {
        return Err(FeederError::InvalidInput(
            "provide a GitHub username, e.g. `feeder releases add @torvalds`".to_string(),
        ));
    }

    if !service.has_token() {
        eprintln!(
            "Warning: no GitHub token found (set GITHUB_TOKEN or run `gh auth login`).\n\
             Only public repos are listed and the rate limit is low.\n"
        );
    }

    println!("Fetching repos for @{}...", login);
    let untracked = service.discover_untracked(login)?;

    if untracked.is_empty() {
        println!("No new repos to track for @{}.", login);
        return Ok(());
    }

    println!("\nUntracked repos by @{}:\n", login);
    for (i, repo) in untracked.iter().enumerate() {
        let mut tags = Vec::new();
        if repo.is_fork {
            tags.push("fork");
        }
        if repo.is_archived {
            tags.push("archived");
        }
        let tags = if tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", tags.join(", "))
        };
        let desc = repo
            .description
            .as_deref()
            .map(|d| format!(" — {}", d))
            .unwrap_or_default();

        println!(
            "  {}. {} (★{}){}{}",
            i + 1,
            repo.full_name(),
            repo.stargazers,
            tags,
            desc
        );
    }
    println!();

    print!("Enter numbers to track (comma-separated, e.g. 1,2,3), or 'q' to cancel: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() || input.eq_ignore_ascii_case("q") {
        println!("Cancelled.");
        return Ok(());
    }

    let selection = parse_selection(input, untracked.len())?;
    if selection.is_empty() {
        println!("Nothing selected.");
        return Ok(());
    }

    let mut added = 0;
    for index in selection {
        let repo = &untracked[index - 1];
        match service.add_owned(repo) {
            Ok(tracked) => {
                println!("  + Now tracking {}", tracked.full_name());
                added += 1;
            }
            Err(FeederError::FeedAlreadyExists(name)) => {
                println!("  - Already tracking {}", name);
            }
            Err(e) => {
                println!("  ! Failed to track {}: {}", repo.full_name(), e);
            }
        }
    }

    println!("\nNow tracking {} new repo(s).", added);
    Ok(())
}

/// Parse a comma-separated list of 1-based indices (e.g. `1,2,3`) into a
/// deduplicated list of selections, validating each is within `1..=max`.
fn parse_selection(input: &str, max: usize) -> FeederResult<Vec<usize>> {
    let mut selected = Vec::new();
    for part in input.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let n: usize = part
            .parse()
            .map_err(|_| FeederError::InvalidInput(format!("not a number: '{}'", part)))?;
        if n == 0 || n > max {
            return Err(FeederError::InvalidInput(format!(
                "number out of range: {} (pick between 1 and {})",
                n, max
            )));
        }
        if !selected.contains(&n) {
            selected.push(n);
        }
    }
    Ok(selected)
}

fn cmd_release_list(service: &Releases) -> FeederResult<()> {
    let repos = service.list()?;

    if repos.is_empty() {
        println!("No repos tracked. Add one with `feeder releases add owner/name`.");
        return Ok(());
    }

    println!("Tracked repos ({}):\n", repos.len());
    for repo in repos {
        println!("  {} ({})", repo.full_name(), repo.url);
    }

    Ok(())
}

fn cmd_release_remove(service: &Releases, query: Option<String>) -> FeederResult<()> {
    let repos = service.list()?;

    if repos.is_empty() {
        println!("No repos to remove.");
        return Ok(());
    }

    // Narrow to repos matching the query (owner/name, case-insensitive); with no
    // query, every tracked repo is a candidate.
    let matches: Vec<_> = match &query {
        Some(q) => {
            let needle = q.to_lowercase();
            repos
                .iter()
                .filter(|r| r.full_name().to_lowercase().contains(&needle))
                .collect()
        }
        None => repos.iter().collect(),
    };

    if matches.is_empty() {
        println!("No tracked repos match '{}'.", query.unwrap_or_default());
        return Ok(());
    }

    let repo = match select_one(&matches, "repos", |r| format!("{} ({})", r.full_name(), r.url))? {
        Some(repo) => repo,
        None => return Ok(()),
    };

    if !confirm(&format!("Remove {} ({})?", repo.full_name(), repo.url))? {
        println!("Cancelled.");
        return Ok(());
    }

    let id = repo
        .id
        .ok_or_else(|| FeederError::FeedNotFound("Repo has no ID".to_string()))?;

    service.remove(id)?;
    println!("Removed: {}", repo.full_name());

    Ok(())
}

fn cmd_release_import(service: &Releases, path: &str) -> FeederResult<()> {
    let content = fs::read_to_string(path)?;

    println!("Importing tracked repos from {}...\n", path);

    let result = service.import_json(&content)?;

    if !result.added.is_empty() {
        println!("Added {} repos:", result.added.len());
        for repo in &result.added {
            println!("  + {}", repo.full_name());
        }
        println!();
    }

    if !result.duplicates.is_empty() {
        println!("Skipped {} already-tracked repos.", result.duplicates.len());
    }

    if !result.invalid.is_empty() {
        println!("Failed {} repos:", result.invalid.len());
        for (reference, error) in &result.invalid {
            println!("  ! {}: {}", reference, error);
        }
    }

    println!(
        "\nImport complete: {} added, {} duplicates, {} failed",
        result.added.len(),
        result.duplicates.len(),
        result.invalid.len()
    );

    Ok(())
}

fn cmd_release_run(
    service: &Releases,
    config: &Config,
    dry_run: bool,
    skip_notify: bool,
) -> FeederResult<()> {
    let repos = service.list()?;
    if repos.is_empty() {
        println!("No repos tracked. Add one with `feeder releases add owner/name`.");
        return Ok(());
    }

    if !service.has_token() {
        eprintln!(
            "Warning: no GitHub token found (set GITHUB_TOKEN or run `gh auth login`).\n\
             Private repos will be skipped and the rate limit is low.\n"
        );
    }

    if skip_notify {
        println!("Checking {} repos (skip-notify mode)...", repos.len());
    } else {
        println!("Checking {} repos for new releases...", repos.len());
    }

    // Bulk-fetch latest release/commit for every repo.
    let fetches = service.fetch_all(|done, total| {
        println!("  fetched {}/{}", done, total);
    })?;
    println!();

    // Notifier is only needed for a real (non-dry, non-skip) run.
    let notification_service = if !dry_run && !skip_notify {
        Some(NotificationService::new(config)?)
    } else {
        None
    };

    let mut error_count = 0;
    let mut empty_count = 0;
    let mut up_to_date = 0;
    let mut new_count = 0;
    let mut notified = 0;

    for fetch in &fetches {
        let repo = &fetch.repo;

        if let Some(err) = &fetch.error {
            println!("  {}: error: {}", repo.full_name(), err);
            error_count += 1;
            continue;
        }

        if matches!(fetch.update, RepoUpdate::None) {
            empty_count += 1;
            continue;
        }

        let cache_key = match fetch.update.cache_key(repo) {
            Some(key) => key,
            None => continue,
        };

        // Already notified about this exact release/commit -> nothing to do.
        if service.is_notified(&cache_key)? {
            up_to_date += 1;
            continue;
        }

        let notification = match Notification::from_repo_update(repo, &fetch.update) {
            Some(n) => n,
            None => continue,
        };
        new_count += 1;

        let kind = match &fetch.update {
            RepoUpdate::Release(_) => "release",
            RepoUpdate::Commit(_) => "commit",
            RepoUpdate::None => "",
        };
        let repo_id = repo.id.unwrap_or_default();

        if dry_run {
            println!("  [DRY RUN] ({}) {}", kind, notification.format());
        } else if skip_notify {
            println!("  [SKIP] ({}) {}", kind, notification.format());
            service.mark_notified(&cache_key, repo_id, &notification.article_title)?;
            notified += 1;
        } else {
            print!("  Sending ({}) {}... ", kind, repo.full_name());
            io::stdout().flush()?;

            match notification_service.as_ref().unwrap().send(&notification) {
                Ok(()) => {
                    println!("OK");
                    service.mark_notified(&cache_key, repo_id, &notification.article_title)?;
                    notified += 1;
                }
                Err(e) => {
                    // Leave it unmarked so it retries next run.
                    println!("FAILED: {}", e);
                }
            }
        }
    }

    println!();
    println!(
        "Checked {} repos: {} new, {} up-to-date, {} without releases/commits, {} errors.",
        fetches.len(),
        new_count,
        up_to_date,
        empty_count,
        error_count
    );

    if dry_run {
        println!("Dry run complete. Would notify {} updates.", new_count);
    } else if skip_notify {
        println!("Marked {} updates as seen (notifications skipped).", notified);
    } else {
        println!("Notified {} updates.", notified);
    }

    Ok(())
}

/// Clone (or update) every tracked repo into `./repos/owner/name`, mirroring the
/// Release Tracker desktop app's "Update Repos" flow but without prompting for a
/// destination. Uses the GitHub token so private repos come down, and reports
/// each repo's outcome plus a summary — a single repo failing never aborts the
/// rest.
fn cmd_get_repos(service: &Releases, config: &Config) -> FeederResult<()> {
    let repos = service.list()?;
    if repos.is_empty() {
        println!("No repos tracked. Add one with `feeder releases add owner/name`.");
        return Ok(());
    }

    let base_dir = std::env::current_dir()?.join("repos");

    if config.github_token.is_none() {
        eprintln!(
            "Warning: no GitHub token found (set GITHUB_TOKEN or run `gh auth login`).\n\
             Private repos will fail to clone.\n"
        );
    }

    println!(
        "Cloning/updating {} repo(s) into {}\n",
        repos.len(),
        base_dir.display()
    );

    let clone_service = CloneService::new(base_dir, config.github_token.clone());

    let mut cloned = 0;
    let mut updated = 0;
    let mut up_to_date = 0;
    let mut skipped = 0;
    let mut failed: Vec<(String, String)> = Vec::new();

    for repo in &repos {
        print!("  {} ... ", repo.full_name());
        io::stdout().flush()?;

        match clone_service.sync(repo) {
            CloneOutcome::Cloned => {
                println!("cloned");
                cloned += 1;
            }
            CloneOutcome::Updated => {
                println!("updated");
                updated += 1;
            }
            CloneOutcome::UpToDate => {
                println!("up to date");
                up_to_date += 1;
            }
            CloneOutcome::SkippedDirty => {
                println!("skipped (uncommitted changes)");
                skipped += 1;
            }
            CloneOutcome::Failed(err) => {
                println!("FAILED: {}", err);
                failed.push((repo.full_name(), err));
            }
        }
    }

    println!();
    println!(
        "Done: {} cloned, {} updated, {} up-to-date, {} skipped, {} failed.",
        cloned,
        updated,
        up_to_date,
        skipped,
        failed.len()
    );

    if !failed.is_empty() {
        println!("\nErrors:");
        for (name, err) in &failed {
            println!("  {}: {}", name, err);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Mod registry mirroring
// ---------------------------------------------------------------------------

type Mods = ModMirrorService<SqliteModArtifactRepository>;

/// Pull the Accessibility Mod Manager registry, announce anything new, and
/// mirror the artifacts it points at.
///
/// An artifact is announced exactly once — the first run it appears — and then
/// downloaded. A download that fails leaves the artifact recorded as `pending`,
/// so the next run retries the file without re-announcing it.
fn cmd_mods_run(
    service: &Mods,
    config: &Config,
    dry_run: bool,
    skip_notify: bool,
    no_download: bool,
) -> FeederResult<()> {
    println!("Pulling mod registry into {}...", service.mirror_dir().display());

    // A dry run inspects the registry without writing snapshots to disk.
    let discovery = service.discover(!dry_run)?;

    for warning in &discovery.warnings {
        eprintln!("  warning: {}", warning);
    }

    if discovery.artifacts.is_empty() {
        println!("Registry lists no artifacts.");
        return Ok(());
    }

    println!("Found {} artifacts in the registry.\n", discovery.artifacts.len());

    let notification_service = if !dry_run && !skip_notify {
        Some(NotificationService::new(config)?)
    } else {
        None
    };

    let mut new_count = 0;
    let mut notified = 0;
    let mut up_to_date = 0;
    let mut downloaded = 0;
    let mut already_on_disk = 0;
    let mut gated = 0;
    let mut retried = 0;
    let mut failed: Vec<(String, String)> = Vec::new();

    for artifact in &discovery.artifacts {
        let existing = service.existing(artifact)?;

        // Anything already mirrored (or known-gated) is settled.
        if let Some(record) = &existing {
            if record.status != ArtifactStatus::Pending {
                up_to_date += 1;
                continue;
            }
            retried += 1;
        }

        let is_new = existing.is_none();
        let notification = Notification::from_mod_artifact(artifact);

        if is_new {
            new_count += 1;

            if dry_run {
                println!("  [DRY RUN] {}", notification.format());
            } else if skip_notify {
                println!("  [SKIP] {}", notification.format());
                notified += 1;
            } else {
                print!("  Sending: {} {}... ", artifact.label, artifact.version);
                io::stdout().flush()?;

                match notification_service.as_ref().unwrap().send(&notification) {
                    Ok(()) => {
                        println!("OK");
                        notified += 1;
                    }
                    Err(e) => {
                        // Leave it unrecorded so the whole thing retries next run.
                        println!("FAILED: {}", e);
                        failed.push((artifact.cache_key(), e.to_string()));
                        continue;
                    }
                }
            }
        }

        if dry_run {
            continue;
        }

        // Gated artifacts are recorded so they are never announced twice, but
        // the bytes need a paid Patreon entitlement, so nothing is fetched.
        let outcome = if artifact.gated {
            MirrorOutcome::Gated
        } else if no_download {
            MirrorOutcome::Failed("skipped (--no-download)".to_string())
        } else {
            service.mirror(artifact)
        };

        let (status, path) = match outcome {
            MirrorOutcome::Mirrored { path, bytes } => {
                println!("    mirrored {} ({})", path, human_bytes(bytes));
                downloaded += 1;
                (ArtifactStatus::Mirrored, Some(path))
            }
            MirrorOutcome::AlreadyOnDisk { path } => {
                already_on_disk += 1;
                (ArtifactStatus::Mirrored, Some(path))
            }
            MirrorOutcome::Gated => {
                gated += 1;
                (ArtifactStatus::Gated, None)
            }
            MirrorOutcome::Failed(err) => {
                if !no_download {
                    println!("    download FAILED: {}", err);
                    failed.push((artifact.cache_key(), err));
                }
                (ArtifactStatus::Pending, None)
            }
        };

        service.record(artifact, status, path.as_deref())?;
    }

    println!();
    println!(
        "Registry: {} new, {} already known, {} pending retried.",
        new_count, up_to_date, retried
    );

    if dry_run {
        println!("Dry run complete. Would notify {} artifacts.", new_count);
        return Ok(());
    }

    println!(
        "Mirror: {} downloaded, {} already on disk, {} Patreon-gated (skipped), {} failed.",
        downloaded,
        already_on_disk,
        gated,
        failed.len()
    );

    if skip_notify {
        println!("Marked {} artifacts as seen (notifications skipped).", notified);
    } else {
        println!("Notified {} artifacts.", notified);
    }

    if !failed.is_empty() {
        println!("\nErrors:");
        for (key, err) in &failed {
            println!("  {}: {}", key, err);
        }
    }

    Ok(())
}

fn cmd_mods_list(service: &Mods) -> FeederResult<()> {
    let records = service.list()?;

    if records.is_empty() {
        println!("Nothing mirrored yet. Run `feeder mods run`.");
        return Ok(());
    }

    println!("Mirror: {}\n", service.mirror_dir().display());

    let mut current_kind = String::new();
    for record in &records {
        if record.kind != current_kind {
            current_kind = record.kind.clone();
            println!("[{}]", current_kind);
        }

        let scope = if record.game_id.is_empty() {
            record.source_id.clone()
        } else {
            format!("{}/{}", record.source_id, record.game_id)
        };

        println!(
            "  {} {} ({}) — {}",
            scope,
            record.version,
            record.status.as_str(),
            record.label
        );

        match (&record.local_path, record.status) {
            (Some(path), _) => println!("    {}", path),
            (None, ArtifactStatus::Gated) => {
                println!("    Patreon-only — metadata recorded, file not mirrored")
            }
            (None, _) => println!("    not on disk yet"),
        }
    }

    println!("\n{} artifacts tracked.", records.len());
    Ok(())
}

/// Render a byte count for a progress line.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;

    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_human_bytes() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(2_648_396), "2.5 MB");
    }

    #[test]
    fn test_parse_selection_basic() {
        assert_eq!(parse_selection("1,2,3", 5).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn test_parse_selection_dedup_and_whitespace() {
        assert_eq!(parse_selection(" 2, 2 , 1 ,", 5).unwrap(), vec![2, 1]);
    }

    #[test]
    fn test_parse_selection_rejects_out_of_range() {
        assert!(matches!(
            parse_selection("1,6", 5),
            Err(FeederError::InvalidInput(_))
        ));
        assert!(matches!(
            parse_selection("0", 5),
            Err(FeederError::InvalidInput(_))
        ));
    }

    #[test]
    fn test_parse_selection_rejects_non_number() {
        assert!(matches!(
            parse_selection("1,x", 5),
            Err(FeederError::InvalidInput(_))
        ));
    }
}
