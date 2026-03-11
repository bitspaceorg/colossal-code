use ratatui::style::Color;

pub(crate) fn is_tool_result_success(result: &str) -> bool {
    let trimmed = result.trim().to_ascii_lowercase();
    !(trimmed.starts_with("error") || trimmed == "failed")
}

pub(crate) fn tool_result_color(result: &str) -> Color {
    if is_tool_result_success(result) {
        Color::Green
    } else {
        Color::Red
    }
}
