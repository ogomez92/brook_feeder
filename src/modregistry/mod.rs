//! Client for the Accessibility Mod Manager plugin registry.
//!
//! The manager is closed-source, but everything it reads to find mods is not:
//! a signed `plugin-registry.json` published as a GitHub release artifact, which
//! points at one per-author index per plugin, which in turn lists every release
//! with its download URL and SHA256. This module walks that same chain so the
//! artifacts can be mirrored locally instead of only ever passing through the
//! installer.
//!
//! Two things stay out of reach on purpose: releases the author put behind a
//! paid Patreon tier (the server answers 401 without an entitlement) are
//! recorded and announced but never fetched, and the registry's RSA-PSS
//! signature is mirrored as-is rather than re-signed.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use reqwest::blocking::{Client, Response};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::errors::{FeederError, FeederResult};

/// The registry the shipped app itself reads. `releases/latest/download/...`
/// always resolves to the newest published registry.
pub const REGISTRY_URL: &str = "https://github.com/RealAmethyst/accessibility-mod-manager-registry/releases/latest/download/plugin-registry.json";
/// Detached signature published next to the registry.
pub const REGISTRY_SIG_URL: &str = "https://github.com/RealAmethyst/accessibility-mod-manager-registry/releases/latest/download/plugin-registry.json.sig";
/// Repo whose releases carry the manager installer itself.
pub const MANAGER_REPO: &str = "RealAmethyst/AccessibilityModManager";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Downloads get longer — installer and framework zips are several MB.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const ATTEMPTS: u32 = 3;
/// Refuse absurd downloads outright; nothing in this registry is close to it.
const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
/// Sidecars carry a hash for the asset they sit next to, rather than being
/// artifacts in their own right.
const HASH_SUFFIX: &str = ".sha256";

