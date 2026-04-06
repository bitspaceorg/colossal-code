use crate::app::{App, SLASH_COMMANDS};
use ratatui::crossterm::event::{Event, KeyEvent};

fn cursor_position(input: &str, character_index: usize) -> (usize, usize) {
    let mut char_count = 0;
    for (row, line) in input.lines().enumerate() {
        let line_len = line.chars().count() + 1;
        if char_count + line_len > character_index {
            return (row, character_index.saturating_sub(char_count));
        }
        char_count += line_len;
    }
    (input.lines().count().saturating_sub(1), 0)
}

fn should_show_autocomplete(input: &str) -> bool {
    let input_trimmed = input.trim_start();
    if !input_trimmed.starts_with('/') {
        return false;
    }

    let prefix = input.chars().take_while(|&c| c != '/').collect::<String>();
    prefix.is_empty() || prefix.chars().all(|c| c.is_whitespace())
}

fn build_autocomplete_suggestions(input: &str) -> Vec<(String, String)> {
    let after_slash = input.trim_start().trim_start_matches('/');
    SLASH_COMMANDS
        .iter()
        .filter(|(cmd, _)| cmd.trim_start_matches('/').starts_with(after_slash))
        .map(|(cmd, desc)| (cmd.to_string(), desc.to_string()))
        .collect()
}

impl App {
    pub(crate) fn get_cursor_row(&self) -> usize {
        cursor_position(&self.input, self.character_index).0
    }

    pub(crate) fn get_cursor_col(&self) -> usize {
        cursor_position(&self.input, self.character_index).1
    }

    pub(crate) fn is_at_start_of_first_line(&self) -> bool {
        self.get_cursor_row() == 0 && self.get_cursor_col() == 0
    }

    pub(crate) fn is_at_end_of_last_line(&self) -> bool {
        let lines: Vec<&str> = self.input.lines().collect();
        let last_line_idx = lines.len().saturating_sub(1);

        if self.get_cursor_row() != last_line_idx {
            return false;
        }

        if let Some(last_line) = lines.last() {
            self.get_cursor_col() >= last_line.chars().count()
        } else {
            true
        }
    }

    pub(crate) fn navigate_history_backwards(&mut self) {
        let total_items = self.command_history.len() + self.queued_messages.len();
        if total_items == 0 {
            return;
        }

        if self.history_index.is_none() {
            self.temp_input = Some(self.input.clone());
            self.history_index = Some(total_items - 1);
        } else if let Some(idx) = self.history_index {
            if idx > 0 {
                self.history_index = Some(idx - 1);
            } else {
                return;
            }
        }

        if let Some(idx) = self.history_index {
            let history_len = self.command_history.len();
            if idx < history_len {
                if let Some(cmd) = self.command_history.get(idx) {
                    self.input = cmd.clone();
                    self.character_index = 0;
                    self.editing_queue_index = None;
                }
            } else {
                let queue_idx = idx - history_len;
                if let Some(queued_msg) = self.queued_messages.get(queue_idx) {
                    self.input = queued_msg.clone();
                    self.character_index = 0;
                    self.editing_queue_index = Some(queue_idx);
                }
            }
        }

        if self.vim_mode_enabled {
            self.sync_input_to_vim();
        }
    }

    pub(crate) fn navigate_history_forwards(&mut self) {
        if let Some(idx) = self.history_index {
            let total_items = self.command_history.len() + self.queued_messages.len();
            if idx < total_items - 1 {
                let new_idx = idx + 1;
                self.history_index = Some(new_idx);
                let history_len = self.command_history.len();
                if new_idx < history_len {
                    if let Some(cmd) = self.command_history.get(new_idx) {
                        self.input = cmd.clone();
                        self.character_index = 0;
                        self.editing_queue_index = None;
                    }
                } else {
                    let queue_idx = new_idx - history_len;
                    if let Some(queued_msg) = self.queued_messages.get(queue_idx) {
                        self.input = queued_msg.clone();
                        self.character_index = 0;
                        self.editing_queue_index = Some(queue_idx);
                    }
                }
            } else {
                self.history_index = None;
                self.editing_queue_index = None;
                if let Some(temp) = self.temp_input.take() {
                    self.input = temp;
                    self.character_index = self.input.chars().count();
                } else {
                    self.input.clear();
                    self.character_index = 0;
                }
            }
        }

        if self.vim_mode_enabled {
            self.sync_input_to_vim();
        }
    }

