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
    pub(crate) preview: String,
    pub(crate) file_path: std::path::PathBuf,
    pub(crate) time_ago_str: String,
    pub(crate) forked_from: Option<String>,
}

impl ConversationMetadata {
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
