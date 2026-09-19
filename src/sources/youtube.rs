use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_LANGUAGE};
use scraper::{Html, Selector};
use serde_json::Value;
use url::Url;

use crate::domain::{Article, Feed, FeedType, SourceType};
use crate::errors::{FeederError, FeederResult};
use crate::sources::http;
use crate::sources::traits::{FeedMetadata, FeedSource};
use crate::sources::rss_atom::RssAtomSource;

/// How many uploads the page fallback reads: the same 15 the RSS feed
/// carries. Reading further back would announce older videos the feed never
/// showed — and so never recorded as notified — as if they were new.
const FEED_ENTRIES: usize = 15;

pub struct YouTubeSource {
    client: Client,
    rss_source: RssAtomSource,
}

impl YouTubeSource {
    pub fn new() -> Self {
        // Without a language YouTube localizes the page by IP ("hace 2 días").
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));

        Self {
            client: Client::builder()
                .user_agent("feeder/1.0 (+https://github.com/feeder)")
                .default_headers(headers)
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
            rss_source: RssAtomSource::new(),
        }
    }

    /// Extract channel ID from various YouTube URL formats
    fn extract_channel_id(&self, url: &str) -> FeederResult<String> {
        // Pattern 1: /channel/UC... URLs
        let channel_regex = Regex::new(r"youtube\.com/channel/(UC[\w-]{22})").unwrap();
        if let Some(caps) = channel_regex.captures(url) {
            return Ok(caps[1].to_string());
        }

        // Pattern 2: the feed URL itself (feeds/videos.xml?channel_id=UC...)
        if let Some(channel_id) = channel_id_from_feed_url(url) {
            return Ok(channel_id);
        }

        // Pattern 3: /@username or /c/customname URLs - need to fetch page and extract
        if url.contains("/@") || url.contains("/c/") || url.contains("/user/") {
            return self.extract_channel_id_from_page(url);
        }

        Err(FeederError::InvalidUrl(
            "Could not extract YouTube channel ID from URL".to_string(),
        ))
    }

    /// Fetch YouTube page and extract channel ID from meta tags or page content
    fn extract_channel_id_from_page(&self, url: &str) -> FeederResult<String> {
        let response = self.client.get(url).send()?;
        let html = response.text()?;
        let document = Html::parse_document(&html);

        // Try to find channel ID in meta tags
        let meta_selector = Selector::parse("meta[itemprop='channelId']").unwrap();
        if let Some(element) = document.select(&meta_selector).next() {
            if let Some(channel_id) = element.value().attr("content") {
                return Ok(channel_id.to_string());
            }
        }

        // Try to find in canonical link
        let link_selector = Selector::parse("link[rel='canonical']").unwrap();
        if let Some(element) = document.select(&link_selector).next() {
            if let Some(href) = element.value().attr("href") {
                let channel_regex = Regex::new(r"youtube\.com/channel/(UC[\w-]{22})").unwrap();
                if let Some(caps) = channel_regex.captures(href) {
                    return Ok(caps[1].to_string());
                }
            }
        }

        // Try to find in page content using regex
        let channel_regex = Regex::new(r#""channelId":"(UC[\w-]{22})""#).unwrap();
        if let Some(caps) = channel_regex.captures(&html) {
            return Ok(caps[1].to_string());
        }

        // Alternative regex pattern
        let alt_regex = Regex::new(r#"channel/(UC[\w-]{22})"#).unwrap();
        if let Some(caps) = alt_regex.captures(&html) {
            return Ok(caps[1].to_string());
        }

        Err(FeederError::FeedValidation(
            "Could not find channel ID on YouTube page".to_string(),
        ))
    }

    /// Build the RSS feed URL from a channel ID
    fn build_feed_url(&self, channel_id: &str) -> String {
        format!(
            "https://www.youtube.com/feeds/videos.xml?channel_id={}",
            channel_id
        )
    }

    /// Read the channel's most recent uploads from its uploads playlist page,
    /// along with the channel name.
    ///
    /// YouTube's RSS server goes down on its own — for hours most mornings,
    /// answering 404 or 500 for every channel — while the site itself keeps
    /// serving. The uploads playlist (`UU` + the channel ID's tail) lists the
    /// same videos as the feed in the same newest-first order, shorts
    /// included, so its first 15 stand in for the feed's 15. Articles get the
    /// feed's `yt:video:{id}` IDs so the dedup cache lines up either way.
    fn fetch_uploads(&self, channel_id: &str) -> FeederResult<(Option<String>, Vec<Article>)> {
        let html = http::get(&self.client, &uploads_page_url(channel_id))?.text()?;
        let data = initial_data(&html)?;
        let articles = uploads_from_initial_data(&data)?;
        let channel_name = data
            .pointer("/header/playlistHeaderRenderer/ownerText/runs/0/text")
            .and_then(Value::as_str)
            .map(str::to_string);
        Ok((channel_name, articles))
    }

    /// Normalize the channel URL by stripping tab paths like /videos, /shorts, /streams
    /// e.g., https://youtube.com/@user/videos -> https://youtube.com/@user
    fn normalize_channel_url(&self, url: &str) -> String {
        // YouTube tab paths that should be stripped
        let tab_paths = ["/videos", "/shorts", "/streams", "/playlists", "/community", "/channels", "/about", "/featured"];

        let mut normalized = url.to_string();
        for path in tab_paths {
            if normalized.ends_with(path) {
                normalized = normalized[..normalized.len() - path.len()].to_string();
                break;
            }
        }
        normalized
    }
}

impl Default for YouTubeSource {
    fn default() -> Self {
        Self::new()
    }
}

impl FeedSource for YouTubeSource {
    fn source_type(&self) -> SourceType {
        SourceType::YouTube
    }

    fn can_handle(&self, url: &str) -> bool {
        url.contains("youtube.com/channel/")
            || url.contains("youtube.com/feeds/videos.xml")
            || url.contains("youtube.com/@")
            || url.contains("youtube.com/c/")
            || url.contains("youtube.com/user/")
    }

    fn validate(&self, url: &str) -> FeederResult<FeedMetadata> {
        // Normalize the URL by stripping tab paths like /videos, /shorts, /streams, etc.
        let normalized_url = self.normalize_channel_url(url);
        let channel_id = self.extract_channel_id(&normalized_url)?;
        let feed_url = self.build_feed_url(&channel_id);

        // Check the feed URL answers before handing it to the RSS source, whose
        // discovery would otherwise go probing youtube.com for /feed/ and such.
        if let Err(feed_error) = http::get(&self.client, &feed_url) {
            // The feed server being down doesn't mean the channel is wrong: if
            // the uploads page reads, store the feed and let it come back.
            let (channel_name, _) = self.fetch_uploads(&channel_id).map_err(|page_error| {
                FeederError::FeedValidation(format!(
                    "YouTube RSS feed: {}; uploads page: {}",
                    feed_error, page_error
                ))
            })?;
            let title = channel_name.ok_or_else(|| {
                FeederError::FeedValidation(format!(
                    "YouTube RSS feed: {}; uploads page has no channel name",
                    feed_error
                ))
            })?;
            return Ok(FeedMetadata {
                title,
                feed_type: FeedType::Atom,
                feed_url,
                source_type: SourceType::YouTube,
                description: None,
            });
        }

        // Use the RSS source to validate the feed
        let mut metadata = self.rss_source.validate(&feed_url)?;
        metadata.source_type = SourceType::YouTube;

        Ok(metadata)
    }

    fn fetch_articles(&self, feed: &Feed) -> FeederResult<Vec<Article>> {
        let feed_error = match self.rss_source.fetch_articles(feed) {
            Ok(articles) => return Ok(articles),
            Err(e) => e,
        };
        let Some(channel_id) = channel_id_from_feed_url(&feed.feed_url) else {
            return Err(feed_error);
        };

        self.fetch_uploads(&channel_id)
            .map(|(_, articles)| articles)
            .map_err(|page_error| {
                FeederError::FeedParse(format!(
                    "YouTube RSS feed: {}; uploads page: {}",
                    feed_error, page_error
                ))
            })
    }
}

/// The channel ID in a `feeds/videos.xml?channel_id=UC...` URL.
fn channel_id_from_feed_url(feed_url: &str) -> Option<String> {
    let url = Url::parse(feed_url).ok()?;
    url.query_pairs()
        .find(|(key, _)| key == "channel_id")
        .map(|(_, value)| value.into_owned())
        .filter(|id| id.starts_with("UC") && id.len() > 2)
}

/// The channel's uploads playlist: its ID with the `UC` prefix swapped for `UU`.
fn uploads_page_url(channel_id: &str) -> String {
    let tail = channel_id.strip_prefix("UC").unwrap_or(channel_id);
    format!("https://www.youtube.com/playlist?list=UU{}", tail)
}

/// The `ytInitialData` JSON a YouTube page embeds in a script tag.
fn initial_data(html: &str) -> FeederResult<Value> {
    const MARKER: &str = "var ytInitialData = ";
    let start = html
        .find(MARKER)
        .ok_or_else(|| FeederError::FeedParse("no ytInitialData on the page".to_string()))?;

    // Parse one JSON value and ignore the `;</script>...` after it.
    serde_json::Deserializer::from_str(&html[start + MARKER.len()..])
        .into_iter::<Value>()
        .next()
        .ok_or_else(|| FeederError::FeedParse("empty ytInitialData".to_string()))?
        .map_err(|e| FeederError::FeedParse(format!("unreadable ytInitialData: {}", e)))
}

/// The first `FEED_ENTRIES` videos of an uploads playlist page, as articles
/// shaped like the RSS feed's.
fn uploads_from_initial_data(data: &Value) -> FeederResult<Vec<Article>> {
    // YouTube answers a bad playlist with a 200 and an alert.
    if let Some(alert) = data
        .pointer("/alerts/0/alertRenderer/text/runs/0/text")
        .and_then(Value::as_str)
    {
        return Err(FeederError::FeedParse(alert.to_string()));
    }

    // Only the playlist itself: the page has a sidebar too, and anything picked
    // up from outside the list would be announced as this channel's upload.
    let list = data
        .pointer("/contents/twoColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .ok_or_else(|| FeederError::FeedParse("no playlist on the uploads page".to_string()))?;

    let mut videos = Vec::new();
    collect_videos(list, &mut videos);
    if videos.is_empty() {
        return Err(FeederError::FeedParse(
            "no videos found on the uploads page".to_string(),
        ));
    }

    Ok(videos
        .into_iter()
        .take(FEED_ENTRIES)
        .map(|(id, title)| {
            let link = format!("https://www.youtube.com/watch?v={}", id);
            Article::new(format!("yt:video:{}", id), title).with_links(vec![link])
        })
        .collect())
}

/// Walk a page's JSON in document order collecting `(video_id, title)` pairs.
/// Handles the current `lockupViewModel` entries and the older
/// `playlistVideoRenderer` ones, which YouTube still serves to some requests.
fn collect_videos(node: &Value, out: &mut Vec<(String, String)>) {
    match node {
        Value::Object(map) => {
            if let Some(lockup) = map.get("lockupViewModel") {
                let id = lockup.get("contentId").and_then(Value::as_str);
                let title = lockup
                    .pointer("/metadata/lockupMetadataViewModel/title/content")
                    .and_then(Value::as_str);
                if let (Some(id), Some(title)) = (id, title) {
                    if is_video_id(id) {
                        out.push((id.to_string(), title.to_string()));
                    }
                }
                return;
            }
            if let Some(renderer) = map.get("playlistVideoRenderer") {
                let id = renderer.get("videoId").and_then(Value::as_str);
                let title = renderer
                    .pointer("/title/runs/0/text")
                    .or_else(|| renderer.pointer("/title/simpleText"))
                    .and_then(Value::as_str);
                if let (Some(id), Some(title)) = (id, title) {
                    if is_video_id(id) {
                        out.push((id.to_string(), title.to_string()));
                    }
                }
                return;
            }
            for child in map.values() {
                collect_videos(child, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_videos(item, out);
            }
        }
        _ => {}
    }
}

/// A video ID: 11 characters of URL-safe base64 (playlist IDs are longer).
fn is_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_handle_youtube_urls() {
        let source = YouTubeSource::new();

        assert!(source.can_handle("https://www.youtube.com/channel/UCxxx"));
        assert!(source.can_handle("https://youtube.com/@username"));
        assert!(source.can_handle("https://www.youtube.com/c/channelname"));
        assert!(source.can_handle("https://www.youtube.com/user/username"));

        assert!(!source.can_handle("https://example.com/feed"));
        assert!(!source.can_handle("https://mastodon.social/@user"));
    }

    #[test]
    fn test_source_type() {
        let source = YouTubeSource::new();
        assert_eq!(source.source_type(), SourceType::YouTube);
    }

    #[test]
    fn test_build_feed_url() {
        let source = YouTubeSource::new();
        let feed_url = source.build_feed_url("UCxxxxxxxxxxxxxxxxxxxxxxx");
        assert_eq!(
            feed_url,
            "https://www.youtube.com/feeds/videos.xml?channel_id=UCxxxxxxxxxxxxxxxxxxxxxxx"
        );
    }

    #[test]
    fn test_normalize_channel_url_strips_videos() {
        let source = YouTubeSource::new();
        assert_eq!(
            source.normalize_channel_url("https://www.youtube.com/@username/videos"),
            "https://www.youtube.com/@username"
        );
    }

    #[test]
    fn test_normalize_channel_url_strips_shorts() {
        let source = YouTubeSource::new();
        assert_eq!(
            source.normalize_channel_url("https://www.youtube.com/@username/shorts"),
            "https://www.youtube.com/@username"
        );
    }

    #[test]
    fn test_normalize_channel_url_strips_streams() {
        let source = YouTubeSource::new();
        assert_eq!(
            source.normalize_channel_url("https://www.youtube.com/@username/streams"),
            "https://www.youtube.com/@username"
        );
    }

    #[test]
    fn test_normalize_channel_url_strips_playlists() {
        let source = YouTubeSource::new();
        assert_eq!(
            source.normalize_channel_url("https://www.youtube.com/@username/playlists"),
            "https://www.youtube.com/@username"
        );
    }

    #[test]
    fn test_normalize_channel_url_preserves_clean_url() {
        let source = YouTubeSource::new();
        assert_eq!(
            source.normalize_channel_url("https://www.youtube.com/@username"),
            "https://www.youtube.com/@username"
        );
    }

    #[test]
    fn test_can_handle_feed_url() {
        let source = YouTubeSource::new();
        assert!(source.can_handle(
            "https://www.youtube.com/feeds/videos.xml?channel_id=UCNYW2vfGrUE6R5mIJYzkRyQ"
        ));
    }

    #[test]
    fn test_extract_channel_id_from_feed_url() {
        let source = YouTubeSource::new();
        assert_eq!(
            source
                .extract_channel_id(
                    "https://www.youtube.com/feeds/videos.xml?channel_id=UCNYW2vfGrUE6R5mIJYzkRyQ"
                )
                .unwrap(),
            "UCNYW2vfGrUE6R5mIJYzkRyQ"
        );
    }

    #[test]
    fn test_channel_id_from_feed_url() {
        assert_eq!(
            channel_id_from_feed_url(
                "https://www.youtube.com/feeds/videos.xml?channel_id=UC_aEa8K-EOJ3D6gOs7HcyNg"
            )
            .as_deref(),
            Some("UC_aEa8K-EOJ3D6gOs7HcyNg")
        );
        assert_eq!(channel_id_from_feed_url("https://www.youtube.com/feeds/videos.xml"), None);
        assert_eq!(channel_id_from_feed_url("not a url"), None);
    }

    #[test]
    fn test_uploads_page_url_swaps_uc_for_uu() {
        assert_eq!(
            uploads_page_url("UCNYW2vfGrUE6R5mIJYzkRyQ"),
            "https://www.youtube.com/playlist?list=UUNYW2vfGrUE6R5mIJYzkRyQ"
        );
    }

    fn lockup(id: &str, title: &str) -> Value {
        serde_json::json!({ "lockupViewModel": {
            "contentId": id,
            "contentType": "LOCKUP_CONTENT_TYPE_VIDEO",
            "metadata": { "lockupMetadataViewModel": { "title": { "content": title } } }
        }})
    }

    fn uploads_page(items: Vec<Value>) -> Value {
        serde_json::json!({
            "contents": { "twoColumnBrowseResultsRenderer": { "tabs": [{ "tabRenderer": {
                "content": { "sectionListRenderer": { "contents": [
                    { "itemSectionRenderer": { "contents": items } }
                ]}}
            }}]}},
            "header": { "playlistHeaderRenderer": { "ownerText": { "runs": [{ "text": "Some Channel" }] } } },
            "sidebar": { "playlistSidebarRenderer": { "items": [lockup("sidebarVid0", "Not an upload")] } }
        })
    }

    #[test]
    fn test_initial_data_read_from_script_tag() {
        let html = r#"<script nonce="x">var ytInitialData = {"a":{"b":"};</script>"}};</script><script>var other = 1;</script>"#;
        let data = initial_data(html).unwrap();
        assert_eq!(data.pointer("/a/b").and_then(Value::as_str), Some("};</script>"));
    }

    #[test]
    fn test_initial_data_missing() {
        assert!(initial_data("<html><body>consent wall</body></html>").is_err());
    }

    #[test]
    fn test_uploads_become_feed_shaped_articles() {
        let page = uploads_page(vec![
            lockup("fINORKvnxXQ", "Newest"),
            lockup("-sbu1KUNi7c", "A short"),
            serde_json::json!({ "continuationItemViewModel": {} }),
        ]);
        let articles = uploads_from_initial_data(&page).unwrap();

        assert_eq!(articles.len(), 2);
        assert_eq!(articles[0].id, "yt:video:fINORKvnxXQ");
        assert_eq!(articles[0].title, "Newest");
        assert_eq!(articles[0].links, vec!["https://www.youtube.com/watch?v=fINORKvnxXQ"]);
        assert_eq!(articles[1].id, "yt:video:-sbu1KUNi7c");
    }

    #[test]
    fn test_uploads_ignore_the_sidebar() {
        let page = uploads_page(vec![lockup("fINORKvnxXQ", "Upload")]);
        let articles = uploads_from_initial_data(&page).unwrap();
        assert!(articles.iter().all(|a| a.id != "yt:video:sidebarVid0"));
    }

    #[test]
    fn test_uploads_capped_at_feed_size() {
        // Anything past the feed's 15 was never in the feed, so never marked
        // notified: reading it would announce old videos as new.
        let items = (0..40)
            .map(|i| lockup(&format!("video{:06}", i), &format!("Video {}", i)))
            .collect();
        let articles = uploads_from_initial_data(&uploads_page(items)).unwrap();

        assert_eq!(articles.len(), FEED_ENTRIES);
        assert_eq!(articles[0].id, "yt:video:video000000");
        assert_eq!(articles[14].id, "yt:video:video000014");
    }

    #[test]
    fn test_uploads_read_older_playlist_renderer() {
        let page = uploads_page(vec![serde_json::json!({ "playlistVideoListRenderer": { "contents": [
            { "playlistVideoRenderer": { "videoId": "XX_puNAxn6s", "title": { "runs": [{ "text": "Old layout" }] } } }
        ]}})]);
        let articles = uploads_from_initial_data(&page).unwrap();

        assert_eq!(articles.len(), 1);
        assert_eq!(articles[0].id, "yt:video:XX_puNAxn6s");
        assert_eq!(articles[0].title, "Old layout");
    }

    #[test]
    fn test_uploads_skip_non_video_ids() {
        let page = uploads_page(vec![
            lockup("PLrAXtmErZgOeiKm4sgNOknGvNjby9efdf", "A playlist"),
            lockup("fINORKvnxXQ", "A video"),
        ]);
        let articles = uploads_from_initial_data(&page).unwrap();

        assert_eq!(articles.len(), 1);
        assert_eq!(articles[0].id, "yt:video:fINORKvnxXQ");
    }

    #[test]
    fn test_uploads_page_alert_is_an_error() {
        let page = serde_json::json!({ "alerts": [{ "alertRenderer": {
            "type": "ERROR", "text": { "runs": [{ "text": "The playlist does not exist." }] }
        }}]});
        let err = uploads_from_initial_data(&page).unwrap_err();
        assert!(err.to_string().contains("The playlist does not exist."));
    }

    #[test]
    fn test_uploads_page_without_videos_is_an_error() {
        assert!(uploads_from_initial_data(&uploads_page(vec![])).is_err());
    }

    #[test]
    fn test_normalize_channel_url_with_channel_id() {
        let source = YouTubeSource::new();
        assert_eq!(
            source.normalize_channel_url("https://www.youtube.com/channel/UCxxx/videos"),
            "https://www.youtube.com/channel/UCxxx"
        );
    }
}
