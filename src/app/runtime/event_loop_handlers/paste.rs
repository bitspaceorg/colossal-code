use crate::app::App;

pub(crate) fn handle_runtime_paste(app: &mut App, data: String) {
    if app.vim_mode_enabled {
        let current_text = app.vim_input_editor.get_text_content();
        let cursor = app.vim_input_editor.state.cursor;

        let lines: Vec<&str> = current_text.lines().collect();
        let mut byte_pos = 0;
        for (row_idx, line) in lines.iter().enumerate() {
            if row_idx < cursor.row {
                byte_pos += line.len() + 1;
            } else if row_idx == cursor.row {
                byte_pos += cursor.col.min(line.len());
                break;
            }
        }

        let mut new_text = current_text;
        new_text.insert_str(byte_pos, &data);
        app.vim_input_editor
            .set_text_content_preserving_mode(&new_text);

        let new_byte_pos = byte_pos + data.len();
        let lines: Vec<&str> = new_text.lines().collect();
        let mut remaining = new_byte_pos;
        let mut new_row = 0;
        let mut new_col = 0;
        for (row_idx, line) in lines.iter().enumerate() {
            let line_len = line.len();
            if remaining <= line_len {
                new_row = row_idx;
                new_col = remaining;
                break;
            }
            remaining = remaining.saturating_sub(line_len + 1);
            new_row = row_idx + 1;
        }

        app.vim_input_editor.state.cursor.row = new_row;
        app.vim_input_editor.state.cursor.col = new_col;
        app.sync_vim_input();
    } else {
        let index = app.byte_index();
        app.input.insert_str(index, &data);
        app.character_index += data.chars().count();
        app.input_modified = true;
        app.update_autocomplete();
    }
}