    pub(crate) fn move_cursor_left(&mut self) {
        let cursor_moved_left = self.character_index.saturating_sub(1);
        self.character_index = self.clamp_cursor(cursor_moved_left);
    }

    pub(crate) fn move_cursor_right(&mut self) {
        let cursor_moved_right = self.character_index.saturating_add(1);
        self.character_index = self.clamp_cursor(cursor_moved_right);
    }

    pub(crate) fn enter_char(&mut self, new_char: char) {
        let index = self.byte_index();
        self.input.insert(index, new_char);
        self.move_cursor_right();
        self.input_modified = true;
        self.update_autocomplete();
    }

    pub(crate) fn byte_index(&self) -> usize {
        self.input
            .char_indices()
            .map(|(i, _)| i)
            .nth(self.character_index)
            .unwrap_or(self.input.len())
    }

    pub(crate) fn delete_char(&mut self) {
        if self.character_index != 0 {
            let current_index = self.character_index;
            let from_left_to_current_index = current_index - 1;
            let before_char_to_delete = self.input.chars().take(from_left_to_current_index);
            let after_char_to_delete = self.input.chars().skip(current_index);
            self.input = before_char_to_delete.chain(after_char_to_delete).collect();
            self.move_cursor_left();
        }
        if self.input.is_empty() {
            self.input_modified = false;
        }
        self.update_autocomplete();
    }

    pub(crate) fn update_autocomplete(&mut self) {
        if should_show_autocomplete(&self.input) {
            self.autocomplete_suggestions = build_autocomplete_suggestions(&self.input);
            self.autocomplete_active = !self.autocomplete_suggestions.is_empty();
            if self.autocomplete_active {
                self.autocomplete_selected_index = 0;
            }
            return;
        }

        self.autocomplete_active = false;
        self.autocomplete_suggestions.clear();
        self.autocomplete_selected_index = 0;
    }

    pub(crate) fn clear_autocomplete(&mut self) {
        self.autocomplete_active = false;
        self.autocomplete_suggestions.clear();
        self.autocomplete_selected_index = 0;
    }

    pub(crate) fn apply_autocomplete_selection(&mut self) -> bool {
        if let Some((cmd, _desc)) = self
            .autocomplete_suggestions
            .get(self.autocomplete_selected_index)
        {
            self.input = cmd.clone();
            self.character_index = self.input.chars().count();
            self.autocomplete_active = false;
            self.autocomplete_suggestions.clear();
            self.autocomplete_selected_index = 0;
            return true;
        }
        false
    }

    pub(crate) fn move_autocomplete_selection_up(&mut self) {
        if self.autocomplete_suggestions.is_empty() {
            return;
        }
        if self.autocomplete_selected_index == 0 {
            self.autocomplete_selected_index = self.autocomplete_suggestions.len() - 1;
        } else {
            self.autocomplete_selected_index -= 1;
        }
    }

    pub(crate) fn move_autocomplete_selection_down(&mut self) {
        if self.autocomplete_suggestions.is_empty() {
            return;
        }
        if self.autocomplete_selected_index >= self.autocomplete_suggestions.len() - 1 {
            self.autocomplete_selected_index = 0;
        } else {
            self.autocomplete_selected_index += 1;
        }
    }

    pub(crate) fn handle_input_up_key(&mut self) {
        if self.autocomplete_active && !self.autocomplete_suggestions.is_empty() {
            self.move_autocomplete_selection_up();
        } else if self.vim_mode_enabled {
            self.navigate_history_backwards();
        } else if self.is_at_start_of_first_line() {
            self.navigate_history_backwards();
        } else {
            self.character_index = 0;
        }
    }

