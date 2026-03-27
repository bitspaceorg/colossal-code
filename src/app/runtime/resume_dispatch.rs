use std::time::SystemTime;

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::app::App;
use crate::app::state::message::{MessageState, MessageType};

impl App {
    pub(crate) fn handle_resume_panel_key(&mut self, key: &KeyEvent) -> bool {
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
}
