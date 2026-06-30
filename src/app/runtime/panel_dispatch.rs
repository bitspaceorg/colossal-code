use agent_core::AgentMessage;
use std::time::SystemTime;

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::app::commands::QueueChoiceAction;
use crate::app::state::message::{MessageState, MessageType, RewindFocus};
use crate::app::{App, SLASH_COMMANDS, UiFocus};

impl App {
    pub(crate) fn focused_ui(&self) -> UiFocus {
        if self.connect.show_connect_modal {
            UiFocus::ConnectModal
        } else if self.show_queue_choice {
            UiFocus::QueueChoice
        } else if self.safety_state.show_approval_prompt {
            UiFocus::ApprovalPrompt
        } else if self.safety_state.show_sandbox_prompt {
            UiFocus::SandboxPrompt
        } else if self.show_summary_history {
            UiFocus::SummaryHistory
        } else if self.viewing_task.is_some() {
            UiFocus::ShellViewer
        } else if self.show_background_tasks {
            UiFocus::ShellList
        } else if self.ui_state.show_help {
            UiFocus::Help
        } else if self.ui_state.show_resume {
            UiFocus::Resume
        } else if self.show_history_panel {
            UiFocus::History
        } else if self.show_rewind {
            UiFocus::Rewind
        } else if self.isolated_changes.show_review_panel {
            UiFocus::IsolatedReview
        } else if self.show_model_selection {
            UiFocus::ModelSelection
        } else if self.show_todos {
            UiFocus::Todos
        } else {
            UiFocus::Input
        }
    }

    pub(crate) fn handle_panel_dispatch_key(&mut self, key: &KeyEvent) -> bool {
        match self.focused_ui() {
            UiFocus::ConnectModal => self.handle_connect_modal_key(key),
            UiFocus::QueueChoice => self.handle_queue_choice_key(key),
            UiFocus::ApprovalPrompt => self.handle_approval_prompt_key(key),
            UiFocus::SandboxPrompt => self.handle_sandbox_prompt_key(key),
            UiFocus::SummaryHistory => self.handle_summary_history_panel_key(key),
            UiFocus::ShellViewer | UiFocus::ShellList => self.handle_shell_overlay_key(key),
            UiFocus::Help => self.handle_help_panel_key(key),
            UiFocus::Resume => self.handle_resume_panel_key(key),
            UiFocus::History => self.handle_history_panel_key(key),
            UiFocus::Rewind => self.handle_rewind_panel_key(key),
            UiFocus::IsolatedReview => self.handle_isolated_review_panel_key(key),
            UiFocus::ModelSelection => self.handle_model_selection_panel_key(key),
            UiFocus::Todos => self.handle_todos_panel_key(key),
            UiFocus::Input => self.handle_normal_mode_global_toggles(key),
        }
    }

    fn handle_queue_choice_key(&mut self, key: &KeyEvent) -> bool {
        if !self.show_queue_choice {
            return false;
        }

        match key.code {
            KeyCode::Char('1') => self.apply_queue_choice_action(QueueChoiceAction::Queue),
            KeyCode::Char('2') => self.apply_queue_choice_action(QueueChoiceAction::Interrupt),
            KeyCode::Char('3') | KeyCode::Esc if !is_ctrl_c(key) => {
                self.apply_queue_choice_action(QueueChoiceAction::Cancel)
            }
            _ if is_ctrl_c(key) => self.apply_queue_choice_action(QueueChoiceAction::Cancel),
            _ => {}
        }

        true
    }

    fn handle_approval_prompt_key(&mut self, key: &KeyEvent) -> bool {
        if !self.safety_state.show_approval_prompt {
            return false;
        }

        match key.code {
            KeyCode::Char('0') => self.apply_approval_prompt_choice(true, false),
            KeyCode::Char('1') => self.apply_approval_prompt_choice(false, false),
            KeyCode::Char('2') | KeyCode::Esc if !is_ctrl_c(key) => {
                self.apply_approval_prompt_choice(false, true)
            }
            _ if is_ctrl_c(key) => self.apply_approval_prompt_choice(false, true),
            _ => {}
        }

        true
    }

