use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, Borders},
    Frame,
};

#[derive(Clone, Debug)]
pub struct Session {
    pub id: usize,
    pub name: String,
    pub status: SessionStatus,
    pub group: Option<String>,
    pub children: Vec<Session>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionStatus {
    Attached,
    Detached,
    Running,
}

pub struct SessionManager {
    pub sessions: Vec<Session>,
    pub selected_index: usize,
    pub list_state: ListState,
}

impl SessionManager {
    pub fn new() -> Self {
        let sessions = vec![
            Session {
                id: 0,
                name: "collosal-2: 8 windows (group collosal: collosal-0,collosal-1,collosal-2)".to_string(),
                status: SessionStatus::Attached,
                group: Some("collosal".to_string()),
                children: vec![
                    Session {
                        id: 1,
                        name: "edtui: \"/home/wise/rust/tui\"".to_string(),
                        status: SessionStatus::Running,
                        group: None,
                        children: vec![],
                    },
                    Session {
                        id: 2,
                        name: "embed".to_string(),
                        status: SessionStatus::Running,
                        group: None,
                        children: vec![],
                    },
                    Session {
                        id: 3,
                        name: "semantic".to_string(),
                        status: SessionStatus::Running,
                        group: None,
                        children: vec![],
                    },
                    Session {
                        id: 4,
                        name: "target/debug/tm".to_string(),
                        status: SessionStatus::Running,
                        group: None,
                        children: vec![],
                    },
                    Session {
                        id: 5,
                        name: "node: \"Owen - tool_agent\"".to_string(),
                        status: SessionStatus::Running,
                        group: None,
                        children: vec![],
                    },
                    Session {
                        id: 6,
                        name: "zsh".to_string(),
                        status: SessionStatus::Running,
                        group: None,
                        children: vec![],
                    },
                    Session {
                        id: 7,
                        name: "claude-".to_string(),
                        status: SessionStatus::Running,
                        group: None,
                        children: vec![],
                    },
                    Session {
                        id: 8,
                        name: "[tmux]*".to_string(),
                        status: SessionStatus::Running,
                        group: None,
                        children: vec![],
                    },
                ],
            },
        ];

        let mut list_state = ListState::default();
        list_state.select(Some(0));

        Self {
            sessions,
            selected_index: 0,
            list_state,
        }
    }


    pub fn next_session(&mut self) {
        let total_items = self.get_total_session_count();
        if total_items > 0 {
            self.selected_index = (self.selected_index + 1) % total_items;
            self.list_state.select(Some(self.selected_index));
        }
    }

    pub fn previous_session(&mut self) {
        let total_items = self.get_total_session_count();
        if total_items > 0 {
            self.selected_index = if self.selected_index == 0 {
                total_items - 1
            } else {
                self.selected_index - 1
            };
            self.list_state.select(Some(self.selected_index));
        }
    }

    fn get_total_session_count(&self) -> usize {
        self.sessions.iter().fold(0, |acc, session| {
            acc + 1 + session.children.len()
        })
    }

    pub fn get_session_count(&self) -> usize {
        self.get_total_session_count()
    }

    pub fn get_selected_session_index(&self) -> usize {
        self.selected_index
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        use ratatui::layout::{Constraint, Layout};
        use ratatui::widgets::Paragraph;

        // Split the given area (should be top 49% of screen) into sessions list and input box
        let layout = Layout::vertical([
            Constraint::Percentage(49),
            Constraint::Percentage(51),
        ]);
        let [sessions_area, input_area] = layout.areas(area);

        let session_items = Self::create_session_list_items_with_selection(&self.sessions, self.selected_index);

        let sessions_list = List::new(session_items)
            .block(
                Block::default()
                    .borders(Borders::NONE)
            );

        frame.render_widget(sessions_list, sessions_area);

        // Render input box with selected session index
        let title = format!(" {} (sort: index) ", self.selected_index);
        let input = Paragraph::new("")
            .block(Block::default().borders(Borders::ALL).title(title));
        frame.render_widget(input, input_area);
    }

    pub fn create_session_list_items_with_selection(sessions: &[Session], selected_index: usize) -> Vec<ListItem> {
        let mut items = Vec::new();
        let mut current_index = 0;

        for session in sessions {
            let status_indicator = match session.status {
                SessionStatus::Attached => " (attached)",
                SessionStatus::Detached => " (detached)",
                SessionStatus::Running => "",
            };

            let is_selected = current_index == selected_index;

            let line = Line::from(vec![
                Span::styled(
                    format!("({}) ", session.id),
                    if is_selected {
                        Style::default().bg(Color::Yellow).fg(Color::Black)
                    } else {
                        Style::default().fg(Color::White)
                    }
                ),
                Span::styled(
                    "- ",
                    if is_selected {
                        Style::default().bg(Color::Yellow).fg(Color::Black)
                    } else {
                        Style::default().fg(Color::White)
                    }
                ),
                Span::styled(
                    format!("{}{}", session.name, status_indicator),
                    if is_selected {
                        Style::default().bg(Color::Yellow).fg(Color::Black)
                    } else {
                        Style::default().fg(Color::White)
                    }
                ),
            ]);

            items.push(ListItem::new(line));
            current_index += 1;

            let child_count = session.children.len();
            for (child_idx, child) in session.children.iter().enumerate() {
                let is_last_child = child_idx == child_count - 1;
                let tree_char = if is_last_child { "└──>" } else { "├──>" };
                let is_child_selected = current_index == selected_index;

                let child_line = Line::from(vec![
                    Span::styled(
                        format!("({}) ", child.id),
                        if is_child_selected {
                            Style::default().bg(Color::Yellow).fg(Color::Black)
                        } else {
                            Style::default().fg(Color::White)
                        }
                    ),
                    Span::styled(
                        tree_char,
                        if is_child_selected {
                            Style::default().bg(Color::Yellow).fg(Color::Black)
                        } else {
                            Style::default().fg(Color::White)
                        }
                    ),
                    Span::styled(
                        format!(" {}: {}", child.id, child.name),
                        if is_child_selected {
                            Style::default().bg(Color::Yellow).fg(Color::Black)
                        } else {
                            Style::default().fg(Color::White)
                        }
                    ),
                ]);

                items.push(ListItem::new(child_line));
                current_index += 1;
            }
        }

        items
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}
