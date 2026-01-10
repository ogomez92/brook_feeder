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

    /// Send a notification to notebrook
    pub fn send(&self, notification: &Notification) -> FeederResult<()> {
        let message = notification.format();
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
