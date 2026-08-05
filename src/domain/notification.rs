use super::{Article, ArtifactKind, Feed, ModArtifact, RepoUpdate, TrackedRepo};

#[derive(Debug, Clone)]
pub struct Notification {
    pub feed_title: String,
    pub article_title: String,
    pub text: String,
    pub links: Vec<String>,
}

impl Notification {
    pub fn from_article(feed: &Feed, article: &Article) -> Self {
        let text = article.content.clone().unwrap_or_default();

        Self {
            feed_title: feed.title.clone(),
            article_title: article.title.clone(),
            text,
            links: article.links.clone(),
        }
    }

    /// Build a notification for a tracked repo's latest release or commit.
    /// Returns `None` when there is nothing to notify about.
    ///
    /// A release renders as `owner/name {releaseName}: {link}` and a commit as
    /// `owner/name {commit subject} {link}`, reusing the same format shape as
    /// article notifications.
    ///
    /// The release link points at `/releases/latest` rather than the tagged
    /// release page, so opening an older notification still lands on the newest
    /// release (and its assets).
    pub fn from_repo_update(repo: &TrackedRepo, update: &RepoUpdate) -> Option<Self> {
        match update {
            RepoUpdate::Release(release) => {
                let title = if release.name.trim().is_empty() {
                    release.tag_name.clone()
                } else {
                    release.name.clone()
                };
                Some(Self {
                    feed_title: repo.full_name(),
                    article_title: format!("new release {}", title),
                    text: String::new(),
                    links: vec![format!(
                        "https://github.com/{}/{}/releases/latest",
                        repo.owner, repo.name
                    )],
                })
            }
            RepoUpdate::Commit(commit) => {
                let subject = commit.message.lines().next().unwrap_or("").trim().to_string();
                Some(Self {
                    feed_title: repo.full_name(),
                    article_title: "new commit".to_string(),
                    text: subject,
                    links: vec![commit.html_url.clone()],
                })
            }
            RepoUpdate::None => None,
        }
    }

    /// Build a notification for a newly seen mod-registry artifact.
    ///
    /// The link is the human-facing page (changelog, release, author site)
    /// where the author published one. A Patreon-gated release is announced —
    /// knowing a new beta exists is the useful part — but says so, and never
    /// links straight at a URL that would answer 401.
    pub fn from_mod_artifact(artifact: &ModArtifact) -> Self {
        let descriptor = match artifact.kind {
            ArtifactKind::ModPackage => match artifact.channel.trim() {
                "" => "release".to_string(),
                channel => format!("{} release", channel),
            },
            ArtifactKind::Dependency => "dependency update".to_string(),
            ArtifactKind::Manager => "installer".to_string(),
        };

        let text = if artifact.gated {
            "Patreon-only, not mirrored".to_string()
        } else {
            String::new()
        };

        Self {
            feed_title: artifact.label.clone(),
            article_title: format!("new {} {}", descriptor, artifact.version),
            text,
            links: artifact.best_link().map(str::to_string).into_iter().collect(),
        }
    }

