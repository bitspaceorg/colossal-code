use std::{sync::Arc, time::SystemTime};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::task;

use crate::app::App;
use crate::app::state::message::{MessageState, MessageType};

impl App {
    pub(crate) fn is_shift_tab(key: &KeyEvent) -> bool {
        matches!(key.code, KeyCode::BackTab)
            || (matches!(key.code, KeyCode::Tab) && key.modifiers.contains(KeyModifiers::SHIFT))
    }

    fn is_ctrl_e(key: &KeyEvent) -> bool {
        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }

        matches!(key.code, KeyCode::Char('e') | KeyCode::Char('E'))
            || matches!(key.code, KeyCode::Char(c) if c == '\u{05}')
    }

    pub(crate) fn handle_normal_mode_global_toggles(&mut self, key: &KeyEvent) -> bool {
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

        if Self::is_ctrl_e(key) {
            let mut efforts = self.current_model_supported_effort_levels();

            if efforts.is_empty() {
                self.status_message = Some("Reasoning not supported for current model".to_string());
                return true;
            }

            let supports_none = efforts.iter().any(|effort| effort == "none");
            let cycle = efforts
                .drain(..)
                .map(|effort| if effort == "none" { None } else { Some(effort) })
                .collect::<Vec<_>>();

            let current = std::env::var("NITE_REASONING_EFFORT")
                .ok()
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty());
            let current_index = cycle
                .iter()
                .position(|effort| effort.as_ref() == current.as_ref())
                .or_else(|| (!supports_none && current.is_none()).then_some(cycle.len() - 1))
                .unwrap_or(cycle.len() - 1);
            let next = cycle[(current_index + 1) % cycle.len()].clone();
            let label = next.as_deref().unwrap_or("none").to_string();

            match next {
                Some(ref value) => unsafe {
                    std::env::set_var("NITE_REASONING_EFFORT", &value);
                },
                None => unsafe {
                    std::env::remove_var("NITE_REASONING_EFFORT");
                },
            }

            self.status_message = Some(format!("Effort set to {label}"));
            return true;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            let mut config = agent_core::safety_config::SafetyConfig::load().unwrap_or_default();
            config.toggle_sandbox();
            let _ = config.save();
            self.safety_state.sandbox_enabled = config.sandbox_enabled;

            if let Some(agent_arc) = &self.agent {
                let agent_clone = Arc::clone(agent_arc);
                let config_clone = config.clone();
                task::spawn(async move {
                    let _ = agent_clone.update_safety_config(config_clone).await;
                });
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

    #[test]
    fn shift_tab_detection_accepts_terminal_variants() {
        let backtab_without_shift = KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE);
        let tab_with_shift = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        let plain_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);

        assert!(App::is_shift_tab(&backtab_without_shift));
        assert!(App::is_shift_tab(&tab_with_shift));
        assert!(!App::is_shift_tab(&plain_tab));
    }

    #[test]
    fn ctrl_e_is_not_treated_as_shift_tab() {
        let ctrl_e = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);

        assert!(!App::is_shift_tab(&ctrl_e));
    }

    #[test]
    fn ctrl_e_detection_accepts_control_character() {
        let ctrl_e = KeyEvent::new(KeyCode::Char('\u{05}'), KeyModifiers::CONTROL);

        assert!(App::is_ctrl_e(&ctrl_e));
    }
}
