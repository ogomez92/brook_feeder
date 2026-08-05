//! Mirrors the Accessibility Mod Manager registry and everything it points at.
//!
//! Discovery walks the same chain the shipped app does — signed registry →
//! per-author plugin index → per-game releases — and adds the two things the
//! app installs alongside a mod: the frameworks each mod depends on (BepInEx,
//! MelonLoader, ...) and the manager's own installer.
//!
//! Announcing and mirroring are deliberately separate. An artifact is announced
//! once, the first run it is seen, keyed on its version; the download is then
//! retried on later runs until it lands, without ever notifying twice. That is
//! what keeps a failed download from going quiet and a flaky network from
//! spamming the channel.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::domain::{
    sanitize_path_segment, ArtifactKind, ArtifactStatus, ModArtifact, ModArtifactRecord,
};
use crate::errors::FeederResult;
use crate::modregistry::{
    split_sidecars, version_from_url, DownloadOutcome, RegistryClient, MANAGER_REPO,
};
use crate::storage::traits::ModArtifactRepository;

/// Everything one pass over the registry turned up.
pub struct Discovery {
    pub artifacts: Vec<ModArtifact>,
    /// Non-fatal problems: a plugin index that would not load, a manager
    /// release that could not be read. Reported, never fatal — one broken
    /// plugin must not cost the run every other plugin.
    pub warnings: Vec<String>,
}

/// What happened when a single artifact was mirrored.
#[derive(Debug)]
pub enum MirrorOutcome {
    Mirrored { path: String, bytes: u64 },
    AlreadyOnDisk { path: String },
    /// Behind a paid Patreon tier — recorded and announced, never fetched.
    Gated,
    Failed(String),
}

pub struct ModMirrorService<R: ModArtifactRepository> {
    repo: R,
    client: RegistryClient,
    mirror_dir: PathBuf,
}

impl<R: ModArtifactRepository> ModMirrorService<R> {
    pub fn new(repo: R, client: RegistryClient, mirror_dir: PathBuf) -> Self {
        Self {
            repo,
            client,
            mirror_dir,
        }
    }

    pub fn mirror_dir(&self) -> &Path {
        &self.mirror_dir
    }

    /// Walk the registry and return every artifact it leads to.
    ///
    /// `snapshot` also writes the registry, its signature, and each plugin index
    /// to the mirror, so the metadata is archived and not just the payloads.
    /// Only a failure to read the registry itself is fatal — it is the root of
    /// the chain, and without it there is nothing to walk.
    pub fn discover(&self, snapshot: bool) -> FeederResult<Discovery> {
        let mut artifacts = Vec::new();
        let mut warnings = Vec::new();
        let mut seen_keys = HashSet::new();

        let (registry, raw) = self.client.fetch_registry()?;

        if snapshot {
            self.client
                .write_snapshot(&self.mirror_dir.join("registry/plugin-registry.json"), &raw)?;

            match self.client.fetch_registry_signature() {
                Ok(sig) => self.client.write_snapshot(
                    &self.mirror_dir.join("registry/plugin-registry.json.sig"),
                    &sig,
                )?,
                Err(e) => warnings.push(format!("registry signature: {}", e)),
            }
        }

        for plugin in &registry.plugins {
            let index_url = match plugin.repo_index_url.as_deref().map(str::trim) {
                Some(url) if !url.is_empty() => url,
                _ => {
                    warnings.push(format!("plugin '{}' has no index URL", plugin.id));
                    continue;
                }
            };

            let (index, raw_index) = match self.client.fetch_index(index_url) {
                Ok(pair) => pair,
                Err(e) => {
                    warnings.push(format!("plugin '{}': {}", plugin.id, e));
                    continue;
                }
            };

            if snapshot {
                let dest = self
                    .mirror_dir
                    .join(format!("plugins/{}/index.json", sanitize_path_segment(&plugin.id)));
                if let Err(e) = self.client.write_snapshot(&dest, &raw_index) {
                    warnings.push(format!("plugin '{}' index snapshot: {}", plugin.id, e));
                }
            }

            for artifact in collect_plugin_artifacts(&plugin.id, plugin.website.as_deref(), &index) {
                if seen_keys.insert(artifact.cache_key()) {
                    artifacts.push(artifact);
                }
            }
        }

        match self.collect_manager_artifacts() {
            Ok(manager) => {
                for artifact in manager {
                    if seen_keys.insert(artifact.cache_key()) {
                        artifacts.push(artifact);
                    }
                }
            }
            Err(e) => warnings.push(format!("{}: {}", MANAGER_REPO, e)),
        }

        Ok(Discovery {
            artifacts,
            warnings,
        })
    }