    /// Format: "{feedTitle} {articleTitle}: {text} {links (if any)}"
    pub fn format(&self) -> String {
        let mut message = format!("{} {}", self.feed_title, self.article_title);

        if !self.text.is_empty() {
            message.push_str(": ");
            message.push_str(&self.text);
        }

        if !self.links.is_empty() {
            message.push(' ');
            message.push_str(&self.links.join(" "));
        }

        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{FeedType, SourceType};

    #[test]
    fn test_notification_format_with_all_fields() {
        let notification = Notification {
            feed_title: "Tech Blog".to_string(),
            article_title: "New Rust Features".to_string(),
            text: "Rust 1.75 introduces async traits".to_string(),
            links: vec!["https://example.com/post".to_string()],
        };

        let formatted = notification.format();
        assert_eq!(
            formatted,
            "Tech Blog New Rust Features: Rust 1.75 introduces async traits https://example.com/post"
        );
    }

    #[test]
    fn test_notification_format_without_links() {
        let notification = Notification {
            feed_title: "Blog".to_string(),
            article_title: "Title".to_string(),
            text: "Content".to_string(),
            links: vec![],
        };

        let formatted = notification.format();
        assert_eq!(formatted, "Blog Title: Content");
    }

    #[test]
    fn test_notification_format_without_text() {
        let notification = Notification {
            feed_title: "Blog".to_string(),
            article_title: "Title".to_string(),
            text: String::new(),
            links: vec!["https://example.com".to_string()],
        };

        let formatted = notification.format();
        assert_eq!(formatted, "Blog Title https://example.com");
    }

    #[test]
    fn test_notification_from_article() {
        let feed = Feed::new(
            "https://example.com/feed".to_string(),
            "https://example.com/feed".to_string(),
            "Example Feed".to_string(),
            FeedType::Rss,
            SourceType::RssAtom,
        );

        let article = Article::new("123".to_string(), "Test Article".to_string())
            .with_content(Some("Article content".to_string()))
            .with_links(vec!["https://example.com/article".to_string()]);

        let notification = Notification::from_article(&feed, &article);

        assert_eq!(notification.feed_title, "Example Feed");
        assert_eq!(notification.article_title, "Test Article");
        assert_eq!(notification.text, "Article content");
        assert_eq!(notification.links, vec!["https://example.com/article"]);
    }

    #[test]
    fn test_notification_from_release_links_to_latest() {
        use crate::domain::{RepoRelease, RepoUpdate, TrackedRepo};

        let repo = TrackedRepo::new(
            "sveltejs".to_string(),
            "kit".to_string(),
            "https://github.com/sveltejs/kit".to_string(),
        );
        let update = RepoUpdate::Release(RepoRelease {
            tag_name: "v1.2.3".to_string(),
            name: "1.2.3".to_string(),
            published_at: None,
            html_url: "https://github.com/sveltejs/kit/releases/tag/v1.2.3".to_string(),
            body: String::new(),
        });

        let notification = Notification::from_repo_update(&repo, &update).unwrap();

        assert_eq!(
            notification.links,
            vec!["https://github.com/sveltejs/kit/releases/latest"]
        );
        assert_eq!(
            notification.format(),
            "sveltejs/kit new release 1.2.3 https://github.com/sveltejs/kit/releases/latest"
        );
    }

    #[test]
    fn test_notification_from_mod_package_names_the_channel() {
        use crate::domain::{ArtifactKind, ModArtifact};

        let artifact = ModArtifact {
            kind: ArtifactKind::ModPackage,
            source_id: "amethyst".to_string(),
            game_id: "masterduel".to_string(),
            label: "Master Duel Access".to_string(),
            version: "1.8".to_string(),
            channel: "stable".to_string(),
            url: Some("https://dl.example.com/md-v1.8-amm.zip".to_string()),
            sha256: None,
            gated: false,
            page_url: None,
        };

        assert_eq!(
            Notification::from_mod_artifact(&artifact).format(),
            "Master Duel Access new stable release 1.8 https://dl.example.com/md-v1.8-amm.zip"
        );
    }

    #[test]
    fn test_notification_from_gated_artifact_says_so_and_omits_the_file() {
        use crate::domain::{ArtifactKind, ModArtifact};

        let artifact = ModArtifact {
            kind: ArtifactKind::ModPackage,
            source_id: "amethyst".to_string(),
            game_id: "dscs".to_string(),
            label: "Cyber Sleuth Access".to_string(),
            version: "1.0-beta22".to_string(),
            channel: "beta".to_string(),
            url: Some("https://dl.example.com/gated.zip".to_string()),
            sha256: None,
            gated: true,
            page_url: Some("https://accessibilitymods.com".to_string()),
        };

        let notification = Notification::from_mod_artifact(&artifact);
        assert_eq!(notification.links, vec!["https://accessibilitymods.com"]);
        assert_eq!(
            notification.format(),
            "Cyber Sleuth Access new beta release 1.0-beta22: Patreon-only, not mirrored \
             https://accessibilitymods.com"
        );
    }

    #[test]
    fn test_notification_from_manager_installer() {
        use crate::domain::{ArtifactKind, ModArtifact};

        let artifact = ModArtifact {
            kind: ArtifactKind::Manager,
            source_id: "RealAmethyst/AccessibilityModManager".to_string(),
            game_id: String::new(),
            label: "Accessibility Mod Manager".to_string(),
            version: "v1.17.0".to_string(),
            channel: "stable".to_string(),
            url: Some("https://example.com/Setup.exe".to_string()),
            sha256: None,
            gated: false,
            page_url: Some("https://example.com/releases/tag/v1.17.0".to_string()),
        };

        assert_eq!(
            Notification::from_mod_artifact(&artifact).format(),
            "Accessibility Mod Manager new installer v1.17.0 \
             https://example.com/releases/tag/v1.17.0"
        );
    }

    #[test]
    fn test_notification_from_commit_keeps_commit_link() {
        use crate::domain::{RepoCommit, RepoUpdate, TrackedRepo};

        let repo = TrackedRepo::new(
            "a".to_string(),
            "b".to_string(),
            "https://github.com/a/b".to_string(),
        );
        let update = RepoUpdate::Commit(RepoCommit {
            sha: "abc123".to_string(),
            message: "fix thing\n\ndetails".to_string(),
            date: None,
            author: "someone".to_string(),
            html_url: "https://github.com/a/b/commit/abc123".to_string(),
        });

        let notification = Notification::from_repo_update(&repo, &update).unwrap();

        assert_eq!(
            notification.links,
            vec!["https://github.com/a/b/commit/abc123"]
        );
    }
}
