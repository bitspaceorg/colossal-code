use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    symbols,
    text::{Line as RatatuiLine, Span},
    widgets::{Block, Borders, List, ListItem, Widget},
};

use crate::app::{App, TodoItem};

const TODO_BORDER_SET: symbols::border::Set = symbols::border::Set {
    top_left: "┌",
    top_right: "┐",
    bottom_left: "└",
    bottom_right: "┘",
    vertical_left: "│",
    vertical_right: "│",
    horizontal_top: "─",
    horizontal_bottom: "─",
};

fn todo_symbol(status: &str) -> &str {
    match status {
        "pending" => "□",
        "in_progress" => "▣",
        "completed" => "☒",
        _ => "□",
    }
}

fn todo_color(status: &str) -> Color {
    match status {
        "pending" | "completed" => Color::DarkGray,
        "in_progress" => Color::Cyan,
        _ => Color::White,
    }
}

fn todo_line(todo: &TodoItem, indent: usize) -> RatatuiLine<'static> {
    let color = todo_color(&todo.status);
    let indent_str = "  ".repeat(indent);
    let text_style = match todo.status.as_str() {
        "completed" => Style::default()
            .fg(color)
            .add_modifier(Modifier::CROSSED_OUT),
        _ => Style::default().fg(color),
    };

    RatatuiLine::from(vec![
        Span::raw(indent_str),
        Span::styled(
            format!("{} ", todo_symbol(&todo.status)),
            Style::default().fg(color),
        ),
        Span::styled(todo.content.clone(), text_style),
    ])
}

fn flatten_todos(todos: &[TodoItem], indent: usize, out: &mut Vec<RatatuiLine<'static>>) {
    for todo in todos {
        out.push(todo_line(todo, indent));
        if !todo.children.is_empty() {
            flatten_todos(&todo.children, indent + 1, out);
        }
    }
}

fn render_todo_buffer(items: Vec<ListItem<'static>>) -> Buffer {
    let width = 120u16;
    let box_height = items.len().max(1) as u16 + 2;
    let area = Rect::new(0, 0, width, box_height);
    let mut buffer = Buffer::empty(area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(TODO_BORDER_SET)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    block.render(area, &mut buffer);
    List::new(items).render(inner, &mut buffer);

    buffer
}

fn buffer_row_to_line(
    buffer: &Buffer,
    row: u16,
    connector_prefix: &Span<'static>,
) -> RatatuiLine<'static> {
    let mut spans = vec![connector_prefix.clone()];
    let mut run_text = String::new();
    let mut run_style: Option<Style> = None;
    let mut row_end = buffer.area.right();

    while row_end > buffer.area.left() {
        let cell = &buffer[(row_end - 1, row)];
        if cell.symbol().is_empty() {
            row_end -= 1;
            continue;
        }
        if cell.symbol() == " " && cell.style() == Style::default() {
            row_end -= 1;
            continue;
        }
        break;
    }

    for x in buffer.area.left()..row_end {
        let cell = &buffer[(x, row)];
        let symbol = cell.symbol();
        if symbol.is_empty() {
            continue;
        }

        let style = cell.style();
        if run_style != Some(style) {
            if let Some(style) = run_style.take() {
                spans.push(Span::styled(std::mem::take(&mut run_text), style));
            }
            run_style = Some(style);
        }
        run_text.push_str(symbol);
    }

    if let Some(style) = run_style {
        spans.push(Span::styled(run_text, style));
    }

    RatatuiLine::from(spans)
}

pub(crate) fn render_todo_artifact_lines(
    todos: &[TodoItem],
    max_width: usize,
    connector_prefix: Span<'static>,
) -> Vec<RatatuiLine<'static>> {
    let _ = max_width;

    let mut lines = Vec::new();
    flatten_todos(todos, 0, &mut lines);

    let items = if lines.is_empty() {
        vec![ListItem::new(RatatuiLine::from(Span::styled(
            "No tasks yet",
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        lines.into_iter().map(ListItem::new).collect::<Vec<_>>()
    };

    let buffer = render_todo_buffer(items);
    (buffer.area.top()..buffer.area.bottom())
        .map(|row| buffer_row_to_line(&buffer, row, &connector_prefix))
        .collect()
}

impl App {
    pub(crate) fn extract_todos_from_json_payload(payload: &str) -> Option<Vec<TodoItem>> {
        let parsed = serde_json::from_str::<serde_json::Value>(payload).ok()?;
        Self::extract_todos_from_value(&parsed)
    }

    pub(crate) fn extract_todos_from_value(parsed: &serde_json::Value) -> Option<Vec<TodoItem>> {
        let todos_value = parsed
            .get("todos")
            .or_else(|| parsed.get("structured_todos"))?;

        let todo_array = match todos_value {
            serde_json::Value::Array(array) => array.clone(),
            serde_json::Value::String(text) => serde_json::from_str::<serde_json::Value>(text)
                .ok()?
                .as_array()?
                .clone(),
            _ => return None,
        };

        Some(
            todo_array
                .iter()
                .filter_map(Self::parse_todo_item)
                .collect::<Vec<_>>(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::render_todo_artifact_lines;
    use crate::app::TodoItem;
    use ratatui::text::Span;

    #[test]
    fn todo_artifact_matches_diff_width() {
        let todos = vec![TodoItem {
            content: "Implement user authentication system".to_string(),
            status: "pending".to_string(),
            active_form: String::new(),
            children: vec![TodoItem {
                content: "Create user database schema".to_string(),
                status: "pending".to_string(),
                active_form: String::new(),
                children: vec![],
            }],
        }];

        let lines = render_todo_artifact_lines(&todos, 100, Span::raw(""));

        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].to_string(), format!("┌{}┐", "─".repeat(118)));
        assert_eq!(lines[3].to_string(), format!("└{}┘", "─".repeat(118)));
    }

    #[test]
    fn todo_artifact_renders_empty_state() {
        let lines = render_todo_artifact_lines(&[], 100, Span::raw(""));

        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].to_string(), format!("┌{}┐", "─".repeat(118)));
        assert!(lines[1].to_string().contains("No tasks yet"));
        assert_eq!(lines[2].to_string(), format!("└{}┘", "─".repeat(118)));
    }
}
