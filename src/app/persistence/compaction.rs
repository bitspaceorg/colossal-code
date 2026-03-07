use color_eyre::Result;

use crate::{
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
