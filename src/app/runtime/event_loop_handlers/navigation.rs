use ratatui::crossterm::event::KeyEvent;

use crate::app::input::vim_context::{ViewportScrollKind, VimKeyContext, VimKeyResult};
use crate::app::input::vim_sync::RichEditor;
use crate::app::{App, MessageState, MessageType, Mode};

// ---------------------------------------------------------------------------
// Nav-mode context – implements VimKeyContext for the read-only message viewer
// ---------------------------------------------------------------------------

/// Wraps the nav-mode editor so the shared processor can drive it.
struct NavModeContext<'a> {
    editor: &'a mut RichEditor,
}

impl VimKeyContext for NavModeContext<'_> {
    fn editor(&mut self) -> &mut RichEditor {
        self.editor
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn exit_on_q(&self) -> bool {
        true
    }

    fn command_on_colon(&self) -> bool {
        true
    }

    fn supports_viewport_scroll(&self) -> bool {
        true
    }

    fn supports_search(&self) -> bool {
        true // Nav mode supports vim `/` search
    }
}

// ---------------------------------------------------------------------------
// Handler – wired from the event loop
// ---------------------------------------------------------------------------

pub(crate) fn handle_runtime_key_navigation_visual_search(app: &mut App, key: KeyEvent) {
    // Set the viewport offset before processing so edtui knows the visible area
    app.editor
        .state
        .set_viewport_offset_y(app.nav_scroll_offset);

    let mut ctx = NavModeContext {
        editor: &mut app.editor,
    };

    let result = app.vim_processor.process_key(&mut ctx, key);

    match result {
        VimKeyResult::ExitRequested => {
            app.mode = Mode::Normal;
            app.nav_snapshot = None;
            app.message_types.push(MessageType::Agent);
            app.message_states.push(MessageState::Sent);
        }

        VimKeyResult::CommandRequested => {
            app.mode = Mode::Command;
            app.command_input.clear();
            app.cached_mode_content = None;
        }

        VimKeyResult::ViewportScroll(kind) => {
            let cursor_row = app.editor.state.cursor.row;
            let visible_rows = app.editor.state.viewport_rows().max(1);
            let last_row = app.editor.state.lines.len().saturating_sub(1);

            let scroll_offset = match kind {
                ViewportScrollKind::Center => cursor_row.saturating_sub(visible_rows / 2),
                ViewportScrollKind::Top => cursor_row,
                ViewportScrollKind::Bottom => {
                    cursor_row.saturating_sub(visible_rows.saturating_sub(1))
                }
            };

            let max_scroll = last_row.saturating_sub(visible_rows.saturating_sub(1));
            app.nav_scroll_offset = scroll_offset.min(max_scroll);
            app.editor
                .state
                .set_viewport_offset_y(app.nav_scroll_offset);
            app.cached_mode_content = None;
        }

        VimKeyResult::ClipboardChanged {
            old_cursor,
            old_selection,
        } => {
            // Flash highlight on yank
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
            // Sync mode after clipboard change
            sync_nav_mode(app);
        }

        VimKeyResult::ModeChanged(_) | VimKeyResult::Handled => {
            sync_nav_mode(app);
        }

        VimKeyResult::Unhandled => {
            // Keys like Enter or Ctrl+C that the processor doesn't handle –
            // in nav mode we just ignore them (Ctrl+C exit is handled by ExitRequested).
        }
    }
}

/// Map edtui editor mode back to app Mode for nav context.
fn sync_nav_mode(app: &mut App) {
    app.mode = match app.editor.get_mode() {
        edtui::EditorMode::Normal => Mode::Navigation,
        edtui::EditorMode::Visual => Mode::Visual,
        edtui::EditorMode::Search => Mode::Search,
        edtui::EditorMode::Insert => Mode::Navigation, // Never stay in Insert for nav
    };
    app.cached_mode_content = None;
}
