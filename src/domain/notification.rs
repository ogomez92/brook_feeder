use super::{Article, ArtifactKind, Feed, ModArtifact, ReleaseAsset, RepoUpdate, TrackedRepo};

/// How many release assets a notification lists before it stops and points at
/// the release page for the rest. Releases with a build per platform/arch can
/// run to dozens of files; a message nobody can read through helps no one.
const MAX_LISTED_ASSETS: usize = 15;

#[derive(Debug, Clone)]
pub struct Notification {
    pub feed_title: String,
    pub article_title: String,
    pub text: String,
    pub links: Vec<String>,
    /// Files to offer for download, each on its own line under the message.
    /// Empty for everything except releases that ship assets.
    pub downloads: Vec<Download>,
}

/// One directly downloadable file, named so the line says what it is before
/// the URL says where it is.
#[derive(Debug, Clone)]
pub struct Download {
    pub label: String,
    pub url: String,
}

impl Notification {
    pub fn from_article(feed: &Feed, article: &Article) -> Self {
        let text = article.content.clone().unwrap_or_default();

        Self {
            feed_title: feed.title.clone(),
            article_title: article.title.clone(),
            text,
            links: article.links.clone(),
            downloads: Vec::new(),
        }
    }

    /// Build a notification for a tracked repo's latest release or commit.
    /// Returns `None` when there is nothing to notify about.
    ///
    /// A release renders as `owner/name {releaseName}: {link}` and a commit as
    /// `owner/name {commit subject} {link}`, reusing the same format shape as
    /// article notifications. A release that ships files lists each of them
    /// below, so a download is one click from the message rather than a trip
    /// through the release page.
    ///
    /// Every release link points at `/releases/latest...` rather than the
    /// tagged release, so opening an older notification still lands on the
    /// newest release and pulls the newest build. (The trade-off: once a newer
    /// release renames a file — most asset names carry the version — that
    /// asset link 404s and the release page link above it is the way in.)
    pub fn from_repo_update(repo: &TrackedRepo, update: &RepoUpdate) -> Option<Self> {
        match update {
            RepoUpdate::Release(release) => {
                let title = if release.name.trim().is_empty() {
                    release.tag_name.clone()
                } else {
                    release.name.clone()
                };
                let page = format!(
                    "https://github.com/{}/{}/releases/latest",
                    repo.owner, repo.name
                );

                let mut downloads: Vec<Download> = release
                    .assets
                    .iter()
                    .take(MAX_LISTED_ASSETS)
                    .map(|asset| Download {
                        label: format!("{} ({})", asset.name, human_size(asset.size)),
                        url: latest_asset_url(&repo.owner, &repo.name, asset),
                    })
                    .collect();

                let listed = downloads.len();
                if release.total_assets > listed {
                    downloads.push(Download {
                        label: format!("+{} more file(s)", release.total_assets - listed),
                        url: page.clone(),
                    });
                }

                Some(Self {
                    feed_title: repo.full_name(),
                    article_title: format!("new release {}", title),
                    text: String::new(),
                    links: vec![page],
                    downloads,
                })
            }
            RepoUpdate::Commit(commit) => {
                let subject = commit.message.lines().next().unwrap_or("").trim().to_string();
                Some(Self {
                    feed_title: repo.full_name(),
                    article_title: "new commit".to_string(),
                    text: subject,
                    links: vec![commit.html_url.clone()],
                    downloads: Vec::new(),
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
            downloads: Vec::new(),
        }
    }

    /// Format: "{feedTitle} {articleTitle}: {text} {links (if any)}", then one
    /// "{label} {url}" line per download when the update ships files.
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

        if !self.downloads.is_empty() {
            message.push_str("\nDownloads:");
            for download in &self.downloads {
                message.push_str(&format!("\n{} {}", download.label, download.url));
            }
        }

        message
    }
}

/// The `/releases/latest/download/{file}` URL for an asset: GitHub redirects it
/// to whichever release is newest, so the link keeps fetching the current build
/// instead of the one that happened to be out when the message was sent.
///
/// GitHub's own tag-pinned `downloadUrl` already carries a correctly encoded
/// filename, so the file name is lifted from there rather than re-encoded;
/// anything unexpected falls back to encoding the asset name as a path segment.
fn latest_asset_url(owner: &str, name: &str, asset: &ReleaseAsset) -> String {
    let base = format!("https://github.com/{}/{}/releases/latest/download/", owner, name);
    let marker = format!("/{}/{}/releases/download/", owner, name);

    if let Some(index) = asset.download_url.find(&marker) {
        // What follows the marker is "{tag}/{file}"; a tag may itself contain
        // slashes (release/1.2), a file name cannot, so take the last segment.
        let rest = &asset.download_url[index + marker.len()..];
        if let Some((_, file)) = rest.rsplit_once('/') {
            if !file.is_empty() {
                return format!("{}{}", base, file);
            }
        }
    }

    match url::Url::parse(&base) {
        Ok(mut parsed) => {
            match parsed.path_segments_mut() {
                Ok(mut segments) => {
                    segments.pop_if_empty().push(&asset.name);
                }
                Err(()) => return format!("{}{}", base, asset.name),
            }
            parsed.to_string()
        }
        Err(_) => format!("{}{}", base, asset.name),
    }
}

/// Render a byte count the way a download would: "12.3 MB", "914 KB".
fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];

