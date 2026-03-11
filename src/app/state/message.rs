use crate::app::render::thinking::encode_generation_stats_message;
use crate::{AssistantMode, HelpTab, UiMessageEvent};
use agent_core::GenerationStats as AgentGenerationStats;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant, SystemTime};

use super::snapshot::AppSnapshot;

/// Message type to distinguish between user and agent messages
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageType {
    User,
    Agent,
}

/// Stores the transcript for a sub-agent so we can replay it inside the UI.
#[derive(Clone, Debug)]
pub struct SubAgentContext {
    pub prefix: String,
    pub step_title: String,
    pub messages: Vec<UiMessage>,
    pub thinking_indicator_active: bool,
    pub is_thinking: bool,
    pub thinking_elapsed_secs: u64,
    pub thinking_token_count: usize,
    pub thinking_current_summary: Option<(String, usize, usize)>,
    pub thinking_position: usize,
    pub thinking_loader_frame: usize,
    pub thinking_current_word: String,
    pub generation_stats: Option<AgentGenerationStats>,
    pub generation_stats_rendered: bool,
    pub started_orchestration: bool,
    pub thinking_last_update: Instant,
    pub thinking_last_word_change: Instant,
    pub thinking_last_tick: Instant,
    pub thinking_start_time: Option<Instant>,
}

impl SubAgentContext {
    pub fn new(prefix: String, step_title: String) -> Self {
        Self {
            prefix,
            step_title,
            messages: Vec::new(),
            thinking_indicator_active: false,
            is_thinking: false,
            thinking_elapsed_secs: 0,
            thinking_token_count: 0,
            thinking_current_summary: None,
            thinking_position: 0,
            thinking_loader_frame: 0,
            thinking_current_word: "thinking".to_string(),
            generation_stats: None,
            generation_stats_rendered: false,
            started_orchestration: false,
            thinking_last_update: Instant::now(),
            thinking_last_word_change: Instant::now(),
            thinking_last_tick: Instant::now(),
            thinking_start_time: None,
        }
    }

    fn push_message(&mut self, content: String, message_type: MessageType) {
        self.messages
            .push(UiMessage::new(content, message_type, MessageState::Sent));
    }

    pub fn add_user_message(&mut self, content: String) {
        self.push_message(content, MessageType::User);
    }

    pub fn add_agent_text(&mut self, content: String) {
        // Remove thinking placeholder before showing agent text
        self.remove_thinking_placeholder();

        let append_to_last = if let Some(last) = self.messages.last() {
            if matches!(last.message_type, MessageType::Agent) {
                !matches!(
                    UiMessageEvent::parse(&last.content),
                    Some(UiMessageEvent::ToolCallStarted { .. })
                        | Some(UiMessageEvent::ToolCallCompleted { .. })
                )
            } else {
                false
            }
        } else {
            false
        };

        if append_to_last {
            if let Some(last) = self.messages.last_mut() {
                last.content.push_str(&content);
                return;
            }
        }

        self.push_message(content, MessageType::Agent);
    }

    pub fn start_thinking(&mut self, summary: String) {
        if !self
            .messages
            .last()
            .map(|message| {
                matches!(
                    UiMessageEvent::parse(&message.content),
                    Some(UiMessageEvent::ThinkingAnimation)
                )
            })
            .unwrap_or(false)
        {
            self.push_message(
                UiMessageEvent::ThinkingAnimation.to_message(),
                MessageType::Agent,
            );
        }
        self.thinking_indicator_active = true;
        self.is_thinking = true;
        self.thinking_loader_frame = 0;
        self.thinking_position = 0;
        self.thinking_last_update = Instant::now();
        self.thinking_last_word_change = Instant::now();
        self.thinking_last_tick = Instant::now();
        self.thinking_start_time = Some(Instant::now());
        self.thinking_current_summary = if summary.is_empty() {
            None
        } else {
            Some((summary, 0, 0))
        };
    }

    pub fn finish_thinking(&mut self, duration_secs: u64) {
        self.remove_thinking_placeholder();
        self.thinking_indicator_active = false;
        self.is_thinking = false;
        self.thinking_elapsed_secs = duration_secs;
        self.thinking_start_time = None;
    }

    fn remove_thinking_placeholder(&mut self) {
        if let Some(last) = self.messages.last().map(|message| &message.content) {
            if matches!(
                UiMessageEvent::parse(last),
                Some(UiMessageEvent::ThinkingAnimation)
            ) {
                self.messages.pop();
            }
        }
        self.thinking_indicator_active = false;
        self.is_thinking = false;
    }

