use ratatui::{
    Frame,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{App, TodoItem};

fn todo_symbol(status: &str) -> &str {
    match status {
        "pending" => "□",
        "in_progress" | "completed" => "○",
        _ => "•",
    }
}

fn todo_color(status: &str) -> Color {
    match status {
        "pending" | "completed" => Color::DarkGray,
        "in_progress" => Color::Cyan,
        _ => Color::White,
    }
}

fn format_todo_text(text: &str, status: &str) -> String {
    match status {
        "completed" => format!("\x1b[9m{}\x1b[29m", text),
        _ => text.to_string(),
    }
}

fn build_todo_lines(todos: &[TodoItem], indent: usize, lines: &mut Vec<Line<'static>>) {
    for todo in todos {
        let symbol = todo_symbol(&todo.status);
        let color = todo_color(&todo.status);
        let text = format_todo_text(&todo.content, &todo.status);
        let indent_str = "  ".repeat(indent);

        lines.push(Line::from(vec![
            Span::raw("│ "),
            Span::raw(indent_str),
            Span::styled(format!("{} ", symbol), Style::default().fg(color)),
            Span::styled(text, Style::default().fg(color)),
            Span::raw(" │"),
        ]));

        if !todo.children.is_empty() {
            build_todo_lines(&todo.children, indent + 1, lines);
        }
    }
}

fn count_todos(todos: &[TodoItem]) -> usize {
    todos
        .iter()
        .map(|todo| 1 + count_todos(&todo.children))
        .sum()
}

impl App {
    pub(crate) fn render_todos_panel(&self, frame: &mut Frame, todos_area: ratatui::layout::Rect) {
        let todos = self.load_todos().unwrap_or_default();
        let todo_count = count_todos(&todos);

        let mut all_lines: Vec<Line<'static>> = Vec::new();
        let title = format!(" Tasks ({}) ", todo_count);
        let border_width = (todos_area.width as usize).saturating_sub(2);
        let remaining = border_width.saturating_sub(title.len());
        let left_dash = remaining / 2;
        let right_dash = remaining - left_dash;

        all_lines.push(Line::from(vec![
            Span::raw("┌"),
            Span::raw("─".repeat(left_dash)),
            Span::styled(title, Style::default().fg(Color::Magenta)),
            Span::raw("─".repeat(right_dash)),
            Span::raw("┐"),
        ]));

        if todos.is_empty() {
            let empty_msg = "No tasks yet";
            let padding = border_width.saturating_sub(empty_msg.len() + 2);
            all_lines.push(Line::from(vec![
                Span::raw("│ "),
                Span::styled(empty_msg, Style::default().fg(Color::DarkGray)),
                Span::raw(" ".repeat(padding)),
                Span::raw(" │"),
            ]));
        } else {
            let mut lines: Vec<Line<'static>> = Vec::new();
            build_todo_lines(&todos, 0, &mut lines);
            all_lines.extend(lines);
        }

        all_lines.push(Line::from(vec![
            Span::raw("└"),
            Span::raw("─".repeat(border_width)),
            Span::raw("┘"),
        ]));
        all_lines.push(Line::from(""));
        all_lines.push(
            Line::from(Span::styled(
                " Esc to close ",
                Style::default().fg(Color::DarkGray),
            ))
            .centered(),
        );

        frame.render_widget(Paragraph::new(all_lines), todos_area);
    }
}

#[cfg(test)]
mod tests {
    use super::count_todos;
    use crate::app::TodoItem;

    #[test]
    fn count_todos_counts_nested_items() {
        let todos = vec![TodoItem {
            content: "parent".to_string(),
            status: "pending".to_string(),
            active_form: "".to_string(),
            children: vec![
                TodoItem {
                    content: "child 1".to_string(),
                    status: "completed".to_string(),
                    active_form: "".to_string(),
                    children: vec![],
                },
                TodoItem {
                    content: "child 2".to_string(),
                    status: "in_progress".to_string(),
                    active_form: "".to_string(),
                    children: vec![TodoItem {
                        content: "grandchild".to_string(),
                        status: "pending".to_string(),
                        active_form: "".to_string(),
                        children: vec![],
                    }],
                },
            ],
        }];

        assert_eq!(count_todos(&todos), 4);
    }
}