    if bytes < 1024 {
        return format!("{} B", bytes);
    }

    let mut value = bytes as f64 / KB;
    let mut unit = UNITS[0];
    for next in &UNITS[1..] {
        if value < 1024.0 {
            break;
        }
        value /= KB;
        unit = next;
    }

    if value < 10.0 {
        format!("{:.1} {}", value, unit)
    } else {
        format!("{:.0} {}", value, unit)
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
            downloads: Vec::new(),
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
            downloads: Vec::new(),
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
            downloads: Vec::new(),
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
            assets: Vec::new(),
            total_assets: 0,
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

    #[test]
    fn test_notification_lists_release_assets_as_latest_downloads() {
        use crate::domain::{ReleaseAsset, RepoRelease, RepoUpdate, TrackedRepo};

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
            assets: vec![
                ReleaseAsset {
                    name: "kit-linux-x64.tar.gz".to_string(),
                    download_url:
                        "https://github.com/sveltejs/kit/releases/download/v1.2.3/kit-linux-x64.tar.gz"
                            .to_string(),
                    size: 12_900_000,
                },
                ReleaseAsset {
                    name: "kit-setup.exe".to_string(),
                    download_url:
                        "https://github.com/sveltejs/kit/releases/download/v1.2.3/kit-setup.exe"
                            .to_string(),
                    size: 936_000,
                },
            ],
            total_assets: 2,
        });

        let notification = Notification::from_repo_update(&repo, &update).unwrap();

        assert_eq!(
            notification.format(),
            "sveltejs/kit new release 1.2.3 https://github.com/sveltejs/kit/releases/latest\n\
             Downloads:\n\
             kit-linux-x64.tar.gz (12 MB) \
             https://github.com/sveltejs/kit/releases/latest/download/kit-linux-x64.tar.gz\n\
             kit-setup.exe (914 KB) \
             https://github.com/sveltejs/kit/releases/latest/download/kit-setup.exe"
        );
    }

    #[test]
    fn test_notification_caps_the_asset_list_and_says_how_many_are_left() {
        use crate::domain::{ReleaseAsset, RepoRelease, RepoUpdate, TrackedRepo};

        let repo = TrackedRepo::new(
            "a".to_string(),
            "b".to_string(),
            "https://github.com/a/b".to_string(),
        );
        let assets: Vec<ReleaseAsset> = (0..20)
            .map(|i| ReleaseAsset {
                name: format!("file{}.zip", i),
                download_url: format!("https://github.com/a/b/releases/download/v1/file{}.zip", i),
                size: 1024,
            })
            .collect();
        let update = RepoUpdate::Release(RepoRelease {
            tag_name: "v1".to_string(),
            name: "v1".to_string(),
            published_at: None,
            html_url: "https://github.com/a/b/releases/tag/v1".to_string(),
            body: String::new(),
            assets,
            total_assets: 42,
        });

        let notification = Notification::from_repo_update(&repo, &update).unwrap();

        assert_eq!(notification.downloads.len(), MAX_LISTED_ASSETS + 1);
        let last = notification.downloads.last().unwrap();
        assert_eq!(last.label, "+27 more file(s)");
        assert_eq!(last.url, "https://github.com/a/b/releases/latest");
    }

    #[test]
    fn test_latest_asset_url_keeps_githubs_encoding_and_odd_tags() {
        let asset = ReleaseAsset {
            name: "my tool.zip".to_string(),
            download_url: "https://github.com/a/b/releases/download/release/1.2/my%20tool.zip"
                .to_string(),
            size: 0,
        };
        assert_eq!(
            latest_asset_url("a", "b", &asset),
            "https://github.com/a/b/releases/latest/download/my%20tool.zip"
        );
    }

    #[test]
    fn test_latest_asset_url_encodes_when_the_download_url_is_unusable() {
        let asset = ReleaseAsset {
            name: "my tool.zip".to_string(),
            download_url: "https://cdn.example.com/whatever".to_string(),
            size: 0,
        };
        assert_eq!(
            latest_asset_url("a", "b", &asset),
            "https://github.com/a/b/releases/latest/download/my%20tool.zip"
        );
    }

    #[test]
    fn test_human_size() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(936_000), "914 KB");
        assert_eq!(human_size(1_572_864), "1.5 MB");
        assert_eq!(human_size(12_900_000), "12 MB");
        assert_eq!(human_size(3_221_225_472), "3.0 GB");
    }

}
