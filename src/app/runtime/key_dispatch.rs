use std::{sync::Arc, time::SystemTime};

use agent_core::AgentMessage;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::task;

use crate::app::state::message::{MessageState, MessageType};
use crate::{App, SLASH_COMMANDS};

impl App {
    fn is_shift_tab(key: &KeyEvent) -> bool {
        matches!(key.code, KeyCode::BackTab)
            || (matches!(key.code, KeyCode::Tab) && key.modifiers.contains(KeyModifiers::SHIFT))
    }

    pub(crate) fn handle_panel_dispatch_key(&mut self, key: &KeyEvent) -> bool {
        self.handle_summary_history_panel_key(key)
            || self.handle_help_panel_key(key)
            || self.handle_resume_panel_key(key)
            || self.handle_history_panel_key(key)
            || self.handle_rewind_panel_key(key)
            || self.handle_model_selection_panel_key(key)
            || self.handle_normal_mode_global_toggles(key)
    }

    fn handle_summary_history_panel_key(&mut self, key: &KeyEvent) -> bool {
        if !self.show_summary_history {
            return false;
        }

        let alt_navigation_toggle = key.modifiers.contains(KeyModifiers::ALT)
            && matches!(key.code, KeyCode::Char('n') | KeyCode::Char('w'));

        if alt_navigation_toggle {
            return false;
        }

        match key.code {
            KeyCode::Esc => {
                self.show_summary_history = false;
                self.messages
                    .push(" ⎿ summary history dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
                self.message_metadata.push(None);
                self.message_timestamps.push(SystemTime::now());
            }
            KeyCode::Char('o') | KeyCode::Char('c')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
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
            KeyCode::Esc => {
                self.ui_state.show_help = false;
                self.messages.push(" ⎿ help dialog dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            KeyCode::Tab => {
                self.ui_state.help_tab = self.ui_state.help_tab.next();
                self.help_commands_selected = 0;
            }
            KeyCode::Up if self.ui_state.help_tab == crate::HelpTab::Commands => {
                if self.help_commands_selected > 0 {
                    self.help_commands_selected -= 1;
                }
            }
            KeyCode::Down if self.ui_state.help_tab == crate::HelpTab::Commands => {
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
            KeyCode::Esc => {
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
            KeyCode::Esc => {
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
            KeyCode::Esc => {
                self.show_rewind = false;
                self.messages.push(" ⎿ rewind dialog dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            KeyCode::Up => {
                if self.rewind_selected > 0 {
                    self.rewind_selected -= 1;
                }
            }
            KeyCode::Down => {
                if self.rewind_selected < self.rewind_points.len().saturating_sub(1) {
                    self.rewind_selected += 1;
                }
            }
            KeyCode::Enter => {
                if self.rewind_selected < self.rewind_points.len() {
                    let point = self.rewind_points[self.rewind_selected].clone();

                    self.messages = point.messages;
                    self.message_types = point.message_types;
                    self.message_states = point.message_states;
                    self.message_metadata = point.message_metadata;
                    self.message_timestamps = point.message_timestamps;

                    self.rewind_points.truncate(self.rewind_selected + 1);
                    self.show_rewind = false;

                    self.messages
                        .push(format!(" ⏮ Rewound to: {}", point.preview));
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
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
            KeyCode::Esc => {
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
                    let selected_model = &self.available_models[self.model_selected_index];
                    let selected_filename = selected_model.filename.clone();
                    let selected_display = selected_model.display_name.clone();
                    self.current_model = Some(selected_filename.clone());
                    self.refresh_context_window();
                    self.show_model_selection = false;

                    if let Err(e) = self.save_config() {
                        self.messages
                            .push(format!(" ⚠ Failed to save model to config: {}", e));
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                    }

                    if let Some(ref tx) = self.agent_tx {
                        let _ = tx.send(AgentMessage::ReloadModel(selected_filename.clone()));
                        self.messages
                            .push(format!(" ⟳ Loading model: {}", selected_display));
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                    } else {
                        self.messages
                            .push(format!(" ✔ Model set to: {}", selected_display));
                        self.message_types.push(MessageType::Agent);
                        self.message_states.push(MessageState::Sent);
                    }
                }
            }
            _ => {}
        }

        true
    }

    fn handle_normal_mode_global_toggles(&mut self, key: &KeyEvent) -> bool {
        if Self::is_shift_tab(key) {
            self.safety_state.assistant_mode = self.safety_state.assistant_mode.next();

            if let Some(safety_mode) = self.safety_state.assistant_mode.to_safety_mode() {
                let mut config =
                    agent_core::safety_config::SafetyConfig::load().unwrap_or_default();
                config.set_mode(safety_mode);
                let _ = config.save();

                if let Some(agent_arc) = &self.agent {
                    let agent_clone = Arc::clone(agent_arc);
                    let config_clone = config.clone();
                    task::spawn(async move {
                        let _ = agent_clone.update_safety_config(config_clone).await;
                    });
                }
            }

            return true;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            self.safety_state.sandbox_enabled = !self.safety_state.sandbox_enabled;

            unsafe {
                if self.safety_state.sandbox_enabled {
                    std::env::set_var("SAFE_MODE", "1");
                } else {
                    std::env::remove_var("SAFE_MODE");
                }
            }

            return true;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::Char('h') | KeyCode::Char('H'))
        {
            self.show_history_panel = !self.show_history_panel;
            if self.show_history_panel {
                self.history_panel_selected = self.orchestrator_history.len().saturating_sub(1);
                self.status_message = Some("History panel opened".to_string());
            } else {
                self.status_message = Some("History panel closed".to_string());
            }
            return true;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o') {
            if self.compaction_history.is_empty() {
                self.messages
                    .push(" ⎿ No summary history yet (run /summarize first)".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
                self.message_metadata.push(None);
                self.message_timestamps.push(SystemTime::now());
            } else {
                self.show_summary_history = true;
                self.summary_history_selected = self.compaction_history.len().saturating_sub(1);
            }
            return true;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn shift_tab_detection_accepts_terminal_variants() {
        let backtab_without_shift = KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE);
        let tab_with_shift = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        let plain_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);

        assert!(App::is_shift_tab(&backtab_without_shift));
        assert!(App::is_shift_tab(&tab_with_shift));
        assert!(!App::is_shift_tab(&plain_tab));
    }
}
