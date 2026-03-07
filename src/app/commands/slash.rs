use agent_core::AgentMessage;

use crate::app::commands::{SlashCommandDispatch, dispatch_slash_command};
use crate::app::orchestrator::control::{
    MessageLog, SpecAgentBridge, SpecCliContext, SpecCliHandler, SpecCommandResult,
};
use crate::app::runtime::r#loop::{apply_command_runtime_route, route_command_runtime};
use crate::{
    App, AssistantMode, HelpTab, MAX_AUTO_SUMMARIZE_THRESHOLD, MIN_AUTO_SUMMARIZE_THRESHOLD,
    MessageState, MessageType, UiMessageEvent,
};

impl App {
    pub(crate) fn handle_slash_command(&mut self) {
        let command = self.input.trim().to_string();

        // Reset streaming tokens for new message (keep generation_stats for context tracking)
        self.streaming_completion_tokens = 0;

        // Add command to messages as user message
        self.messages.push(command.clone());
        self.message_types.push(MessageType::User);
        self.message_states.push(MessageState::Sent);

        // Clear input
        self.input.clear();
        self.reset_cursor();
        self.input_modified = false;
        // Sync clear to vim editor if vim mode is enabled
        if self.vim_mode_enabled {
            self.sync_input_to_vim();
        }

        // Parse and execute command
        let dispatch = dispatch_slash_command(&command);
        let runtime_route = route_command_runtime(&dispatch, self.messages.len());
        if apply_command_runtime_route(self, runtime_route) {
            return;
        }

        match dispatch {
            SlashCommandDispatch::Clear => {
                // Trigger save before clearing
                self.persistence_state.save_pending = true;

                // Clear all messages except the command itself
                let command_msg = self.messages.pop().unwrap();
                let command_type = self.message_types.pop().unwrap();
                let command_state = self.message_states.pop();

                self.messages.clear();
                self.message_types.clear();
                self.message_states.clear();

                // Add back the command
                self.messages.push(command_msg);
                self.message_types.push(command_type);
                if let Some(state) = command_state {
                    self.message_states.push(state);
                }

                // Add confirmation message
                self.messages.push(
                    UiMessageEvent::Command("Conversation history cleared".to_string())
                        .to_message(),
                );
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);

                // Reset generation stats
                self.clear_generation_stats();
                self.streaming_completion_tokens = 0;

                // Clear agent context
                if let Some(tx) = &self.agent_tx {
                    let _ = tx.send(AgentMessage::ClearContext);
                }
            }
            SlashCommandDispatch::Exit => {
                // Add confirmation message
                self.messages
                    .push(UiMessageEvent::Command("Exiting...".to_string()).to_message());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);

                // Trigger save before exit
                self.persistence_state.save_pending = true;

                // Set exit flag
                self.exit = true;
            }
            SlashCommandDispatch::Export => {}
            SlashCommandDispatch::Summarize { .. } => {}
            SlashCommandDispatch::AutoSummarize { command } => {
                self.handle_auto_summarize_threshold_command(&command);
            }
            SlashCommandDispatch::Help => {
                // Open help panel
                self.ui_state.show_help = true;
                self.ui_state.help_tab = HelpTab::General; // Start on general tab
                self.help_commands_selected = 0; // Reset selection
            }
            SlashCommandDispatch::Resume => {
                // Open resume panel and load conversations
                if let Err(e) = self.load_conversations_list() {
                    self.messages
                        .push(format!(" ⎿ Error loading conversations: {}", e));
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                } else {
                    self.ui_state.show_resume = true;
                    self.is_fork_mode = false; // Normal resume
                    self.resume_selected = 0; // Reset selection
                }
            }
            SlashCommandDispatch::Rewind => {
                // Open rewind panel to restore to previous conversation state
                if self.rewind_points.is_empty() {
                    self.messages
                        .push(" ⎿ No rewind points available yet".to_string());
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                } else {
                    self.show_rewind = true;
                    self.rewind_selected = self.rewind_points.len().saturating_sub(1); // Start at most recent
                }
            }
            SlashCommandDispatch::Fork => {
                // Fork (copy) a conversation - same UI but creates new ID
                if let Err(e) = self.load_conversations_list() {
                    self.messages
                        .push(format!(" ⎿ Error loading conversations: {}", e));
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                } else {
                    self.ui_state.show_resume = true; // Use same UI
                    self.is_fork_mode = true; // Fork mode - don't track ID
                    self.resume_selected = 0; // Reset selection
                }
            }
            SlashCommandDispatch::Vim => {
                // Toggle vim mode
                self.vim_mode_enabled = !self.vim_mode_enabled;

                // Sync current input to vim editor when enabling
                if self.vim_mode_enabled {
                    self.sync_input_to_vim();
                }

                let _ = self.save_vim_mode_setting();

                let status = if self.vim_mode_enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                self.messages.push(
                    UiMessageEvent::Command(format!("Vim keybindings {}", status)).to_message(),
                );
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            SlashCommandDispatch::Todos => {
                // Toggle todos panel
                if self.show_todos {
                    // Closing the panel - add dismissal message
                    self.messages.push(" ⎿ todos dialog dismissed".to_string());
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                }
                self.show_todos = !self.show_todos;
            }
            SlashCommandDispatch::Shells => {
                // Toggle background tasks panel
                if self.show_background_tasks {
                    // Closing the panel - add dismissal message
                    self.messages.push(" ⎿ shells dialog dismissed".to_string());
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                }
                self.show_background_tasks = !self.show_background_tasks;
            }
            SlashCommandDispatch::Model => {
                // Open model selection panel
                if let Err(e) = self.load_models() {
                    self.messages
                        .push(format!(" ⎿ Error loading models: {}", e));
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                } else {
                    self.show_model_selection = true;
                    self.model_selected_index = 0;
                }
            }
            SlashCommandDispatch::Safety { args } => {
                if args.is_empty() {
                    // No args - show current status (no UI spam)
                    if let Ok(config) = agent_core::safety_config::SafetyConfig::load() {
                        self.messages
                            .push(format!("[SAFETY] {}", config.status_string()));
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                    }
                } else {
                    // Handle subcommands (silently update, sync with assistant_mode)
                    match args[0].as_str() {
                        "yolo" => {
                            let mut config =
                                agent_core::safety_config::SafetyConfig::load().unwrap_or_default();
                            config.set_mode(agent_core::safety_config::SafetyMode::Yolo);
                            let _ = config.save();
                            self.safety_state.assistant_mode = AssistantMode::Yolo;
                        }
                        "regular" => {
                            let mut config =
                                agent_core::safety_config::SafetyConfig::load().unwrap_or_default();
                            config.set_mode(agent_core::safety_config::SafetyMode::Regular);
                            let _ = config.save();
                            self.safety_state.assistant_mode = AssistantMode::None;
                        }
                        "readonly" | "read-only" => {
                            let mut config =
                                agent_core::safety_config::SafetyConfig::load().unwrap_or_default();
                            config.set_mode(agent_core::safety_config::SafetyMode::ReadOnly);
                            let _ = config.save();
                            self.safety_state.assistant_mode = AssistantMode::ReadOnly;
                        }
                        "permissions" | "perms" => {
                            let mut config =
                                agent_core::safety_config::SafetyConfig::load().unwrap_or_default();
                            config.toggle_ask_permission();
                            let _ = config.save();
                        }
                        "sandbox" => {
                            let mut config =
                                agent_core::safety_config::SafetyConfig::load().unwrap_or_default();
                            config.toggle_sandbox();
                            let _ = config.save();
                            self.safety_state.sandbox_enabled = config.sandbox_enabled;
                        }
                        _ => {}
                    }
                }
            }
            SlashCommandDispatch::Review { .. } => {}
            SlashCommandDispatch::Spec { .. } => {}
            SlashCommandDispatch::Invalid { message } => {
                self.messages.push(format!(" ⎿ {}", message));
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            SlashCommandDispatch::Unknown { command } => {
                // Unknown command
                self.messages.push(
                    UiMessageEvent::Command(format!("Unknown command '{}'", command)).to_message(),
                );
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
        }
    }

    fn handle_auto_summarize_threshold_command(&mut self, command: &str) -> bool {
        let parts: Vec<&str> = command.split_whitespace().collect();

        if parts.len() == 1 {
            let status_text = format!(
                " ⎿ Auto-summarize triggers when {}. Use '/autosummarize [percent-used]' to change it.",
                self.auto_summarize_hint()
            );
            self.messages.push(status_text);
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);
            return true;
        }

        let value_token = parts[1].trim().trim_end_matches('%');
        match value_token.parse::<f32>() {
            Ok(value) => {
                if !(MIN_AUTO_SUMMARIZE_THRESHOLD..=MAX_AUTO_SUMMARIZE_THRESHOLD).contains(&value) {
                    self.messages.push(format!(
                        " ⎿ Enter a value between {:.0}% and {:.0}% (percent of context used).",
                        MIN_AUTO_SUMMARIZE_THRESHOLD, MAX_AUTO_SUMMARIZE_THRESHOLD
                    ));
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                    return true;
                }

                self.auto_summarize_threshold = Self::clamp_auto_summarize_threshold(value);
                if let Err(e) = self.save_config() {
                    self.messages.push(format!(
                        " ⎿ Auto-summarize updated but failed to persist setting: {}",
                        e
                    ));
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                    return true;
                }

                self.messages.push(format!(
                    " ⎿ Auto-summarize now triggers when {}.",
                    self.auto_summarize_hint()
                ));
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
                true
            }
            Err(_) => {
                self.messages.push(
                    " ⎿ Invalid auto-summarize threshold. Provide a numeric percent of context used."
                        .to_string(),
                );
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
                true
            }
        }
    }