    /// The manager installer itself, from the latest GitHub release. A
    /// `.sha256` sidecar is read for its digest rather than mirrored as a
    /// separate artifact.
    fn collect_manager_artifacts(&self) -> FeederResult<Vec<ModArtifact>> {
        let release = self.client.fetch_latest_release(MANAGER_REPO)?;
        let (primary, sidecars) = split_sidecars(&release.assets);

        let artifacts = primary
            .into_iter()
            .map(|asset| {
                let sha256 = sidecars
                    .get(&asset.name)
                    .and_then(|url| self.client.fetch_sidecar_hash(url).ok())
                    .flatten();

                ModArtifact {
                    kind: ArtifactKind::Manager,
                    source_id: MANAGER_REPO.to_string(),
                    game_id: String::new(),
                    label: "Accessibility Mod Manager".to_string(),
                    version: release.tag_name.clone(),
                    channel: "stable".to_string(),
                    url: Some(asset.url.clone()),
                    sha256,
                    gated: false,
                    page_url: Some(release.html_url.clone()),
                }
            })
            .collect();

        Ok(artifacts)
    }

    /// What we already know about an artifact, if anything.
    pub fn existing(&self, artifact: &ModArtifact) -> FeederResult<Option<ModArtifactRecord>> {
        self.repo.get(&artifact.cache_key())
    }

    /// Download an artifact into the mirror, verifying its published SHA256.
    pub fn mirror(&self, artifact: &ModArtifact) -> MirrorOutcome {
        if artifact.gated {
            return MirrorOutcome::Gated;
        }

        let url = match artifact.url.as_deref() {
            Some(url) => url,
            None => return MirrorOutcome::Failed("no download URL".to_string()),
        };

        let relative = match artifact.relative_path() {
            Some(path) => path,
            None => return MirrorOutcome::Failed("no usable file name".to_string()),
        };

        let dest = self.mirror_dir.join(&relative);

        match self
            .client
            .download(url, &dest, artifact.sha256.as_deref())
        {
            Ok(DownloadOutcome::Downloaded { bytes }) => MirrorOutcome::Mirrored {
                path: relative,
                bytes,
            },
            Ok(DownloadOutcome::AlreadyPresent { .. }) => {
                MirrorOutcome::AlreadyOnDisk { path: relative }
            }
            Err(e) => MirrorOutcome::Failed(e.to_string()),
        }
    }

    pub fn record(
        &self,
        artifact: &ModArtifact,
        status: ArtifactStatus,
        local_path: Option<&str>,
    ) -> FeederResult<()> {
        self.repo.record(artifact, status, local_path)
    }

    pub fn list(&self) -> FeederResult<Vec<ModArtifactRecord>> {
        self.repo.get_all()
    }
}

