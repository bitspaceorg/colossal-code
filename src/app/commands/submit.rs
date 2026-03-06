use agent_core::AgentMessage;
use color_eyre::Result;

use crate::app::render::panels::survey::SurveyQuestion;
use crate::app::state::ui_message_event::UiMessageEvent;
use crate::{persistence, App, MessageState, MessageType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueChoiceAction {
    Queue,
    Interrupt,
    Cancel,
}

pub(crate) fn parse_queue_choice(choice: &str) -> Option<QueueChoiceAction> {
    match choice.trim() {
        "1" => Some(QueueChoiceAction::Queue),
        "2" => Some(QueueChoiceAction::Interrupt),
        "3" => Some(QueueChoiceAction::Cancel),
        _ => None,
    }
}

impl App {
    pub(crate) fn save_to_history(&mut self, command: &str) {
        if command.trim().is_empty() {
            return;
        }

        // Don't add duplicate if it's the same as the last entry
        if let Some(last_command) = self.command_history.last() {
            if last_command == command {
                return; // Skip duplicate
            }
        }

        // Add to in-memory history
        self.command_history.push(command.to_string());

        // Keep only last 1000 commands
        if self.command_history.len() > 1000 {
            self.command_history
                .drain(0..self.command_history.len() - 1000);
        }

        // Write to file - escape newlines and backslashes
        let _ =
            crate::app::persistence::history::save_history(&self.history_file_path, &self.command_history);
    }

    /// Ensure conversation ID exists, generating one if needed
    /// This should be called when the first real message is sent
    pub(crate) fn ensure_conversation_id(&mut self) -> Result<()> {
        if self.persistence_state.current_conversation_id.is_none() {
            // Generate new conversation ID
            let new_id = uuid::Uuid::new_v4().to_string();
            persistence::conversations::ensure_conversation_workspace(&new_id)?;

            // Set conversation ID and path (path will be set later during save)
            self.persistence_state.current_conversation_id = Some(new_id);
        }
        Ok(())
    }

    pub(crate) fn submit_message(&mut self) {
        if !self.input.is_empty() {
            // Check if input is a slash command
            let is_slash_command = self.input.trim().starts_with('/');

            // Check if we're editing a queued message
            if let Some(idx) = self.editing_queue_index.take() {
                // Update the queued message with edited content
                if idx < self.queued_messages.len() {
                    self.queued_messages[idx] = self.input.clone();
                }
                self.input.clear();
                self.reset_cursor();
                self.input_modified = false;
                // Sync clear to vim editor if vim mode is enabled
                if self.vim_mode_enabled {
                    self.sync_input_to_vim();
                }
                return;
            }

            // Check if we're in queue choice mode
            if self.show_queue_choice {
                match parse_queue_choice(&self.input) {
                    Some(QueueChoiceAction::Queue) => {
                        // Queue message - add to queue
                        let user_message = self.queue_choice_input.clone();
                        self.save_to_history(&user_message); // Save to file history
                        self.queued_messages.push(user_message);
                    }
                    Some(QueueChoiceAction::Interrupt) => {
                        // Interrupt & send new message
                        // Send cancel message to agent first
                        if let Some(tx) = &self.agent_tx {
                            let _ = tx.send(AgentMessage::Cancel);
                        }

                        // Store message to send after cancel completes
                        self.agent_state.interrupt_pending =
                            Some(self.queue_choice_input.clone());

                        // Clear UI state immediately
                        if let Some(last_msg) = self.messages.last() {
                            if matches!(
                                UiMessageEvent::parse(last_msg),
                                Some(UiMessageEvent::ThinkingAnimation)
                            ) {
                                self.messages.pop();
                                self.message_types.pop();
                                self.message_states.pop();
                                self.thinking_indicator_active = false;
                            }
                        }

                        self.is_thinking = false;
                        self.thinking_indicator_active = false;
                        self.thinking_start_time = None;
                        self.thinking_token_count = 0;
                        self.thinking_current_summary = None;
                        self.thinking_position = 0;
                        self.thinking_raw_content.clear();
                    }
                    Some(QueueChoiceAction::Cancel) => {
                        // Cancel - discard message
                    }
                    None => {
                        // Invalid choice, keep the popup
                        self.input.clear();
                        self.reset_cursor();
                        self.input_modified = false;
                        return;
                    }
                }
                self.input.clear();
                self.reset_cursor();
                self.input_modified = false;
                self.show_queue_choice = false;
                self.queue_choice_input.clear();
                return;
            }

            // Check if we're in approval prompt mode
            if self.safety_state.show_approval_prompt {
                let choice = self.input.trim();
                match choice {
                    "0" => {
                        // Approve
                        if let Some(tx) = &self.agent_tx {
                            let _ = tx.send(AgentMessage::ApprovalResponse(true));
                        }
                        self.messages.push(" ⎿ Approved".to_string());
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                    }
                    "1" => {
                        // Deny
                        if let Some(tx) = &self.agent_tx {
                            let _ = tx.send(AgentMessage::ApprovalResponse(false));
                        }
                        self.messages.push(" ⎿ Denied".to_string());
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                    }
                    "2" => {
                        // Interrupt - deny and interrupt
                        if let Some(tx) = &self.agent_tx {
                            let _ = tx.send(AgentMessage::ApprovalResponse(false));
                            let _ = tx.send(AgentMessage::Cancel);
                        }
                        self.messages
                            .push(" ⎿ Interrupted. What should Nite do instead?".to_string());
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                        self.ensure_generation_stats_marker();
                    }
                    _ => {
                        // Invalid choice, keep the popup
                        self.input.clear();
                        self.reset_cursor();
                        self.input_modified = false;
                        return;
                    }
                }
                self.input.clear();
                self.reset_cursor();
                self.input_modified = false;
                self.safety_state.show_approval_prompt = false;
                self.safety_state.approval_prompt_content.clear();
                return;
            }

            // Check if we're in sandbox permission prompt mode
            if self.safety_state.show_sandbox_prompt {
                let choice = self.input.trim();
                match choice {
                    "0" => {
                        // Accept - add path to writable roots dynamically
                        let path =
                            std::path::PathBuf::from(&self.safety_state.sandbox_blocked_path);
                        let path_display = self.safety_state.sandbox_blocked_path.clone();

                        // Add the root in an async context
                        tokio::spawn(async move {
                            if let Err(_e) = agent_core::add_writable_root(path).await {
                                // eprintln!("Failed to add writable root: {}", e);
                            }
                        });

                        self.messages
                            .push(format!(" ⎿ Added '{}' to writable roots", path_display));
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                        self.messages.push(
                            " ⎿ The agent can now write to this path. Continuing..."
                                .to_string(),
                        );
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                    }
                    "1" => {
                        // Deny - just close the prompt
                        self.messages.push(" ⎿ Sandbox access denied".to_string());
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                    }
                    "2" => {
                        // Interrupt - let user tell Nite what to do instead
                        if let Some(tx) = &self.agent_tx {
                            let _ = tx.send(AgentMessage::Cancel);
                        }
                        // Agent will be interrupted, user can type their message
                        self.messages
                            .push(" ⎿ Interrupted. What should Nite do instead?".to_string());
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                        self.ensure_generation_stats_marker();
                    }
                    _ => {
                        // Invalid choice, keep the popup
                        self.input.clear();
                        self.reset_cursor();
                        self.input_modified = false;
                        return;
                    }
                }
                self.input.clear();
                self.reset_cursor();
                self.input_modified = false;
                self.safety_state.show_sandbox_prompt = false;
                self.safety_state.sandbox_blocked_path.clear();
                return;
            }

            // Check if main survey is active and input is a valid number choice
            let is_survey_choice = if self.survey.is_active() {
                self.survey.check_number_input(&self.input)
            } else {
                None
            };

            if let Some(is_dismiss) = is_survey_choice {
                // Clear input without adding to messages
                self.input.clear();
                self.reset_cursor();
                self.input_modified = false;

                // Dismiss the survey and show thank you message if not dismiss option
                self.survey.dismiss();
                if !is_dismiss {
                    self.survey.show_thank_you();
                }
            } else if self.agent_state.agent_processing || self.thinking_indicator_active {
                // Agent is currently processing - show queue options popup
                let user_message = self.input.clone();

                // Store message and show queue choice - don't add to messages yet
                self.queue_choice_input = user_message;
                self.show_queue_choice = true;

                self.input.clear();
                self.reset_cursor();
                self.input_modified = false;
                // Sync clear to vim editor if vim mode is enabled
                if self.vim_mode_enabled {
                    self.sync_input_to_vim();
                }
            } else if is_slash_command {
                // Execute command immediately if agent is not processing
                self.handle_slash_command();
            } else {
                // Normal message submission - agent is not processing
                let user_message = self.input.clone();

                // Ensure conversation ID exists (generate if this is the first message)
                if let Err(_e) = self.ensure_conversation_id() {
                    // eprintln!("[ERROR] Failed to generate conversation ID: {}", e);
                }

                // Preserve context tokens from previous turn before clearing stats
                if let Some(stats) = &self.generation_stats {
                    self.last_known_context_tokens =
                        stats.prompt_tokens.saturating_add(stats.completion_tokens);
                }
                // Clear generation stats from previous message when new message is added to UI
                self.clear_generation_stats();
                // Reset streaming tokens for new turn
                self.streaming_completion_tokens = 0;

                self.messages.push(user_message.clone());
                self.message_types.push(MessageType::User);
                self.input.clear();
                self.reset_cursor();
                self.input_modified = false;
                // Sync clear to vim editor if vim mode is enabled
                if self.vim_mode_enabled {
                    self.sync_input_to_vim();
                }

                // Reset agent response tracking for new conversation turn
                self.agent_state.agent_response_started = false;

                // Save to history
                self.save_to_history(&user_message);

                // Show thinking animation immediately
                self.messages
                    .push(UiMessageEvent::ThinkingAnimation.to_message());
                self.message_types.push(MessageType::Agent);
                self.is_thinking = true;
                self.thinking_indicator_active = true;
                self.thinking_start_time = Some(std::time::Instant::now());
                self.thinking_token_count = 0;

                // Clear raw thinking content for new conversation turn
                self.thinking_raw_content.clear();

                // Send message to agent if available - processing happens in background task
                if let Some(tx) = &self.agent_tx {
                    self.agent_state.agent_processing = true;
                    self.agent_state.agent_interrupted = false; // Reset interrupted flag for new message
                    let _ = tx.send(AgentMessage::UserInput(user_message.clone()));
                }

                // Trigger survey check after message is sent
                let question = SurveyQuestion::new(
                    "How is Nite doing this session?".to_string(),
                    true,
                    vec![
                        "Dismiss".to_string(),
                        "Bad".to_string(),
                        "Fine".to_string(),
                        "Good".to_string(),
                    ],
                );
                self.survey.on_message_sent(Some(question));
            }
        }
    }
}
