use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn feeder_cmd() -> Command {
    Command::cargo_bin("feeder").unwrap()
}

/// A `run` invocation isolated from the developer's machine: its own database,
/// its own mirror directory, and no mod-registry step.
///
/// That last part matters. Feeds and releases are driven by database contents,
/// so an empty temp database makes them network-free; the mod registry is a
/// fixed remote, so leaving it on would make every `cargo test` fetch it and
/// download hundreds of megabytes of mod artifacts into the repo.
fn isolated_run(temp_dir: &TempDir, args: &[&str]) -> Command {
    let mut cmd = feeder_cmd();
    cmd.arg("run")
        .args(args)
        .env("FEEDER_DB_PATH", temp_dir.path().join("test.db"))
        .env("FEEDER_MOD_MIRROR", temp_dir.path().join("mirror"))
        .env("FEEDER_MODS", "0")
        .env("NOTEBROOK_URL", "http://localhost:8080")
        .env("NOTEBROOK_TOKEN", "test-token")
        .env("NOTEBROOK_CHANNEL", "test-channel");
    cmd
}

#[test]
fn test_help_shows_skip_notify_flag() {
    feeder_cmd()
        .arg("run")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--skip-notify"));
}

#[test]
fn test_help_shows_dry_run_flag() {
    feeder_cmd()
        .arg("run")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--dry-run"));
}

#[test]
fn test_skip_notify_flag_description() {
    feeder_cmd()
        .arg("run")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Skip notifications but still mark articles as seen"));
}

#[test]
fn test_run_with_skip_notify_shows_mode_message() {
    let temp_dir = TempDir::new().unwrap();

    isolated_run(&temp_dir, &["--skip-notify"])
        .assert()
        .success()
        .stdout(predicate::str::contains("skip-notify mode"));
}

#[test]
fn test_run_with_both_flags_skip_notify_takes_precedence() {
    let temp_dir = TempDir::new().unwrap();

    // When both flags are set, skip-notify mode message should appear
    isolated_run(&temp_dir, &["--dry-run", "--skip-notify"])
        .assert()
        .success()
        .stdout(predicate::str::contains("skip-notify mode"));
}

mod skip_notify_integration {
    use super::*;

    #[test]
    fn test_skip_notify_no_feeds_configured() {
        let temp_dir = TempDir::new().unwrap();

        isolated_run(&temp_dir, &["--skip-notify"])
            .assert()
            .success()
            .stdout(predicate::str::contains("No feeds configured"));
    }

    #[test]
    fn test_dry_run_no_feeds_configured() {
        let temp_dir = TempDir::new().unwrap();

        isolated_run(&temp_dir, &["--dry-run"])
            .assert()
            .success()
            .stdout(predicate::str::contains("No feeds configured"));
    }

    #[test]
    fn test_dry_run_shows_fetching_message_without_skip_notify() {
        let temp_dir = TempDir::new().unwrap();

        // dry-run without skip-notify should NOT show skip-notify mode
        isolated_run(&temp_dir, &["--dry-run"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Fetching feeds..."))
            .stdout(predicate::str::contains("skip-notify mode").not());
    }
}

mod mod_registry {
    use super::*;

    #[test]
    fn test_mods_help_lists_subcommands() {
        feeder_cmd()
            .arg("mods")
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("run"))
            .stdout(predicate::str::contains("list"));
    }

    #[test]
    fn test_mods_run_help_shows_no_download_flag() {
        feeder_cmd()
            .arg("mods")
            .arg("run")
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("--no-download"))
            .stdout(predicate::str::contains("--dir"));
    }

