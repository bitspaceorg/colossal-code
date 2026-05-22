use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime};

use super::message::{MessageState, MessageType, UIMessageMetadata};

/// Saved conversation data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SavedConversation {
    pub(crate) id: String,
    pub(crate) created_at: SystemTime,
    pub(crate) updated_at: SystemTime,
    pub(crate) git_branch: Option<String>,
    pub(crate) working_directory: String,
    pub(crate) message_count: usize,
    #[serde(default)]
    pub(crate) title: Option<String>,
    pub(crate) preview: String,
    pub(crate) messages: Vec<ConversationMessage>,
    #[serde(default)]
    pub(crate) forked_from: Option<String>,
    #[serde(default)]
    pub(crate) forked_at: Option<SystemTime>,
}

/// Individual message in a conversation (OLD FORMAT - kept for compatibility)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ConversationMessage {
    pub(crate) role: String,
    pub(crate) content: String,
}

/// Enhanced saved conversation with complete UI state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EnhancedSavedConversation {
    pub(crate) id: String,
    pub(crate) created_at: SystemTime,
    pub(crate) updated_at: SystemTime,
    pub(crate) git_branch: Option<String>,
    pub(crate) working_directory: String,
    pub(crate) message_count: usize,
    #[serde(default)]
    pub(crate) title: Option<String>,
    pub(crate) preview: String,
    pub(crate) ui_messages: Vec<SavedUIMessage>,
    pub(crate) agent_conversation: Option<String>,
    #[serde(default)]
    pub(crate) forked_from: Option<String>,
    #[serde(default)]
    pub(crate) forked_at: Option<SystemTime>,
}

/// Individual UI message with complete state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SavedUIMessage {
    pub(crate) content: String,
    pub(crate) message_type: MessageType,
    pub(crate) message_state: MessageState,
    pub(crate) timestamp: SystemTime,
    pub(crate) metadata: Option<UIMessageMetadata>,
}

/// Metadata for displaying conversation in list
#[derive(Debug, Clone)]
pub(crate) struct ConversationMetadata {
    pub(crate) id: String,
    pub(crate) updated_at: SystemTime,
    pub(crate) git_branch: Option<String>,
    pub(crate) message_count: usize,
    pub(crate) title: Option<String>,
    pub(crate) preview: String,
    pub(crate) file_path: std::path::PathBuf,
    pub(crate) time_ago_str: String,
    pub(crate) forked_from: Option<String>,
}

impl ConversationMetadata {
    pub(crate) fn display_title(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.preview)
    }

    pub(crate) fn calculate_time_ago(updated_at: SystemTime) -> String {
        let elapsed = updated_at.elapsed().unwrap_or(Duration::from_secs(0));
        let secs = elapsed.as_secs();

        if secs < 60 {
            format!("{}s ago", secs)
        } else if secs < 3600 {
            format!("{}m ago", secs / 60)
        } else if secs < 86400 {
            format!("{}h ago", secs / 3600)
        } else if secs < 604800 {
            format!("{}d ago", secs / 86400)
        } else if secs < 2592000 {
            format!("{}w ago", secs / 604800)
        } else if secs < 31536000 {
            format!("{}mo ago", secs / 2592000)
        } else {
            format!("{}y ago", secs / 31536000)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ConversationMetadata;
    use std::time::SystemTime;

    #[test]
    fn display_title_prefers_generated_title_and_falls_back_to_preview() {
        let titled = ConversationMetadata {
            id: "1".to_string(),
            updated_at: SystemTime::now(),
            git_branch: None,
            message_count: 1,
            title: Some("Generated Title".to_string()),
            preview: "preview text".to_string(),
            file_path: "/tmp/one.json".into(),
            time_ago_str: "1m ago".to_string(),
            forked_from: None,
        };
        let legacy = ConversationMetadata {
            id: "2".to_string(),
            updated_at: SystemTime::now(),
            git_branch: None,
            message_count: 1,
            title: None,
            preview: "legacy preview".to_string(),
            file_path: "/tmp/two.json".into(),
            time_ago_str: "1m ago".to_string(),
            forked_from: None,
        };

        assert_eq!(titled.display_title(), "Generated Title");
        assert_eq!(legacy.display_title(), "legacy preview");
    }
}
