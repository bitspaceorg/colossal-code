use color_eyre::Result;
use ratatui::crossterm::event::{Event, KeyEvent, KeyEventKind};

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
}