    /// `FEEDER_MODS=0` must keep a combined `run` off the network entirely —
    /// this is what stops the test suite (and anyone's timer) from pulling the
    /// registry when they did not ask for it.
    #[test]
    fn test_run_reports_the_registry_step_as_disabled() {
        let temp_dir = TempDir::new().unwrap();

        isolated_run(&temp_dir, &["--dry-run"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Mod registry: disabled"))
            .stdout(predicate::str::contains("Pulling mod registry").not());
    }

    /// Nothing is written to the mirror when the step is disabled.
    #[test]
    fn test_disabled_run_creates_no_mirror() {
        let temp_dir = TempDir::new().unwrap();

        isolated_run(&temp_dir, &["--skip-notify"]).assert().success();

        assert!(
            !temp_dir.path().join("mirror").exists(),
            "a disabled mod-registry step must not create a mirror directory"
        );
    }

    #[test]
    fn test_mods_list_is_empty_for_a_fresh_database() {
        let temp_dir = TempDir::new().unwrap();

        feeder_cmd()
            .arg("mods")
            .arg("list")
            .env("FEEDER_DB_PATH", temp_dir.path().join("test.db"))
            .env("FEEDER_MOD_MIRROR", temp_dir.path().join("mirror"))
            .env("NOTEBROOK_URL", "http://localhost:8080")
            .env("NOTEBROOK_TOKEN", "test-token")
            .env("NOTEBROOK_CHANNEL", "test-channel")
            .assert()
            .success()
            .stdout(predicate::str::contains("Nothing mirrored yet"));
    }
}

/// How a run's errors are reported: as one message to the channel, with the
/// process exiting 0 — unless that message (or any other notification) could
/// not be posted, which is the one failure that still exits 1.
mod error_reporting {
    use super::*;
    use feeder::domain::{Feed, FeedType, SourceType};
    use feeder::storage::sqlite::{SqliteFeedRepository, SqliteStorage};
    use feeder::storage::FeedRepository;

    /// A Notebrook nobody is listening on. Port 9 (discard) is never bound
    /// on a modern host, so posting there fails fast rather than hanging.
    const DEAD_NOTEBROOK: &str = "http://127.0.0.1:9";

    /// Seed the temp database with a feed whose URL refuses connections, so
    /// the feeds part produces exactly one error without touching the network.
    fn seed_broken_feed(temp_dir: &TempDir) {
        let storage = SqliteStorage::new(temp_dir.path().join("test.db").to_str().unwrap()).unwrap();
        let repo = SqliteFeedRepository::new(storage);
        repo.add(&Feed::new(
            "http://127.0.0.1:9/".to_string(),
            "http://127.0.0.1:9/feed.xml".to_string(),
            "Broken Feed".to_string(),
            FeedType::Rss,
            SourceType::RssAtom,
        ))
        .unwrap();
    }

    fn run_with_dead_notebrook(temp_dir: &TempDir, args: &[&str]) -> Command {
        let mut cmd = isolated_run(temp_dir, args);
        cmd.env("NOTEBROOK_URL", DEAD_NOTEBROOK);
        cmd
    }

    /// A feed that fails is an error to report, not a reason to fail the run.
    #[test]
    fn test_part_errors_are_reported_and_exit_zero() {
        let temp_dir = TempDir::new().unwrap();
        seed_broken_feed(&temp_dir);

        run_with_dead_notebrook(&temp_dir, &["--dry-run"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Broken Feed: error:"))
            .stdout(predicate::str::contains("[DRY RUN] Feeder run finished with 1 error"))
            .stdout(predicate::str::contains("[feeds] Broken Feed:"));
    }

    /// The report is posted like any other notification, and a report that
    /// cannot be posted is the failure the exit code is for.
    #[test]
    fn test_unpostable_error_report_exits_one() {
        let temp_dir = TempDir::new().unwrap();
        seed_broken_feed(&temp_dir);

        run_with_dead_notebrook(&temp_dir, &[])
            .assert()
            .failure()
            .stdout(predicate::str::contains("Sending error report... FAILED"))
            .stderr(predicate::str::contains("could not post the error report"));
    }

    /// With nothing to report there is nothing to post, so an unreachable
    /// Notebrook is not, by itself, a failure.
    #[test]
    fn test_clean_run_does_not_touch_notebrook() {
        let temp_dir = TempDir::new().unwrap();

        run_with_dead_notebrook(&temp_dir, &[])
            .assert()
            .success()
            .stdout(predicate::str::contains("Sending error report").not());
    }

    /// `--skip-notify` skips the report too — it is a notification.
    #[test]
    fn test_skip_notify_prints_the_report_instead_of_posting() {
        let temp_dir = TempDir::new().unwrap();
        seed_broken_feed(&temp_dir);

        run_with_dead_notebrook(&temp_dir, &["--skip-notify"])
            .assert()
            .success()
            .stdout(predicate::str::contains("[SKIP] Feeder run finished with 1 error"));
    }
}
