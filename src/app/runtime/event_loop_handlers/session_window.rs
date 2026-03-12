use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Mode};

pub(crate) fn handle_runtime_key_session_window(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') => {
            app.leave_alt_w_view();
            app.cached_mode_content = None;
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::ALT) => {
            app.leave_alt_w_view();
            app.cached_mode_content = None;
        }
        KeyCode::Up => {
            app.session_manager.previous_session();
        }
        KeyCode::Down => {
            app.session_manager.next_session();
        }
        KeyCode::Enter => {
            if let Some(session) = app.session_manager.get_selected_session() {
                if let Some(prefix) = session.prefix.clone() {
                    if app.sub_agent_contexts.contains_key(&prefix) {
                        app.expanded_sub_agent = Some(prefix.clone());
                        app.expanded_sub_agent_before_alt_w = None;
                        app.mode_before_sub_agent = None;
                        app.mode = Mode::Normal;
                        app.cached_mode_content = None;
                    } else {
                        app.status_message = Some(format!("No activity yet for: {}", session.name));
                    }
                } else {
                    app.expanded_sub_agent = None;
                    app.expanded_sub_agent_before_alt_w = None;
                    app.leave_alt_w_view();
                    app.cached_mode_content = None;
                }
            }
        }
        KeyCode::Char('d') => {
            let session_info = app
                .session_manager
                .get_selected_session()
                .map(|s| (s.name.clone(), s.group.clone()));
            if let Some((name, group)) = session_info {
                if group.as_deref() == Some("orchestrator") {
                    app.status_message = Some("Cannot detach orchestrator sessions".to_string());
                } else {
                    app.session_manager.toggle_detach();
                    let badge = app
                        .session_manager
                        .get_selected_status_badge()
                        .unwrap_or("");
                    app.status_message = Some(format!("Session {} {}", name, badge));
                }
            }
        }
        KeyCode::Char('x') => {
            let is_orchestrator = app
                .session_manager
                .get_selected_session()
                .map(|s| s.group.as_deref() == Some("orchestrator"))
                .unwrap_or(false);
            if is_orchestrator {
                app.status_message = Some("Cannot kill orchestrator sessions".to_string());
            } else if let Some(name) = app.session_manager.kill_selected() {
                app.status_message = Some(format!("Killed session: {}", name));
            }
        }
        KeyCode::Esc => {
            if app.expanded_sub_agent.is_some() {
                app.expanded_sub_agent = None;
            }
        }
        _ => {}
    }
}
