use channels::ChannelClient;

use crate::config::Config;
use crate::domain::Notification;
use crate::errors::{FeederError, FeederResult};

pub struct NotificationService {
    client: ChannelClient,
    channel: String,
}

impl NotificationService {
    pub fn new(config: &Config) -> FeederResult<Self> {
        let client = ChannelClient::new(&config.notebrook_url, &config.notebrook_token)?;

        Ok(Self {
            client,
            channel: config.notebrook_channel.clone(),
        })
    }

    /// Send a notification to notebrook, truncating text if too large
    pub fn send(&self, notification: &Notification) -> FeederResult<()> {
        // Try with full message first
        let message = notification.format();
        match self.client.send_message(&self.channel, &message) {
            Ok(_) => return Ok(()),
            Err(channels::ChannelError::PayloadTooLarge) => {}
            Err(e) => return Err(e.into()),
        }

        // Message too large, try truncating the text
        let mut truncated = notification.clone();

        // Binary search for max text length that fits
        let mut high = truncated.text.len();

        while high > 0 {
            let mid = high / 2;
            truncated.text = truncate_to_char_boundary(&notification.text, mid);

            let message = truncated.format();
            match self.client.send_message(&self.channel, &message) {
                Ok(_) => return Ok(()),
                Err(channels::ChannelError::PayloadTooLarge) => {
                    high = mid;
                }
                Err(e) => return Err(e.into()),
            }
        }

        // Try with no text at all
        truncated.text = String::new();
        let message = truncated.format();
        self.client.send_message(&self.channel, &message)?;
        Ok(())
    }

    /// Send multiple notifications
    pub fn send_all(&self, notifications: &[Notification]) -> FeederResult<Vec<FeederError>> {
        let mut errors = Vec::new();

        for notification in notifications {
            if let Err(e) = self.send(notification) {
                errors.push(e);
            }
        }

        Ok(errors)
    }
}

/// Truncate string to at most `max_chars` characters, respecting char boundaries
fn truncate_to_char_boundary(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}
