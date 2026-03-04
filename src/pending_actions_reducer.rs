use std::time::{Instant, SystemTime};

use agent_core::AgentMessage;
use clipboard::{ClipboardContext, ClipboardProvider};
use color_eyre::Result;

use crate::agent_stream_reducer::AgentStreamOutcome;
use crate::state_domain::{MessageState, MessageType};
use crate::ui_message_event::UiMessageEvent;
use crate::{App, CompactOptions, git_ops};

impl App {
    pub(super) async fn process_pending_actions(&mut self, outcome: AgentStreamOutcome) {
        let AgentStreamOutcome {
            process_queued,
            process_interrupt,
            pending_todos,
            create_rewind,
            pending_file_change,
            check_auto_summarize,
            trigger_mid_stream_auto_summarize,
            mut schedule_resume_prompt,
        } = outcome;

        // Process interrupt message after rx borrow is dropped
        if let Some(interrupt_msg) = process_interrupt {
            // Check if interrupt message is a command
            if interrupt_msg.trim().starts_with('/') {
                // Execute command
                self.input = interrupt_msg.clone();
                self.handle_slash_command();
            } else {
                // Add interrupt message
                let user_message = interrupt_msg.clone();
                self.messages.push(user_message.clone());
                self.message_types.push(MessageType::User);
                self.message_states.push(MessageState::Sent);
                self.save_to_history(&user_message);

                // Show thinking animation immediately
                self.messages
                    .push(UiMessageEvent::ThinkingAnimation.to_message());
                self.message_types.push(MessageType::Agent);
                self.is_thinking = true;
                self.thinking_indicator_active = true;
                self.thinking_start_time = Some(Instant::now());
                self.thinking_token_count = 0;

                // Clear raw thinking content for new conversation turn
                self.thinking_raw_content.clear();

                // Send to agent
                if let Some(tx) = &self.agent_tx {
                    self.agent_state.agent_processing = true;
                    let _ = tx.send(AgentMessage::UserInput(user_message));
                }
            }
        }

        // Save pending todos if any (after rx borrow is dropped)
        if let Some(todos) = pending_todos {
            if let Err(_e) = self.save_todos(&todos) {
                // eprintln!("[ERROR] Failed to save todos: {}", e);
            }
        }

        // Track file change after rx borrow is dropped
        if let Some((tool_name, args, result)) = pending_file_change {
            self.track_file_change(&tool_name, &args, &result);
        }

        // Create rewind point after rx borrow is dropped
        if create_rewind {
            self.create_rewind_point();
        }

        // Handle mid-stream auto-summarize trigger (from streaming checks)
        if trigger_mid_stream_auto_summarize {
            self.trigger_mid_stream_auto_summarize();
        }

        // Check for auto-summarization after rx borrow is dropped
        if check_auto_summarize && self.queued_messages.is_empty() {
            if let Some(percent_left) = self.get_context_percent_left() {
                let percent_used = 100.0 - percent_left;
                if percent_used >= self.auto_summarize_threshold {
                    // Trigger auto-summarization
                    self.compaction_resume_prompt = None;
                    self.compaction_resume_ready = false;
                    self.is_auto_summarize = true;
                    self.compact_pending = Some(CompactOptions {
                        custom_instructions: Some(
                            "This is an automatic summarization triggered because context is running low. \
                             Preserve all important context for continuing the conversation."
                                .to_string(),
                        ),
                    });
                }
            }
        }

        // Check for context sync timeout
        if self.context_sync_pending {
            if let Some(started) = self.context_sync_started {
                if started.elapsed() > std::time::Duration::from_secs(5) {
                    // Timeout - proceed with warning
                    self.messages
                        .push(" ⎿ Warning: Context sync timed out".to_string());
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                    self.context_sync_pending = false;
                    self.context_sync_started = None;
                    self.context_inject_expected = false;
                    schedule_resume_prompt = true;
                }
            }
        } else {
            self.maybe_send_compaction_resume_prompt();
        }

        if schedule_resume_prompt {
            self.maybe_send_compaction_resume_prompt();
        }

        // Process queued message after rx borrow is dropped
        // Block queue processing while context sync is pending
        if process_queued && !self.context_sync_pending {
            // Check if user is editing the next message to send (index 0)
            let is_editing_next_message = self.editing_queue_index == Some(0);

            // Only process if NOT editing the next message
            if !is_editing_next_message && !self.queued_messages.is_empty() {
                let queued_msg = self.queued_messages.remove(0);

                // Check if queued message is a command
                if queued_msg.trim().starts_with('/') {
                    // Execute command
                    self.input = queued_msg;
                    self.handle_slash_command();
                } else {
                    // Preserve context tokens from previous turn before clearing stats
                    if let Some(stats) = &self.generation_stats {
                        self.last_known_context_tokens =
                            stats.prompt_tokens.saturating_add(stats.completion_tokens);
                    }
                    // Regular message - clear generation stats from previous message when new message is added to UI
                    self.clear_generation_stats();
                    // Reset streaming tokens for new turn
                    self.streaming_completion_tokens = 0;

                    self.messages.push(queued_msg.clone());
                    self.message_types.push(MessageType::User);
                    self.message_states.push(MessageState::Queued);
                    // Don't save_to_history here - already saved when queued

                    // Show thinking animation immediately
                    self.messages
                        .push(UiMessageEvent::ThinkingAnimation.to_message());
                    self.message_types.push(MessageType::Agent);
                    self.is_thinking = true;
                    self.thinking_indicator_active = true;
                    self.thinking_start_time = Some(Instant::now());
                    self.thinking_token_count = 0;

                    // Clear raw thinking content for new conversation turn
                    self.thinking_raw_content.clear();

                    if let Some(tx) = &self.agent_tx {
                        self.agent_state.agent_processing = true;
                        let _ = tx.send(AgentMessage::UserInput(queued_msg));
                    }
                }
            }
            // If editing next message, agent will wait until user submits or cancels
        }

        // Handle pending export
        if self.export_pending {
            self.export_pending = false;
            self.handle_pending_export().await;
        }

        // Handle pending code review
        if let Some(options) = self.review_pending.take() {
            // Execute git commands based on options
            let git_context = git_ops::build_review_git_context(&options).await;

            match git_context {
                Ok(context) => {
                    // Build the review prompt
                    let prompt = self.build_review_prompt(&options, &context);

                    // Add prompt as user message
                    self.messages.push("/review".to_string());
                    self.message_types.push(MessageType::User);
                    self.message_states.push(MessageState::Sent);

                    // Show thinking animation
                    self.messages
                        .push(UiMessageEvent::ThinkingAnimation.to_message());
                    self.message_types.push(MessageType::Agent);
                    self.is_thinking = true;
                    self.thinking_indicator_active = true;
                    self.thinking_start_time = Some(Instant::now());
                    self.thinking_token_count = 0;
                    self.thinking_raw_content.clear();

                    // Send to agent
                    if let Some(tx) = &self.agent_tx {
                        self.agent_state.agent_processing = true;
                        let _ = tx.send(AgentMessage::UserInput(prompt));
                    }
                }
                Err(e) => {
                    self.messages.push(format!(" ⎿ Review failed: {}", e));
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                }
            }
        }

        // Handle pending spec command
        if let Some(command) = self.spec_pending.take() {
            self.handle_spec_command(&command).await;
        }

        // Handle pending orchestration from tool call
        if let Some(goal) = self.orchestration_pending.take() {
            match self.load_spec(&goal) {
                Ok(()) => {
                    if let Some(ref spec) = self.current_spec {
                        self.messages.push(format!(
                            " ⎿ Started orchestration: {} ({} steps)",
                            spec.title,
                            spec.steps.len()
                        ));
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                        self.message_metadata.push(None);
                        self.message_timestamps.push(SystemTime::now());
                    }
                }
                Err(e) => {
                    self.messages
                        .push(format!(" ⎿ Orchestration failed: {}", e));
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                    self.message_metadata.push(None);
                    self.message_timestamps.push(SystemTime::now());
                }
            }
        }

        // Handle pending compaction
        if let Some(options) = self.compact_pending.take() {
            // Build compaction prompt with all conversation context
            let prompt = self.build_compact_prompt(&options);

            // Show thinking animation (command already recorded as a user message)
            self.messages
                .push(UiMessageEvent::ThinkingAnimation.to_message());
            self.message_types.push(MessageType::Agent);
            self.is_thinking = true;
            self.thinking_indicator_active = true;
            self.thinking_start_time = Some(Instant::now());
            self.thinking_token_count = 0;
            self.thinking_raw_content.clear();

            // Set compaction flag
            self.agent_state.is_compacting = true;

            // Send to agent
            if let Some(tx) = &self.agent_tx {
                self.agent_state.agent_processing = true;
                self.agent_state.agent_interrupted = false;
                let _ = tx.send(AgentMessage::UserInput(prompt));
            }
        }

        // Handle resume load pending
        if self.resume_load_pending {
            self.resume_load_pending = false;

            if self.resume_selected < self.resume_conversations.len() {
                // Auto-save current conversation before loading a new one
                if self.persistence_state.current_conversation_id.is_some() && !self.messages.is_empty()
                {
                    if let Err(e) = self.save_conversation().await {
                        self.messages.push(format!(
                            " ⎿ Warning: Failed to auto-save before resume: {}",
                            e
                        ));
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                    }
                }

                let metadata = self.resume_conversations[self.resume_selected].clone();
                let is_fork = self.is_fork_mode; // Capture before load

                match self.load_conversation(&metadata).await {
                    Ok(_) => {
                        // If fork mode, reset conversation ID (next save will create new file)
                        if is_fork {
                            self.persistence_state.current_conversation_id = None;
                            self.persistence_state.current_conversation_path = None;
                        }
                        // Close resume panel
                        self.ui_state.show_resume = false;
                    }
                    Err(e) => {
                        self.messages
                            .push(format!(" ⎿ Error loading conversation: {}", e));
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                    }
                }
            }
        }

        // Handle save pending (auto-save on /clear or /exit)
        if self.persistence_state.save_pending {
            self.persistence_state.save_pending = false;
            if let Err(_e) = self.save_conversation().await {
                // eprintln!("[ERROR] Failed to save conversation: {}", e);
            }
        }
    }

    async fn handle_pending_export(&mut self) {
        if let Some(agent) = &self.agent {
            if let Some(json_string) = agent.export_conversation().await {
                // Try to copy to clipboard
                let clipboard_result: Result<(), Box<dyn std::error::Error>> =
                    ClipboardContext::new().and_then(|mut ctx| ctx.set_contents(json_string));

                if clipboard_result.is_ok() {
                    self.messages.push(
                        UiMessageEvent::Command("Conversation exported to clipboard".to_string())
                            .to_message(),
                    );
                } else {
                    self.messages.push(
                        UiMessageEvent::Command("Failed to copy to clipboard".to_string())
                            .to_message(),
                    );
                }
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
                return;
            }
        }

        self.messages.push(
            UiMessageEvent::Command("No conversation history available".to_string()).to_message(),
        );
        self.message_types.push(MessageType::Agent);
        self.message_states.push(MessageState::Sent);
    }
}