    fn handle_sandbox_prompt_key(&mut self, key: &KeyEvent) -> bool {
        if !self.safety_state.show_sandbox_prompt {
            return false;
        }

        match key.code {
            KeyCode::Char('0') => self.apply_sandbox_prompt_choice(0),
            KeyCode::Char('1') => self.apply_sandbox_prompt_choice(1),
            KeyCode::Char('2') | KeyCode::Esc if !is_ctrl_c(key) => {
                self.apply_sandbox_prompt_choice(2)
            }
            _ if is_ctrl_c(key) => self.apply_sandbox_prompt_choice(2),
            _ => {}
        }

        true
    }

    fn handle_shell_overlay_key(&mut self, key: &KeyEvent) -> bool {
        if self.viewing_task.is_some() {
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ') if !is_ctrl_c(key) => {
                    self.viewing_task = None;
                    self.messages.push(" ⎿ shell viewer dismissed".to_string());
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                }
                _ if is_ctrl_c(key) => {
                    self.viewing_task = None;
                    self.messages.push(" ⎿ shell viewer dismissed".to_string());
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                }
                KeyCode::Char('k') => {
                    if let Some((session_id, _, _, _)) = self.viewing_task.take() {
                        self.background_tasks
                            .retain(|(sid, _, _, _)| sid != &session_id);
                        if self
                            .active_foreground_shell
                            .as_ref()
                            .map(|(sid, _, _)| sid == &session_id)
                            .unwrap_or(false)
                        {
                            self.active_foreground_shell = None;
                        }
                        kill_shell_session_async(session_id);
                    }
                }
                _ => return true,
            }

