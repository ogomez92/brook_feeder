# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo build              # Development build
cargo build --release    # Release build
cargo test               # Run all tests
cargo test <test_name>   # Run specific test
cargo run -- <command>   # Run CLI (e.g., cargo run -- list)
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

**Storage** (`src/storage/`): SQLite with two tables:
- `feeds`: Stores original URL, resolved feed URL, title, type, source
- `notified_articles`: Cache keyed by `{feed_title}:{article_id}` for deduplication

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

```bash
# Install to /opt/feeder
sudo mkdir -p /opt/feeder
sudo cp target/release/feeder /opt/feeder/
sudo cp .env feeder.db /opt/feeder/

# Install and enable timer (runs every 120 min)
sudo cp services/feeder.service services/feeder.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now feeder.timer
```
