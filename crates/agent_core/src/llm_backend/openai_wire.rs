use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct OpenAiUsage {
    pub completion_tokens: Option<usize>,
    pub prompt_tokens: Option<usize>,
    pub total_tokens: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAiChatMessage {
    #[serde(default)]
    pub content: Option<OpenAiChatMessageContent>,
    pub role: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<OpenAiStreamToolCall>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum OpenAiChatMessageContent {
    Text(String),
    Parts(Vec<OpenAiChatMessagePart>),
}

#[derive(Debug, Deserialize)]
pub struct OpenAiChatMessagePart {
    #[serde(rename = "type")]
    pub _part_type: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiStreamToolCall {
    #[serde(default)]
    pub index: Option<usize>,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub _typ: Option<String>,
    pub function: Option<OpenAiToolFunction>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiToolFunction {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

impl OpenAiChatMessageContent {
    pub fn to_text(&self) -> String {
        match self {
            OpenAiChatMessageContent::Text(text) => text.clone(),
            OpenAiChatMessageContent::Parts(parts) => parts
                .iter()
                .filter_map(|part| part.text.as_ref())
                .cloned()
                .collect::<Vec<_>>()
                .join(""),
        }
    }
}

impl OpenAiChatMessage {
    pub fn content_text(&self) -> String {
        self.content
            .as_ref()
            .map(|content| content.to_text())
            .unwrap_or_default()
    }
}

#[derive(Debug, Deserialize)]
pub struct OpenAiChatChoice {
    pub message: OpenAiChatMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAiChatResponse {
    pub id: Option<String>,
    pub model: Option<String>,
    pub created: Option<u64>,
    pub system_fingerprint: Option<String>,
    pub object: Option<String>,
    pub choices: Vec<OpenAiChatChoice>,
    pub usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAiStreamResponse {
    pub id: Option<String>,
    pub model: Option<String>,
    pub created: Option<u64>,
    pub system_fingerprint: Option<String>,
    pub object: Option<String>,
    pub choices: Vec<OpenAiStreamChoice>,
    pub usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAiStreamChoice {
    pub index: usize,
    pub delta: OpenAiStreamDelta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAiStreamDelta {
    #[serde(default)]
    pub content: Option<OpenAiChatMessageContent>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<OpenAiStreamToolCall>>,
}

pub fn estimate_tokens(text: &str) -> usize {
    text.split_whitespace().count().max(1)
}

pub fn current_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
