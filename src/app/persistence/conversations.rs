use color_eyre::Result;
use std::path::{Path, PathBuf};

pub(crate) fn conversations_dir_from_home(home: &Path) -> PathBuf {
    home.join(".config").join(".nite").join("conversations")
}

pub fn conversations_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| color_eyre::eyre::eyre!("Could not determine home directory"))?;
    Ok(conversations_dir_from_home(Path::new(&home)))
}

pub fn initialize_conversations_dir() -> Result<()> {
    let dir = conversations_dir()?;
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}

pub fn ensure_conversation_workspace(conversation_id: &str) -> Result<PathBuf> {
    let dir = conversations_dir()?.join(conversation_id);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn list_conversation_files() -> Result<Vec<PathBuf>> {
    let dir = conversations_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            files.push(path);
        }
    }
    Ok(files)
}

pub fn read_conversation_file(path: &Path) -> Result<String> {
    Ok(std::fs::read_to_string(path)?)
}

pub fn write_conversation_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

pub fn remove_conversation_file(path: &Path) -> Result<()> {
    std::fs::remove_file(path)?;
    Ok(())
}

pub fn current_git_branch() -> Option<String> {
    let current_dir = std::env::current_dir().ok()?;
    let mut git_dir = current_dir.clone();

    loop {
        if git_dir.join(".git").exists() {
            let head_path = git_dir.join(".git").join("HEAD");
            if let Ok(head_content) = std::fs::read_to_string(&head_path) {
                if head_content.starts_with("ref: refs/heads/") {
                    return Some(
                        head_content
                            .trim_start_matches("ref: refs/heads/")
                            .trim()
                            .to_string(),
                    );
                }
            }
            break;
        }
        if !git_dir.pop() {
            break;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversations_dir_is_under_home_config() {
        let base = std::env::temp_dir().join("cocode_persistence_test_home");
        let path = conversations_dir_from_home(&base);
        assert!(path.ends_with(".config/.nite/conversations"));
    }
}