            return true;
        }

        if !self.show_background_tasks {
            return false;
        }

        match key.code {
            KeyCode::Esc if !is_ctrl_c(key) => {
                self.show_background_tasks = false;
                self.messages.push(" ⎿ shells dialog dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            _ if is_ctrl_c(key) => {
                self.show_background_tasks = false;
                self.messages.push(" ⎿ shells dialog dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            KeyCode::Up => {
                if !self.shell_panel_entries().is_empty() && self.background_tasks_selected > 0 {
                    self.background_tasks_selected -= 1;
                }
            }
            KeyCode::Down => {
                let shell_count = self.shell_panel_entries().len();
                if shell_count > 0 && self.background_tasks_selected < shell_count - 1 {
                    self.background_tasks_selected += 1;
                }
            }
            KeyCode::Char('k') => {
                let shell_entries = self.shell_panel_entries();
                if !shell_entries.is_empty() && self.background_tasks_selected < shell_entries.len()
                {
                    let (session_id, _command, _log_file, _start_time) =
                        shell_entries[self.background_tasks_selected].clone();
                    self.background_tasks
                        .retain(|(sid, _, _, _)| sid != &session_id);
                    if self
                        .active_foreground_shell
                        .as_ref()
                        .map(|(sid, _, _)| sid == &session_id)
                        .unwrap_or(false)
                    {
                        self.active_foreground_shell = None;
                    }
                    let remaining_count = self.shell_panel_entries().len();
                    if self.background_tasks_selected >= remaining_count
                        && self.background_tasks_selected > 0
                    {
                        self.background_tasks_selected -= 1;
                    }
                    kill_shell_session_async(session_id);
                }
            }
            KeyCode::Enter => {
                let shell_entries = self.shell_panel_entries();
                if !shell_entries.is_empty() && self.background_tasks_selected < shell_entries.len()
                {
                    let task = &shell_entries[self.background_tasks_selected];
                    self.viewing_task =
                        Some((task.0.clone(), task.1.clone(), task.2.clone(), task.3));
                    self.show_background_tasks = false;
                }
            }
            _ => return true,
        }

        true
    }

    fn handle_summary_history_panel_key(&mut self, key: &KeyEvent) -> bool {
        if !self.show_summary_history {
            return false;
        }

        let alt_navigation_toggle = key
            .modifiers
            .contains(ratatui::crossterm::event::KeyModifiers::ALT)
            && matches!(key.code, KeyCode::Char('n') | KeyCode::Char('w'));

        if alt_navigation_toggle {
            return false;
        }

        match key.code {
            KeyCode::Esc if !is_ctrl_c(key) => {
                self.show_summary_history = false;
                self.messages
                    .push(" ⎿ summary history dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
                self.message_metadata.push(None);
                self.message_timestamps.push(SystemTime::now());
            }
            _ if is_ctrl_c(key) => {
                self.show_summary_history = false;
                self.messages
                    .push(" ⎿ summary history dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
                self.message_metadata.push(None);
                self.message_timestamps.push(SystemTime::now());
            }
            KeyCode::Char('o') | KeyCode::Char('c')
                if key
                    .modifiers
                    .contains(ratatui::crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.show_summary_history = false;
                self.messages
                    .push(" ⎿ summary history dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
                self.message_metadata.push(None);
                self.message_timestamps.push(SystemTime::now());
            }
            KeyCode::Up => {
                if self.summary_history_selected > 0 {
                    self.summary_history_selected -= 1;
                }
            }
            KeyCode::Down => {
                if self.summary_history_selected < self.compaction_history.len().saturating_sub(1) {
                    self.summary_history_selected += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(entry) = self.compaction_history.get(self.summary_history_selected) {
                    self.messages.push(entry.summary.clone());
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                    self.message_metadata.push(None);
                    self.message_timestamps.push(SystemTime::now());
                }
            }
            _ => {}
        }

        true
    }

    fn handle_help_panel_key(&mut self, key: &KeyEvent) -> bool {
        if !self.ui_state.show_help {
            return false;
        }

        match key.code {
            KeyCode::Esc if !is_ctrl_c(key) => {
                self.ui_state.show_help = false;
                self.messages.push(" ⎿ help dialog dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            _ if is_ctrl_c(key) => {
                self.ui_state.show_help = false;
                self.messages.push(" ⎿ help dialog dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            KeyCode::Tab => {
                self.ui_state.help_tab = self.ui_state.help_tab.next();
                self.help_commands_selected = 0;
            }
            KeyCode::Up if self.ui_state.help_tab == crate::app::HelpTab::Commands => {
                if self.help_commands_selected > 0 {
                    self.help_commands_selected -= 1;
                }
            }
            KeyCode::Down if self.ui_state.help_tab == crate::app::HelpTab::Commands => {
                if self.help_commands_selected < SLASH_COMMANDS.len().saturating_sub(1) {
                    self.help_commands_selected += 1;
                }
            }
            _ => {}
        }

        true
    }

    fn handle_resume_panel_key(&mut self, key: &KeyEvent) -> bool {
        if !self.ui_state.show_resume {
            return false;
        }

        match key.code {
            KeyCode::Esc if !is_ctrl_c(key) => {
                self.ui_state.show_resume = false;
                self.messages.push(" ⎿ resume dialog dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            _ if is_ctrl_c(key) => {
                self.ui_state.show_resume = false;
                self.messages.push(" ⎿ resume dialog dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            KeyCode::Up => {
                if self.resume_selected > 0 {
                    self.resume_selected -= 1;
                }
            }
            KeyCode::Down => {
                if self.resume_selected < self.resume_conversations.len().saturating_sub(1) {
                    self.resume_selected += 1;
                }
            }
            KeyCode::Enter => {
                if self.resume_selected < self.resume_conversations.len() {
                    self.resume_load_pending = true;
                }
            }
            KeyCode::Char('d') => {
                if self.resume_selected < self.resume_conversations.len() {
                    let metadata = self.resume_conversations[self.resume_selected].clone();
                    if let Err(e) = self.delete_conversation(&metadata) {
                        self.messages
                            .push(format!(" ⎿ Error deleting conversation: {}", e));
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                    } else {
                        let _ = self.load_conversations_list();
                        if self.resume_selected >= self.resume_conversations.len()
                            && self.resume_selected > 0
                        {
                            self.resume_selected -= 1;
                        }
                        if self.resume_conversations.is_empty() {
                            self.ui_state.show_resume = false;
                            self.messages.push(" ⎿ conversation deleted".to_string());
                            self.message_types.push(MessageType::Agent);
                            self.message_states.push(MessageState::Sent);
                        }
                    }
                }
            }
            KeyCode::Char('f') => {
                if self.resume_selected < self.resume_conversations.len() {
                    let metadata = self.resume_conversations[self.resume_selected].clone();
                    self.persistence_state.current_forked_from = Some(metadata.id.clone());
                    self.persistence_state.current_forked_at = Some(SystemTime::now());
                    self.is_fork_mode = true;
                    self.resume_load_pending = true;
                }
            }
            _ => {}
        }

        true
    }

    fn handle_history_panel_key(&mut self, key: &KeyEvent) -> bool {
        if !self.show_history_panel {
            return false;
        }

        match key.code {
            KeyCode::Esc if !is_ctrl_c(key) => {
                self.show_history_panel = false;
            }
            _ if is_ctrl_c(key) => {
                self.show_history_panel = false;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.history_panel_selected > 0 {
                    self.history_panel_selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.history_panel_selected + 1 < self.orchestrator_history.len() {
                    self.history_panel_selected += 1;
                }
            }
            _ => return false,
        }

        true
    }

    fn handle_rewind_panel_key(&mut self, key: &KeyEvent) -> bool {
        if !self.show_rewind {
            return false;
        }

        match key.code {
            KeyCode::Esc if !is_ctrl_c(key) => {
                self.show_rewind = false;
                self.messages.push(" ⎿ rewind dialog dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            _ if is_ctrl_c(key) => {
                self.show_rewind = false;
                self.messages.push(" ⎿ rewind dialog dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                self.rewind_focus = match self.rewind_focus {
                    RewindFocus::Points => RewindFocus::Scope,
                    RewindFocus::Scope => RewindFocus::Points,
                };
            }
            KeyCode::Up | KeyCode::Char('k') => match self.rewind_focus {
                RewindFocus::Points => {
                    if self.rewind_selected > 0 {
                        self.rewind_selected -= 1;
                    }
                }
                RewindFocus::Scope => {
                    self.rewind_restore_scope = self.rewind_restore_scope.prev();
                }
            },
            KeyCode::Down | KeyCode::Char('j') => match self.rewind_focus {
                RewindFocus::Points => {
                    if self.rewind_selected < self.rewind_points.len().saturating_sub(1) {
                        self.rewind_selected += 1;
                    }
                }
                RewindFocus::Scope => {
                    self.rewind_restore_scope = self.rewind_restore_scope.next();
                }
            },
            KeyCode::Enter => {
                if self.rewind_selected < self.rewind_points.len() {
                    let point = self.rewind_points[self.rewind_selected].clone();
                    let restore_scope = self.rewind_restore_scope;

                    if restore_scope.restores_conversation() {
                        self.messages = point.messages;
                        self.message_types = point.message_types;
                        self.message_states = point.message_states;
                        self.message_metadata = point.message_metadata;
                        self.message_timestamps = point.message_timestamps;
                    }

                    self.rewind_points.truncate(self.rewind_selected + 1);
                    self.show_rewind = false;

                    self.messages.push(format!(
                        " ⏮ Rewound to: {} ({})",
                        point.preview,
                        restore_scope.label()
                    ));
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                    self.message_metadata.push(None);
                    self.message_timestamps.push(SystemTime::now());

                    if restore_scope.restores_code() {
                        match (&self.agent_tx, point.fs_checkpoint_id.clone()) {
                            (Some(tx), Some(checkpoint_id)) => {
                                let _ = tx
                                    .send(AgentMessage::RestoreExecutionCheckpoint(checkpoint_id));
                            }
                            _ => {
                                self.messages.push(
                                    " ⎿ No filesystem checkpoint was available for that rewind point"
                                        .to_string(),
                                );
                                self.message_types.push(MessageType::Agent);
                                self.message_states.push(MessageState::Sent);
                                self.message_metadata.push(None);
                                self.message_timestamps.push(SystemTime::now());
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        true
    }

    fn handle_isolated_review_panel_key(&mut self, key: &KeyEvent) -> bool {
        if !self.isolated_changes.show_review_panel {
            return false;
        }

        match key.code {
            KeyCode::Esc if !is_ctrl_c(key) => {
                self.isolated_changes.show_review_panel = false;
                self.messages
                    .push(" ⎿ isolated changes review dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            _ if is_ctrl_c(key) => {
                self.isolated_changes.show_review_panel = false;
                self.messages
                    .push(" ⎿ isolated changes review dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.isolated_changes.review_selected > 0 {
                    self.isolated_changes.review_selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.isolated_changes.review_selected + 1
                    < self.isolated_changes.review_entries.len()
                {
                    self.isolated_changes.review_selected += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(tx) = &self.agent_tx {
                    let _ = tx.send(AgentMessage::ApplyExecutionChanges);
                }
            }
            KeyCode::Char('d') => {
                if let Some(tx) = &self.agent_tx {
                    let _ = tx.send(AgentMessage::DiscardExecutionChanges);
                }
            }
            _ => {}
        }

        true
    }

    fn handle_model_selection_panel_key(&mut self, key: &KeyEvent) -> bool {
        if !self.show_model_selection {
            return false;
        }

        match key.code {
            KeyCode::Esc if !is_ctrl_c(key) => {
                self.show_model_selection = false;
                self.messages
                    .push(" ⎿ model selection dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            _ if is_ctrl_c(key) => {
                self.show_model_selection = false;
                self.messages
                    .push(" ⎿ model selection dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            KeyCode::Up => {
                if self.model_selected_index > 0 {
                    self.model_selected_index -= 1;
                }
            }
            KeyCode::Down => {
                if self.model_selected_index < self.available_models.len().saturating_sub(1) {
                    self.model_selected_index += 1;
                }
            }
            KeyCode::Enter => {
                if self.model_selected_index < self.available_models.len() {
                    let selected_model = self.available_models[self.model_selected_index].clone();
                    self.show_model_selection = false;

                    let result =
                        if self.agent_state.agent_processing || self.thinking_indicator_active {
                            self.pending_model_switch = Some(selected_model.clone());
                            Ok(format!(
                                " ⎿ model switch to {} will apply after the current response",
                                selected_model.display_name
                            ))
                        } else {
                            self.activate_model_info(&selected_model)
                        };

                    match result {
                        Ok(message) => {
                            self.messages.push(message);
                            self.message_types.push(MessageType::Agent);
                            self.message_states.push(MessageState::Sent);
                        }
                        Err(e) => {
                            self.messages
                                .push(format!(" ⚠ Failed to switch model: {}", e));
                            self.message_types.push(MessageType::Agent);
                            self.message_states.push(MessageState::Sent);
                        }
                    }
                }
            }
            _ => {}
        }

        true
    }

    fn handle_todos_panel_key(&mut self, key: &KeyEvent) -> bool {
        if !self.show_todos {
            return false;
        }

        if matches!(key.code, KeyCode::Esc) || is_ctrl_c(key) {
            self.show_todos = false;
            self.messages.push(" ⎿ todos dialog dismissed".to_string());
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);
        }

        true
    }
}

fn is_ctrl_c(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('c')
        && key
            .modifiers
            .contains(ratatui::crossterm::event::KeyModifiers::CONTROL)
}

fn kill_shell_session_async(session_id: String) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let _ = agent_core::kill_shell_session(session_id).await;
        });
    });
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex as StdMutex, OnceLock};

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::app::{App, ModelInfo, UiFocus};

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

    #[tokio::test]
    async fn busy_model_selection_defers_switch_until_idle() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        app.agent_state.agent_processing = true;
        app.show_model_selection = true;
        app.available_models = vec![ModelInfo {
            filename: "demo.gguf".to_string(),
            display_name: "Demo Model".to_string(),
            connection_id: None,
            provider_name: None,
            size_mb: 0.0,
            quantization: None,
            architecture: None,
            parameter_count: None,
            file_hash: None,
            author: None,
            version: None,
            context_length: None,
            supported_effort_levels: Vec::new(),
        }];

        assert!(app.handle_model_selection_panel_key(&KeyEvent::from(KeyCode::Enter)));

        assert!(!app.show_model_selection);
        assert_eq!(
            app.pending_model_switch
                .as_ref()
                .map(|model| model.display_name.as_str()),
            Some("Demo Model")
        );
    }

    #[tokio::test]
    async fn focused_ui_prefers_prompts_over_shell_panels() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        app.connect.show_connect_modal = false;
        app.show_background_tasks = true;
        app.safety_state.show_approval_prompt = true;

        assert_eq!(app.focused_ui(), UiFocus::ApprovalPrompt);
    }

    #[tokio::test]
    async fn approval_prompt_keys_do_not_fall_through_to_input() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        app.safety_state.show_approval_prompt = true;

        assert!(app.handle_panel_dispatch_key(&KeyEvent::from(KeyCode::Char('x'))));
        assert!(app.input.is_empty());
    }

    #[tokio::test]
    async fn focused_ui_prefers_shell_viewer_over_shell_list() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        app.connect.show_connect_modal = false;
        app.show_background_tasks = true;
        app.viewing_task = Some((
            "shell-1".to_string(),
            "sleep 10".to_string(),
            "/tmp/shell.log".to_string(),
            std::time::Instant::now(),
        ));

        assert_eq!(app.focused_ui(), UiFocus::ShellViewer);
    }

    #[tokio::test]
    async fn queue_choice_keys_do_not_touch_input_buffer() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        app.connect.show_connect_modal = false;
        app.show_queue_choice = true;
        app.queue_choice_input = "queued message".to_string();

        assert!(app.handle_panel_dispatch_key(&KeyEvent::from(KeyCode::Char('1'))));
        assert!(app.input.is_empty());
        assert_eq!(app.queued_messages, vec!["queued message".to_string()]);
        assert!(!app.show_queue_choice);
    }

    #[tokio::test]
    async fn todos_focus_consumes_escape_without_input_mutation() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        app.connect.show_connect_modal = false;
        app.show_todos = true;
        app.input = "should stay".to_string();

        assert!(app.handle_panel_dispatch_key(&KeyEvent::from(KeyCode::Esc)));
        assert!(!app.show_todos);
        assert_eq!(app.input, "should stay");
    }

    #[tokio::test]
    async fn help_focus_consumes_ctrl_c_before_input_handler() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        app.connect.show_connect_modal = false;
        app.ui_state.show_help = true;
        app.input = "preserve".to_string();

        assert!(
            app.handle_panel_dispatch_key(&KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
            ))
        );
        assert!(!app.ui_state.show_help);
        assert_eq!(app.input, "preserve");
    }

    #[tokio::test]
    async fn queue_choice_ctrl_c_cancels_prompt_without_touching_input() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        app.connect.show_connect_modal = false;
        app.show_queue_choice = true;
        app.queue_choice_input = "queued".to_string();
        app.input = "stay".to_string();

        assert!(
            app.handle_panel_dispatch_key(&KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
            ))
        );
        assert!(!app.show_queue_choice);
        assert!(app.queued_messages.is_empty());
        assert_eq!(app.input, "");
    }

    #[tokio::test]
    async fn focused_ui_prefers_connect_modal_over_other_panels() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        app.show_model_selection = true;
        app.connect.show_connect_modal = true;

        assert_eq!(app.focused_ui(), UiFocus::ConnectModal);
    }

    #[tokio::test]
    async fn focused_ui_prefers_rewind_over_model_selection() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        app.connect.show_connect_modal = false;
        app.show_model_selection = true;
        app.show_rewind = true;

        assert_eq!(app.focused_ui(), UiFocus::Rewind);
    }

    #[tokio::test]
    async fn resume_focus_consumes_escape_without_touching_input() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        app.connect.show_connect_modal = false;
        app.ui_state.show_resume = true;
        app.input = "keep me".to_string();

        assert!(app.handle_panel_dispatch_key(&KeyEvent::from(KeyCode::Esc)));
        assert!(!app.ui_state.show_resume);
        assert_eq!(app.input, "keep me");
    }

    #[tokio::test]
    async fn rewind_focus_consumes_ctrl_c_without_touching_input() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        app.connect.show_connect_modal = false;
        app.show_rewind = true;
        app.input = "keep me".to_string();

        assert!(
            app.handle_panel_dispatch_key(&KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
            ))
        );
        assert!(!app.show_rewind);
        assert_eq!(app.input, "keep me");
    }
}
