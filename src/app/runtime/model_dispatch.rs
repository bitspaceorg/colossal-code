use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::app::App;
use crate::app::state::message::{MessageState, MessageType};

impl App {
    pub(crate) fn handle_model_selection_panel_key(&mut self, key: &KeyEvent) -> bool {
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
                    let connection_id = selected_model.connection_id.clone();
                    self.show_model_selection = false;

                    let result = if let Some(connection_id) = connection_id.as_deref() {
                        self.select_connected_model(connection_id, selected_filename.clone())
                            .map(|provider_name| {
                                format!(
                                    " ✔ Model set to: {} via {}",
                                    selected_display, provider_name
                                )
                            })
                    } else {
                        self.activate_local_model(selected_filename.clone())
                            .map(|_| format!(" ✔ Model set to: {}", selected_display))
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
}
