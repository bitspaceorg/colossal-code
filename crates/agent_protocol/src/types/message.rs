//! Message types for the A2A protocol
//!
//! Messages are the primary unit of communication between agents.
//! They contain one or more parts which can be text, files, or structured data.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A message exchanged between agents
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// Unique identifier for this message
    pub message_id: String,

    /// Role of the sender
    pub role: Role,

    /// Content parts of the message
    pub parts: Vec<MessagePart>,

    /// Optional context ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,

    /// Optional task ID this message belongs to
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,

    /// Reference to another message this is replying to
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_id: Option<String>,

    /// Additional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Role of the message sender
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Message from the user/client agent
    User,
    /// Message from the server/responding agent
    Agent,
}

/// A part of a message content
///
/// Messages can contain multiple parts of different types,
/// allowing for rich multi-modal communication.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessagePart {
    /// Plain text content
    Text(TextPart),
    /// File content (URI or inline)
    File(FilePart),
    /// Structured data
    Data(DataPart),
}

/// Text content part
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPart {
    /// The text content
    pub text: String,

    /// Optional content type (defaults to text/plain)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

/// File content part
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePart {
    /// File content - either a URI or inline data
    pub file: FileContent,
}

/// File content - can be a reference or inline data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    /// Optional name for the file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// MIME type of the file
    pub media_type: String,

    /// URI reference to the file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,

    /// Base64-encoded inline data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<String>,
}

/// Structured data part
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataPart {
    /// Arbitrary JSON data
    pub data: serde_json::Value,

    /// Schema reference for the data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
}

impl Message {
    /// Create a new user message with text content
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            message_id: Uuid::new_v4().to_string(),
            role: Role::User,
            parts: vec![MessagePart::text(text)],
            context_id: None,
            task_id: None,
            reference_id: None,
            metadata: None,
        }
    }

    /// Create a new agent message with text content
    pub fn agent(text: impl Into<String>) -> Self {
        Self {
            message_id: Uuid::new_v4().to_string(),
            role: Role::Agent,
            parts: vec![MessagePart::text(text)],
            context_id: None,
            task_id: None,
            reference_id: None,
            metadata: None,
        }
    }

    /// Create a new message with the given role and parts
    pub fn new(role: Role, parts: Vec<MessagePart>) -> Self {
        Self {
            message_id: Uuid::new_v4().to_string(),
            role,
            parts,
            context_id: None,
            task_id: None,
            reference_id: None,
            metadata: None,
        }
    }

    /// Set the context ID
    pub fn with_context(mut self, context_id: impl Into<String>) -> Self {
        self.context_id = Some(context_id.into());
        self
    }

    /// Set the task ID
    pub fn with_task(mut self, task_id: impl Into<String>) -> Self {
        self.task_id = Some(task_id.into());
        self
    }

    /// Set a reference to another message
    pub fn in_reply_to(mut self, reference_id: impl Into<String>) -> Self {
        self.reference_id = Some(reference_id.into());
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Add a part to the message
    pub fn add_part(&mut self, part: MessagePart) {
        self.parts.push(part);
    }

    /// Get all text parts concatenated
    pub fn text_content(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                MessagePart::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Check if message contains any files
    pub fn has_files(&self) -> bool {
        self.parts.iter().any(|p| matches!(p, MessagePart::File(_)))
    }

    /// Get all file parts
    pub fn files(&self) -> impl Iterator<Item = &FilePart> {
        self.parts.iter().filter_map(|p| match p {
            MessagePart::File(f) => Some(f),
            _ => None,
        })
    }

    /// Get all data parts
    pub fn data(&self) -> impl Iterator<Item = &DataPart> {
        self.parts.iter().filter_map(|p| match p {
            MessagePart::Data(d) => Some(d),
            _ => None,
        })
    }
}

impl MessagePart {
    /// Create a text part
    pub fn text(content: impl Into<String>) -> Self {
        MessagePart::Text(TextPart {
            text: content.into(),
            content_type: None,
        })
    }

    /// Create a text part with specific content type
    pub fn text_with_type(content: impl Into<String>, content_type: impl Into<String>) -> Self {
        MessagePart::Text(TextPart {
            text: content.into(),
            content_type: Some(content_type.into()),
        })
    }

    /// Create a file part from a URI
    pub fn file_uri(uri: impl Into<String>, media_type: impl Into<String>) -> Self {
        MessagePart::File(FilePart {
            file: FileContent {
                name: None,
                media_type: media_type.into(),
                uri: Some(uri.into()),
                bytes: None,
            },
        })
    }

    /// Create a file part from inline bytes
    pub fn file_bytes(bytes: &[u8], media_type: impl Into<String>, name: Option<String>) -> Self {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        MessagePart::File(FilePart {
            file: FileContent {
                name,
                media_type: media_type.into(),
                uri: None,
                bytes: Some(encoded),
            },
        })
    }

    /// Create a data part
    pub fn data(data: serde_json::Value) -> Self {
        MessagePart::Data(DataPart { data, schema: None })
    }

    /// Create a data part with schema
    pub fn data_with_schema(data: serde_json::Value, schema: impl Into<String>) -> Self {
        MessagePart::Data(DataPart {
            data,
            schema: Some(schema.into()),
        })
    }
}

/// Parameters for sending a message (SendMessage method)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageParams {
    /// The message to send
    pub message: Message,

    /// Configuration for this request
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration: Option<MessageConfiguration>,
}

/// Configuration for message sending
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageConfiguration {
    /// Accepted output content types
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_output_modes: Option<Vec<String>>,

    /// Whether to include history in streaming
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_length: Option<usize>,

    /// Push notification configuration
    #[serde(skip_serializing_if = "Option::is_none", rename = "pushNotificationConfig")]
    pub push_notification_config: Option<super::push_notification::PushNotificationConfig>,

    /// Whether to block until completion
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocking: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_message() {
        let msg = Message::user("Hello, agent!");
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.text_content(), "Hello, agent!");
    }

    #[test]
    fn test_agent_message() {
        let msg = Message::agent("Hello, user!");
        assert_eq!(msg.role, Role::Agent);
        assert_eq!(msg.text_content(), "Hello, user!");
    }

    #[test]
    fn test_message_with_file() {
        let mut msg = Message::user("Check this file");
        msg.add_part(MessagePart::file_uri("https://example.com/file.txt", "text/plain"));

        assert!(msg.has_files());
        assert_eq!(msg.files().count(), 1);
    }

    #[test]
    fn test_message_serialization() {
        let msg = Message::user("Hello!")
            .with_context("ctx-123")
            .with_task("task-456");

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: Message = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.text_content(), "Hello!");
        assert_eq!(parsed.context_id, Some("ctx-123".to_string()));
    }
}
