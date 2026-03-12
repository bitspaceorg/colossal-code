use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{ListItem, ListState},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct Session {
    pub id: usize,
    pub name: String,
    pub status: SessionStatus,
    pub group: Option<String>,
    pub children: Vec<Session>,
    pub prefix: Option<String>,
    pub started_at: Option<Instant>,
    pub completed_at: Option<Instant>,
    pub role: Option<SessionRole>,
    pub worktree_branch: Option<String>,
    pub worktree_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SessionStatus {
    Attached,
    Detached,
    Running,
    /// Agent is pending execution
    Pending,
    /// Agent is currently executing
    InProgress,
    /// Agent completed successfully
    Completed,
    /// Agent failed
    Failed,
}

/// Represents an orchestrator stack entry for session display.
#[derive(Clone, Debug)]
pub struct OrchestratorEntry {
    pub spec_id: String,
    pub spec_title: String,
    pub prefix: String,
    pub step_title: String,
    pub role: SessionRole,
    pub status: SessionStatus,
    pub started_at: Option<Instant>,
    pub completed_at: Option<Instant>,
    pub worktree_branch: Option<String>,
    pub worktree_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionRole {
    Implementor,
    Summarizer,
    Verifier,
    Merge,
}

pub struct SessionManager {
    pub sessions: Vec<Session>,
    pub selected_index: usize,
    pub list_state: ListState,
    /// Optional orchestrator entries for agent session display
    pub orchestrator_entries: Vec<OrchestratorEntry>,
}

impl SessionManager {
    pub fn new() -> Self {
        let mut list_state = ListState::default();
        list_state.select(None);

        Self {
            sessions: Vec::new(),
            selected_index: 0,
            list_state,
            orchestrator_entries: Vec::new(),
        }
    }

    /// Update sessions from orchestrator stack entries.
    /// Converts orchestrator entries into the session tree structure.
    pub fn update_from_orchestrator(&mut self, entries: Vec<OrchestratorEntry>) {
        self.orchestrator_entries = entries.clone();
        self.sessions.clear();

        if entries.is_empty() {
            self.list_state.select(None);
            return;
        }

        let mut grouped: BTreeMap<String, (String, Vec<OrchestratorEntry>)> = BTreeMap::new();
        for entry in entries {
            grouped
                .entry(entry.spec_id.clone())
                .or_insert_with(|| (entry.spec_title.clone(), Vec::new()))
                .1
                .push(entry);
        }

        for (_spec_id, (spec_title, mut steps)) in grouped {
            steps.sort_by(|a, b| a.prefix.cmp(&b.prefix));
            let mut root = Session {
                id: 0,
                name: format!("Spec: {}", spec_title),
                status: SessionStatus::Pending,
                group: Some("orchestrator".to_string()),
                children: Vec::new(),
                prefix: None,
                started_at: None,
                completed_at: None,
                role: None,
                worktree_branch: None,
                worktree_path: None,
            };
            for entry in steps {
                Self::insert_entry(&mut root, entry);
            }
            self.sessions.push(root);
        }

        let mut next_id = 0;
        Self::assign_ids(&mut self.sessions, &mut next_id);

        let total_items = self.get_total_session_count();
        if total_items == 0 {
            self.selected_index = 0;
            self.list_state.select(None);
        } else {
            self.selected_index = self.selected_index.min(total_items - 1);
            self.list_state.select(Some(self.selected_index));
        }
    }

    fn insert_entry(root: &mut Session, entry: OrchestratorEntry) {
        if entry.prefix.is_empty() {
            root.status = entry.status;
            root.started_at = entry.started_at;
            root.completed_at = entry.completed_at;
            root.role = Some(entry.role.clone());
            root.worktree_branch = entry.worktree_branch.clone();
            root.worktree_path = entry.worktree_path.clone();
            return;
        }

        let mut segments: Vec<&str> = entry.prefix.split('.').collect();
        if segments.is_empty() {
            return;
        }

        let mut current = root;
        let mut path = Vec::new();
        for segment in segments.drain(..) {
            path.push(segment.to_string());
            let key = path.join(".");
            let existing_index = current
                .children
                .iter()
                .enumerate()
                .find(|(_, child)| child.prefix.as_deref() == Some(key.as_str()))
                .map(|(idx, _)| idx);

            if let Some(idx) = existing_index {
                current = current.children.get_mut(idx).expect("child exists");
            } else {
                current.children.push(Session {
                    id: 0,
                    name: format!("Step {}", key),
                    status: SessionStatus::Pending,
                    group: Some("orchestrator".to_string()),
                    children: Vec::new(),
                    prefix: Some(key.clone()),
                    started_at: None,
                    completed_at: None,
                    role: Some(entry.role.clone()),
                    worktree_branch: entry.worktree_branch.clone(),
                    worktree_path: entry.worktree_path.clone(),
                });
                let idx = current.children.len() - 1;
                current = current.children.get_mut(idx).expect("child just inserted");
            }
        }

        current.name = format!("{} › {}", entry.prefix, entry.step_title);
        current.status = entry.status;
        current.role = Some(entry.role.clone());
        current.worktree_branch = entry.worktree_branch.clone();
        current.worktree_path = entry.worktree_path.clone();
        if current.started_at.is_none() {
            current.started_at = entry.started_at;
        }
        if entry.completed_at.is_some() {
            current.completed_at = entry.completed_at;
        }
    }

    fn assign_ids(tree: &mut [Session], counter: &mut usize) {
        for session in tree {
            session.id = *counter;
            *counter += 1;
            Self::assign_ids(&mut session.children, counter);
        }
    }

    /// Clear orchestrator entries and remove from session list.
    pub fn clear_orchestrator_entries(&mut self) {
        self.orchestrator_entries.clear();
        self.sessions
            .retain(|s| s.group.as_deref() != Some("orchestrator"));
        if self.sessions.is_empty() {
            self.list_state.select(None);
            self.selected_index = 0;
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
        self.sessions.iter().map(Self::count_nodes).sum()
    }

    fn count_nodes(session: &Session) -> usize {
        1 + session
            .children
            .iter()
            .map(Self::count_nodes)
            .sum::<usize>()
    }

    /// Get the currently selected session.
    pub fn get_selected_session(&self) -> Option<&Session> {
        let path = Self::index_path(&self.sessions, self.selected_index)?;
        Self::session_from_path(&self.sessions, &path)
    }

    fn session_from_path<'a>(sessions: &'a [Session], path: &[usize]) -> Option<&'a Session> {
        let (head, tail) = path.split_first()?;
        let mut node = sessions.get(*head)?;
        for idx in tail {
            node = node.children.get(*idx)?;
        }
        Some(node)
    }

    fn session_from_path_mut<'a>(
        sessions: &'a mut [Session],
        path: &[usize],
    ) -> Option<&'a mut Session> {
        let (head, tail) = path.split_first()?;
        let mut node = sessions.get_mut(*head)?;
        for idx in tail {
            node = node.children.get_mut(*idx)?;
        }
        Some(node)
    }

    fn index_path(sessions: &[Session], target: usize) -> Option<Vec<usize>> {
        fn dfs(
            sessions: &[Session],
            target: usize,
            current: &mut usize,
            trail: &mut Vec<usize>,
        ) -> Option<Vec<usize>> {
            for (idx, session) in sessions.iter().enumerate() {
                if *current == target {
                    trail.push(idx);
                    return Some(trail.clone());
                }
                *current += 1;
                trail.push(idx);
                if let Some(found) = dfs(&session.children, target, current, trail) {
                    return Some(found);
                }
                trail.pop();
            }
            None
        }
        let mut count = 0;
        let mut trail = Vec::new();
        dfs(sessions, target, &mut count, &mut trail)
    }

    /// Toggle the status of the selected session between attached/detached.
    pub fn toggle_detach(&mut self) {
        if let Some(path) = Self::index_path(&self.sessions, self.selected_index) {
            if let Some(session) = Self::session_from_path_mut(&mut self.sessions, &path) {
                session.status = match session.status {
                    SessionStatus::Attached => SessionStatus::Detached,
                    SessionStatus::Detached => SessionStatus::Attached,
                    ref other => other.clone(),
                };
            }
        }
    }

    /// Remove the selected session from the list.
    pub fn kill_selected(&mut self) -> Option<String> {
        let path = Self::index_path(&self.sessions, self.selected_index)?;
        if path.is_empty() {
            return None;
        }

        let removed = if path.len() == 1 {
            self.sessions.remove(path[0])
        } else {
            let mut cursor = &mut self.sessions;
            for idx in &path[..path.len() - 1] {
                cursor = &mut cursor[*idx].children;
            }
            cursor.remove(*path.last().unwrap())
        };

        let total = self.get_total_session_count();
        if total == 0 {
            self.selected_index = 0;
            self.list_state.select(None);
        } else if self.selected_index >= total {
            self.selected_index = total - 1;
            self.list_state.select(Some(self.selected_index));
        }

        Some(removed.name)
    }

    /// Get status badge text for the selected session.
    pub fn get_selected_status_badge(&self) -> Option<&'static str> {
        self.get_selected_session().map(|s| match s.status {
            SessionStatus::Attached => "[attached]",
            SessionStatus::Detached => "[detached]",
            SessionStatus::Running => "[running]",
            SessionStatus::Pending => "[pending]",
            SessionStatus::InProgress => "[in-progress]",
            SessionStatus::Completed => "[completed]",
            SessionStatus::Failed => "[failed]",
        })
    }

    pub fn create_session_list_items_with_selection(
        sessions: &[Session],
        selected_index: usize,
    ) -> Vec<ListItem<'static>> {
        let mut items: Vec<ListItem<'static>> = Vec::new();
        let mut current_index = 0;
        for session in sessions {
            Self::build_list_items(session, 0, selected_index, &mut current_index, &mut items);
        }
        items
    }

    fn build_list_items(
        session: &Session,
        depth: usize,
        selected_index: usize,
        current_index: &mut usize,
        items: &mut Vec<ListItem<'static>>,
    ) {
        let is_selected = *current_index == selected_index;
        let indent = "  ".repeat(depth);
        let status_icon = match session.status {
            SessionStatus::Pending => "○",
            SessionStatus::InProgress => "◐",
            SessionStatus::Completed => "●",
            SessionStatus::Failed => "✗",
            SessionStatus::Attached => "@",
            SessionStatus::Detached => "⚑",
            SessionStatus::Running => "●",
        };
        let runtime = Self::runtime_for(session);
        let name = session.name.clone();
        let mut spans = vec![
            Span::raw(indent),
            Span::styled(
                status_icon,
                if is_selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
            Span::raw(" "),
            Span::styled(
                name,
                if is_selected {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::White)
                },
            ),
        ];
        if let Some(role) = session.role.as_ref() {
            let (label, color) = Self::role_badge(role);
            spans.push(Span::raw(" "));
            spans.push(Span::styled(label, Style::default().fg(color)));
        }
        if let Some(branch) = session.worktree_branch.as_ref() {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("· {}", branch),
                Style::default().fg(Color::DarkGray),
            ));
        }
        if let Some(runtime) = runtime {
            spans.push(Span::styled(
                format!(" · {}", runtime),
                Style::default().fg(Color::DarkGray),
            ));
        }
        let line = Line::from(spans);
        items.push(ListItem::new(line));
        *current_index += 1;
        for child in &session.children {
            Self::build_list_items(child, depth + 1, selected_index, current_index, items);
        }
    }

    fn runtime_for(session: &Session) -> Option<String> {
        let start = session.started_at?;
        let duration = if let Some(end) = session.completed_at {
            end.duration_since(start)
        } else {
            start.elapsed()
        };
        Some(Self::format_duration(duration))
    }

    fn format_duration(duration: Duration) -> String {
        let secs = duration.as_secs();
        if secs < 60 {
            format!("{}s", secs)
        } else {
            let minutes = secs / 60;
            let seconds = secs % 60;
            format!("{}m {}s", minutes, seconds)
        }
    }

    fn role_badge(role: &SessionRole) -> (&'static str, Color) {
        match role {
            SessionRole::Implementor => ("[impl]", Color::Gray),
            SessionRole::Summarizer => ("[summarize]", Color::Cyan),
            SessionRole::Verifier => ("[verifier]", Color::Magenta),
            SessionRole::Merge => ("[merge]", Color::LightBlue),
        }
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}
