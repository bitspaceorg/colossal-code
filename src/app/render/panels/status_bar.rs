use ratatui::Frame;

use crate::app::App;

impl App {
    pub(crate) fn render_history_panel(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
        crate::app::input::history::render_history_panel(
            frame,
            area,
            &self.orchestrator_history,
            self.history_panel_selected,
        );
    }
}
