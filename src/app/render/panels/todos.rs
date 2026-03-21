use ratatui::{
    Frame,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph},
};

use crate::app::{App, TodoItem};

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

fn format_todo_text(text: &str, status: &str) -> String {
    match status {
        "completed" => text.to_string(),
        _ => text.to_string(),
    }
}

fn build_todo_lines(todos: &[TodoItem], indent: usize, lines: &mut Vec<Line<'static>>) {
    for todo in todos {
        let symbol = todo_symbol(&todo.status);
        let color = todo_color(&todo.status);
        let text = format_todo_text(&todo.content, &todo.status);
        let indent_str = "  ".repeat(indent);

        let text_style = match todo.status.as_str() {
            "completed" => Style::default()
                .fg(color)
                .add_modifier(Modifier::CROSSED_OUT),
            _ => Style::default().fg(color),
        };

        lines.push(Line::from(vec![
            Span::raw(indent_str),
            Span::styled(format!("{} ", symbol), Style::default().fg(color)),
            Span::styled(text, text_style),
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

fn count_todos_by_status(todos: &[TodoItem], status: &str) -> usize {
    todos
        .iter()
        .map(|todo| {
            usize::from(todo.status == status) + count_todos_by_status(&todo.children, status)
        })
        .sum()
}

fn visible_todo_lines(todos: &[TodoItem]) -> u16 {
    todos
        .iter()
        .map(|todo| 1 + visible_todo_lines(&todo.children))
        .sum()
}

impl App {
    pub(crate) fn todos_panel_height(&self) -> u16 {
        let todos = self.load_todos().unwrap_or_default();
        let content_height = visible_todo_lines(&todos).max(1);
        content_height.saturating_add(4)
    }

    pub(crate) fn render_todos_panel(&self, frame: &mut Frame, todos_area: ratatui::layout::Rect) {
        let todos = self.load_todos().unwrap_or_default();
        let todo_count = count_todos(&todos);
        let in_progress_count = count_todos_by_status(&todos, "in_progress");
        let completed_count = count_todos_by_status(&todos, "completed");
        let pending_count = todo_count.saturating_sub(in_progress_count + completed_count);
        let accent = Color::Magenta;

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(accent))
            .title(Line::from(Span::styled(
                " Tasks ",
                Style::default().fg(accent),
            )))
            .title_bottom(
                Line::from(Span::styled(" Esc to close ", Style::default().fg(accent))).centered(),
            );

        let inner = block.inner(todos_area);
        frame.render_widget(block, todos_area);

        let count_text = if todo_count == 0 {
            " 0 tasks".to_string()
        } else {
            format!(
                " {} tasks • {} pending • {} active • {} completed",
                todo_count, pending_count, in_progress_count, completed_count
            )
        };
        let count_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                count_text,
                Style::default().fg(Color::DarkGray),
            ))),
            count_area,
        );

        if todos.is_empty() {
            let empty_area = ratatui::layout::Rect {
                x: inner.x,
                y: inner.y + 2,
                width: inner.width,
                height: inner.height.saturating_sub(2),
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "No tasks yet",
                    Style::default().fg(Color::DarkGray),
                ))),
                empty_area,
            );
        } else {
            let mut lines: Vec<Line<'static>> = Vec::new();
            build_todo_lines(&todos, 0, &mut lines);
            let items: Vec<ListItem<'static>> = lines.into_iter().map(ListItem::new).collect();

            let list_area = ratatui::layout::Rect {
                x: inner.x,
                y: inner.y + 2,
                width: inner.width,
                height: inner.height.saturating_sub(2),
            };
            frame.render_widget(List::new(items), list_area);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{count_todos, count_todos_by_status, visible_todo_lines};
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

    #[test]
    fn visible_todo_lines_counts_nested_items() {
        let todos = vec![TodoItem {
            content: "parent".to_string(),
            status: "pending".to_string(),
            active_form: "".to_string(),
            children: vec![TodoItem {
                content: "child".to_string(),
                status: "completed".to_string(),
                active_form: "".to_string(),
                children: vec![],
            }],
        }];

        assert_eq!(visible_todo_lines(&todos), 2);
    }

    #[test]
    fn count_todos_by_status_counts_nested_items() {
        let todos = vec![TodoItem {
            content: "parent".to_string(),
            status: "in_progress".to_string(),
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
                    children: vec![],
                },
            ],
        }];

        assert_eq!(count_todos_by_status(&todos, "in_progress"), 2);
        assert_eq!(count_todos_by_status(&todos, "completed"), 1);
    }
}
