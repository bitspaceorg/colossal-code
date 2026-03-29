use color_eyre::Result;
use ratatui::crossterm::event::{Event, KeyEvent, KeyEventKind, MouseEvent, MouseEventKind};
use std::time::{Duration, Instant};

use crate::app::runtime::event_loop_handlers;
use crate::app::{App, Mode, Phase};

impl App {
    pub(crate) fn handle_runtime_event(&mut self, runtime_event: Event) -> Result<()> {
        match runtime_event {
            Event::Paste(data)
                if self.phase == Phase::Input
                    && self.mode == Mode::Normal
                    && !self.show_background_tasks
                    && !self.ui_state.show_help
                    && self.viewing_task.is_none() =>
            {
                event_loop_handlers::handle_runtime_paste(self, data);
            }
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.handle_runtime_key(key);
            }
            Event::Mouse(mouse) => self.handle_runtime_mouse(mouse),
            _ => {}
        }

        Ok(())
    }

    fn handle_runtime_key(&mut self, key: KeyEvent) {
        if std::env::var("NITE_DEBUG_KEYS").ok().as_deref() == Some("1") {
            eprintln!(
                "[NITE KEY] code={:?} modifiers={:?} kind={:?} state={:?}",
                key.code, key.modifiers, key.kind, key.state
            );
        }

        match self.mode {
            Mode::Normal => event_loop_handlers::handle_runtime_key_normal(self, key),
            Mode::Navigation | Mode::Visual | Mode::Search => {
                event_loop_handlers::handle_runtime_key_navigation_visual_search(self, key)
            }
            Mode::Command => event_loop_handlers::handle_runtime_key_command(self, key),
            Mode::SessionWindow => {
                event_loop_handlers::handle_runtime_key_session_window(self, key)
            }
        }
    }

    fn handle_runtime_mouse(&mut self, mouse: MouseEvent) {
        if !self.scroll_messages_enabled || self.phase != Phase::Input {
            return;
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_messages_with_mouse(mouse.column, mouse.row, -4)
            }
            MouseEventKind::ScrollDown => {
                self.scroll_messages_with_mouse(mouse.column, mouse.row, 4)
            }
            _ => {}
        }
    }

    fn scroll_messages_with_mouse(&mut self, column: u16, row: u16, delta: isize) {
        if self.mode != Mode::Normal
            || self.show_background_tasks
            || self.viewing_task.is_some()
            || self.ui_state.show_help
            || self.ui_state.show_resume
            || self.show_rewind
            || self.show_todos
            || self.show_model_selection
            || self.should_render_spec_tree(None)
            || !Self::rect_contains_point(self.last_messages_area, column, row)
        {
            return;
        }

        let visible_lines = self.last_messages_area.height as usize;
        if visible_lines == 0 {
            return;
        }

        let total_lines = self.last_message_total_lines;
        let max_scroll = total_lines.saturating_sub(visible_lines);
        if max_scroll == 0 {
            self.follow_messages_tail = true;
            self.message_scroll_offset = 0;
            return;
        }

        let current = if self.follow_messages_tail {
            max_scroll
        } else {
            self.message_scroll_offset.min(max_scroll)
        };

        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current.saturating_add(delta as usize).min(max_scroll)
        };

        self.message_scroll_offset = next;
        self.follow_messages_tail = next >= max_scroll;
        self.last_message_scroll_at = Some(Instant::now());
    }

    fn rect_contains_point(rect: ratatui::layout::Rect, column: u16, row: u16) -> bool {
        column >= rect.x && column < rect.right() && row >= rect.y && row < rect.bottom()
    }

    pub(crate) fn should_show_terminal_cursor(&self) -> bool {
        let scrolling_active = self
            .last_message_scroll_at
            .is_some_and(|ts| ts.elapsed() < Duration::from_millis(150));

        !scrolling_active
            && self.phase == Phase::Input
            && matches!(self.mode, Mode::Normal | Mode::SessionWindow)
    }
}

#[cfg(test)]
mod tests {
    use super::App;
    use ratatui::layout::Rect;

    #[test]
    fn rect_contains_point_is_inside_only_for_area_bounds() {
        let rect = Rect::new(10, 5, 20, 4);

        assert!(App::rect_contains_point(rect, 10, 5));
        assert!(App::rect_contains_point(rect, 29, 8));
        assert!(!App::rect_contains_point(rect, 30, 8));
        assert!(!App::rect_contains_point(rect, 29, 9));
        assert!(!App::rect_contains_point(rect, 9, 5));
    }
}
