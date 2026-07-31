# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo build              # Development build
cargo build --release    # Release build
cargo test               # Run all tests
cargo test <test_name>   # Run specific test
cargo run -- <command>   # Run CLI (e.g., cargo run -- list)
cargo run -- run --dry-run      # Preview notifications without sending
cargo run -- run --skip-notify  # Mark articles as seen without notifying

# Release tracking (GitHub repos)
cargo run -- releases import releases.json   # Import tracked repos (JSON is import-only)
cargo run -- releases add owner/name         # Track a single repo (URL or owner/name)
cargo run -- releases add @username          # List a user's/org's untracked repos, pick some (e.g. 1,2,3)
cargo run -- releases list                   # List tracked repos
cargo run -- releases run                     # Notify new releases (or latest commit if none)
cargo run -- releases run --dry-run          # Preview without sending

# Clone/update tracked repos to local disk (run manually, not in the timer)
cargo run -- getrepos                         # Clone every tracked repo into ./repos/owner/name (pulls if already present)
```

## Architecture

Feeder is a multi-source feed aggregator CLI that notifies new articles to Notebrook.

### Layer Structure

```
CLI (src/cli/) → Services (src/services/) → Sources/Storage (src/sources/, src/storage/)
                                                    ↓
                                           Domain (src/domain/)
```

### Key Components

**Sources** (`src/sources/`): Each feed type implements the `FeedSource` trait. The `SourceRegistry` auto-detects source type from URL and routes to the appropriate handler. All sources delegate feed parsing to `RssAtomSource` after URL conversion.

- YouTube: Converts `/@username` URLs to XML feed by scraping channel ID
- Mastodon: Converts `instance/@user` to `.rss` endpoint
- WordPress: Detects via `/wp-json/`, uses `/feed/` endpoint
- Blogger: Detects `.blogspot.com`, uses `/feeds/posts/default`

**Storage** (`src/storage/`): SQLite with four tables:
- `feeds`: Stores original URL, resolved feed URL, title, type, source
- `notified_articles`: Cache keyed by `{feed_title}:{article_id}` for deduplication
- `tracked_repos`: GitHub repos to watch for releases (owner, name, url); unique on `owner/name` (case-insensitive)
- `notified_releases`: Cache keyed by `{owner}/{name}:release:{tag}` or `{owner}/{name}:commit:{sha}`

**Release Tracking** (`src/github/`, `src/services/release_service.rs`): Watches GitHub
repositories and notifies new releases (falling back to the latest default-branch commit for
repos without releases). `GithubClient` bulk-fetches via the GraphQL API in batches of 30 aliased
sub-queries, tolerant of missing/renamed repos and transient failures (retries + per-repo error
isolation so a silent scheduled run never aborts wholesale). Notifications go to the same Notebrook
channel as feeds, one message per release/commit with the link. Release messages link to
`github.com/owner/name/releases/latest` (not the tagged release page) so an old notification still
opens the newest release and its assets; commit messages link to the specific commit. The
`releases import` command reads
a Release Tracker JSON export (`releases.json`) — **only the `repos` array is used**; notified state
lives in the normal database, not the JSON.

`releases add` accepts a single `owner/name`/URL, or `@username` to interactively bulk-add: it lists
that user's/org's owned repos (via `repositoryOwner.repositories`, paginated) minus the ones already
tracked, then prompts for a comma-separated selection (`1,2,3`). Already-tracked repos are filtered
out, and it reports "No new repos to track" when nothing is left.

The GitHub token comes from `GITHUB_TOKEN`, falling back to `gh auth token` (private repos + higher
rate limits).

**Local Checkouts** (`src/services/clone_service.rs`): The `getrepos` command clones every tracked
repo to `./repos/owner/name` (relative to the current directory), mirroring the Release Tracker
desktop app's "Update Repos" flow but without prompting for a folder. A repo not yet on disk is
cloned; an existing one is `git pull --ff-only`'d; a working copy with uncommitted changes is left
untouched. The GitHub token is injected into the URL only for the git invocation (and reset out of
`origin` after clone) so private repos come down without the token being persisted to
`.git/config`, and it's scrubbed from any displayed error. Each repo's outcome is printed with a
summary; one repo failing never aborts the rest. This is a **manual** command — deliberately not in
`feeder run` or the timer.

**Notebrook Integration** (`lib/`): Separate crate providing `ChannelClient` for sending messages to notebrook channels.

### Configuration

Environment variables loaded from `.env`:
- `NOTEBROOK_URL`, `NOTEBROOK_TOKEN`, `NOTEBROOK_CHANNEL`
- Database stored at `./feeder.db` (configurable via `FEEDER_DB_PATH`)

### Notification Format

```
{feedTitle} {articleTitle}: {text} {links}
```

## Systemd Deployment

Service files in `services/` for running as a scheduled task:

The service runs from `/home/feeder` (the repo checkout). `/home/feeder/feeder` — the path
`feeder.service` execs — is a **symlink** to `target/release/feeder`, so a release build is
picked up by the next timer run with no copy step. The symlink is gitignored; `cargo clean`
or a debug-only build leaves it dangling until the next `cargo build --release`.

```bash
# Deploy: just rebuild
cargo build --release

# Install and enable timer (runs every 120 min)
sudo cp services/feeder.service services/feeder.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now feeder.timer
```

A single `feeder run` checks both feeds **and** GitHub releases, so the one
`feeder.timer` covers everything. (`feeder releases run` still exists for
running only the release check manually.)
