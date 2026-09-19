//! What went wrong during a run, collected so it can be **reported once, at the
//! end**, instead of failing the process.
//!
//! A scheduled run has nobody watching its exit code, but somebody does read
//! the Notebrook channel — so a broken feed, a repo GitHub would not answer
//! for, or a registry that answered 504 becomes a message there, and the
//! process still exits 0. The one thing that stays a non-zero exit is a
//! notification that could not be posted: the channel is the reporting path,
//! so when *it* is down the exit code is the only signal left.

use std::fmt::Display;

use super::Notification;

/// Errors lines beyond this many are summed up as `+N more` — a run where the
/// GraphQL endpoint was down produces one error per tracked repo, and three
/// hundred lines is not a report anyone reads.
const MAX_REPORT_LINES: usize = 25;

/// How many affected items are named when several share one error message.
const MAX_NAMED_ITEMS: usize = 3;

/// One thing that went wrong, attributed to the part of the run it happened in
/// (`feeds`, `releases`, `mods`) and — when it is about one item rather than
/// the whole part — to the item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunError {
    pub part: String,
    /// The feed, repo, or artifact concerned; `None` when the whole part failed.
    pub item: Option<String>,
    pub message: String,
}

#[derive(Debug, Default, Clone)]
pub struct RunReport {
    /// Errors that stopped one item or one part but not the run. Reported.
    pub errors: Vec<RunError>,
    /// Notifications that could not be posted. The item concerned is left
    /// unmarked so it is re-sent next run; the process exits non-zero.
    pub notify_failures: Vec<RunError>,
}

impl RunReport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a failure of the whole `part` (the registry fetch, the GraphQL
    /// batch, the database).
    pub fn part_failed(&mut self, part: &str, error: impl Display) {
        self.errors.push(RunError {
            part: part.to_string(),
            item: None,
            message: error.to_string(),
        });
    }

    /// Record a failure of one `item` within `part` (a feed, a repo, an artifact).
    pub fn item_failed(&mut self, part: &str, item: impl Display, error: impl Display) {
        self.errors.push(RunError {
            part: part.to_string(),
            item: Some(item.to_string()),
            message: error.to_string(),
        });
    }

    /// Record a notification that could not be posted for `item`.
    pub fn notify_failed(&mut self, part: &str, item: impl Display, error: impl Display) {
        self.notify_failures.push(RunError {
            part: part.to_string(),
            item: Some(item.to_string()),
            message: error.to_string(),
        });
    }

    pub fn is_clean(&self) -> bool {
        self.errors.is_empty() && self.notify_failures.is_empty()
    }

    /// Every line of the report, one per distinct `(part, message)`: when the
    /// same error hit many items — the GraphQL batch failing hits every repo
    /// in it — they are folded into one line naming the first few.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = fold(&self.errors);

        if !self.notify_failures.is_empty() {
            lines.push(format!(
                "[notify] {} notification(s) could not be posted and will be retried next run",
                self.notify_failures.len()
            ));
            lines.extend(fold(&self.notify_failures));
        }

        lines
    }

    pub fn error_count(&self) -> usize {
        self.errors.len() + self.notify_failures.len()
    }
}

/// Fold errors sharing a `(part, message)` into one line each, in first-seen
/// order.
fn fold(errors: &[RunError]) -> Vec<String> {
    let mut groups: Vec<(&RunError, Vec<&str>)> = Vec::new();

    for error in errors {
        match groups
            .iter_mut()
            .find(|(first, _)| first.part == error.part && first.message == error.message)
        {
            Some((_, items)) => items.extend(error.item.as_deref()),
            None => groups.push((error, error.item.as_deref().into_iter().collect())),
        }
    }

    groups
        .into_iter()
        .map(|(error, items)| match items.len() {
            0 => format!("[{}] {}", error.part, error.message),
            1 => format!("[{}] {}: {}", error.part, items[0], error.message),
            n => {
                let named = items
                    .iter()
                    .take(MAX_NAMED_ITEMS)
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ");
                let more = n.saturating_sub(MAX_NAMED_ITEMS);
                if more > 0 {
                    format!("[{}] {} items ({}, +{} more): {}", error.part, n, named, more, error.message)
                } else {
                    format!("[{}] {} items ({}): {}", error.part, n, named, error.message)
                }
            }
        })
        .collect()
}

impl Notification {
    /// The message that reports a run's errors to the channel: `None` when
    /// there is nothing to report.
    pub fn from_run_report(report: &RunReport) -> Option<Self> {
        if report.is_clean() {
            return None;
        }

        let lines = report.lines();
        let mut shown: Vec<String> = lines.iter().take(MAX_REPORT_LINES).cloned().collect();
        if lines.len() > MAX_REPORT_LINES {
            shown.push(format!("+{} more", lines.len() - MAX_REPORT_LINES));
        }

        let count = report.error_count();
        Some(Self {
            feed_title: "Feeder".to_string(),
            article_title: format!(
                "run finished with {} error{}",
                count,
                if count == 1 { "" } else { "s" }
            ),
            text: shown.join("\n"),
            links: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_report_has_no_notification() {
        assert!(Notification::from_run_report(&RunReport::new()).is_none());
    }

    #[test]
    fn whole_part_failure_is_one_line() {
        let mut report = RunReport::new();
        report.part_failed("mods", "GitHub API error: HTTP 504 from https://example");

        assert_eq!(
            report.lines(),
            vec!["[mods] GitHub API error: HTTP 504 from https://example"]
        );

        let notification = Notification::from_run_report(&report).unwrap();
        assert_eq!(notification.feed_title, "Feeder");
        assert_eq!(notification.article_title, "run finished with 1 error");
        assert!(notification.format().contains("HTTP 504"));
    }

    #[test]
    fn same_error_across_items_is_folded() {
        let mut report = RunReport::new();
        for repo in ["a/one", "b/two", "c/three", "d/four", "e/five"] {
            report.item_failed("releases", repo, "HTTP request failed: graphql");
        }
        report.item_failed("feeds", "SoundGuys", "no root element");

        assert_eq!(
            report.lines(),
            vec![
                "[releases] 5 items (a/one, b/two, c/three, +2 more): HTTP request failed: graphql",
                "[feeds] SoundGuys: no root element",
            ]
        );
        assert_eq!(
            Notification::from_run_report(&report).unwrap().article_title,
            "run finished with 6 errors"
        );
    }

    #[test]
    fn notify_failures_are_listed_and_counted() {
        let mut report = RunReport::new();
        report.notify_failed("feeds", "Xataka: some article", "connection refused");

        let lines = report.lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("[notify] 1 notification(s)"));
        assert_eq!(lines[1], "[feeds] Xataka: some article: connection refused");
        assert!(!report.is_clean());
    }

    #[test]
    fn long_reports_are_capped() {
        let mut report = RunReport::new();
        for i in 0..40 {
            report.item_failed("feeds", format!("feed {}", i), format!("error {}", i));
        }

        let text = Notification::from_run_report(&report).unwrap().text;
        assert_eq!(text.lines().filter(|l| !l.is_empty()).count(), MAX_REPORT_LINES + 1);
        assert!(text.ends_with("+15 more"));
    }
}
