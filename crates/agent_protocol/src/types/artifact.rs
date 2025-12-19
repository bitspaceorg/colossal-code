//! Artifact types for the A2A protocol
//!
//! Artifacts are outputs produced by agents during task execution.
//! They can be files, data, or other content that results from processing.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::message::{DataPart, FilePart, TextPart};

/// An artifact produced by an agent during task execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    /// Unique identifier for this artifact
    pub artifact_id: String,

    /// Human-readable name for the artifact
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Description of what this artifact represents
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The artifact content parts
    pub parts: Vec<ArtifactPart>,

    /// Index for ordering (useful for streaming updates)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,

    /// Whether the artifact is still being generated
    #[serde(default)]
    pub streaming: bool,

    /// Whether the artifact is complete
    #[serde(default = "default_true")]
    pub complete: bool,

    /// Additional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

fn default_true() -> bool {
    true
}

/// A part of an artifact
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ArtifactPart {
    /// Text content
    Text(TextPart),
    /// File content
    File(FilePart),
    /// Structured data
    Data(DataPart),
}

impl Artifact {
    /// Create a new empty artifact
    pub fn new() -> Self {
        Self {
            artifact_id: Uuid::new_v4().to_string(),
            name: None,
            description: None,
            parts: vec![],
            index: None,
            streaming: false,
            complete: true,
            metadata: None,
        }
    }

    /// Create a text artifact
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            artifact_id: Uuid::new_v4().to_string(),
            name: None,
            description: None,
            parts: vec![ArtifactPart::Text(TextPart {
                text: content.into(),
                content_type: None,
            })],
            index: None,
            streaming: false,
            complete: true,
            metadata: None,
        }
    }

    /// Create a file artifact from a URI
    pub fn file(uri: impl Into<String>, media_type: impl Into<String>) -> Self {
        use super::message::FileContent;

        Self {
            artifact_id: Uuid::new_v4().to_string(),
            name: None,
            description: None,
            parts: vec![ArtifactPart::File(FilePart {
                file: FileContent {
                    name: None,
                    media_type: media_type.into(),
                    uri: Some(uri.into()),
                    bytes: None,
                },
            })],
            index: None,
            streaming: false,
            complete: true,
            metadata: None,
        }
    }

    /// Create a data artifact
    pub fn data(data: serde_json::Value) -> Self {
        Self {
            artifact_id: Uuid::new_v4().to_string(),
            name: None,
            description: None,
            parts: vec![ArtifactPart::Data(DataPart { data, schema: None })],
            index: None,
            streaming: false,
            complete: true,
            metadata: None,
        }
    }

    /// Set the artifact name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the artifact description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the artifact index
    pub fn with_index(mut self, index: usize) -> Self {
        self.index = Some(index);
        self
    }

    /// Mark as streaming (incomplete)
    pub fn as_streaming(mut self) -> Self {
        self.streaming = true;
        self.complete = false;
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Add a part to the artifact
    pub fn add_part(&mut self, part: ArtifactPart) {
        self.parts.push(part);
    }

    /// Mark the artifact as complete
    pub fn mark_complete(&mut self) {
        self.streaming = false;
        self.complete = true;
    }

    /// Get concatenated text content
    pub fn text_content(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                ArtifactPart::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Default for Artifact {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_artifact() {
        let artifact = Artifact::text("Hello, world!")
            .with_name("greeting")
            .with_description("A friendly greeting");

        assert_eq!(artifact.text_content(), "Hello, world!");
        assert_eq!(artifact.name, Some("greeting".to_string()));
        assert!(artifact.complete);
    }

    #[test]
    fn test_streaming_artifact() {
        let artifact = Artifact::text("Partial...").as_streaming();

        assert!(artifact.streaming);
        assert!(!artifact.complete);
    }
}