    /// Handle /spec commands: /spec, /spec split <index>, /spec status, /spec abort
    pub(crate) async fn handle_spec_command(&mut self, command: &str) {
        let mut handler = SpecCliHandler::new(SpecCliContext {
            current_spec: &mut self.current_spec,
            orchestrator_control: self.orchestrator_control.as_ref(),
            orchestrator_history: &self.orchestrator_history,
            orchestrator_paused: &mut self.orchestrator_paused,
            status_message: &mut self.status_message,
            message_log: MessageLog {
                messages: &mut self.messages,
                types: &mut self.message_types,
                states: &mut self.message_states,
                metadata: &mut self.message_metadata,
                timestamps: &mut self.message_timestamps,
            },
        });

        let agent_ref = self
            .agent
            .as_deref()
            .map(|agent| agent as &(dyn SpecAgentBridge + Send + Sync));

        if let SpecCommandResult::Handled = handler.execute(agent_ref, command).await {
            return;
        }

        let parts: Vec<&str> = command.split_whitespace().collect();

        if parts.len() >= 2 && parts[0].eq_ignore_ascii_case("/spec") {
            // Load a new spec: /spec <path|goal>
            let path_or_goal = parts[1..].join(" ");
            if let Err(e) = self.load_spec(&path_or_goal) {
                self.messages.push(format!("Failed to load spec: {}", e));
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
                self.message_metadata.push(None);
                self.message_timestamps.push(std::time::SystemTime::now());
            }
            // Success case: no message - tool activity will show in compressed view
        } else {
            self.messages.push("[SPEC] Unknown spec command. Available: /spec, /spec split <index>, /spec status, /spec abort, /spec pause, /spec resume, /spec rerun, /spec history".to_string());
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);
            self.message_metadata.push(None);
            self.message_timestamps.push(std::time::SystemTime::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::commands::{SlashCommandDispatch, dispatch_slash_command};
    use crate::app::runtime::r#loop::route_command_runtime;

    #[test]
    fn slash_dispatch_clear_routes_to_none() {
        let dispatch = dispatch_slash_command("/clear");
        let route = route_command_runtime(&dispatch, 5);
        assert!(matches!(
            route,
            crate::app::runtime::r#loop::CommandRuntimeRoute::None
        ));
        assert!(matches!(dispatch, SlashCommandDispatch::Clear));
    }

    #[test]
    fn slash_dispatch_exit_routes_to_none() {
        let dispatch = dispatch_slash_command("/exit");
        let route = route_command_runtime(&dispatch, 5);
        assert!(matches!(
            route,
            crate::app::runtime::r#loop::CommandRuntimeRoute::None
        ));
        assert!(matches!(dispatch, SlashCommandDispatch::Exit));
    }

    #[test]
    fn slash_dispatch_help_routes_to_none() {
        let dispatch = dispatch_slash_command("/help");
        let route = route_command_runtime(&dispatch, 5);
        assert!(matches!(
            route,
            crate::app::runtime::r#loop::CommandRuntimeRoute::None
        ));
        assert!(matches!(dispatch, SlashCommandDispatch::Help));
    }

    #[test]
    fn slash_dispatch_export_routes_to_export() {
        let dispatch = dispatch_slash_command("/export");
        let route = route_command_runtime(&dispatch, 5);
        assert!(matches!(
            route,
            crate::app::runtime::r#loop::CommandRuntimeRoute::Export
        ));
    }

    #[test]
    fn slash_dispatch_summarize_with_messages_routes_to_compact() {
        let dispatch = dispatch_slash_command("/summarize");
        let route = route_command_runtime(&dispatch, 5);
        assert!(matches!(
            route,
            crate::app::runtime::r#loop::CommandRuntimeRoute::Compact(_)
        ));
    }

    #[test]
    fn slash_dispatch_summarize_empty_routes_to_summarize_empty() {
        let dispatch = dispatch_slash_command("/summarize");
        let route = route_command_runtime(&dispatch, 1);
        assert!(matches!(
            route,
            crate::app::runtime::r#loop::CommandRuntimeRoute::SummarizeEmpty
        ));
    }

    #[test]
    fn slash_dispatch_vim_routes_to_none() {
        let dispatch = dispatch_slash_command("/vim");
        let route = route_command_runtime(&dispatch, 5);
        assert!(matches!(
            route,
            crate::app::runtime::r#loop::CommandRuntimeRoute::None
        ));
        assert!(matches!(dispatch, SlashCommandDispatch::Vim));
    }

    #[test]
    fn slash_dispatch_spec_routes_to_spec() {
        let dispatch = dispatch_slash_command("/spec status");
        let route = route_command_runtime(&dispatch, 5);
        match route {
            crate::app::runtime::r#loop::CommandRuntimeRoute::Spec(cmd) => {
                assert_eq!(cmd, "/spec status");
            }
            _ => panic!("expected spec route"),
        }
    }
}
