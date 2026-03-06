use color_eyre::Result;
use serde_json::{Value, json};
use std::time::SystemTime;

use crate::{
    App, ConversationMetadata, EnhancedSavedConversation, MessageState, MessageType,
    SavedConversation, SavedUIMessage,
    app::persistence,
};

impl App {
    pub(crate) fn initialize_conversations_dir() -> Result<()> {
        persistence::conversations::initialize_conversations_dir()
    }

    pub(crate) async fn save_conversation(&mut self) -> Result<()> {
        self.reconcile_message_vectors();

        if self.messages.is_empty() {
            return Ok(());
        }

        // Export agent conversation for LLM context restoration
        let agent_conversation = match &self.agent {
            Some(agent) => agent.export_conversation().await,
            None => None,
        };

        // Build UI messages with full state
        let mut ui_messages = Vec::new();

        for i in 0..self.messages.len() {
            let content = self.messages[i].clone();
            let message_type = self
                .message_types
                .get(i)
                .cloned()
                .unwrap_or(MessageType::User);
            let message_state = self
                .message_states
                .get(i)
                .copied()
                .unwrap_or(MessageState::Sent);
            let timestamp = self
                .message_timestamps
                .get(i)
                .copied()
                .unwrap_or_else(SystemTime::now);
            let metadata = self.message_metadata.get(i).and_then(|m| m.clone());

            ui_messages.push(SavedUIMessage {
                content,
                message_type,
                message_state,
                timestamp,
                metadata,
            });
        }

        // Extract preview from first user message in UI
        let preview = self
            .messages
            .iter()
            .enumerate()
            .find(|(i, _)| matches!(self.message_types.get(*i), Some(MessageType::User)))
            .map(|(_, msg)| msg.chars().take(100).collect::<String>())
            .unwrap_or_else(|| "No preview available".to_string());

        // Check if we're updating existing conversation or creating new one
        let (conversation_id, created_at, file_path, forked_from, forked_at) =
            if let (Some(id), Some(path)) = (
                &self.persistence_state.current_conversation_id,
                &self.persistence_state.current_conversation_path,
            ) {
                // UPDATE EXISTING - preserve ID, created_at, and fork metadata
                let (existing_created_at, existing_forked_from, existing_forked_at) =
                    if let Ok(content) = persistence::conversations::read_conversation_file(path) {
                        if let Ok(existing) =
                            serde_json::from_str::<EnhancedSavedConversation>(&content)
                        {
                            (
                                existing.created_at,
                                existing.forked_from,
                                existing.forked_at,
                            )
                        } else {
                            (SystemTime::now(), None, None)
                        }
                    } else {
                        (SystemTime::now(), None, None)
                    };

                (
                    id.clone(),
                    existing_created_at,
                    path.clone(),
                    existing_forked_from,
                    existing_forked_at,
                )
            } else {
                // CREATE NEW - generate new ID
                persistence::conversations::initialize_conversations_dir()?;
                let conversations_dir = Self::get_conversations_dir()?;

                let new_id = uuid::Uuid::new_v4().to_string();
                let new_path = conversations_dir.join(format!("{}.json", new_id));
                let now = SystemTime::now();

                (
                    new_id,
                    now,
                    new_path,
                    self.persistence_state.current_forked_from.clone(),
                    self.persistence_state.current_forked_at,
                )
            };

        // Create/update conversation
        let now = SystemTime::now();
        let conversation = EnhancedSavedConversation {
            id: conversation_id.clone(),
            created_at,
            updated_at: now,
            git_branch: Self::get_current_git_branch(),
            working_directory: std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| String::from("unknown")),
            message_count: ui_messages.len(),
            preview,
            ui_messages,
            agent_conversation,
            forked_from,
            forked_at,
        };

        // Ensure directory exists
        persistence::conversations::initialize_conversations_dir()?;

