use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn feeder_cmd() -> Command {
    Command::cargo_bin("feeder").unwrap()
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
    let db_path = temp_dir.path().join("test.db");

    feeder_cmd()
        .arg("run")
        .arg("--skip-notify")
        .env("FEEDER_DB_PATH", db_path.to_str().unwrap())
        .env("NOTEBROOK_URL", "http://localhost:8080")
        .env("NOTEBROOK_TOKEN", "test-token")
        .env("NOTEBROOK_CHANNEL", "test-channel")
        .assert()
        .success()
        .stdout(predicate::str::contains("skip-notify mode"));
}

#[test]
fn test_run_with_both_flags_skip_notify_takes_precedence() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // When both flags are set, skip-notify mode message should appear
    feeder_cmd()
        .arg("run")
        .arg("--dry-run")
        .arg("--skip-notify")
        .env("FEEDER_DB_PATH", db_path.to_str().unwrap())
        .env("NOTEBROOK_URL", "http://localhost:8080")
        .env("NOTEBROOK_TOKEN", "test-token")
        .env("NOTEBROOK_CHANNEL", "test-channel")
        .assert()
        .success()
        .stdout(predicate::str::contains("skip-notify mode"));
}

mod skip_notify_integration {
    use super::*;

    #[test]
    fn test_skip_notify_no_feeds_configured() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test2.db");

        feeder_cmd()
            .arg("run")
            .arg("--skip-notify")
            .env("FEEDER_DB_PATH", db_path.to_str().unwrap())
            .env("NOTEBROOK_URL", "http://localhost:8080")
            .env("NOTEBROOK_TOKEN", "test-token")
            .env("NOTEBROOK_CHANNEL", "test-channel")
            .assert()
            .success()
            .stdout(predicate::str::contains("No feeds configured"));
    }

    #[test]
    fn test_dry_run_no_feeds_configured() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test3.db");

        feeder_cmd()
            .arg("run")
            .arg("--dry-run")
            .env("FEEDER_DB_PATH", db_path.to_str().unwrap())
            .env("NOTEBROOK_URL", "http://localhost:8080")
            .env("NOTEBROOK_TOKEN", "test-token")
            .env("NOTEBROOK_CHANNEL", "test-channel")
            .assert()
            .success()
            .stdout(predicate::str::contains("No feeds configured"));
    }

    #[test]
    fn test_dry_run_shows_fetching_message_without_skip_notify() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test4.db");

        // dry-run without skip-notify should NOT show skip-notify mode
        feeder_cmd()
            .arg("run")
            .arg("--dry-run")
            .env("FEEDER_DB_PATH", db_path.to_str().unwrap())
            .env("NOTEBROOK_URL", "http://localhost:8080")
            .env("NOTEBROOK_TOKEN", "test-token")
            .env("NOTEBROOK_CHANNEL", "test-channel")
            .assert()
            .success()
            .stdout(predicate::str::contains("Fetching feeds..."))
            .stdout(predicate::str::contains("skip-notify mode").not());
    }
}
