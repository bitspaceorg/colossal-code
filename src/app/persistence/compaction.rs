use color_eyre::Result;

use crate::app::persistence::db::reader;
use crate::app::persistence::db::writer::WriteOp;
use crate::app::{App, ConversationMetadata, TodoItem, persistence};

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
        let writer = self
            .db_writer
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("audit database unavailable"))?;
        writer.send(WriteOp::Todos {
            conversation_id: conversation_id.clone(),
            content: json,
        });
        Ok(())
    }

    pub(crate) fn load_todos(&self) -> Result<Vec<TodoItem>> {
        let conversation_id = self
            .persistence_state
            .current_conversation_id
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("No active conversation"))?;
        if let Some(writer) = &self.db_writer {
            writer.flush();
        }
        let Some(content) = reader::load_todos(conversation_id)? else {
            return Ok(Vec::new());
        };
        let todos: Vec<TodoItem> = serde_json::from_str(&content)?;
        Ok(todos)
    }

    pub(crate) fn get_current_git_branch() -> Option<String> {
        persistence::conversations::current_git_branch()
    }

    pub(crate) fn load_conversations_list(&mut self) -> Result<()> {
        // Drain queued saves so the list reflects the latest state.
        if let Some(writer) = &self.db_writer {
            writer.flush();
        }
        self.resume_conversations = reader::list_conversations()?;
        Ok(())
    }

    pub(crate) fn delete_conversation(&mut self, metadata: &ConversationMetadata) -> Result<()> {
        let writer = self
            .db_writer
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("audit database unavailable"))?;
        writer.send(WriteOp::DeleteConversation {
            id: metadata.id.clone(),
        });
        writer.flush();
        Ok(())
    }
}
