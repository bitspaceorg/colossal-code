use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;
use crate::app::connect::{ConnectAuthMethod, ConnectModalMode};
use crate::app::state::message::{MessageState, MessageType};

impl App {
    pub(crate) fn handle_connect_modal_key(&mut self, key: &KeyEvent) -> bool {
        if !self.connect.show_connect_modal {
            return false;
        }

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.close_connect_modal();
            self.push_connect_status(" ⎿ connect dialog dismissed".to_string());
            return true;
        }

        match self.connect.mode {
            ConnectModalMode::Providers => match key.code {
                KeyCode::Esc => {
                    self.close_connect_modal();
                    self.push_connect_status(" ⎿ connect dialog dismissed".to_string());
                }
                KeyCode::Up => {
                    if self.connect.selected_index > 0 {
                        self.connect.selected_index -= 1;
                    }
                }
                KeyCode::Down => {
                    let len = self.filtered_connect_providers().len();
                    if self.connect.selected_index + 1 < len {
                        self.connect.selected_index += 1;
                    }
                }
                KeyCode::Backspace => self.backspace_connect_input(),
                KeyCode::Left => {
                    self.connect.input_cursor = self.connect.input_cursor.saturating_sub(1);
                }
                KeyCode::Right => {
                    self.connect.input_cursor =
                        (self.connect.input_cursor + 1).min(self.connect.filter.chars().count());
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    self.insert_connect_char(c);
                }
                KeyCode::Enter => {
                    let filtered = self.filtered_connect_providers();
                    if let Some(provider) = filtered.get(self.connect.selected_index).cloned() {
                        self.begin_provider_auth_flow(provider);
                    }
                }
                _ => {}
            },
            ConnectModalMode::AuthMethod => match key.code {
                KeyCode::Esc => {
                    self.connect.mode = ConnectModalMode::Providers;
                    self.connect.selected_auth_method = None;
                    self.connect.selected_index = 0;
                }
                KeyCode::Up => {
                    if self.connect.selected_index > 0 {
                        self.connect.selected_index -= 1;
                    }
                }
                KeyCode::Down => {
                    if self.connect.selected_index + 1
                        < self.auth_methods_for_selected_provider().len()
                    {
                        self.connect.selected_index += 1;
                    }
                }
                KeyCode::Enter => {
                    if let Some(method) = self
                        .auth_methods_for_selected_provider()
                        .get(self.connect.selected_index)
                        .copied()
                    {
                        self.connect.selected_auth_method = Some(method);
                        self.connect.selected_index = 0;
                        self.connect.mode = match method {
                            ConnectAuthMethod::ApiKey => ConnectModalMode::ApiKey,
                            ConnectAuthMethod::OpenAiSubscription
                            | ConnectAuthMethod::ClaudeCode => ConnectModalMode::Subscription,
                        };
                    }
                }
                _ => {}
            },
            ConnectModalMode::ApiKey => match key.code {
                KeyCode::Esc => {
                    self.connect.mode = if self.auth_methods_for_selected_provider().len() > 1 {
                        ConnectModalMode::AuthMethod
                    } else {
                        ConnectModalMode::Providers
                    };
                    self.connect.input.clear();
                    self.connect.input_cursor = 0;
                }
                KeyCode::Backspace => self.backspace_connect_input(),
                KeyCode::Left => {
                    self.connect.input_cursor = self.connect.input_cursor.saturating_sub(1);
                }
                KeyCode::Right => {
                    self.connect.input_cursor =
                        (self.connect.input_cursor + 1).min(self.connect.input.chars().count());
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    self.insert_connect_char(c);
                }
                KeyCode::Enter => {
                    let sanitized = self.sanitized_connect_api_key();
                    if !sanitized.is_empty() {
                        self.connect.input = sanitized;
                        self.connect.input_cursor = self.connect.input.chars().count();
                        match self.save_connect_selection() {
                            Ok(connection) => match self.activate_connection(&connection) {
                                Ok(()) => {
                                    self.close_connect_modal();
                                    self.push_connect_status(format!(
                                        " ⎿ connected {} · use /model to select a model",
                                        connection.provider_name
                                    ));
                                }
                                Err(error) => {
                                    self.close_connect_modal();
                                    self.push_connect_status(format!(
                                        " ⎿ connection saved, but activation failed: {}",
                                        error
                                    ));
                                }
                            },
                            Err(error) => {
                                self.push_connect_status(format!(
                                    " ⎿ failed to save connection: {}",
                                    error
                                ));
                            }
                        }
                    }
                }
                _ => {}
            },
            ConnectModalMode::Subscription => match key.code {
                KeyCode::Esc => {
                    self.connect.mode = ConnectModalMode::AuthMethod;
                }
                KeyCode::Enter => {
                    if self.connect.selected_auth_method == Some(ConnectAuthMethod::ClaudeCode) {
                        // Claude Code auth
                        if self.connect.oauth_state.access_token.is_some() {
                            match self.save_connect_selection() {
                                Ok(connection) => match self.activate_connection(&connection) {
                                    Ok(()) => {
                                        self.close_connect_modal();
                                        self.push_connect_status(format!(
                                            " ⎿ connected {} · use /model to select a model",
                                            connection.provider_name
                                        ));
                                    }
                                    Err(error) => {
                                        self.close_connect_modal();
                                        self.push_connect_status(format!(
                                            " ⎿ connection saved, but activation failed: {}",
                                            error
                                        ));
                                    }
                                },
                                Err(error) => {
                                    self.push_connect_status(format!(
                                        " ⎿ failed to save connection: {}",
                                        error
                                    ));
                                }
                            }
                        } else if !self.connect.oauth_state.started {
                            if let Err(error) = self.start_selected_oauth_auth() {
                                self.push_connect_status(format!(
                                    " ⎿ failed to start auth: {}",
                                    error
                                ));
                            }
                        } else {
                            match self.poll_selected_oauth_auth() {
                                Ok(true) => match self.save_connect_selection() {
                                    Ok(connection) => match self.activate_connection(&connection) {
                                        Ok(()) => {
                                            self.close_connect_modal();
                                            self.push_connect_status(format!(
                                                " ⎿ connected {} · use /model to select a model",
                                                connection.provider_name
                                            ));
                                        }
                                        Err(error) => {
                                            self.close_connect_modal();
                                            self.push_connect_status(format!(
                                                " ⎿ connection saved, but activation failed: {}",
                                                error
                                            ));
                                        }
                                    },
                                    Err(error) => {
                                        self.push_connect_status(format!(
                                            " ⎿ failed to save connection: {}",
                                            error
                                        ));
                                    }
                                },
                                Ok(false) => {}
                                Err(error) => {
                                    self.push_connect_status(format!(" ⎿ auth failed: {}", error));
                                }
                            }
                        }
                    } else {
                        // OpenAI subscription auth
                        if self.connect.subscription_state.access_token.is_some()
                            && self.connect.subscription_state.refresh_token.is_some()
                        {
                            match self.save_connect_selection() {
                                Ok(connection) => match self.activate_connection(&connection) {
                                    Ok(()) => {
                                        self.close_connect_modal();
                                        self.push_connect_status(format!(
                                            " ⎿ connected {} · use /model to select a model",
                                            connection.provider_name
                                        ));
                                    }
                                    Err(error) => {
                                        self.close_connect_modal();
                                        self.push_connect_status(format!(
                                            " ⎿ connection saved, but activation failed: {}",
                                            error
                                        ));
                                    }
                                },
                                Err(error) => {
                                    self.push_connect_status(format!(
                                        " ⎿ failed to save connection: {}",
                                        error
                                    ));
                                }
                            }
                        } else if !self.connect.subscription_state.started {
                            if let Err(error) = self.start_openai_subscription_auth() {
                                self.push_connect_status(format!(
                                    " ⎿ failed to start OpenAI subscription auth: {}",
                                    error
                                ));
                            }
                        } else {
                            match self.poll_openai_subscription_auth() {
                                Ok(true) => match self.save_connect_selection() {
                                    Ok(connection) => match self.activate_connection(&connection) {
                                        Ok(()) => {
                                            self.close_connect_modal();
                                            self.push_connect_status(format!(
                                                " ⎿ connected {} · use /model to select a model",
                                                connection.provider_name
                                            ));
                                        }
                                        Err(error) => {
                                            self.close_connect_modal();
                                            self.push_connect_status(format!(
                                                " ⎿ connection saved, but activation failed: {}",
                                                error
                                            ));
                                        }
                                    },
                                    Err(error) => {
                                        self.push_connect_status(format!(
                                            " ⎿ failed to save connection: {}",
                                            error
                                        ));
                                    }
                                },
                                Ok(false) => {}
                                Err(error) => {
                                    self.push_connect_status(format!(
                                        " ⎿ OpenAI subscription auth failed: {}",
                                        error
                                    ));
                                }
                            }
                        }
                    }
                }
                _ => {}
            },
            ConnectModalMode::Models => {
                self.close_connect_modal();
                self.push_connect_status(
                    " ⎿ provider connected · use /model to select a model".to_string(),
                );
            }
        }

        true
    }
    fn push_connect_status(&mut self, message: String) {
        self.messages.push(message);
        self.message_types.push(MessageType::Agent);
        self.message_states.push(MessageState::Sent);
    }
}
