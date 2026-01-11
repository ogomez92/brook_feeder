# Feeder

A multi-source feed aggregator CLI that sends new articles to [Notebrook](https://github.com/anthropics/notebrook).

## Supported Sources

| Source | Example URL | Auto-detected |
|--------|-------------|---------------|
| RSS/Atom | `https://blog.rust-lang.org/feed.xml` | Yes |
| YouTube | `https://youtube.com/@ChannelName` | Yes |
| Mastodon | `https://mastodon.social/@user` | Yes |
| WordPress | `https://example.com` (with wp-json) | Yes |
| Blogger | `https://example.blogspot.com` | Yes |

## Installation

### From Source

```bash
git clone https://github.com/ogomez92/feeder.git
cd feeder
cargo build --release
```

Binary will be at `target/release/feeder`.

### Configuration

Copy `.env.example` to `.env` and configure:

```bash
NOTEBROOK_URL=https://your-notebrook-instance.com
NOTEBROOK_TOKEN=your-api-token
NOTEBROOK_CHANNEL=feeds
```

## Usage

```bash
# Add feeds
feeder add https://blog.rust-lang.org/feed.xml
feeder add https://youtube.com/@ThePrimeTime
feeder add https://mastodon.social/@Gargron

# List configured feeds
feeder list

# Fetch and notify new articles
feeder run

# Dry run (show what would be sent)
feeder run --dry-run

# Skip notifications but mark articles as seen
# Useful after adding a feed to avoid notifications for old articles
feeder run --skip-notify

# Import/export OPML
feeder import feeds.opml
feeder export -o feeds.opml

# Remove a feed (interactive)
feeder remove
```

## Running as a Service

Systemd files are provided in `services/`:

```bash
# Install
sudo mkdir -p /opt/feeder
sudo cp target/release/feeder /opt/feeder/
sudo cp .env feeder.db /opt/feeder/
sudo cp services/feeder.service services/feeder.timer /etc/systemd/system/

# Enable (runs every 2 hours)
sudo systemctl daemon-reload
sudo systemctl enable --now feeder.timer

# Check status
systemctl status feeder.timer
journalctl -u feeder.service
```

## Contributing

### Building

```bash
cargo build              # Development
cargo build --release    # Release
cargo test               # Run tests
```

### Architecture

```
CLI (src/cli/) → Services (src/services/) → Sources/Storage
                                                  ↓
                                            Domain Models
```

- **Sources** (`src/sources/`): Implement `FeedSource` trait. Add new sources by creating a new file and registering in `SourceRegistry`.
- **Storage** (`src/storage/`): SQLite repositories for feeds and notification cache.
- **Services** (`src/services/`): Business logic for feed management, fetching, and notifications.
- **Notebrook client** (`lib/`): Separate crate for Notebrook API.

### Adding a New Source

1. Create `src/sources/newsource.rs` implementing `FeedSource` trait
2. Add detection logic in `can_handle()`
3. Implement URL-to-feed conversion in `validate()`
4. Register in `src/sources/registry.rs`

## License

MIT