/// Turn one plugin index into artifacts: every release of every game, plus the
/// frameworks those games are installed on top of.
fn collect_plugin_artifacts(
    plugin_id: &str,
    website: Option<&str>,
    index: &crate::modregistry::PluginIndex,
) -> Vec<ModArtifact> {
    let mut artifacts = Vec::new();

    for (game_id, releases) in &index.releases_by_game_id {
        let label = index
            .games
            .iter()
            .find(|g| &g.game_id == game_id)
            .map(|g| g.label())
            .unwrap_or_else(|| game_id.clone());

        for release in releases {
            if release.version.trim().is_empty() {
                continue;
            }

            // A free release carries its own URL; a gated one only names where
            // the file sits on the author's server.
            let (url, gated) = match release.package_url.as_deref().map(str::trim) {
                Some(url) if !url.is_empty() => (Some(url.to_string()), false),
                _ => match release
                    .patreon
                    .as_ref()
                    .and_then(|p| p.server_url.as_deref())
                    .map(str::trim)
                {
                    Some(url) if !url.is_empty() => (Some(url.to_string()), true),
                    _ => (None, release.patreon.is_some()),
                },
            };

            if url.is_none() {
                continue;
            }

            artifacts.push(ModArtifact {
                kind: ArtifactKind::ModPackage,
                source_id: plugin_id.to_string(),
                game_id: game_id.clone(),
                label: label.clone(),
                version: release.version.trim().to_string(),
                channel: release.channel.trim().to_string(),
                url,
                sha256: non_empty(release.sha256.as_deref()),
                gated,
                page_url: non_empty(release.changelog_url.as_deref())
                    .or_else(|| non_empty(website)),
            });
        }
    }

    for game in &index.games {
        for dependency in &game.dependencies {
            let fix = match &dependency.fix {
                Some(fix) => fix,
                None => continue,
            };
            let url = match non_empty(fix.download_url.as_deref()) {
                Some(url) => url,
                None => continue,
            };

            let sha256 = fix
                .auto_install
                .as_ref()
                .and_then(|a| non_empty(a.sha256.as_deref()));

            artifacts.push(ModArtifact {
                kind: ArtifactKind::Dependency,
                source_id: dependency.id.clone(),
                game_id: String::new(),
                label: dependency.id.clone(),
                version: version_from_url(&url, sha256.as_deref()),
                channel: String::new(),
                url: Some(url),
                sha256,
                gated: false,
                page_url: None,
            });
        }
    }

    artifacts
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modregistry::PluginIndex;

    fn index() -> PluginIndex {
        serde_json::from_str(
            r#"{
              "pluginId": "amethyst",
              "games": [
                {
                  "gameId": "dsts",
                  "displayName": "Digimon Story Time Stranger",
                  "modName": "Time Stranger Access",
                  "dependencies": [{
                    "id": "melonloader",
                    "fix": {
                      "downloadUrl": "https://github.com/LavaGang/MelonLoader/releases/download/v0.7.2/MelonLoader.x64.zip",
                      "autoInstall": {"sha256": "5cedb66d"}
                    }
                  }]
                },
                {
                  "gameId": "masterduel",
                  "displayName": "Master Duel",
                  "dependencies": []
                }
              ],
              "releasesByGameId": {
                "dsts": [
                  {"gameId": "dsts", "version": "1.0", "channel": "stable",
                   "packageUrl": "https://dl.example.com/dsts/1.0/dsts-v1.0-amm.zip",
                   "sha256": "aaa", "changelogUrl": "https://example.com/notes"},
                  {"gameId": "dsts", "version": "1.1-beta", "channel": "beta",
                   "packageUrl": null, "sha256": "bbb",
                   "patreon": {"campaignId": "1",
                               "serverUrl": "https://dl.example.com/dsts/1.1/dsts-v1.1-amm.zip"}},
                  {"gameId": "dsts", "version": "", "packageUrl": "https://dl.example.com/x.zip"}
                ],
                "masterduel": [
                  {"gameId": "masterduel", "version": "1.8", "channel": "stable",
                   "packageUrl": "https://dl.example.com/md/1.8/md-v1.8-amm.zip", "sha256": "ccc"}
                ]
              }
            }"#,
        )
        .unwrap()
    }

    fn artifacts() -> Vec<ModArtifact> {
        collect_plugin_artifacts("amethyst", Some("https://accessibilitymods.com"), &index())
    }

    #[test]
    fn free_releases_are_downloadable_and_verified() {
        let free = artifacts()
            .into_iter()
            .find(|a| a.game_id == "masterduel")
            .unwrap();

        assert!(!free.gated);
        assert_eq!(free.sha256.as_deref(), Some("ccc"));
        assert_eq!(
            free.relative_path().unwrap(),
            "plugins/amethyst/masterduel/1.8/md-v1.8-amm.zip"
        );
    }

    #[test]
    fn patreon_releases_are_recorded_but_flagged_gated() {
        let gated = artifacts()
            .into_iter()
            .find(|a| a.version == "1.1-beta")
            .unwrap();

        assert!(gated.gated);
        assert!(gated.url.is_some(), "the location is public even when the bytes are not");
        // Nothing links a reader at a URL that will 401 on them.
        assert_eq!(gated.best_link(), Some("https://accessibilitymods.com"));
    }

    #[test]
    fn a_changelog_wins_over_the_author_website_as_the_link() {
        let free = artifacts()
            .into_iter()
            .find(|a| a.game_id == "dsts" && a.version == "1.0")
            .unwrap();
        assert_eq!(free.best_link(), Some("https://example.com/notes"));
    }

    #[test]
    fn releases_without_a_version_are_skipped() {
        assert!(artifacts().iter().all(|a| !a.version.is_empty()));
    }

    #[test]
    fn game_labels_come_from_the_mod_name() {
        let dsts = artifacts().into_iter().find(|a| a.game_id == "dsts").unwrap();
        assert_eq!(dsts.label, "Time Stranger Access");
    }

    #[test]
    fn dependencies_are_collected_and_versioned_from_their_tag() {
        let dep = artifacts()
            .into_iter()
            .find(|a| a.kind == ArtifactKind::Dependency)
            .unwrap();

        assert_eq!(dep.source_id, "melonloader");
        assert_eq!(dep.version, "v0.7.2");
        assert_eq!(dep.relative_path().unwrap(), "deps/melonloader/v0.7.2/MelonLoader.x64.zip");
    }

    #[test]
    fn every_artifact_has_a_distinct_cache_key() {
        let all = artifacts();
        let keys: HashSet<_> = all.iter().map(|a| a.cache_key()).collect();
        assert_eq!(keys.len(), all.len());
    }

    #[test]
    fn plugin_ids_stay_one_path_segment() {
        assert_eq!(sanitize_path_segment("amethyst"), "amethyst");

        let evil = sanitize_path_segment("../../evil");
        assert!(!evil.contains('/'), "index snapshot path escaped: {}", evil);
        assert_ne!(evil, "..");
    }
}
