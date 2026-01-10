use super::{Article, Feed};

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
}