    pub fn add_tool_call_started(&mut self, tool_name: &str, formatted_args: String) {
        if tool_name == "orchestrate_task" {
            self.started_orchestration = true;
        }
        self.push_message(
            UiMessageEvent::ToolCallStarted {
                tool_name: tool_name.to_string(),
                args: formatted_args,
            }
            .to_message(),
            MessageType::Agent,
        );
    }

    pub fn complete_tool_call(&mut self, tool_name: &str, formatted_result: String) {
        for message in self.messages.iter_mut().rev() {
            let Some(UiMessageEvent::ToolCallStarted {
                tool_name: started_tool,
                args,
            }) = UiMessageEvent::parse(&message.content)
            else {
                continue;
            };
            if started_tool == tool_name {
                message.content = UiMessageEvent::ToolCallCompleted {
                    tool_name: tool_name.to_string(),
                    args,
                    result: formatted_result,
                }
                .to_message();
                break;
            }
        }
    }

    pub fn set_generation_stats(
        &mut self,
        tokens_per_sec: f32,
        prompt_tokens: usize,
        completion_tokens: usize,
    ) {
        self.generation_stats = Some(AgentGenerationStats {
            avg_completion_tok_per_sec: tokens_per_sec,
            completion_tokens,
            prompt_tokens,
            time_to_first_token_sec: 0.0,
            stop_reason: "end_turn".to_string(),
        });
        self.generation_stats_rendered = false;
    }

    pub(crate) fn to_snapshot(&self) -> AppSnapshot {
        // Calculate elapsed time: use live calculation if thinking is active, otherwise use stored value
        let elapsed_secs = if self.thinking_indicator_active {
            self.thinking_start_time
                .map(|t| t.elapsed().as_secs())
                .unwrap_or(0)
        } else {
            self.thinking_elapsed_secs
        };

        AppSnapshot {
            messages: self
                .messages
                .iter()
                .map(|message| message.content.clone())
                .collect(),
            message_types: self
                .messages
                .iter()
                .map(|message| message.message_type.clone())
                .collect(),
            thinking_indicator_active: self.thinking_indicator_active,
            thinking_elapsed_secs: elapsed_secs,
            thinking_token_count: self.thinking_token_count,
            thinking_current_summary: self.thinking_current_summary.clone(),
            thinking_position: self.thinking_position,
            thinking_loader_frame: self.thinking_loader_frame,
            thinking_current_word: self.thinking_current_word.clone(),
            generation_stats: self.generation_stats.clone(),
        }
    }

    pub fn ensure_generation_stats_marker(&mut self) {
        if self.generation_stats_rendered {
            return;
        }

        let has_marker = self.messages.iter().rev().take(6).any(|msg| {
            matches!(
                UiMessageEvent::parse(&msg.content),
                Some(UiMessageEvent::GenerationStats { .. })
            )
        });
        if has_marker {
            self.generation_stats_rendered = true;
            return;
        }

        if let Some(stats) = self.generation_stats.clone() {
            self.push_message(encode_generation_stats_message(&stats), MessageType::Agent);
            self.generation_stats_rendered = true;
        }
    }

