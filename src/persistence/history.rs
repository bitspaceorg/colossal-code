use color_eyre::Result;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub fn history_file_path_for_cwd(cwd: &Path) -> Result<PathBuf> {
    let cwd_str = cwd.to_string_lossy();
    let mut hasher = Sha256::new();
    hasher.update(cwd_str.as_bytes());
    let hash = hasher.finalize();
    let hash_str = format!("{:x}", hash);

    let mut history_dir = dirs::config_dir()
        .ok_or_else(|| color_eyre::eyre::eyre!("Could not find config directory"))?;
    history_dir.push(".nite");
    history_dir.push("history");
    std::fs::create_dir_all(&history_dir)?;

    history_dir.push(hash_str);
    Ok(history_dir)
}

pub fn load_history(history_file: &Path) -> Vec<String> {
    let Ok(contents) = std::fs::read_to_string(history_file) else {
        return Vec::new();
    };

    let history = contents
        .lines()
        .map(|s| s.replace("\\n", "\n").replace("\\\\", "\\"))
        .collect();
    deduplicate_history(history)
}

pub fn deduplicate_history(history: Vec<String>) -> Vec<String> {
    let mut deduplicated = Vec::new();
    let mut last_entry: Option<&String> = None;
    for entry in &history {
        if Some(entry) != last_entry {
            deduplicated.push(entry.clone());
            last_entry = Some(entry);
        }
    }
    deduplicated
}

pub fn save_history(history_file: &Path, command_history: &[String]) -> Result<()> {
    let escaped_history: Vec<String> = command_history
        .iter()
        .map(|cmd| cmd.replace("\\", "\\\\").replace("\n", "\\n"))
        .collect();
    let contents = escaped_history.join("\n");
    std::fs::write(history_file, contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deduplicates_only_consecutive_entries() {
        let input = vec![
            "a".to_string(),
            "a".to_string(),
            "b".to_string(),
            "a".to_string(),
        ];
        let output = deduplicate_history(input);
        assert_eq!(output, vec!["a", "b", "a"]);
    }
}
