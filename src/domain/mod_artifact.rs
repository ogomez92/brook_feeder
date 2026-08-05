//! Downloadable artifacts discovered through the Accessibility Mod Manager
//! registry.
//!
//! The registry itself only carries metadata — every actual file lives on
//! whatever host its author chose (the mod author's own CDN, a GitHub release,
//! the upstream framework's release page). A [`ModArtifact`] is one such file
//! plus enough context to name it, place it on disk, and dedupe it across runs.

/// What an artifact is, which decides where it lands in the mirror and how its
/// notification reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    /// A game mod package published by a plugin author.
    ModPackage,
    /// A framework a mod is installed on top of (BepInEx, MelonLoader, ...),
    /// hosted by that framework's own author.
    Dependency,
    /// The Accessibility Mod Manager installer itself.
    Manager,
}

impl ArtifactKind {
    /// Stable string used in the database and in the mirror path.
    pub fn as_str(&self) -> &'static str {
        match self {
            ArtifactKind::ModPackage => "mod",
            ArtifactKind::Dependency => "dep",
            ArtifactKind::Manager => "manager",
        }
    }
}

/// How far an artifact got: what `mods list` reports and what a later run
/// decides to retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactStatus {
    /// Downloaded and (where a hash was published) verified.
    Mirrored,
    /// Seen, but the download needs a paid Patreon entitlement we don't have.
    Gated,
    /// Seen and announced, but not on disk yet — a failed download, or a run
    /// with `--no-download`. Retried on the next run without re-notifying.
    Pending,
}

impl ArtifactStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ArtifactStatus::Mirrored => "mirrored",
            ArtifactStatus::Gated => "gated",
            ArtifactStatus::Pending => "pending",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "mirrored" => ArtifactStatus::Mirrored,
            "gated" => ArtifactStatus::Gated,
            _ => ArtifactStatus::Pending,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModArtifact {
    pub kind: ArtifactKind,
    /// Registry plugin id (`amethyst`), dependency id (`bepinex`), or
    /// `owner/name` for the manager.
    pub source_id: String,
    /// Game the artifact belongs to; empty for dependencies and the manager.
    pub game_id: String,
    /// Human-readable name used in notifications.
    pub label: String,
    pub version: String,
    /// `stable`, `beta`, or empty when the source doesn't distinguish.
    pub channel: String,
    /// Where the file lives. Present even when [`gated`](Self::gated) is set —
    /// the URL is public, the bytes behind it are not.
    pub url: Option<String>,
    /// Publisher-declared SHA256, verified after download when present.
    pub sha256: Option<String>,
    /// The download needs a paid Patreon entitlement, so it is recorded and
    /// announced but never fetched.
    pub gated: bool,
    /// A page a human can open: changelog, release page, or the author's site.
    pub page_url: Option<String>,
}

impl ModArtifact {
    /// Dedup key, stable across runs. Mod packages are keyed by version so a
    /// re-published build under the same version is *not* re-announced;
    /// dependencies are keyed by their upstream version tag.
    pub fn cache_key(&self) -> String {
        match self.kind {
            ArtifactKind::ModPackage => format!(
                "{}:{}/{}:{}",
                self.kind.as_str(),
                self.source_id,
                self.game_id,
                self.version
            ),
            _ => format!("{}:{}:{}", self.kind.as_str(), self.source_id, self.version),
        }
    }

    /// Mirror path relative to the mirror root, ending in the file name.
    /// Returns `None` for an artifact with no URL to take a name from.
    pub fn relative_path(&self) -> Option<String> {
        let file = self.file_name()?;
        Some(match self.kind {
            ArtifactKind::ModPackage => format!(
                "plugins/{}/{}/{}/{}",
                sanitize_path_segment(&self.source_id),
                sanitize_path_segment(&self.game_id),
                sanitize_path_segment(&self.version),
                file
            ),
            ArtifactKind::Dependency => format!(
                "deps/{}/{}/{}",
                sanitize_path_segment(&self.source_id),
                sanitize_path_segment(&self.version),
                file
            ),
            ArtifactKind::Manager => {
                format!("manager/{}/{}", sanitize_path_segment(&self.version), file)
            }
        })
    }

    /// Last path segment of the URL, with any query string dropped.
    pub fn file_name(&self) -> Option<String> {
        let url = self.url.as_ref()?;
        let path = url.split(['?', '#']).next().unwrap_or(url);
        let name = path.rsplit('/').next().unwrap_or("").trim();
        if name.is_empty() {
            None
        } else {
            Some(sanitize_path_segment(name))
        }
    }

