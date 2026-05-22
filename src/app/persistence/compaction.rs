use color_eyre::Result;

use crate::app::{
    App, ConversationMetadata, EnhancedSavedConversation, SavedConversation, TodoItem, persistence,
};

impl App {
    pub(crate) fn get_conversations_dir() -> Result<std::path::PathBuf> {
        persistence::conversations::conversations_dir()
    }

    pub(crate) fn save_todos(&self, todos: &[TodoItem]) -> Result<()> {
        let json = serde_json::to_string_pretty(todos)?;
        let conversation_id = self
            .persistence_state
            .current_conversation_id
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("No active conversation"))?;
        persistence::todos::write_todos_json(conversation_id, &json)?;
        Ok(())
    }

    pub(crate) fn load_todos(&self) -> Result<Vec<TodoItem>> {
        let conversation_id = self
            .persistence_state
            .current_conversation_id
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("No active conversation"))?;
        let Some(content) = persistence::todos::read_todos_json(conversation_id)? else {
            return Ok(Vec::new());
        };
        let todos: Vec<TodoItem> = serde_json::from_str(&content)?;
        Ok(todos)
    }

    pub(crate) fn get_current_git_branch() -> Option<String> {
        persistence::conversations::current_git_branch()
    }

    pub(crate) fn load_conversations_list(&mut self) -> Result<()> {
        let mut conversations = Vec::new();

        for path in persistence::conversations::list_conversation_files()? {
            if let Ok(content) = persistence::conversations::read_conversation_file(&path) {
                if let Ok(conv) = serde_json::from_str::<EnhancedSavedConversation>(&content) {
                    conversations.push(ConversationMetadata {
                        time_ago_str: ConversationMetadata::calculate_time_ago(conv.updated_at),
                        id: conv.id,
                        updated_at: conv.updated_at,
                        git_branch: conv.git_branch,
                        message_count: conv.message_count,
                        title: conv.title,
                        preview: conv.preview,
                        file_path: path.clone(),
                        forked_from: conv.forked_from,
                    });
                } else if let Ok(conv) = serde_json::from_str::<SavedConversation>(&content) {
                    conversations.push(ConversationMetadata {
                        time_ago_str: ConversationMetadata::calculate_time_ago(conv.updated_at),
                        id: conv.id,
                        updated_at: conv.updated_at,
                        git_branch: conv.git_branch,
                        message_count: conv.message_count,
                        title: conv.title,
                        preview: conv.preview,
                        file_path: path.clone(),
                        forked_from: conv.forked_from,
                    });
                }
            }
        }

        conversations.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        self.resume_conversations = conversations;
        Ok(())
    }

    pub(crate) fn delete_conversation(&mut self, metadata: &ConversationMetadata) -> Result<()> {
        persistence::conversations::remove_conversation_file(&metadata.file_path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{EnhancedSavedConversation, SavedConversation};
    use std::time::SystemTime;

    #[test]
    fn conversation_metadata_supports_title_and_preview_fallback() {
        let now = SystemTime::now();
        let new_conv = EnhancedSavedConversation {
            id: "new".to_string(),
            created_at: now,
            updated_at: now,
            git_branch: None,
            working_directory: "/tmp".to_string(),
            message_count: 2,
            title: Some("Generated Title".to_string()),
            preview: "first user message".to_string(),
            ui_messages: Vec::new(),
            agent_conversation: None,
            forked_from: None,
            forked_at: None,
        };
        let old_conv = SavedConversation {
            id: "old".to_string(),
            created_at: now,
            updated_at: now,
            git_branch: None,
            working_directory: "/tmp".to_string(),
            message_count: 1,
            title: None,
            preview: "legacy preview".to_string(),
            messages: Vec::new(),
            forked_from: None,
            forked_at: None,
        };

        let new_meta = ConversationMetadata {
            time_ago_str: ConversationMetadata::calculate_time_ago(new_conv.updated_at),
            id: new_conv.id,
            updated_at: new_conv.updated_at,
            git_branch: new_conv.git_branch,
            message_count: new_conv.message_count,
            title: new_conv.title,
            preview: new_conv.preview,
            file_path: "/tmp/new.json".into(),
            forked_from: new_conv.forked_from,
        };
        let old_meta = ConversationMetadata {
            time_ago_str: ConversationMetadata::calculate_time_ago(old_conv.updated_at),
            id: old_conv.id,
            updated_at: old_conv.updated_at,
            git_branch: old_conv.git_branch,
            message_count: old_conv.message_count,
            title: old_conv.title,
            preview: old_conv.preview,
            file_path: "/tmp/old.json".into(),
            forked_from: old_conv.forked_from,
        };

        assert_eq!(new_meta.title.as_deref(), Some("Generated Title"));
        assert_eq!(old_meta.title, None);
        assert_eq!(old_meta.preview, "legacy preview");
    }
}