    pub(crate) fn handle_input_down_key(&mut self) {
        if self.autocomplete_active && !self.autocomplete_suggestions.is_empty() {
            self.move_autocomplete_selection_down();
            return;
        }

        let lines: Vec<&str> = self.input.lines().collect();
        let cursor_row = self.get_cursor_row();
        let last_line_idx = lines.len().saturating_sub(1);

        if cursor_row < last_line_idx {
            let mut char_count = 0;
            for (row, line) in lines.iter().enumerate() {
                if row == cursor_row + 1 {
                    self.character_index = char_count + line.chars().count();
                    break;
                }
                char_count += line.chars().count() + 1;
            }

            if self.vim_mode_enabled {
                self.sync_input_to_vim();
            }
        } else if self.is_at_end_of_last_line() {
            if self.history_index.is_some() {
                self.navigate_history_forwards();
            }
        } else {
            self.character_index = self.input.chars().count();
            if self.vim_mode_enabled {
                self.sync_input_to_vim();
            }
        }
    }

    pub(crate) fn handle_input_char_key(&mut self, key: KeyEvent, to_insert: char) {
        if self.vim_mode_enabled {
            self.vim_input_editor.handle_event(Event::Key(key));
            self.sync_vim_input();
            return;
        }
        self.enter_char(to_insert);
    }

    pub(crate) fn handle_input_backspace_key(&mut self, key: KeyEvent) {
        if self.vim_mode_enabled {
            self.vim_input_editor.handle_event(Event::Key(key));
            self.sync_vim_input();
            return;
        }
        self.delete_char();
    }

    pub(crate) fn clamp_cursor(&self, new_cursor_pos: usize) -> usize {
        new_cursor_pos.clamp(0, self.input.chars().count())
    }

    pub(crate) fn reset_cursor(&mut self) {
        self.character_index = 0;
    }

    pub(crate) fn sync_vim_input(&mut self) {
        self.input = self.vim_input_editor.get_text_content();
        let cursor = self.vim_input_editor.state.cursor;
        let lines: Vec<&str> = self.input.lines().collect();
        let mut char_index = 0;
        for (row_idx, line) in lines.iter().enumerate() {
            if row_idx < cursor.row {
                char_index += line.len() + 1;
            } else if row_idx == cursor.row {
                char_index += cursor.col.min(line.len());
                break;
            }
        }
        self.character_index = char_index.min(self.input.len());
    }

    pub(crate) fn sync_input_to_vim(&mut self) {
        self.vim_input_editor
            .set_text_content_preserving_mode(&self.input);

        let char_idx = self.character_index;
        let lines: Vec<&str> = self.input.lines().collect();
        let mut remaining = char_idx;
        let mut row = 0;
        let mut col = 0;

        for (row_idx, line) in lines.iter().enumerate() {
            let line_len = line.len();
            if remaining <= line_len {
                row = row_idx;
                col = remaining;
                break;
            }
            remaining = remaining.saturating_sub(line_len + 1);
            row = row_idx + 1;
        }

        self.vim_input_editor.state.cursor.row = row;
        self.vim_input_editor.state.cursor.col = col;
    }
}

#[cfg(test)]
mod tests {
    use super::{build_autocomplete_suggestions, cursor_position, should_show_autocomplete};

    #[test]
    fn cursor_position_tracks_row_and_column() {
        assert_eq!(cursor_position("hello\nworld", 0), (0, 0));
        assert_eq!(cursor_position("hello\nworld", 3), (0, 3));
        assert_eq!(cursor_position("hello\nworld", 8), (1, 2));
    }

    #[test]
    fn autocomplete_only_shows_for_slash_commands() {
        assert!(should_show_autocomplete("/he"));
        assert!(should_show_autocomplete("   /he"));
        assert!(!should_show_autocomplete("@/he"));
        assert!(!should_show_autocomplete("hello /he"));
    }

    #[test]
    fn autocomplete_filters_matching_commands() {
        let suggestions = build_autocomplete_suggestions("/sp");
        assert!(suggestions.iter().any(|(cmd, _)| cmd == "/spec"));
        assert!(suggestions.iter().any(|(cmd, _)| cmd == "/spec split"));
        assert!(suggestions.iter().all(|(cmd, _)| cmd.starts_with("/sp")));
    }
}
