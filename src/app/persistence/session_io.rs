use color_eyre::Result;
use std::time::SystemTime;

use crate::app::persistence::db::writer::{ConversationRecord, MessageRecord, WriteOp};
use crate::app::persistence::db::{now_ms, reader, system_time_ms};
use crate::app::{App, ConversationMetadata, MessageState, MessageType};

impl App {
    pub(crate) fn build_title_summary(&self) -> String {
        self.messages
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                matches!(
                    self.message_types.get(*i),
                    Some(MessageType::User | MessageType::Agent)
                )
            })
            .take(6)
            .map(|(i, msg)| {
                let role = if matches!(self.message_types.get(i), Some(MessageType::User)) {
                    "User"
                } else {
                    "Assistant"
                };
                format!("{}: {}", role, msg.replace('\n', " "))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(crate) fn initialize_conversations_dir() -> Result<()> {
        // Legacy JSON directory; kept only as the import source.
        crate::app::persistence::conversations::initialize_conversations_dir()
    }

    pub(crate) async fn save_conversation(&mut self) -> Result<()> {
        self.reconcile_message_vectors();

        if self.messages.is_empty() {
            return Ok(());
        }
        let Some(writer) = self.db_writer.clone() else {
            return Err(color_eyre::eyre::eyre!("audit database unavailable"));
        };

        // Export agent conversation for LLM context restoration
        let agent_conversation = match &self.agent {
            Some(agent) => agent.export_conversation().await,
            None => None,
        };

        // Build message projection rows with full state
        let conversation_id = match &self.persistence_state.current_conversation_id {
            Some(id) => id.clone(),
            None => uuid::Uuid::new_v4().to_string(),
        };

        let mut records = Vec::with_capacity(self.messages.len());
        for i in 0..self.messages.len() {
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
            let metadata = self
                .message_metadata
                .get(i)
                .and_then(|m| m.as_ref())
                .and_then(|m| serde_json::to_string(m).ok());

            records.push(MessageRecord {
                id: format!("{conversation_id}:{i}"),
                seq: i as i64,
                msg_type: serde_json::to_string(&message_type)?,
                msg_state: serde_json::to_string(&message_state)?,
                content: self.messages[i].clone(),
                metadata,
                tool_call_id: None,
                created_at_ms: system_time_ms(timestamp),
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

        let title = self
            .persistence_state
            .current_conversation_title
            .clone()
            .filter(|title| !title.trim().is_empty());

        // created_at only applies on first insert; updates preserve it.
        // A NULL title never overwrites a stored one (COALESCE in the upsert).
        writer.send(WriteOp::UpsertConversation(ConversationRecord {
            id: conversation_id.clone(),
            parent_id: None,
            title: title.clone(),
            preview,
            git_branch: Self::get_current_git_branch(),
            working_directory: std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string())),
            forked_from: self.persistence_state.current_forked_from.clone(),
            forked_at_ms: self.persistence_state.current_forked_at.map(system_time_ms),
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
            agent_context: agent_conversation,
        }));
        writer.send(WriteOp::ReplaceMessages {
            conversation_id: conversation_id.clone(),
            messages: records,
        });

        // Track this conversation for future updates
        self.persistence_state.current_conversation_id = Some(conversation_id);
        self.persistence_state.current_conversation_title = title;

        Ok(())
    }

    pub(crate) async fn load_conversation(
        &mut self,
        metadata: &ConversationMetadata,
    ) -> Result<()> {
        let loaded = reader::load_conversation(&metadata.id)?
            .ok_or_else(|| color_eyre::eyre::eyre!("Conversation not found in database"))?;

        // Restore agent conversation for LLM context
        if let (Some(agent), Some(agent_json)) = (&self.agent, &loaded.agent_context) {
            agent.restore_conversation(agent_json).await.map_err(|e| {
                color_eyre::eyre::eyre!("Failed to restore agent conversation: {}", e)
            })?;
        }

        // Clear current UI state
        self.clear_rewind_state();
        self.subagent_conversations.clear();
        self.reset_orchestrator_views();
        self.messages.clear();
        self.message_types.clear();
        self.message_states.clear();
        self.message_metadata.clear();
        self.message_timestamps.clear();

        // Restore UI messages with complete state
        for message in loaded.messages {
            self.messages.push(message.content);
            self.message_types.push(message.message_type);
            self.message_states.push(message.message_state);
            self.message_metadata.push(message.metadata);
            self.message_timestamps.push(message.timestamp);
        }

        let title = loaded.title;

        // Track this conversation for future updates (unless in fork mode)
        if self.is_fork_mode {
            // In fork mode: don't track the ID so a new conversation is
            // created on save. Fork metadata is already set in the 'f'
            // key handler.
            self.persistence_state.current_conversation_id = None;
            self.persistence_state.current_conversation_path = None;
            self.persistence_state.current_conversation_title = title;
            self.is_fork_mode = false;

            // Close resume panel and show fork confirmation
            self.ui_state.show_resume = false;
            self.messages.push(format!(
                " ⎇ conversation forked from '{}'",
                metadata.preview
            ));
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);

            // Trigger immediate save to create the fork
            self.persistence_state.save_pending = true;
        } else {
            if let Some(writer) = &self.db_writer {
                writer.send(WriteOp::TouchConversation {
                    id: metadata.id.clone(),
                });
            }
            self.persistence_state.current_conversation_id = Some(metadata.id.clone());
            self.persistence_state.current_conversation_path = None;
            self.persistence_state.current_conversation_title = title;
        }

        self.audit_event(
            "conversation.resumed",
            serde_json::json!({
                "conversation_id": metadata.id,
                "forked": self.persistence_state.current_conversation_id.is_none(),
            }),
        );

        // Make this chat's persisted sub-chats navigable via Alt+W.
        self.rehydrate_subagent_sessions(&metadata.id);

        Ok(())
    }
}
