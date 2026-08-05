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

# Accessibility Mod Manager registry mirroring
cargo run -- mods run                         # Pull registry, notify new artifacts, download them
cargo run -- mods run --dry-run               # Preview without notifying, downloading, or recording
cargo run -- mods run --skip-notify           # Mirror + record without notifying (use to seed a baseline)
cargo run -- mods run --no-download           # Notify/record only; files fetched on a later run
cargo run -- mods run --dir /path/to/mirror   # Override the mirror directory
cargo run -- mods list                        # Show every artifact seen and where it landed
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

**Storage** (`src/storage/`): SQLite with five tables:
- `feeds`: Stores original URL, resolved feed URL, title, type, source
- `notified_articles`: Cache keyed by `{feed_title}:{article_id}` for deduplication
- `tracked_repos`: GitHub repos to watch for releases (owner, name, url); unique on `owner/name` (case-insensitive)
- `notified_releases`: Cache keyed by `{owner}/{name}:release:{tag}` or `{owner}/{name}:commit:{sha}`
- `mod_artifacts`: Mod-registry inventory + dedup cache, keyed by
  `mod:{plugin}/{game}:{version}`, `dep:{id}:{version}`, or `manager:{owner}/{name}:{tag}`;
  carries the mirror path and a `mirrored`/`gated`/`pending` status

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

**Mod Registry Mirroring** (`src/modregistry/`, `src/services/mod_mirror_service.rs`): The
Accessibility Mod Manager is closed-source, but the chain it reads to find mods is public, and
`mods run` walks the same chain: the signed `plugin-registry.json` published as a GitHub release
artifact → each plugin's `repoIndexUrl` → that author's per-game releases. It mirrors three kinds of
artifact into `./modmirror` (override with `FEEDER_MOD_MIRROR` or `--dir`):

- **mod packages** — `plugins/{plugin}/{game}/{version}/`
- **dependencies** — the frameworks a mod installs on top of (BepInEx, MelonLoader, DSCSModLoader,
  BizHawk) and game installers the registry points at, under `deps/{id}/{version}/`
- **manager installer** — the app's own setup binary from its latest GitHub release, under
  `manager/{tag}/`

Registry metadata is snapshotted too (`registry/plugin-registry.json`, its `.sig`, and each
`plugins/{id}/index.json`), so the metadata is archived and not just the payloads.

Every download is verified against the publisher's SHA256 — from the index for mods and
dependencies, from the release's `.sha256` sidecar for the installer (that file is BOM-prefixed;
`parse_sidecar_hash` handles it, because failing to read it downgrades the download to unverified
*silently*). Bytes land in a `.part` file and are renamed into place only after verification.

**Announcing and mirroring are separate.** An artifact is announced once — the first run its
`cache_key` is seen — then downloaded. A failed download records `pending` and is retried on later
runs *without* re-notifying. Releases behind a paid Patreon tier (the server answers 401 without an
entitlement) are recorded and announced as `gated` but never fetched, and their notification links
to the author's site rather than a URL that would 401.

Unlike feeds and releases, which are driven by database contents, the registry is a fixed remote —
so this step always reaches the network. **`FEEDER_MODS=0` turns it off for `feeder run`** (an
explicit `feeder mods run` still works). The integration tests set it, along with a temp
`FEEDER_MOD_MIRROR`; without that, `cargo test` fetches the registry and downloads ~400 MB of mod
artifacts into the repo.

**Notebrook Integration** (`lib/`): Separate crate providing `ChannelClient` for sending messages to notebrook channels.

### Configuration

Environment variables loaded from `.env`:
- `NOTEBROOK_URL`, `NOTEBROOK_TOKEN`, `NOTEBROOK_CHANNEL`
- Database stored at `./feeder.db` (configurable via `FEEDER_DB_PATH`)
- `FEEDER_MOD_MIRROR`: mod-registry mirror directory (default `./modmirror`, gitignored)
- `FEEDER_MODS`: set to `0`/`false`/`off`/`no` to drop the mod-registry step from `feeder run`

### Notification Format

```
{feedTitle} {articleTitle}: {text} {links}
```

## Systemd Deployment

Service files in `services/` for running as a scheduled task:

The service runs from `/home/feeder` (the repo checkout). `/home/feeder/feeder` — the path
`feeder.service` execs — is a **plain copy** of `target/release/feeder`, gitignored. Deploying
means rebuilding *and* copying: a build alone changes nothing the timer runs. Do not turn it
back into a symlink to `target/release/feeder`; that was tried and caused problems.

```bash
# Deploy: rebuild, then copy over the running binary
cargo build --release
cp target/release/feeder /home/feeder/feeder

# Install and enable timer (runs every 120 min)
sudo cp services/feeder.service services/feeder.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now feeder.timer
```

A single `feeder run` checks feeds, GitHub releases, **and** the mod registry, so the
one `feeder.timer` covers everything. (`feeder releases run` and `feeder mods run`
still exist for running one part manually.)

Each part runs independently: a failure in one is reported but never skips the
others, and `run` exits non-zero if any part failed.
