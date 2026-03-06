use color_eyre::Result;
use std::path::PathBuf;

use crate::app::persistence::conversations;

pub fn todos_path(conversation_id: &str) -> Result<PathBuf> {
    let conversation_dir = conversations::ensure_conversation_workspace(conversation_id)?;
    Ok(conversation_dir.join("todos.json"))
}

pub fn write_todos_json(conversation_id: &str, content: &str) -> Result<()> {
    let path = todos_path(conversation_id)?;
    std::fs::write(path, content)?;
    Ok(())
}

pub fn read_todos_json(conversation_id: &str) -> Result<Option<String>> {
    let path = todos_path(conversation_id)?;
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(std::fs::read_to_string(path)?))
}
