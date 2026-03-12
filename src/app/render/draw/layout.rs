use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
};

use crate::app::{App, Mode};

impl App {
    pub(crate) fn render_input_top_right_indicator(&self, frame: &mut Frame, input_area: Rect) {
        let indicator_y = input_area.y.saturating_sub(1);

        if (self.mode == Mode::Navigation || self.mode == Mode::Search)
            && !self.editor.state.search_matches().is_empty()
        {
            let num_results = self.editor.state.search_matches().len();
            let cursor_pos = self.editor.state.cursor;
            let current_line = cursor_pos.row + 1;
            let total_lines = self.editor.state.lines.len();
            let search_info = format!("{} results [{}/{}]", num_results, current_line, total_lines);

            self.render_indicator_text(frame, input_area, indicator_y, &search_info, Color::Cyan);
            return;
        }

        if let Some((mode_text, mode_color)) = self.safety_state.assistant_mode.to_display() {
            let cycle_hint = "(shift + tab to cycle)";
            let full_text = format!("{} {}", mode_text, cycle_hint);
            let total_width = full_text.len() as u16;
            let start_x = input_area.x + input_area.width.saturating_sub(total_width + 1);

            self.render_indicator_text_at(frame, start_x, indicator_y, &mode_text, mode_color);
            let cycle_start_x = start_x + mode_text.len() as u16;
            self.render_indicator_text_at(
                frame,
                cycle_start_x,
                indicator_y,
                &format!(" {}", cycle_hint),
                Color::DarkGray,
            );
        }
    }

    fn render_indicator_text(
        &self,
        frame: &mut Frame,
        input_area: Rect,
        y: u16,
        text: &str,
        color: Color,
    ) {
        let total_width = text.len() as u16;
        let start_x = input_area.x + input_area.width.saturating_sub(total_width + 1);
        self.render_indicator_text_at(frame, start_x, y, text, color);
    }

    fn render_indicator_text_at(
        &self,
        frame: &mut Frame,
        start_x: u16,
        y: u16,
        text: &str,
        color: Color,
    ) {
        let frame_area = frame.area();
        let mut current_x = start_x;
        for ch in text.chars() {
            if current_x < frame_area.width && y < frame_area.height {
                if let Some(cell) = frame.buffer_mut().cell_mut((current_x, y)) {
                    cell.set_char(ch);
                    cell.set_style(Style::default().fg(color));
                }
                current_x += 1;
            }
        }
    }
}