        // Save to file
        let json = serde_json::to_string_pretty(&conversation)?;
        persistence::conversations::write_conversation_file(&file_path, &json)?;

        // Track this conversation for future updates
        self.persistence_state.current_conversation_id = Some(conversation_id);
        self.persistence_state.current_conversation_path = Some(file_path);

        Ok(())
    }

    pub(crate) async fn load_conversation(&mut self, metadata: &ConversationMetadata) -> Result<()> {
        // Read the conversation file
        let content = persistence::conversations::read_conversation_file(&metadata.file_path)?;

        // Try to load as enhanced format first, fall back to old format
        let (ui_messages, agent_conversation) =
            if let Ok(enhanced) = serde_json::from_str::<EnhancedSavedConversation>(&content) {
                (enhanced.ui_messages, enhanced.agent_conversation)
            } else if let Ok(old_conv) = serde_json::from_str::<SavedConversation>(&content) {
                // Convert old format to UI messages (basic conversion)
                let ui_msgs: Vec<SavedUIMessage> = old_conv
                    .messages
                    .iter()
                    .map(|m| {
                        let message_type = if m.role == "user" {
                            MessageType::User
                        } else {
                            MessageType::Agent
                        };

                        SavedUIMessage {
                            content: m.content.clone(),
                            message_type,
                            message_state: MessageState::Sent,
                            timestamp: old_conv.created_at,
                            metadata: None,
                        }
                    })
                    .collect();

                // Build agent conversation JSON from old format
                let messages: Vec<Value> = old_conv
                    .messages
                    .iter()
                    .map(|m| json!({"role": m.role, "content": m.content}))
                    .collect();
                let agent_json = serde_json::to_string(&messages).ok();

                (ui_msgs, agent_json)
            } else {
                return Err(color_eyre::eyre::eyre!("Failed to parse conversation file"));
            };

        // Restore agent conversation for LLM context
        if let (Some(agent), Some(agent_json)) = (&self.agent, &agent_conversation) {
            agent.restore_conversation(agent_json).await.map_err(|e| {
                color_eyre::eyre::eyre!("Failed to restore agent conversation: {}", e)
            })?;
        }

        // Clear current UI state
        self.messages.clear();
        self.message_types.clear();
        self.message_states.clear();
        self.message_metadata.clear();
        self.message_timestamps.clear();

        // Restore UI messages with complete state
        for ui_msg in ui_messages {
            self.messages.push(ui_msg.content);
            self.message_types.push(ui_msg.message_type);
            self.message_states.push(ui_msg.message_state);
            self.message_metadata.push(ui_msg.metadata);
            self.message_timestamps.push(ui_msg.timestamp);
        }

        // Update the conversation file's timestamp (only if NOT in fork mode)
        if !self.is_fork_mode {
            if let Ok(mut enhanced) = serde_json::from_str::<EnhancedSavedConversation>(&content) {
                enhanced.updated_at = SystemTime::now();
                let json = serde_json::to_string_pretty(&enhanced)?;
                persistence::conversations::write_conversation_file(&metadata.file_path, &json)?;
            }
        }

        // Track this conversation for future updates (unless in fork mode)
        if self.is_fork_mode {
            // In fork mode: don't track the ID/path so a new conversation is created on save
            // Fork metadata is already set in the 'f' key handler
            self.persistence_state.current_conversation_id = None;
            self.persistence_state.current_conversation_path = None;
            // Reset fork mode flag
            self.is_fork_mode = false;

            // Close resume panel and show fork confirmation
            self.ui_state.show_resume = false;
            self.messages
                .push(format!(" ⎇ conversation forked from '{}'", metadata.preview));
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);

            // Trigger immediate save to create the fork
            self.persistence_state.save_pending = true;
        } else {
            self.persistence_state.current_conversation_id = Some(metadata.id.clone());
            self.persistence_state.current_conversation_path = Some(metadata.file_path.clone());
        }

        Ok(())
    }
}
