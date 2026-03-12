use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, Mode};

pub(crate) fn handle_runtime_key_command(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.mode = Mode::Navigation;
            app.command_input.clear();
            app.cached_mode_content = None;
        }
        KeyCode::Enter => {
            if let Ok(line_num) = app.command_input.trim().parse::<usize>()
                && line_num > 0
            {
                let current_col = app.editor.state.cursor.col;
                let target_row = line_num.saturating_sub(1);
                let max_row = app.editor.state.lines.len().saturating_sub(1);
                app.editor.state.cursor.row = target_row.min(max_row);
                let line_len = app
                    .editor
                    .state
                    .lines
                    .len_col(app.editor.state.cursor.row)
                    .unwrap_or(0);
                app.editor.state.cursor.col = current_col.min(line_len.saturating_sub(1).max(0));
            }
            app.mode = Mode::Navigation;
            app.command_input.clear();
            app.cached_mode_content = None;
        }
        KeyCode::Char(c) => {
            app.command_input.push(c);
            app.cached_mode_content = None;
        }
        KeyCode::Backspace => {
            app.command_input.pop();
            app.cached_mode_content = None;
        }
        _ => {}
    }
}
