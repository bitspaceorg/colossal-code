use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Safety configuration modes for the agent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SafetyMode {
    /// YOLO mode: No permissions, no sandbox - maximum speed
    Yolo,
    /// Regular mode: Ask permissions, sandbox enabled - balanced
    Regular,
    /// Read-only mode: Only read/search tools, no modifications - safe exploration
    ReadOnly,
}

impl Default for SafetyMode {
    fn default() -> Self {
        SafetyMode::Regular
    }
}

impl std::fmt::Display for SafetyMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SafetyMode::Yolo => write!(f, "yolo"),
            SafetyMode::Regular => write!(f, "regular"),
            SafetyMode::ReadOnly => write!(f, "readonly"),
        }
    }
}

impl std::str::FromStr for SafetyMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "yolo" => Ok(SafetyMode::Yolo),
            "regular" => Ok(SafetyMode::Regular),
            "readonly" | "read-only" | "read_only" => Ok(SafetyMode::ReadOnly),
            _ => Err(anyhow::anyhow!("Invalid safety mode: {}", s)),
        }
    }
}

/// Configuration for agent safety and permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyConfig {
    /// Current safety mode
    pub mode: SafetyMode,

    /// Whether to ask for permission before executing tools (can be overridden)
    pub ask_permission: bool,

    /// Whether sandbox is enabled (can be overridden)
    pub sandbox_enabled: bool,
}

impl Default for SafetyConfig {
    fn default() -> Self {
        SafetyConfig::from_mode(SafetyMode::Regular)
    }
}

impl SafetyConfig {
    /// Create config from a specific safety mode
    pub fn from_mode(mode: SafetyMode) -> Self {
        match mode {
            SafetyMode::Yolo => SafetyConfig {
                mode,
                ask_permission: false,
                sandbox_enabled: false,
            },
            SafetyMode::Regular => SafetyConfig {
                mode,
                ask_permission: true,
                sandbox_enabled: true,
            },
            SafetyMode::ReadOnly => SafetyConfig {
                mode,
                ask_permission: false, // No need to ask for read-only operations
                sandbox_enabled: true,
            },
        }
    }

    /// Get the config file path
    pub fn get_config_path() -> Result<PathBuf> {
        let home = std::env::var("HOME")
            .map_err(|_| anyhow::anyhow!("HOME environment variable not set"))?;
        Ok(PathBuf::from(home)
            .join(".config")
            .join(".nite")
            .join("safety.toml"))
    }

    /// Load config from file, or create default if it doesn't exist
    pub fn load() -> Result<Self> {
        let config_path = Self::get_config_path()?;

        if !config_path.exists() {
            let config = Self::default();
            config.save()?;
            return Ok(config);
        }

        let content = fs::read_to_string(&config_path)?;
        let config: SafetyConfig = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse safety config: {}", e))?;
        Ok(config)
    }

    /// Save config to file
    pub fn save(&self) -> Result<()> {
        let config_path = Self::get_config_path()?;

        // Ensure parent directory exists
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize safety config: {}", e))?;
        fs::write(&config_path, content)?;
        Ok(())
    }

    /// Check if a tool is allowed based on current mode
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        if self.mode != SafetyMode::ReadOnly {
            return true;
        }

        // In read-only mode, only allow read/search tools
        matches!(
            tool_name,
            "read_file" | "list_files" | "grep" | "glob" | "web_search" | "web_fetch"
        )
    }

    /// Get the system prompt suffix based on current mode
    pub fn get_system_prompt_suffix(&self) -> Option<String> {
        match self.mode {
            SafetyMode::ReadOnly => Some(
                "\n\n**READ-ONLY MODE ACTIVE**\n\
                You are currently in read-only mode. You can:\n\
                - Read files (read_file)\n\
                - Search and list files (grep, glob, list_files)\n\
                - Search the web (web_search, web_fetch)\n\n\
                You CANNOT:\n\
                - Write or edit files\n\
                - Execute bash commands\n\
                - Make any filesystem modifications\n\n\
                If the user requests modifications, politely explain that you're in read-only mode \
                and they need to switch to regular or yolo mode first."
                    .to_string(),
            ),
            _ => None,
        }
    }

    /// Set safety mode and update related flags
    pub fn set_mode(&mut self, mode: SafetyMode) {
        *self = Self::from_mode(mode);
    }

    /// Toggle ask_permission (independent of mode)
    pub fn toggle_ask_permission(&mut self) {
        self.ask_permission = !self.ask_permission;
    }

    /// Toggle sandbox (independent of mode)
    pub fn toggle_sandbox(&mut self) {
        self.sandbox_enabled = !self.sandbox_enabled;
    }

    /// Get a human-readable status string
    pub fn status_string(&self) -> String {
        format!(
            "Mode: {} | Permissions: {} | Sandbox: {}",
            self.mode,
            if self.ask_permission { "ON" } else { "OFF" },
            if self.sandbox_enabled { "ON" } else { "OFF" }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safety_modes() {
        let yolo = SafetyConfig::from_mode(SafetyMode::Yolo);
        assert!(!yolo.ask_permission);
        assert!(!yolo.sandbox_enabled);

        let regular = SafetyConfig::from_mode(SafetyMode::Regular);
        assert!(regular.ask_permission);
        assert!(regular.sandbox_enabled);

        let readonly = SafetyConfig::from_mode(SafetyMode::ReadOnly);
        assert!(!readonly.ask_permission);
        assert!(readonly.sandbox_enabled);
    }

    #[test]
    fn test_tool_permissions() {
        let readonly = SafetyConfig::from_mode(SafetyMode::ReadOnly);
        assert!(readonly.is_tool_allowed("read_file"));
        assert!(readonly.is_tool_allowed("grep"));
        assert!(!readonly.is_tool_allowed("write_file"));
        assert!(!readonly.is_tool_allowed("bash"));

        let regular = SafetyConfig::from_mode(SafetyMode::Regular);
        assert!(regular.is_tool_allowed("write_file"));
        assert!(regular.is_tool_allowed("bash"));
    }

    #[test]
    fn test_mode_parsing() {
        assert_eq!("yolo".parse::<SafetyMode>().unwrap(), SafetyMode::Yolo);
        assert_eq!(
            "regular".parse::<SafetyMode>().unwrap(),
            SafetyMode::Regular
        );
        assert_eq!(
            "readonly".parse::<SafetyMode>().unwrap(),
            SafetyMode::ReadOnly
        );
        assert_eq!(
            "read-only".parse::<SafetyMode>().unwrap(),
            SafetyMode::ReadOnly
        );
    }
}
