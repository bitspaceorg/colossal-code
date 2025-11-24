/// Model configuration based on LMStudio's model.yaml format
/// Supports detection of thinking/reasoning models and custom tag configuration
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::fs;
use anyhow::{Result, Context};

/// Main model configuration structure following LMStudio's model.yaml format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Model identifier in format "organization/name"
    pub model: String,

    /// Base model reference (simplified - can be string or array in full spec)
    #[serde(default)]
    pub base: Option<String>,

    /// Tags associated with the model
    #[serde(default)]
    pub tags: Vec<String>,

    /// Metadata overrides
    #[serde(default, rename = "metadataOverrides")]
    pub metadata_overrides: Option<MetadataOverrides>,

    /// Custom fields for model-specific configuration
    #[serde(default, rename = "customFields")]
    pub custom_fields: Vec<CustomField>,

    /// Custom thinking/reasoning tag configuration
    #[serde(default, rename = "thinkingTags")]
    pub thinking_tags: Option<ThinkingTags>,
}

/// Metadata overrides following LMStudio's VirtualModelDefinitionMetadataOverrides
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataOverrides {
    /// Model domain type
    #[serde(default)]
    pub domain: Option<String>,

    /// Model architectures
    #[serde(default)]
    pub architectures: Vec<String>,

    /// Whether the model is trained for reasoning
    /// Can be true, false, or "mixed"
    #[serde(default)]
    pub reasoning: Option<ReasoningCapability>,

    /// Whether the model is trained for tool use
    #[serde(default, rename = "trainedForToolUse")]
    pub trained_for_tool_use: Option<bool>,

    /// Whether the model supports vision
    #[serde(default)]
    pub vision: Option<bool>,

    /// Context lengths supported
    #[serde(default, rename = "contextLengths")]
    pub context_lengths: Vec<u64>,
}

/// Reasoning capability can be boolean or "mixed"
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReasoningCapability {
    Boolean(bool),
    Mixed(String), // "mixed"
}

impl ReasoningCapability {
    pub fn is_reasoning(&self) -> bool {
        match self {
            ReasoningCapability::Boolean(b) => *b,
            ReasoningCapability::Mixed(s) => s == "mixed",
        }
    }
}

/// Custom field definition for model-specific settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomField {
    /// Unique key for the field
    pub key: String,

    /// Display name shown to users
    #[serde(rename = "displayName")]
    pub display_name: String,

    /// Description of what this field does
    pub description: String,

    /// Field type (boolean, string, select, number)
    #[serde(rename = "type")]
    pub field_type: String,

    /// Default value
    #[serde(rename = "defaultValue")]
    pub default_value: serde_json::Value,

    /// Effects that this field triggers
    #[serde(default)]
    pub effects: Vec<CustomFieldEffect>,
}

/// Effect triggered by a custom field
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomFieldEffect {
    /// Effect type (e.g., "setJinjaVariable", "setSystemPrompt")
    #[serde(rename = "type")]
    pub effect_type: String,

    /// Variable name (for setJinjaVariable)
    #[serde(default)]
    pub variable: Option<String>,

    /// Additional effect-specific data
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// Custom configuration for thinking/reasoning tags
/// This extends the LMStudio format with explicit tag configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingTags {
    /// Opening tag for thinking content (e.g., "<think>", "<thinking>", "<reasoning>")
    #[serde(rename = "openTag")]
    pub open_tag: String,

    /// Closing tag for thinking content (e.g., "</think>", "</thinking>", "</reasoning>")
    #[serde(rename = "closeTag")]
    pub close_tag: String,

    /// Whether to render summary every N tokens (default: 200)
    #[serde(default = "default_summary_interval", rename = "summaryInterval")]
    pub summary_interval: usize,
}

fn default_summary_interval() -> usize {
    200
}

impl Default for ThinkingTags {
    fn default() -> Self {
        Self {
            open_tag: "<think>".to_string(),
            close_tag: "</think>".to_string(),
            summary_interval: 200,
        }
    }
}

impl ModelConfig {
    /// Load model configuration from a model.yaml file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(path.as_ref())
            .with_context(|| format!("Failed to read model config from {:?}", path.as_ref()))?;

        let config: ModelConfig = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse model.yaml at {:?}", path.as_ref()))?;

