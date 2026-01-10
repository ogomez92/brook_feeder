use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Article {
    pub id: String,
    pub title: String,
    pub content: Option<String>,
    pub links: Vec<String>,
    pub published: Option<String>,
}

impl Article {
    pub fn new(id: String, title: String) -> Self {
        Self {
            id,
            title,
            content: None,
            links: Vec::new(),
            published: None,
        }
    }

    pub fn cache_key(&self, feed_title: &str) -> String {
        format!("{}:{}", feed_title, self.id)
    }

    pub fn with_content(mut self, content: Option<String>) -> Self {
        self.content = content;
        self
    }

    pub fn with_links(mut self, links: Vec<String>) -> Self {
        self.links = links;
        self
    }

    pub fn with_published(mut self, published: Option<String>) -> Self {
        self.published = published;
        self
    }
}
