use crate::{EditorMode, EditorState, events::key::KeyEventHandler};

#[derive(Clone, Debug)]
pub struct ReadOnlyEventHandler {
    key_handler: KeyEventHandler,
}

impl Default for ReadOnlyEventHandler {
    fn default() -> Self {
        Self {
            key_handler: KeyEventHandler::create_readonly_handler(),
        }
    }
}

impl ReadOnlyEventHandler {
    pub fn on_event(&mut self, event: ratatui::crossterm::event::Event, state: &mut EditorState) {
        // Prevent entering insert mode by immediately switching back
        if state.mode == EditorMode::Insert {
            state.mode = EditorMode::Normal;
            return;
        }

        // Handle the event based on its type
        match event {
            ratatui::crossterm::event::Event::Key(key_event) => {
                self.key_handler.on_event(key_event, state);
            }
            _ => {
                // We ignore non-key events in our read-only handler
            }
        }

        // Ensure we never stay in insert mode
        if state.mode == EditorMode::Insert {
            state.mode = EditorMode::Normal;
        }
    }
}
