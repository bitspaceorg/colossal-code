use agent_core::AgentMessage;
use std::sync::Arc;
use tokio::task;

use crate::app::commands::{SlashCommandDispatch, dispatch_slash_command};
use crate::app::runtime::r#loop::{apply_command_runtime_route, route_command_runtime};
use crate::app::{App, AssistantMode, HelpTab, MessageState, MessageType, UiMessageEvent};

impl App {
    pub(crate) fn handle_slash_command(&mut self, record_message: bool) {
        let command = self.input.trim().to_string();
        let dispatch = dispatch_slash_command(&command);

        self.audit_event(
            "command.dispatched",
            serde_json::json!({ "command": command }),
        );

        // Reset streaming tokens for new message (keep generation_stats for context tracking)
        self.streaming_completion_tokens = 0;

        if record_message
            && !matches!(
                dispatch,
                SlashCommandDispatch::Rewind
                    | SlashCommandDispatch::Undo
                    | SlashCommandDispatch::Redo
            )
        {
            // Add command to messages as user message
            self.messages.push(command.clone());
            self.message_types.push(MessageType::User);
            self.message_states.push(MessageState::Sent);
        }

        // Clear input
        self.input.clear();
        self.reset_cursor();
        self.input_modified = false;
        // Sync clear to vim editor if vim mode is enabled
        if self.vim_mode_enabled {
            self.sync_input_to_vim();
        }

        // Parse and execute command
        let runtime_route = route_command_runtime(&dispatch, self.messages.len());
        if apply_command_runtime_route(self, runtime_route) {
            return;
        }

        match dispatch {
            SlashCommandDispatch::New => {}
            SlashCommandDispatch::Apply => {
                if self.isolated_changes.pending_count == 0 {
                    self.messages
                        .push(" ⎿ No isolated changes are pending".to_string());
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                } else {
                    if let Some(tx) = &self.agent_tx {
                        let _ = tx.send(AgentMessage::RefreshExecutionState);
                    }
                    self.isolated_changes.show_review_panel = true;
                }
            }
            SlashCommandDispatch::Clear => {
                // Trigger save before clearing
                self.persistence_state.save_pending = true;
                self.clear_rewind_state();

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
            SlashCommandDispatch::Connect => {
                self.open_connect_modal();
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
                    self.status_message = Some("No rewind points available yet".to_string());
                } else {
                    self.show_rewind = true;
                    self.rewind_selected = self.rewind_points.len().saturating_sub(1); // Start at most recent
                    self.rewind_restore_scope =
                        crate::app::state::message::RewindRestoreScope::default();
                    self.rewind_focus = crate::app::state::message::RewindFocus::default();
                }
            }
            SlashCommandDispatch::Undo => {
                if !self.undo_rewind_restore() && !self.undo_last_snapshot_step() {
                    self.status_message = Some("Nothing to undo".to_string());
                }
            }
            SlashCommandDispatch::Redo => {
                if !self.redo_rewind_restore() {
                    self.status_message = Some("Nothing to redo".to_string());
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
            SlashCommandDispatch::Reload => {
                // Full terminal clear + redraw on the next frame, dropping
                // any stale cells or cached render state.
                self.pending_terminal_reload = true;
                self.cached_mode_content = None;
                self.messages.push(" ⎿ UI reloaded".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
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
                if let Err(e) = self.refresh_available_models_cache(true) {
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
                    // No args - show current status in the below-input infobar
                    if let Ok(config) = agent_core::safety_config::SafetyConfig::load() {
                        self.status_message = Some(format!("[SAFETY] {}", config.status_string()));
                    }
                } else {
                    let mut config_changed = false;
                    let previous_mode = self.safety_state.assistant_mode;
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
                                agent_clone.apply_safety_config_immediately(config.clone());
                                let reminder = AssistantMode::transition_reminder(
                                    previous_mode,
                                    self.safety_state.assistant_mode,
                                );
                                task::spawn(async move {
                                    let _ = agent_clone.update_safety_config(config).await;
                                    if let Some(reminder) = reminder {
                                        let _ = agent_clone.inject_system_reminder(&reminder).await;
                                    }
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
    use std::sync::{Mutex as StdMutex, OnceLock};
    use std::time::SystemTime;

    use agent_core::FsCheckpointId;

    use crate::app::commands::{SlashCommandDispatch, dispatch_slash_command};
    use crate::app::runtime::r#loop::route_command_runtime;
    use crate::app::{App, MessageState, MessageType, RewindPoint};

    fn test_rewind_point(preview: &str, messages: Vec<String>) -> RewindPoint {
        RewindPoint {
            message_count: messages.len(),
            preview: preview.to_string(),
            message_types: vec![MessageType::User; messages.len()],
            message_states: vec![MessageState::Sent; messages.len()],
            message_metadata: vec![None; messages.len()],
            message_timestamps: vec![SystemTime::UNIX_EPOCH; messages.len()],
            messages,
            timestamp: SystemTime::UNIX_EPOCH,
            file_changes: Vec::new(),
            fs_checkpoint_id: None,
            review_entries: Vec::new(),
        }
    }

    fn env_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| StdMutex::new(()))
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(previous) = &self.previous {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

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

    #[tokio::test]
    async fn clear_clears_rewind_points_and_execution_checkpoint_state() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        app.rewind_points.push(RewindPoint {
            messages: vec!["old convo".to_string()],
            message_types: vec![MessageType::User],
            message_states: vec![MessageState::Sent],
            message_metadata: vec![None],
            message_timestamps: vec![SystemTime::UNIX_EPOCH],
            timestamp: SystemTime::UNIX_EPOCH,
            preview: "old convo".to_string(),
            message_count: 1,
            file_changes: Vec::new(),
            fs_checkpoint_id: Some(FsCheckpointId("checkpoint-1".to_string())),
            review_entries: Vec::new(),
        });
        app.current_execution_checkpoint_id = Some(FsCheckpointId("checkpoint-1".to_string()));
        app.current_file_changes.push(crate::app::FileChange {
            path: "file.txt".to_string(),
            insertions: 1,
            deletions: 0,
        });
        app.input = "/clear".to_string();

        app.handle_slash_command(true);

        assert!(
            app.rewind_points.is_empty(),
            "rewind points should be cleared"
        );
        assert!(
            app.current_execution_checkpoint_id.is_none(),
            "execution checkpoint should be cleared"
        );
        assert!(
            app.current_file_changes.is_empty(),
            "tracked file changes should be cleared"
        );
    }

    #[tokio::test]
    async fn rewind_command_opens_modal_without_recording_command_message() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        app.rewind_points
            .push(test_rewind_point("latest", vec!["hello".to_string()]));
        app.input = "/rewind".to_string();

        app.handle_slash_command(true);

        assert!(app.show_rewind);
        assert!(app.messages.is_empty());
    }

    #[tokio::test]
    async fn undo_then_redo_roundtrips_latest_snapshot() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        let older = test_rewind_point("older", vec!["first".to_string()]);
        let newer = test_rewind_point("newer", vec!["first".to_string(), "second".to_string()]);
        app.rewind_points = vec![older, newer];
        app.messages = vec!["first".to_string(), "second".to_string()];
        app.message_types = vec![MessageType::User, MessageType::User];
        app.message_states = vec![MessageState::Sent, MessageState::Sent];
        app.message_metadata = vec![None, None];
        app.message_timestamps = vec![SystemTime::UNIX_EPOCH, SystemTime::UNIX_EPOCH];

        app.input = "/undo".to_string();
        app.handle_slash_command(true);

        assert_eq!(app.messages, vec!["first".to_string()]);
        assert_eq!(app.rewind_points.len(), 1);

        app.input = "/redo".to_string();
        app.handle_slash_command(true);

        assert_eq!(
            app.messages,
            vec!["first".to_string(), "second".to_string()]
        );
        assert_eq!(app.rewind_points.len(), 2);
    }
}