        Ok(config)
    }

    /// Load model configuration from ~/.config/.nite/models/<model_name>/model.yaml
    pub fn from_model_name(model_name: &str) -> Result<Self> {
        let config_path = Self::get_config_path(model_name)?;
        Self::from_file(config_path)
    }

    /// Get the path to a model's configuration directory
    pub fn get_config_path(model_name: &str) -> Result<PathBuf> {
        let home = dirs::home_dir()
            .context("Failed to get home directory")?;

        let config_dir = home.join(".config").join(".nite").join("models").join(model_name);
        let config_file = config_dir.join("model.yaml");

        Ok(config_file)
    }

    /// Check if this model has thinking/reasoning capability
    pub fn has_thinking_capability(&self) -> bool {
        // Check metadata reasoning flag
        if let Some(metadata) = &self.metadata_overrides {
            if let Some(reasoning) = &metadata.reasoning {
                if reasoning.is_reasoning() {
                    return true;
                }
            }
        }

        // Check for enableThinking custom field
        if self.custom_fields.iter().any(|f| f.key == "enableThinking") {
            return true;
        }

        // Check if thinking tags are explicitly configured
        if self.thinking_tags.is_some() {
            return true;
        }

        // Check tags
        if self.tags.iter().any(|t| t.contains("thinking") || t.contains("reasoning")) {
            return true;
        }

        false
    }

    /// Get the thinking tags configuration, or default if not specified
    pub fn get_thinking_tags(&self) -> ThinkingTags {
        self.thinking_tags.clone().unwrap_or_default()
    }

    /// Detect thinking capability from model filename (fallback method)
    /// This is the old detection method, kept for backward compatibility
    pub fn detect_from_filename(filename: &str) -> bool {
        let lower = filename.to_lowercase();
        lower.contains("thinking") || lower.contains("reasoning") || lower.contains("thought")
    }

    /// Try to load model config, fall back to filename detection
    pub fn load_or_detect(model_name: &str, model_filename: &str) -> (bool, ThinkingTags) {
        match Self::from_model_name(model_name) {
            Ok(config) => {
                let has_thinking = config.has_thinking_capability();
                let tags = config.get_thinking_tags();
                (has_thinking, tags)
            }
            Err(_) => {
                // Fall back to filename detection
                let has_thinking = Self::detect_from_filename(model_filename);
                (has_thinking, ThinkingTags::default())
            }
        }
    }
}

/// Helper function to create a default model.yaml for a thinking model
pub fn create_default_thinking_config(
    model_name: &str,
    open_tag: &str,
    close_tag: &str,
) -> Result<ModelConfig> {
    Ok(ModelConfig {
        model: model_name.to_string(),
        base: None,
        tags: vec!["thinking".to_string(), "reasoning".to_string()],
        metadata_overrides: Some(MetadataOverrides {
            domain: Some("text".to_string()),
            architectures: vec![],
            reasoning: Some(ReasoningCapability::Boolean(true)),
            trained_for_tool_use: None,
            vision: None,
            context_lengths: vec![],
        }),
        custom_fields: vec![CustomField {
            key: "enableThinking".to_string(),
            display_name: "Enable Thinking".to_string(),
            description: "Enable the model to think before answering".to_string(),
            field_type: "boolean".to_string(),
            default_value: serde_json::Value::Bool(true),
            effects: vec![CustomFieldEffect {
                effect_type: "setJinjaVariable".to_string(),
                variable: Some("enable_thinking".to_string()),
                extra: serde_json::Value::Null,
            }],
        }],
        thinking_tags: Some(ThinkingTags {
            open_tag: open_tag.to_string(),
            close_tag: close_tag.to_string(),
            summary_interval: 200,
        }),
    })
}

/// Save a model configuration to ~/.config/.nite/models/<model_name>/model.yaml
pub fn save_model_config(model_name: &str, config: &ModelConfig) -> Result<()> {
    let config_file = ModelConfig::get_config_path(model_name)?;

    // Create directory if it doesn't exist
    if let Some(parent) = config_file.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory {:?}", parent))?;
    }

    let yaml = serde_yaml::to_string(config)
        .context("Failed to serialize model config to YAML")?;

    fs::write(&config_file, yaml)
        .with_context(|| format!("Failed to write model config to {:?}", config_file))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_config() {
        let yaml = r#"
model: "test/thinking-model"
tags:
  - thinking
  - reasoning
metadataOverrides:
  reasoning: true
thinkingTags:
  openTag: "<think>"
  closeTag: "</think>"
  summaryInterval: 200
"#;

        let config: ModelConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.model, "test/thinking-model");
        assert!(config.has_thinking_capability());

        let tags = config.get_thinking_tags();
        assert_eq!(tags.open_tag, "<think>");
        assert_eq!(tags.close_tag, "</think>");
        assert_eq!(tags.summary_interval, 200);
    }

    #[test]
    fn test_custom_tags() {
        let yaml = r#"
model: "test/custom-model"
thinkingTags:
  openTag: "<reasoning>"
  closeTag: "</reasoning>"
  summaryInterval: 150
"#;

        let config: ModelConfig = serde_yaml::from_str(yaml).unwrap();
        let tags = config.get_thinking_tags();
        assert_eq!(tags.open_tag, "<reasoning>");
        assert_eq!(tags.close_tag, "</reasoning>");
        assert_eq!(tags.summary_interval, 150);
    }

    #[test]
    fn test_filename_detection() {
        assert!(ModelConfig::detect_from_filename("qwen-thinking-7b.gguf"));
        assert!(ModelConfig::detect_from_filename("deepseek-reasoning-v2.gguf"));
        assert!(!ModelConfig::detect_from_filename("llama-3-8b.gguf"));
    }
}
