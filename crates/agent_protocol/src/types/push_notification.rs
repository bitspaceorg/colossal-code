//! Push notification types for the A2A protocol
//!
//! Push notifications allow agents to receive async updates via webhooks
//! when they can't maintain a persistent connection.

use serde::{Deserialize, Serialize};

/// Configuration for receiving push notifications
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushNotificationConfig {
    /// Unique identifier for this configuration (optional in v0.3.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// URL to receive notifications
    pub url: String,

    /// Authentication for the webhook endpoint
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication: Option<PushNotificationAuthenticationInfo>,

    /// Token for validating notifications
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// Authentication info for push notification delivery
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushNotificationAuthenticationInfo {
    /// Supported authentication schemes (e.g. Bearer)
    pub schemes: Vec<String>,

    /// Credentials (e.g., bearer token)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credentials: Option<String>,
}

/// A push notification payload
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushNotification {
    /// Task ID this notification is for
    pub task_id: String,

    /// Context ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,

    /// The notification content
    pub event: PushNotificationEvent,

    /// Timestamp of the notification
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Types of push notification events
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PushNotificationEvent {
    /// Task status has changed
    #[serde(rename = "statusUpdate")]
    StatusUpdate(super::task::TaskStatusUpdateEvent),

    /// New artifact available
    #[serde(rename = "artifactUpdate")]
    ArtifactUpdate(super::task::TaskArtifactUpdateEvent),

    /// New message available
    #[serde(rename = "message")]
    Message(super::message::Message),
}

impl PushNotificationConfig {
    /// Create a new push notification config
    pub fn new(id: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: Some(id.into()),
            url: url.into(),
            authentication: None,
            token: None,
        }
    }

    /// Add bearer authentication
    pub fn with_bearer_auth(mut self, token: impl Into<String>) -> Self {
        self.authentication = Some(PushNotificationAuthenticationInfo {
            schemes: vec!["Bearer".to_string()],
            credentials: Some(token.into()),
        });
        self
    }

    /// Add a validation token
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }
}

/// Parameters for setting push notification config
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPushNotificationConfigParams {
    /// Task ID to configure notifications for
    pub task_id: String,

    /// The push notification configuration
    pub push_notification_config: PushNotificationConfig,
}

/// Parameters for getting push notification config
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetPushNotificationConfigParams {
    /// Task ID to get configuration for
    pub task_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_notification_config() {
        let config = PushNotificationConfig::new("notif-1", "https://example.com/webhook")
            .with_bearer_auth("secret-token")
            .with_token("validation-token");

        assert_eq!(config.url, "https://example.com/webhook");
        assert_eq!(config.id.as_deref(), Some("notif-1"));
        assert!(config.authentication.is_some());
        assert!(config.token.is_some());
    }
}