    pub fn update_thinking_animation(
        &mut self,
        snowflake_len: usize,
        thinking_words: &[&'static str],
    ) {
        if !self.thinking_indicator_active {
            return;
        }

        let now = Instant::now();
        if snowflake_len > 0
            && now.duration_since(self.thinking_last_update) >= Duration::from_millis(100)
        {
            self.thinking_loader_frame = (self.thinking_loader_frame + 1) % snowflake_len.max(1);
            self.thinking_last_update = now;
        }

        if now.duration_since(self.thinking_last_word_change) >= Duration::from_secs(4) {
            use rand::seq::SliceRandom;
            if let Some(word) = thinking_words.choose(&mut rand::thread_rng()) {
                self.thinking_current_word = word.to_string();
                self.thinking_position = 0;
            }
            self.thinking_last_word_change = now;
        }

        if now.duration_since(self.thinking_last_tick) >= Duration::from_millis(40) {
            let text_with_dots = if let Some((summary, _, _)) = &self.thinking_current_summary {
                format!("{}...", summary)
            } else {
                format!("{}...", self.thinking_current_word)
            };
            let text_len = text_with_dots.chars().count();
            let sweep = text_len + 7;
            if sweep > 0 {
                self.thinking_position = (self.thinking_position + 1) % sweep;
            }
            self.thinking_last_tick = now;
        }
    }
}

impl Default for SubAgentContext {
    fn default() -> Self {
        Self::new(String::new(), String::new())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentConnector {
    None,
    Continue,
    End,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub(crate) enum MessageState {
    Sent,        // Normal sent message
    Queued,      // Message queued, waiting to be sent
    Interrupted, // Message generation was interrupted (partial)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiMessage {
    pub content: String,
    pub message_type: MessageType,
    pub(crate) message_state: MessageState,
}

impl UiMessage {
    fn new(content: String, message_type: MessageType, message_state: MessageState) -> Self {
        Self {
            content,
            message_type,
            message_state,
        }
    }
}

/// Rich metadata for different message types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum UIMessageMetadata {
    Thinking {
        summaries: Vec<String>,
        token_count: usize,
        duration_secs: u64,
    },
    ToolCall {
        tool_name: String,
        arguments: String,
        result: Option<String>,
        status: String, // "started", "completed", "failed"
    },
    GenerationStats {
        tokens_per_sec: f32,
        token_count: usize,
        time_to_first_token: f32,
        stop_reason: String,
    },
    Error {
        error_message: String,
    },
    Interrupt {
        reason: String,
    },
    BackgroundTask {
        session_id: String,
        command: String,
        log_file: String,
    },
    Command {
        command: String,
        feedback: String,
    },
}

/// Model information with metadata
#[derive(Debug, Clone)]
pub(crate) struct ModelInfo {
    pub(crate) filename: String,
    pub(crate) display_name: String,
    pub(crate) size_mb: f64,
    pub(crate) quantization: Option<String>,
    pub(crate) architecture: Option<String>,
    pub(crate) parameter_count: Option<String>,
    pub(crate) file_hash: Option<String>,
    pub(crate) author: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) context_length: Option<usize>,
}

/// File change statistics for rewind points
#[derive(Debug, Clone)]
pub(crate) struct FileChange {
    pub(crate) path: String,
    pub(crate) insertions: usize,
    pub(crate) deletions: usize,
}

/// Options for /summarize command
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompactOptions {
    /// Optional custom instructions for summarization
    pub(crate) custom_instructions: Option<String>,
}

/// Stored summary result for a compaction request
#[derive(Debug, Clone)]
pub(crate) struct CompactionEntry {
    pub(crate) summary: String,
}

/// Rewind point capturing conversation state at a specific moment
#[derive(Debug, Clone)]
pub(crate) struct RewindPoint {
    pub(crate) messages: Vec<String>,
    pub(crate) message_types: Vec<MessageType>,
    pub(crate) message_states: Vec<MessageState>,
    pub(crate) message_metadata: Vec<Option<UIMessageMetadata>>,
    pub(crate) message_timestamps: Vec<SystemTime>,
    pub(crate) timestamp: SystemTime,
    pub(crate) preview: String, // Description of this rewind point
    pub(crate) message_count: usize,
    pub(crate) file_changes: Vec<FileChange>, // Files modified in this rewind point
}

#[derive(Clone)]
pub(crate) struct UiState {
    pub(crate) show_help: bool,
    pub(crate) help_tab: HelpTab,
    pub(crate) show_resume: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            show_help: false,
            help_tab: HelpTab::General,
            show_resume: false,
        }
    }
}

#[derive(Clone)]
pub(crate) struct AgentState {
    pub(crate) agent_processing: bool,
    pub(crate) agent_interrupted: bool,
    pub(crate) is_compacting: bool,
    pub(crate) agent_response_started: bool,
    pub(crate) interrupt_pending: Option<String>,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            agent_processing: false,
            agent_interrupted: false,
            is_compacting: false,
            agent_response_started: false,
            interrupt_pending: None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct SafetyState {
    pub(crate) assistant_mode: AssistantMode,
    pub(crate) show_approval_prompt: bool,
    pub(crate) approval_prompt_content: String,
    pub(crate) show_sandbox_prompt: bool,
    pub(crate) sandbox_blocked_path: String,
    pub(crate) sandbox_enabled: bool,
}

impl Default for SafetyState {
    fn default() -> Self {
        Self {
            assistant_mode: AssistantMode::None,
            show_approval_prompt: false,
            approval_prompt_content: String::new(),
            show_sandbox_prompt: false,
            sandbox_blocked_path: String::new(),
            sandbox_enabled: false,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct PersistenceState {
    pub(crate) save_pending: bool,
    pub(crate) current_conversation_id: Option<String>,
    pub(crate) current_conversation_path: Option<std::path::PathBuf>,
    pub(crate) current_forked_from: Option<String>,
    pub(crate) current_forked_at: Option<SystemTime>,
}