// ---------------------------------------------------------------------------
// Registry / index models
//
// Every field is optional-with-default: this is someone else's schema, it has
// already gone from `registryVersion` 1 to 2, and a silent scheduled run must
// not die because a new key appeared or an old one went away.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRegistry {
    #[serde(default)]
    pub registry_version: Option<serde_json::Value>,
    #[serde(default)]
    pub plugins: Vec<RegistryPlugin>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryPlugin {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub repo_index_url: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginIndex {
    #[serde(default)]
    pub plugin_id: String,
    #[serde(default)]
    pub games: Vec<IndexGame>,
    #[serde(default)]
    pub releases_by_game_id: HashMap<String, Vec<IndexRelease>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexGame {
    pub game_id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub mod_name: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<IndexDependency>,
}

impl IndexGame {
    /// Prefer the author's own mod name, falling back to the game's name.
    pub fn label(&self) -> String {
        match self.mod_name.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name.to_string(),
            _ if !self.display_name.trim().is_empty() => self.display_name.trim().to_string(),
            _ => self.game_id.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexDependency {
    pub id: String,
    #[serde(default)]
    pub fix: Option<DependencyFix>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyFix {
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default)]
    pub auto_install: Option<AutoInstall>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoInstall {
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexRelease {
    #[serde(default)]
    pub game_id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub channel: String,
    /// Free download. `None` means the release is gated — see [`patreon`](Self::patreon).
    #[serde(default)]
    pub package_url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub changelog_url: Option<String>,
    #[serde(default)]
    pub patreon: Option<PatreonGate>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatreonGate {
    #[serde(default)]
    pub campaign_id: Option<String>,
    /// Where the gated package lives. Public URL, 401 without an entitlement.
    #[serde(default)]
    pub server_url: Option<String>,
}

/// One downloadable file attached to a GitHub release.
#[derive(Debug, Clone)]
pub struct ReleaseAsset {
    pub name: String,
    pub url: String,
}

/// The latest release of a GitHub repo, reduced to what the mirror needs.
#[derive(Debug, Clone)]
pub struct LatestRelease {
    pub tag_name: String,
    pub html_url: String,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    browser_download_url: String,
}

/// Result of mirroring one file.
#[derive(Debug)]
pub enum DownloadOutcome {
    /// Freshly downloaded and, where a hash was published, verified.
    Downloaded { bytes: u64 },
    /// Already on disk with the expected size/hash; nothing fetched.
    AlreadyPresent { bytes: u64 },
}

pub struct RegistryClient {
    client: Client,
    github_token: Option<String>,
}

impl RegistryClient {
    pub fn new(github_token: Option<String>) -> FeederResult<Self> {
        let client = Client::builder()
            .user_agent("feeder/1.0 (+https://github.com/feeder)")
            .timeout(DOWNLOAD_TIMEOUT)
            .build()?;

        Ok(Self {
            client,
            github_token,
        })
    }

    /// Fetch the signed registry, returning both the parsed form and the raw
    /// bytes so the exact published file can be mirrored verbatim.
    pub fn fetch_registry(&self) -> FeederResult<(PluginRegistry, Vec<u8>)> {
        let raw = self.get_bytes(REGISTRY_URL)?;
        let parsed: PluginRegistry = serde_json::from_slice(&raw).map_err(|e| {
            FeederError::JsonParse(format!("plugin-registry.json: {}", e))
        })?;
        Ok((parsed, raw))
    }

    /// Fetch the registry's detached signature. Mirrored as-is; feeder does not
    /// hold the public key, so this is archived rather than checked.
    pub fn fetch_registry_signature(&self) -> FeederResult<Vec<u8>> {
        self.get_bytes(REGISTRY_SIG_URL)
    }

    /// Fetch one plugin's index (its games, dependencies, and every release).
    pub fn fetch_index(&self, url: &str) -> FeederResult<(PluginIndex, Vec<u8>)> {
        let raw = self.get_bytes(url)?;
        let parsed: PluginIndex = serde_json::from_slice(&raw)
            .map_err(|e| FeederError::JsonParse(format!("{}: {}", url, e)))?;
        Ok((parsed, raw))
    }

    /// Latest release of `owner/name` via the REST API. The token is optional —
    /// these repos are public — but lifts the anonymous rate limit when set.
    pub fn fetch_latest_release(&self, repo: &str) -> FeederResult<LatestRelease> {
        let url = format!("https://api.github.com/repos/{}/releases/latest", repo);

        let raw = self.with_retries(&url, |client| {
            let mut req = client
                .get(&url)
                .timeout(REQUEST_TIMEOUT)
                .header("Accept", "application/vnd.github+json");
            if let Some(token) = &self.github_token {
                req = req.bearer_auth(token);
            }
            req.send()
        })?;

        let release: GithubRelease = serde_json::from_slice(&raw)
            .map_err(|e| FeederError::JsonParse(format!("{}: {}", url, e)))?;

        Ok(LatestRelease {
            tag_name: release.tag_name,
            html_url: release.html_url,
            assets: release
                .assets
                .into_iter()
                .filter(|a| !a.browser_download_url.is_empty())
                .map(|a| ReleaseAsset {
                    name: a.name,
                    url: a.browser_download_url,
                })
                .collect(),
        })
    }

    /// Read a `.sha256` sidecar asset and return the hex digest it carries.
    /// The file is either a bare digest or `<digest>  <filename>`.
    pub fn fetch_sidecar_hash(&self, url: &str) -> FeederResult<Option<String>> {
        let raw = self.get_bytes(url)?;
        Ok(parse_sidecar_hash(&String::from_utf8_lossy(&raw)))
    }

    /// Download `url` to `dest`, verifying `expected_sha256` when the publisher
    /// declared one. A file already on disk matching that hash is left alone, so
    /// re-running the mirror costs nothing.
    ///
    /// The bytes land in a temporary file and are only moved into place after
    /// verification, so an interrupted or corrupt download never leaves
    /// something that looks mirrored.
    pub fn download(
        &self,
        url: &str,
        dest: &Path,
        expected_sha256: Option<&str>,
    ) -> FeederResult<DownloadOutcome> {
        if let Some(existing) = self.existing_match(dest, expected_sha256)? {
            return Ok(DownloadOutcome::AlreadyPresent { bytes: existing });
        }

        let body = self.with_retries(url, |client| client.get(url).send())?;

        if let Some(expected) = expected_sha256 {
            let actual = hex_digest(&body);
            if !actual.eq_ignore_ascii_case(expected.trim()) {
                return Err(FeederError::FeedValidation(format!(
                    "SHA256 mismatch for {} (expected {}, got {})",
                    url,
                    expected.trim(),
                    actual
                )));
            }
        }

        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }

        // Same directory as the destination so the rename stays on one filesystem.
        let tmp = dest.with_extension("part");
        fs::write(&tmp, &body)?;
        fs::rename(&tmp, dest)?;

        Ok(DownloadOutcome::Downloaded {
            bytes: body.len() as u64,
        })
    }

    /// Write a fetched snapshot (the registry, a plugin index, a signature)
    /// straight to the mirror.
    pub fn write_snapshot(&self, dest: &Path, bytes: &[u8]) -> FeederResult<()> {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(dest, bytes)?;
        Ok(())
    }

    /// Is `dest` already the file we were about to fetch? With a published hash
    /// this is exact; without one, any existing non-empty file is taken as
    /// mirrored, since these URLs are immutable per version.
    fn existing_match(&self, dest: &Path, expected_sha256: Option<&str>) -> FeederResult<Option<u64>> {
        let metadata = match fs::metadata(dest) {
            Ok(m) => m,
            Err(_) => return Ok(None),
        };
        if !metadata.is_file() || metadata.len() == 0 {
            return Ok(None);
        }

        match expected_sha256 {
            Some(expected) => {
                let actual = hex_digest(&fs::read(dest)?);
                Ok(actual
                    .eq_ignore_ascii_case(expected.trim())
                    .then_some(metadata.len()))
            }
            None => Ok(Some(metadata.len())),
        }
    }

    fn get_bytes(&self, url: &str) -> FeederResult<Vec<u8>> {
        self.with_retries(url, |client| {
            client.get(url).timeout(REQUEST_TIMEOUT).send()
        })
    }

    /// Run a request with retries on transient failures, returning the body.
    /// A 4xx is not retried — a 401 on a gated package or a 404 on a renamed
    /// repo will not improve by asking again.
    fn with_retries(
        &self,
        url: &str,
        send: impl Fn(&Client) -> reqwest::Result<Response>,
    ) -> FeederResult<Vec<u8>> {
        let mut last_error = String::new();

        for attempt in 1..=ATTEMPTS {
            match send(&self.client) {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        return read_body(response);
                    }

                    last_error = format!("HTTP {} from {}", status.as_u16(), url);
                    if status.is_client_error() {
                        break;
                    }
                }
                Err(e) => last_error = format!("{}: {}", url, e),
            }

            if attempt < ATTEMPTS {
                std::thread::sleep(Duration::from_millis(500 * attempt as u64));
            }
        }

        Err(FeederError::Github(last_error))
    }
}

/// Read a response body, refusing anything over [`MAX_DOWNLOAD_BYTES`] — both
/// up front via `Content-Length` and while reading, since that header is a
/// claim, not a guarantee.
fn read_body(response: Response) -> FeederResult<Vec<u8>> {
    if let Some(len) = response.content_length() {
        if len > MAX_DOWNLOAD_BYTES {
            return Err(FeederError::PayloadTooLarge);
        }
    }

    let mut buffer = Vec::new();
    let mut reader = response.take(MAX_DOWNLOAD_BYTES + 1);
    reader.read_to_end(&mut buffer)?;

    if buffer.len() as u64 > MAX_DOWNLOAD_BYTES {
        return Err(FeederError::PayloadTooLarge);
    }

    Ok(buffer)
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Pull the digest out of a `.sha256` sidecar's text.
///
/// These files are written by whatever tool the publisher happened to use, so
/// they turn up as a bare digest, as `<digest>  <filename>`, and — as GitHub
/// serves the manager's — prefixed with a UTF-8 BOM. Missing the digest is
/// worse than it sounds: it does not fail loudly, it silently downgrades the
/// download to unverified.
fn parse_sidecar_hash(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|token| token.trim_matches(|c| c == '\u{feff}' || c == '\u{200b}'))
        .find(|token| is_sha256_hex(token))
        .map(str::to_lowercase)
}

fn is_sha256_hex(token: &str) -> bool {
    token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit())
}

/// Split a release-asset list into real artifacts and the `.sha256` sidecars
/// that describe them, keyed by the asset name they belong to.
pub fn split_sidecars(assets: &[ReleaseAsset]) -> (Vec<&ReleaseAsset>, HashMap<String, String>) {
    let mut hashes = HashMap::new();
    let mut primary = Vec::new();

    for asset in assets {
        match asset.name.strip_suffix(HASH_SUFFIX) {
            Some(target) => {
                hashes.insert(target.to_string(), asset.url.clone());
            }
            None => primary.push(asset),
        }
    }

    (primary, hashes)
}

/// Pull a version out of a GitHub release download URL
/// (`.../releases/download/<tag>/<file>`), which is how the registry pins its
/// framework dependencies. Falls back to a short digest so a dependency without
/// a recognizable tag still gets a stable, content-addressed key.
pub fn version_from_url(url: &str, sha256: Option<&str>) -> String {
    if let Some(rest) = url.split("/releases/download/").nth(1) {
        if let Some(tag) = rest.split('/').next() {
            let tag = tag.trim();
            if !tag.is_empty() {
                return tag.to_string();
            }
        }
    }

    match sha256 {
        Some(hash) if hash.len() >= 12 => hash[..12].to_lowercase(),
        _ => "unversioned".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_registry_with_unknown_fields() {
        let json = r#"{
            "registryVersion": "2",
            "updatedAt": "2026-07-24T18:00:00Z",
            "somethingNew": {"nested": true},
            "plugins": [{
                "id": "amethyst",
                "name": "Amethyst's Accessibility Mods",
                "repoIndexUrl": "https://example.com/index.json",
                "website": "https://example.com",
                "isBuiltIn": true
            }]
        }"#;

        let registry: PluginRegistry = serde_json::from_str(json).unwrap();
        assert_eq!(registry.plugins.len(), 1);
        assert_eq!(registry.plugins[0].id, "amethyst");
        assert_eq!(
            registry.plugins[0].repo_index_url.as_deref(),
            Some("https://example.com/index.json")
        );
    }

    #[test]
    fn parses_free_and_gated_releases() {
        let json = r#"{
            "pluginId": "amethyst",
            "games": [{
                "gameId": "dsts",
                "displayName": "Digimon Story Time Stranger",
                "modName": "Time Stranger Access",
                "dependencies": [{
                    "id": "melonloader",
                    "fix": {
                        "downloadUrl": "https://github.com/LavaGang/MelonLoader/releases/download/v0.7.2/MelonLoader.x64.zip",
                        "autoInstall": {"kind": "extractZip", "sha256": "5ced"}
                    }
                }]
            }],
            "releasesByGameId": {
                "dsts": [
                    {"gameId": "dsts", "version": "1.0", "channel": "stable",
                     "packageUrl": "https://example.com/a.zip", "sha256": "aa"},
                    {"gameId": "dsts", "version": "1.1-beta", "channel": "beta",
                     "packageUrl": null, "sha256": "bb",
                     "patreon": {"campaignId": "1", "serverUrl": "https://example.com/b.zip"}}
                ]
            }
        }"#;

        let index: PluginIndex = serde_json::from_str(json).unwrap();
        assert_eq!(index.games[0].label(), "Time Stranger Access");
        assert_eq!(index.games[0].dependencies.len(), 1);

        let releases = &index.releases_by_game_id["dsts"];
        assert_eq!(releases[0].package_url.as_deref(), Some("https://example.com/a.zip"));
        assert!(releases[1].package_url.is_none());
        assert_eq!(
            releases[1].patreon.as_ref().unwrap().server_url.as_deref(),
            Some("https://example.com/b.zip")
        );
    }

    #[test]
    fn game_label_falls_back_through_display_name_to_id() {
        let json = r#"{"gameId": "x", "displayName": "Some Game", "modName": ""}"#;
        let game: IndexGame = serde_json::from_str(json).unwrap();
        assert_eq!(game.label(), "Some Game");

        let json = r#"{"gameId": "x"}"#;
        let game: IndexGame = serde_json::from_str(json).unwrap();
        assert_eq!(game.label(), "x");
    }

    #[test]
    fn version_comes_from_the_release_tag() {
        assert_eq!(
            version_from_url(
                "https://github.com/BepInEx/BepInEx/releases/download/v5.4.23.5/BepInEx_win_x64.zip",
                None
            ),
            "v5.4.23.5"
        );
    }

    #[test]
    fn version_falls_back_to_a_short_digest() {
        assert_eq!(
            version_from_url("https://example.com/thing.zip", Some("ABCDEF0123456789")),
            "abcdef012345"
        );
        assert_eq!(version_from_url("https://example.com/thing.zip", None), "unversioned");
    }

    #[test]
    fn sidecars_are_matched_to_their_asset() {
        let assets = vec![
            ReleaseAsset {
                name: "Setup.exe".to_string(),
                url: "https://example.com/Setup.exe".to_string(),
            },
            ReleaseAsset {
                name: "Setup.exe.sha256".to_string(),
                url: "https://example.com/Setup.exe.sha256".to_string(),
            },
        ];

        let (primary, hashes) = split_sidecars(&assets);
        assert_eq!(primary.len(), 1);
        assert_eq!(primary[0].name, "Setup.exe");
        assert_eq!(
            hashes.get("Setup.exe").map(String::as_str),
            Some("https://example.com/Setup.exe.sha256")
        );
    }

    #[test]
    fn reads_a_digest_out_of_every_sidecar_shape() {
        let digest = "665d0d154e95f5546d67091239075c1e84007a59c1e0ff5caccadd20f8a1d0c0";

        // Bare digest, sha256sum's two-space form, and a leading UTF-8 BOM —
        // the last is what GitHub actually serves for the manager installer.
        assert_eq!(parse_sidecar_hash(digest).as_deref(), Some(digest));
        assert_eq!(
            parse_sidecar_hash(&format!("{}  Setup.exe\n", digest)).as_deref(),
            Some(digest)
        );
        assert_eq!(
            parse_sidecar_hash(&format!("\u{feff}{}", digest)).as_deref(),
            Some(digest)
        );
        assert_eq!(
            parse_sidecar_hash(&format!("\u{feff}{}\r\n", digest.to_uppercase())).as_deref(),
            Some(digest)
        );
    }

    #[test]
    fn a_sidecar_without_a_digest_yields_nothing() {
        assert_eq!(parse_sidecar_hash("not a hash at all"), None);
        assert_eq!(parse_sidecar_hash(""), None);
    }

    #[test]
    fn recognizes_a_sha256_token() {
        assert!(is_sha256_hex(&"a".repeat(64)));
        assert!(!is_sha256_hex(&"a".repeat(63)));
        assert!(!is_sha256_hex(&"z".repeat(64)));
    }

    #[test]
    fn digest_matches_a_known_value() {
        // SHA256 of the empty input.
        assert_eq!(
            hex_digest(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