    /// The link worth putting in a notification: the human-facing page when the
    /// author published one, otherwise the file itself.
    pub fn best_link(&self) -> Option<&str> {
        self.page_url
            .as_deref()
            .or(if self.gated { None } else { self.url.as_deref() })
    }
}

/// A stored artifact: what was seen, where it went, and how far it got.
#[derive(Debug, Clone)]
pub struct ModArtifactRecord {
    pub cache_key: String,
    pub kind: String,
    pub source_id: String,
    pub game_id: String,
    pub label: String,
    pub version: String,
    pub channel: String,
    pub url: Option<String>,
    pub sha256: Option<String>,
    /// Path within the mirror root, once the file is actually on disk.
    pub local_path: Option<String>,
    pub status: ArtifactStatus,
    pub seen_at: String,
}

/// Keep a registry-supplied string usable as exactly one path segment: no
/// separators, no traversal, no control characters, never empty.
///
/// Everything that reaches the mirror's filesystem path — plugin ids, game ids,
/// versions, file names — is attacker-controlled in the sense that it comes
/// from a JSON document on someone else's server, so it all goes through here.
pub fn sanitize_path_segment(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();

    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "_".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(version: &str) -> ModArtifact {
        ModArtifact {
            kind: ArtifactKind::ModPackage,
            source_id: "amethyst".to_string(),
            game_id: "dsts".to_string(),
            label: "Digimon Story Time Stranger".to_string(),
            version: version.to_string(),
            channel: "beta".to_string(),
            url: Some(format!(
                "https://downloads.example.com/releases/dsts/{}/dsts-v{}-amm.zip",
                version, version
            )),
            sha256: None,
            gated: false,
            page_url: None,
        }
    }

    #[test]
    fn cache_key_is_versioned_per_game() {
        assert_eq!(package("1.0-beta04").cache_key(), "mod:amethyst/dsts:1.0-beta04");
        assert_ne!(package("1.0-beta04").cache_key(), package("1.0-beta05").cache_key());
    }

    #[test]
    fn relative_path_nests_by_plugin_game_version() {
        assert_eq!(
            package("1.0-beta04").relative_path().unwrap(),
            "plugins/amethyst/dsts/1.0-beta04/dsts-v1.0-beta04-amm.zip"
        );
    }

    #[test]
    fn file_name_drops_query_string() {
        let mut a = package("1.0");
        a.url = Some("https://example.com/a/b/thing.zip?token=abc".to_string());
        assert_eq!(a.file_name().unwrap(), "thing.zip");
    }

    #[test]
    fn path_segments_cannot_escape_the_mirror_root() {
        let mut a = package("1.0");
        a.game_id = "../../etc".to_string();
        a.version = "..".to_string();
        a.url = Some("https://example.com/../../passwd".to_string());

        let path = a.relative_path().unwrap();
        let components: Vec<_> = path.split('/').collect();

        // Registry-supplied text may still *contain* dots — what it may not do
        // is become a `..` component, add a level, or collapse into nothing.
        assert_eq!(components.len(), 5, "segment count changed: {}", path);
        assert!(
            components.iter().all(|c| !c.is_empty() && *c != "." && *c != ".."),
            "traversal survived sanitizing: {}",
            path
        );
    }

    #[test]
    fn a_segment_that_sanitizes_to_nothing_still_yields_one() {
        assert_eq!(sanitize_path_segment(".."), "_");
        assert_eq!(sanitize_path_segment("."), "_");
        assert_eq!(sanitize_path_segment("   "), "_");
        assert_eq!(sanitize_path_segment(""), "_");
    }

    #[test]
    fn gated_artifacts_never_offer_the_file_as_a_link() {
        let mut a = package("1.0");
        a.gated = true;
        assert_eq!(a.best_link(), None);

        a.page_url = Some("https://accessibilitymods.com".to_string());
        assert_eq!(a.best_link(), Some("https://accessibilitymods.com"));
    }

    #[test]
    fn dependency_key_ignores_game() {
        let dep = ModArtifact {
            kind: ArtifactKind::Dependency,
            source_id: "bepinex".to_string(),
            game_id: String::new(),
            label: "BepInEx".to_string(),
            version: "v5.4.23.5".to_string(),
            channel: String::new(),
            url: Some("https://example.com/BepInEx.zip".to_string()),
            sha256: None,
            gated: false,
            page_url: None,
        };
        assert_eq!(dep.cache_key(), "dep:bepinex:v5.4.23.5");
        assert_eq!(dep.relative_path().unwrap(), "deps/bepinex/v5.4.23.5/BepInEx.zip");
    }
}
