use edtui::clipboard::ClipboardTrait;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, MessageState, MessageType, Mode};

pub(crate) fn handle_runtime_key_navigation_visual_search(app: &mut App, key: KeyEvent) {
    if app.mode == Mode::Navigation && key.code == KeyCode::Char('q') {
        app.mode = Mode::Normal;
        app.nav_snapshot = None;
        app.message_types.push(MessageType::Agent);
        app.message_states.push(MessageState::Sent);
        return;
    }

    if app.mode == Mode::Navigation
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char('c')
    {
        app.mode = Mode::Normal;
        app.nav_snapshot = None;
        app.message_types.push(MessageType::Agent);
        app.message_states.push(MessageState::Sent);
        return;
    }

    if app.mode == Mode::Navigation && key.code == KeyCode::Char(':') {
        app.mode = Mode::Command;
        app.command_input.clear();
        app.cached_mode_content = None;
        return;
    }

    let old_clipboard_content = app.editor.state.clip.get_text();
    let old_selection = app.editor.state.selection.clone();
    let old_cursor = app.editor.state.cursor;
    app.editor.handle_event(Event::Key(key));

    let new_clipboard_content = app.editor.state.clip.get_text();
    if new_clipboard_content != old_clipboard_content && !new_clipboard_content.is_empty() {
        if let Some(sel) = old_selection {
            app.flash_highlight = Some((sel, std::time::Instant::now()));
        } else {
            let line_selection = edtui::state::selection::Selection::new(
                edtui::Index2::new(old_cursor.row, 0),
                edtui::Index2::new(
                    old_cursor.row,
                    app.editor
                        .state
                        .lines
                        .len_col(old_cursor.row)
                        .unwrap_or(0)
                        .saturating_sub(1),
                ),
            );
            app.flash_highlight = Some((line_selection, std::time::Instant::now()));
        }
    }

    app.mode = match app.editor.get_mode() {
        edtui::EditorMode::Normal => Mode::Navigation,
        edtui::EditorMode::Visual => Mode::Visual,
        edtui::EditorMode::Search => Mode::Search,
        edtui::EditorMode::Insert => Mode::Navigation,
    };
    app.cached_mode_content = None;
}
