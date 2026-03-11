use std::time::Instant;

use agent_core::AgentMessage;

use crate::app::state::message::{CompactionEntry, MessageState, MessageType};
use crate::app::state::orchestrator::ToolCallStatus;
use crate::app::state::ui_message_event::UiMessageEvent;
use crate::{App, StepToolCallEntry, TodoItem};

/// Outcome flags produced by draining the agent message channel.
/// These are deferred effects that must be applied after releasing the channel borrow.
pub(crate) struct AgentStreamOutcome {
    pub process_queued: bool,
    pub process_interrupt: Option<String>,
    pub pending_todos: Option<Vec<TodoItem>>,
    pub create_rewind: bool,
    pub pending_file_change: Option<(String, String, String)>,
    pub check_auto_summarize: bool,
    pub trigger_mid_stream_auto_summarize: bool,
    pub schedule_resume_prompt: bool,
}

impl AgentStreamOutcome {
    fn new() -> Self {
        Self {
            process_queued: false,
            process_interrupt: None,
            pending_todos: None,
            create_rewind: false,
            pending_file_change: None,
            check_auto_summarize: false,
            trigger_mid_stream_auto_summarize: false,
            schedule_resume_prompt: false,
        }
    }
}

impl App {
    /// Drain the agent message channel, updating UI state for each message.
    /// Returns deferred outcome flags that the caller must apply after this borrow ends.
    pub(crate) fn drain_agent_rx(&mut self) -> AgentStreamOutcome {
        let mut outcome = AgentStreamOutcome::new();
        if let Some(rx) = &mut self.agent_rx {
            while let Ok(msg) = rx.try_recv() {
                // Skip processing agent messages if we've interrupted
                if self.agent_state.agent_interrupted {
                    match msg {
                        AgentMessage::GenerationStats(stats) => {
                            Self::record_generation_stats_fields(
                                &mut self.generation_stats,
                                &mut self.generation_stats_rendered,
                                stats,
                            );
                        }
                        AgentMessage::Done => {
                            self.agent_state.agent_interrupted = false;
                            // Still check for auto-summarization even after interruption
                            // If context is low, we should summarize regardless
                            outcome.check_auto_summarize = true;
                        }
                        _ => {}
                    }
                    continue;
                }

                // Skip main agent messages while orchestration is running
                if self.orchestration_in_progress {
                    match &msg {
                        AgentMessage::ThinkingContent(_, _)
                        | AgentMessage::AgentResponse(_, _)
                        | AgentMessage::ThinkingSummary(_)
                        | AgentMessage::GenerationStats(_)
                        | AgentMessage::ToolCallStarted(_, _)
                        | AgentMessage::ToolCallCompleted(_, _) => {
                            // Discard main agent output while orchestrating
                            // Subagent tool calls go through OrchestratorEvent, not here
                            continue;
                        }
                        AgentMessage::Done => {
                            // Agent finished but orchestration still running - ignore
                            continue;
                        }
                        _ => {}
                    }
                }

                match msg {
                    AgentMessage::ThinkingContent(thinking, token_count) => {
                        if self.limit_thinking_to_first_token
                            && self.agent_state.agent_response_started
                        {
                            continue;
                        }
                        // Add or maintain thinking animation placeholder
                        let should_add_thinking = if let Some(last_msg) = self.messages.last() {
                            !matches!(
                                UiMessageEvent::parse(last_msg),
                                Some(UiMessageEvent::ThinkingAnimation)
                            )
                        } else {
                            true
                        };

                        if should_add_thinking {
                            self.messages
                                .push(UiMessageEvent::ThinkingAnimation.to_message());
                            self.message_types.push(MessageType::Agent);
                            self.thinking_indicator_active = true;
                        }
                        self.is_thinking = true;
                        // Don't reset thinking_start_time here - it was already set on submit
                        if self.thinking_start_time.is_none() {
                            self.thinking_start_time = Some(Instant::now());
                        }

                        // Accumulate raw thinking content for export
                        self.thinking_raw_content.push_str(&thinking);

                        // Use actual token count from tokenizer
                        self.thinking_token_count += token_count;

                        // Check for mid-stream auto-summarize (inlined to avoid borrow conflict)
                        // Only check if not already compacting/triggered and no queued messages
                        if !self.agent_state.is_compacting
                            && !self.is_auto_summarize
                            && self.queued_messages.is_empty()
                        {
                            if let Some(limit) = self.current_context_tokens {
                                if limit > 0 {
                                    // Use preserved context from previous turn + current streaming
                                    let streaming_tokens = self.streaming_completion_tokens
                                        + self.thinking_token_count;
                                    let used = self
                                        .last_known_context_tokens
                                        .saturating_add(streaming_tokens);
                                    if used > 0 {
                                        let remaining = limit.saturating_sub(used);
                                        let percent_left = (remaining as f32 / limit as f32
                                            * 100.0)
                                            .clamp(0.0, 100.0);
                                        let percent_used = 100.0 - percent_left;
                                        if percent_used >= self.auto_summarize_threshold {
                                            outcome.trigger_mid_stream_auto_summarize = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    AgentMessage::ThinkingSummary(summary) => {
                        if self.limit_thinking_to_first_token {
                            continue;
                        }
                        // Parse summary format: "text|token_count|chunk_count"
                        let (summary_text, token_count, chunk_count) =
                            if let Some(last_pipe) = summary.rfind('|') {
                                let chunk_str = &summary[last_pipe + 1..];
                                let chunk_count = chunk_str.parse::<usize>().unwrap_or(0);

                                let summary_without_chunk = &summary[..last_pipe];
                                if let Some(first_pipe) = summary_without_chunk.rfind('|') {
                                    let text = summary_without_chunk[..first_pipe].to_string();
                                    let token_str = &summary_without_chunk[first_pipe + 1..];
                                    let token_count = token_str.parse::<usize>().unwrap_or(0);
                                    (text, token_count, chunk_count)
                                } else {
                                    (summary.clone(), 0, 0)
                                }
                            } else {
                                (summary.clone(), 0, 0)
                            };

                        // If we have a current summary, move it to a static tree line
                        if let Some((old_summary, old_tokens, old_chunks)) =
                            self.thinking_current_summary.take()
                        {
                            Self::remove_thinking_animation_placeholder(
                                &mut self.messages,
                                &mut self.message_types,
                            );

                            // Add old summary as static tree line with token count and chunk count
                            self.messages.push(Self::format_thinking_tree_line(
                                old_summary,
                                old_tokens,
                                old_chunks,
                                false,
                            ));
                            self.message_types.push(MessageType::Agent);

                            Self::ensure_thinking_animation_placeholder(
                                &mut self.messages,
                                &mut self.message_types,
                                self.thinking_indicator_active,
                            );
                        }
                        // Store new summary as current (will show with snowflake)
                        self.thinking_current_summary =
                            Some((summary_text, token_count, chunk_count));
                        // Reset animation position to start wave from beginning
                        self.thinking_position = 0;
                    }
                    AgentMessage::AgentResponse(text, token_count) => {
                        // Skip empty responses
                        if text.is_empty() {
                            continue;
                        }

                        // Accumulate completion tokens for real-time context tracking
                        self.streaming_completion_tokens += token_count;

                        // Check for mid-stream auto-summarize (inlined to avoid borrow conflict)
                        // Only check if not already compacting/triggered and no queued messages
                        if !self.agent_state.is_compacting
                            && !self.is_auto_summarize
                            && self.queued_messages.is_empty()
                        {
                            if let Some(limit) = self.current_context_tokens {
                                if limit > 0 {
                                    // Use preserved context from previous turn + current streaming
                                    let streaming_tokens = self.streaming_completion_tokens
                                        + self.thinking_token_count;
                                    let used = self
                                        .last_known_context_tokens
                                        .saturating_add(streaming_tokens);
                                    if used > 0 {
                                        let remaining = limit.saturating_sub(used);
                                        let percent_left = (remaining as f32 / limit as f32
                                            * 100.0)
                                            .clamp(0.0, 100.0);
                                        let percent_used = 100.0 - percent_left;
                                        if percent_used >= self.auto_summarize_threshold {
                                            outcome.trigger_mid_stream_auto_summarize = true;
                                        }
                                    }
                                }
                            }
                        }

                        // IMPORTANT: Remove thinking animation FIRST, unconditionally
                        Self::remove_thinking_animation_placeholder(
                            &mut self.messages,
                            &mut self.message_types,
                        );

                        // THEN convert summary to static tree line if it exists
                        if let Some((final_summary, token_count, chunk_count)) =
                            self.thinking_current_summary.take()
                        {
                            self.messages.push(Self::format_thinking_tree_line(
                                final_summary,
                                token_count,
                                chunk_count,
                                true,
                            ));
                            self.message_types.push(MessageType::Agent);
                        }
                        self.is_thinking = false;
                        // Note: Don't clear thinking_raw_content here - it will be used in export

                        // Check if we should append to existing message or create new one
                        // Determine if the last rendered entry is a thinking tree line.
                        let last_is_thinking_tree = self
                            .messages
                            .last()
                            .map(|msg| msg.starts_with("├── ") || msg.starts_with("└── "))
                            .unwrap_or(false);

                        let should_create_new = if !self.agent_state.agent_response_started {
                            // First chunk of agent response - always create new message
                            true
                        } else if last_is_thinking_tree {
                            // When a thinking summary was just rendered, force a fresh bubble
                            true
                        } else if let Some(last_msg) = self.messages.last() {
                            // Already started - check if last message is a special marker
                            // If last message starts with '[', it's a tool call or error, so create new
                            last_msg.starts_with('[')
                        } else {
                            true
                        };

                        if should_create_new {
                            self.messages.push(text);
                            self.message_types.push(MessageType::Agent);
                            self.agent_state.agent_response_started = true;
                        } else {
                            // Append to existing agent response
                            if let Some(last_msg) = self.messages.last_mut() {
                                last_msg.push_str(&text);
                            }
                        }

                        Self::ensure_thinking_animation_placeholder(
                            &mut self.messages,
                            &mut self.message_types,
                            self.thinking_indicator_active,
                        );
                    }
                    AgentMessage::ToolCallStarted(tool_name, arguments) => {
                        // Keep thinking animation visible by moving it below tool call output
                        Self::remove_thinking_animation_placeholder(
                            &mut self.messages,
                            &mut self.message_types,
                        );

                        // Convert summary to static tree line if it exists, but keep the animation running
                        if let Some((current_summary, token_count, chunk_count)) =
                            self.thinking_current_summary.take()
                        {
                            self.messages.push(Self::format_thinking_tree_line(
                                current_summary,
                                token_count,
                                chunk_count,
                                false,
                            ));
                            self.message_types.push(MessageType::Agent);
                        }

                        // Store raw arguments for file change tracking
                        self.last_tool_args = Some((tool_name.clone(), arguments.clone()));

                        // Format arguments for display
                        let formatted_args = Self::format_tool_arguments(&tool_name, &arguments);
                        self.messages.push(
                            UiMessageEvent::ToolCallStarted {
                                tool_name: tool_name.clone(),
                                args: formatted_args,
                            }
                            .to_message(),
                        );
                        self.message_types.push(MessageType::Agent);
                        self.thinking_indicator_active = true;

                        if self.current_spec.is_some() {
                            if let Some(prefix) = self.active_step_prefix.clone() {
                                let planned_label = if prefix.contains('.') {
                                    self.step_label_overrides.get(&prefix).cloned()
                                } else {
                                    None
                                };
                                let label = planned_label.unwrap_or_else(|| {
                                    Self::describe_tool_call(&tool_name, &arguments)
                                });
                                let entry_id = self.next_tool_call_id;
                                self.next_tool_call_id = self.next_tool_call_id.saturating_add(1);
                                let session_metadata = self.orchestrator_sessions.get(&prefix);
                                let role = session_metadata
                                    .map(|entry| entry.role.clone())
                                    .unwrap_or(crate::SessionRole::Implementor);
                                let worktree_branch = session_metadata
                                    .and_then(|entry| entry.worktree_branch.clone());
                                let worktree_path =
                                    session_metadata.and_then(|entry| entry.worktree_path.clone());
                                self.step_tool_calls
                                    .entry(prefix.clone())
                                    .or_default()
                                    .push(StepToolCallEntry {
                                        id: entry_id,
                                        label,
                                        status: ToolCallStatus::Started,
                                        role,
                                        worktree_branch,
                                        worktree_path,
                                    });
                                self.active_tool_call = Some((prefix, entry_id));
                            }
                        }

                        Self::ensure_thinking_animation_placeholder(
                            &mut self.messages,
                            &mut self.message_types,
                            self.thinking_indicator_active,
                        );

                        // Keep thinking animation running during tool calls
                        // Don't stop thinking animation - let it continue during tool execution
                        // self.is_thinking = false;
                        // self.thinking_start_time = None;
                        // self.thinking_token_count = 0;
                        // Note: Don't clear thinking_raw_content here - it will be used in export
                    }
                    AgentMessage::ToolCallCompleted(tool_name, result) => {
                        // Check for actual tool errors, not just content containing "error"
                        let is_error = result.starts_with("Error:")
                            || result.starts_with("error:")
                            || result.starts_with("Failed:")
                            || result.starts_with("failed:")
                            || result.starts_with("Permission denied")
                            || result.starts_with("No such file")
                            || result.starts_with("Command failed")
                            || (result.len() < 500 && result.contains("\"error\""))
                            || (result.len() < 500
                                && result.contains("\"is_error\": true"))
                            // Check for YAML-format tool results with status: Failure
                            || result.contains("status: Failure");

                        if let Some((prefix, entry_id)) = self.active_tool_call.take() {
                            if let Some(entries) = self.step_tool_calls.get_mut(&prefix) {
                                if let Some(entry) =
                                    entries.iter_mut().find(|entry| entry.id == entry_id)
                                {
                                    entry.status = if is_error {
                                        ToolCallStatus::Error
                                    } else {
                                        ToolCallStatus::Completed
                                    };
                                }
                            }
                        }

                        // Check for sandbox permission errors and offer to add to writable roots
                        if let Ok(result_yaml) = serde_yaml::from_str::<serde_yaml::Value>(&result)
                        {
                            if let Some(obj) = result_yaml.as_mapping() {
                                if let Some(msg) = obj
                                    .get(serde_yaml::Value::String("message".to_string()))
                                    .and_then(|v| v.as_str())
                                {
                                    // Check if this is a sandbox permission error
                                    if msg.contains("Sandbox denied")
                                        || msg.contains("permission denied")
                                    {
                                        // Try to extract file path from the error message
                                        // Typical format: "Sandbox denied (code N): path/to/file"
                                        if let Some(path_start) = msg.find("): ") {
                                            let potential_path = msg[path_start + 3..].trim();
                                            // Basic validation that it looks like a path
                                            if potential_path.starts_with('/')
                                                || potential_path.starts_with('.')
                                            {
                                                // Show sandbox permission prompt
                                                self.safety_state.sandbox_blocked_path =
                                                    potential_path.to_string();
                                                self.safety_state.show_sandbox_prompt = true;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Special handling for todo_write tool
                        if tool_name == "todo_write" {
                            // Parse the result to extract todos and store them for saving
                            if let Ok(result_json) =
                                serde_json::from_str::<serde_json::Value>(&result)
                            {
                                if let Some(todos_array) =
                                    result_json.get("todos").and_then(|v| v.as_array())
                                {
                                    let todos: Vec<TodoItem> = todos_array
                                        .iter()
                                        .filter_map(|t| Self::parse_todo_item(t))
                                        .collect();

                                    // Store todos to be saved after message processing
                                    outcome.pending_todos = Some(todos);
                                }
                            }
                        }

                        // Special handling for orchestrate_task tool
                        if tool_name == "orchestrate_task" {
                            if let Ok(result_json) =
                                serde_json::from_str::<serde_json::Value>(&result)
                            {
                                if result_json.get("status").and_then(|v| v.as_str())
                                    == Some("orchestration_requested")
                                {
                                    if let Some(goal) =
                                        result_json.get("goal").and_then(|v| v.as_str())
                                    {
                                        self.orchestration_pending = Some(goal.to_string());
                                        // Block main agent while orchestration runs
                                        self.orchestration_in_progress = true;
                                        // Stop the main agent's thinking animation
                                        self.thinking_indicator_active = false;
                                        Self::remove_thinking_animation_placeholder(
                                            &mut self.messages,
                                            &mut self.message_types,
                                        );
                                    }
                                }
                            }
                        }

                        // Track file changes for Write and Edit tools using stored raw arguments
                        if let Some((stored_tool, stored_args)) = &self.last_tool_args {
                            if stored_tool == &tool_name {
                                outcome.pending_file_change =
                                    Some((tool_name.clone(), stored_args.clone(), result.clone()));
                            }
                        }

                        // If thinking is active, remove thinking animation temporarily
                        Self::remove_thinking_animation_placeholder(
                            &mut self.messages,
                            &mut self.message_types,
                        );

                        // Find and replace the started message with completed
                        for msg in self.messages.iter_mut().rev() {
                            let Some(UiMessageEvent::ToolCallStarted {
                                tool_name: started_tool,
                                args,
                            }) = UiMessageEvent::parse(msg)
                            else {
                                continue;
                            };
                            if started_tool == tool_name {
                                let formatted_result =
                                    Self::format_tool_result(&tool_name, &result);
                                *msg = UiMessageEvent::ToolCallCompleted {
                                    tool_name: tool_name.clone(),
                                    args,
                                    result: formatted_result,
                                }
                                .to_message();
                                break;
                            }
                        }

                        // Re-add thinking animation at the bottom while tool results stream
                        // But NOT if orchestration just started - it has its own animation
                        if !self.orchestration_in_progress {
                            self.thinking_indicator_active = true;
                            Self::ensure_thinking_animation_placeholder(
                                &mut self.messages,
                                &mut self.message_types,
                                self.thinking_indicator_active,
                            );
                        }
                    }
                    AgentMessage::RequestApproval(content) => {
                        self.safety_state.show_approval_prompt = true;
                        self.safety_state.approval_prompt_content = content;
                    }
                    AgentMessage::ThinkingComplete(_residual_tokens) => {
                        // Thinking content has stopped streaming, but keep the indicator
                        // active so the user still sees progress while tool calls run.
                        self.is_thinking = false;
                    }
                    AgentMessage::Error(err) => {
                        // IMPORTANT: Remove thinking animation FIRST, unconditionally
                        Self::remove_thinking_animation_placeholder(
                            &mut self.messages,
                            &mut self.message_types,
                        );

                        // For errors, discard the thinking summary (don't convert to static tree line)
                        self.thinking_current_summary = None;

                        // Add the error message
                        self.messages.push(format!("[Error: {}]", err));
                        self.message_types.push(MessageType::Agent);
                        self.agent_state.agent_processing = false;
                        self.is_thinking = false;
                        self.thinking_indicator_active = false;
                        self.thinking_start_time = None;
                        self.thinking_token_count = 0;
                        self.agent_state.agent_response_started = false;
                    }
                    AgentMessage::GenerationStats(stats) => {
                        // Store the generation stats
                        Self::record_generation_stats_fields(
                            &mut self.generation_stats,
                            &mut self.generation_stats_rendered,
                            stats,
                        );
                    }
                    AgentMessage::BackgroundTaskStarted(session_id, command, log_file) => {
                        // Add background task to the list with current time as start time
                        self.background_tasks.push((
                            session_id,
                            command,
                            log_file,
                            std::time::Instant::now(),
                        ));
                    }
                    AgentMessage::ContextCleared => {
                        // Context cleared - if no inject expected, sync is complete
                        if !self.context_inject_expected {
                            self.context_sync_pending = false;
                            self.context_sync_started = None;
                        }
                        // Otherwise wait for ContextInjected
                    }
                    AgentMessage::ContextInjected => {
                        // Context injection complete - sync is done
                        self.context_sync_pending = false;
                        self.context_sync_started = None;
                        self.context_inject_expected = false;
                        outcome.schedule_resume_prompt = true;
                    }
                    AgentMessage::Done => {
                        // IMPORTANT: Remove thinking animation FIRST, unconditionally
                        Self::remove_thinking_animation_placeholder(
                            &mut self.messages,
                            &mut self.message_types,
                        );

                        // THEN convert summary to static tree line if it exists
                        if let Some((final_summary, token_count, chunk_count)) =
                            self.thinking_current_summary.take()
                        {
                            self.messages.push(Self::format_thinking_tree_line(
                                final_summary,
                                token_count,
                                chunk_count,
                                true,
                            ));
                            self.message_types.push(MessageType::Agent);
                        }
                        self.agent_state.agent_processing = false;
                        self.is_thinking = false;
                        self.thinking_indicator_active = false;
                        self.thinking_start_time = None;
                        self.thinking_token_count = 0;
                        self.streaming_completion_tokens = 0; // Reset for next turn
                        self.agent_state.agent_response_started = false;

                        // Handle compaction completion
                        if self.agent_state.is_compacting {
                            let was_auto_summarize = self.is_auto_summarize;

                            // Find the summary (last agent message that's not a marker)
                            let mut summary = String::new();
                            for (_i, (msg, msg_type)) in self
                                .messages
                                .iter()
                                .zip(self.message_types.iter())
                                .enumerate()
                                .rev()
                            {
                                if matches!(msg_type, MessageType::Agent)
                                    && !msg.starts_with("[")
                                    && !msg.starts_with(" ⎿")
                                    && !msg.starts_with("●")
                                {
                                    summary = msg.clone();
                                    break;
                                }
                            }

                            // Check if we got a valid summary before clearing anything
                            if summary.is_empty() {
                                // Compaction failed - preserve conversation state
                                self.agent_state.is_compacting = false;
                                self.is_auto_summarize = false;
                                self.compaction_resume_prompt = None;
                                self.compaction_resume_ready = false;

                                // Show error banner but keep all existing messages
                                let error_msg = if was_auto_summarize {
                                    " ⎿ Auto-summarization failed: no summary generated. Conversation preserved."
                                } else {
                                    " ⎿ Summarization failed: no summary generated. Conversation preserved."
                                };
                                self.messages.push(error_msg.to_string());
                                self.message_types.push(MessageType::Agent);
                                self.message_states.push(MessageState::Sent);
                                self.message_metadata.push(None);
                                self.message_timestamps.push(std::time::SystemTime::now());

                                // Skip to allow retry later
                                continue;
                            }

                            // Valid summary - now safe to clear and proceed
                            self.agent_state.is_compacting = false;
                            self.is_auto_summarize = false;

                            // Capture a rewind point before wiping the transcript
                            if let Some(rewind_point) = Self::snapshot_rewind_point(
                                &self.messages,
                                &self.message_types,
                                &self.message_states,
                                &self.message_metadata,
                                &self.message_timestamps,
                                &self.current_file_changes,
                            ) {
                                self.rewind_points.push(rewind_point);
                                self.current_file_changes.clear();
                                if self.rewind_points.len() > 50 {
                                    self.rewind_points.remove(0);
                                }
                            }

                            // Clear all messages
                            self.messages.clear();
                            self.message_types.clear();
                            self.message_states.clear();
                            self.message_metadata.clear();
                            self.message_timestamps.clear();

                            // Clear agent context and inject summary as new context
                            if let Some(tx) = &self.agent_tx {
                                // Start context sync - will wait for ContextInjected
                                self.context_sync_pending = true;
                                self.context_sync_started = Some(Instant::now());
                                self.context_inject_expected = true;
                                let _ = tx.send(AgentMessage::ClearContext);
                                let _ = tx.send(AgentMessage::InjectContext(summary.clone()));
                            }

                            // Add the summary as the new context (summary is guaranteed non-empty here)
                            self.last_compacted_summary = Some(summary.clone());

                            self.compaction_history.push(CompactionEntry {
                                summary: summary.clone(),
                            });
                            if self.compaction_history.len() > crate::MAX_COMPACTION_HISTORY {
                                self.compaction_history.remove(0);
                            }
                            self.summary_history_selected =
                                self.compaction_history.len().saturating_sub(1);

                            // Different banner for auto vs manual summarization
                            let banner_text = if was_auto_summarize {
                                "Context low · auto-summarized · ctrl+o for history"
                            } else {
                                "Conversation summarized · ctrl+o for history"
                            };
                            let banner_line =
                                format!("{}{}", crate::SUMMARY_BANNER_PREFIX, banner_text);
                            self.messages.push(banner_line);
                            self.message_types.push(MessageType::Agent);
                            self.message_states.push(MessageState::Sent);
                            self.message_metadata.push(None);
                            self.message_timestamps.push(std::time::SystemTime::now());

                            // Different user message for auto vs manual
                            let user_msg = if was_auto_summarize {
                                "[auto-summarized]".to_string()
                            } else {
                                "/summarize".to_string()
                            };
                            self.messages.push(user_msg);
                            self.message_types.push(MessageType::User);
                            self.message_states.push(MessageState::Sent);
                            self.message_metadata.push(None);
                            self.message_timestamps.push(std::time::SystemTime::now());

                            let result_msg = if was_auto_summarize {
                                " ⎿ Auto-summarized (ctrl+o to see full summary)"
                            } else {
                                " ⎿ Summarized (ctrl+o to see full summary)"
                            };
                            self.messages.push(result_msg.to_string());
                            self.message_types.push(MessageType::Agent);
                            self.message_states.push(MessageState::Sent);
                            self.message_metadata.push(None);
                            self.message_timestamps.push(std::time::SystemTime::now());

                            if self.compaction_resume_prompt.is_some() {
                                self.compaction_resume_ready = true;
                            }

                            // Skip normal done handling for compaction
                            continue;
                        }

                        // Check for interrupt pending FIRST
                        if let Some(interrupt_msg) = self.agent_state.interrupt_pending.take() {
                            // Mark last message (interrupted one) as Interrupted
                            {
                                if let Some(last_state) = self.message_states.last_mut() {
                                    if matches!(last_state, MessageState::Sent) {
                                        *last_state = MessageState::Interrupted;
                                    }
                                }
                            }

                            // Add interrupt marker message
                            self.messages.push("● Interrupted".to_string());
                            self.message_types.push(MessageType::Agent);
                            self.message_states.push(MessageState::Sent);

                            // Add the prompt message
                            self.messages
                                .push(" ⎿ What should Nite do instead?".to_string());
                            self.message_types.push(MessageType::Agent);
                            self.message_states.push(MessageState::Sent);

                            Self::ensure_generation_stats_marker_fields(
                                &mut self.messages,
                                &mut self.message_types,
                                &mut self.message_states,
                                &mut self.message_metadata,
                                &mut self.message_timestamps,
                                &self.generation_stats,
                                &mut self.generation_stats_rendered,
                            );

                            // Set flag to process interrupt after rx is dropped
                            outcome.process_interrupt = Some(interrupt_msg);
                        } else {
                            // Update last message state from Queued to Sent if needed
                            {
                                if let Some(last_state) = self.message_states.last_mut() {
                                    if matches!(last_state, MessageState::Queued) {
                                        *last_state = MessageState::Sent;
                                    }
                                }
                            }

                            // Set flag to create a rewind point after rx is dropped
                            outcome.create_rewind = true;

                            outcome.process_queued = true; // Set flag to process queued message after rx is dropped

                            // Set flag to check for auto-summarization after rx borrow is dropped
                            outcome.check_auto_summarize = true;

                            Self::ensure_generation_stats_marker_fields(
                                &mut self.messages,
                                &mut self.message_types,
                                &mut self.message_states,
                                &mut self.message_metadata,
                                &mut self.message_timestamps,
                                &self.generation_stats,
                                &mut self.generation_stats_rendered,
                            );
                        }
                    }
                    AgentMessage::ModelLoaded => {
                        // Model has been loaded successfully
                        self.messages
                            .push(" ✔ Model loaded successfully".to_string());
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                    }
                    _ => {}
                }
            }
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::AgentStreamOutcome;

    #[test]
    fn agent_stream_outcome_defaults_to_no_deferred_effects() {
        let outcome = AgentStreamOutcome::new();

        assert!(!outcome.process_queued);
        assert!(outcome.process_interrupt.is_none());
        assert!(outcome.pending_todos.is_none());
        assert!(!outcome.create_rewind);
        assert!(outcome.pending_file_change.is_none());
        assert!(!outcome.check_auto_summarize);
        assert!(!outcome.trigger_mid_stream_auto_summarize);
        assert!(!outcome.schedule_resume_prompt);
    }
}
