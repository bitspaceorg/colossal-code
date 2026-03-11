use agent_core::AgentMessage;
use std::sync::Arc;
use tokio::task;

use crate::app::commands::{SlashCommandDispatch, dispatch_slash_command};
use crate::app::runtime::r#loop::{apply_command_runtime_route, route_command_runtime};
use crate::{
    App, AssistantMode, HelpTab, MessageState, MessageType, UiMessageEvent,
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
                    let mut config_changed = false;
                    // Handle subcommands (silently update, sync with assistant_mode)
                    match args[0].as_str() {
                        "yolo" => {
                            let mut config =
                                agent_core::safety_config::SafetyConfig::load().unwrap_or_default();
                            config.set_mode(agent_core::safety_config::SafetyMode::Yolo);
                            let _ = config.save();
                            self.safety_state.assistant_mode = AssistantMode::Yolo;
                            config_changed = true;
                        }
                        "regular" => {
                            let mut config =
                                agent_core::safety_config::SafetyConfig::load().unwrap_or_default();
                            config.set_mode(agent_core::safety_config::SafetyMode::Regular);
                            let _ = config.save();
                            self.safety_state.assistant_mode = AssistantMode::None;
                            config_changed = true;
                        }
                        "readonly" | "read-only" => {
                            let mut config =
                                agent_core::safety_config::SafetyConfig::load().unwrap_or_default();
                            config.set_mode(agent_core::safety_config::SafetyMode::ReadOnly);
                            let _ = config.save();
                            self.safety_state.assistant_mode = AssistantMode::ReadOnly;
                            config_changed = true;
                        }
                        "permissions" | "perms" => {
                            let mut config =
                                agent_core::safety_config::SafetyConfig::load().unwrap_or_default();
                            config.toggle_ask_permission();
                            let _ = config.save();
                            config_changed = true;
                        }
                        "sandbox" => {
                            let mut config =
                                agent_core::safety_config::SafetyConfig::load().unwrap_or_default();
                            config.toggle_sandbox();
                            let _ = config.save();
                            self.safety_state.sandbox_enabled = config.sandbox_enabled;
                            config_changed = true;
                        }
                        _ => {}
                    }

                    if config_changed {
                        if let Ok(config) = agent_core::safety_config::SafetyConfig::load() {
                            if let Some(agent_arc) = &self.agent {
                                let agent_clone = Arc::clone(agent_arc);
                                task::spawn(async move {
                                    let _ = agent_clone.update_safety_config(config).await;
                                });
                            }
                        }
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
