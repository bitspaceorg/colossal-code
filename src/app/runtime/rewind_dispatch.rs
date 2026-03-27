use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::app::App;
use crate::app::state::message::{MessageState, MessageType};

impl App {
    pub(crate) fn handle_rewind_panel_key(&mut self, key: &KeyEvent) -> bool {
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
}
