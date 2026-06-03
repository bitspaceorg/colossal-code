use ratatui::{
    Frame,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph},
};

use crate::app::App;

impl App {
    pub(crate) fn shell_panel_entries(&self) -> Vec<(String, String, String, std::time::Instant)> {
        let mut entries = Vec::new();
        if let Some((session_id, command, started_at)) = &self.active_foreground_shell {
            entries.push((
                session_id.clone(),
                command.clone(),
                agent_core::shell_session_log_path(session_id),
                *started_at,
            ));
        }
        entries.extend(self.background_tasks.iter().filter_map(
            |(session_id, command, log_file, started_at)| {
                if self
                    .active_foreground_shell
                    .as_ref()
                    .map(|(active_id, _, _)| active_id == session_id)
                    .unwrap_or(false)
                {
                    None
                } else {
                    Some((
                        session_id.clone(),
                        command.clone(),
                        log_file.clone(),
                        *started_at,
                    ))
                }
            },
        ));
        entries
    }

    pub(crate) fn render_background_tasks(
        &self,
        frame: &mut Frame,
        task_area: ratatui::layout::Rect,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Shells ")
            .title_bottom(
                Line::from(" ↑/↓ to select · Enter to view · k to kill · Esc to close ").centered(),
            );

        let shell_entries = self.shell_panel_entries();
        let task_count_text = format!(" {} active shells", shell_entries.len());
        let items: Vec<ListItem> = shell_entries
            .iter()
            .enumerate()
            .map(|(idx, (_session_id, command, _log_file, _start_time))| {
                let is_selected = idx == self.background_tasks_selected;
                let max_cmd_len = task_area.width.saturating_sub(10) as usize;
                let display_cmd = if command.len() > max_cmd_len {
                    format!("{} …", &command[..max_cmd_len.saturating_sub(2)])
                } else {
                    command.clone()
                };

                let line = if is_selected {
                    Line::from(vec![
                        Span::styled(">  ", Style::default().fg(Color::Blue)),
                        Span::styled(display_cmd, Style::default().fg(Color::Blue)),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw("   "),
                        Span::styled(display_cmd, Style::default().fg(Color::White)),
                    ])
                };

                ListItem::new(line)
            })
            .collect();

        let inner = block.inner(task_area);
        frame.render_widget(block, task_area);

        let count_line = Line::from(Span::styled(
            task_count_text,
            Style::default().fg(Color::DarkGray),
        ));
        let count_para = Paragraph::new(count_line);
        let count_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(count_para, count_area);

        let list_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: inner.height.saturating_sub(2),
        };
        frame.render_widget(List::new(items), list_area);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex as StdMutex, OnceLock};
    use std::time::Instant;

    use agent_core::AgentMessage;
    use tokio::sync::mpsc;

    use crate::app::App;

    fn env_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| StdMutex::new(()))
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(previous) = &self.previous {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    #[tokio::test]
    async fn shell_panel_entries_include_foreground_and_background_shells() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        app.background_tasks.push((
            "bg-1".to_string(),
            "sleep 10".to_string(),
            "/tmp/bg.log".to_string(),
            Instant::now(),
        ));
        app.active_foreground_shell = Some((
            "fg-1".to_string(),
            "while true { }".to_string(),
            Instant::now(),
        ));

        let entries = app.shell_panel_entries();

        assert_eq!(
            entries.len(),
            2,
            "foreground shell should also appear in /shells"
        );
    }

    #[tokio::test]
    async fn foreground_shell_messages_update_shell_panel_entries() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        let (tx, rx) = mpsc::unbounded_channel();
        app.agent_rx = Some(rx);

        tx.send(AgentMessage::ForegroundShellStarted(
            "fg-1".to_string(),
            "while true { }".to_string(),
        ))
        .expect("send foreground shell started");
        let _ = app.drain_agent_rx();
        assert_eq!(app.shell_panel_entries().len(), 1);

        tx.send(AgentMessage::ForegroundShellFinished("fg-1".to_string()))
            .expect("send foreground shell finished");
        let _ = app.drain_agent_rx();
        assert!(app.shell_panel_entries().is_empty());
    }
}
