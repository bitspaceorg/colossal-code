use std::time::Instant;

use agent_core::AgentMessage;
use color_eyre::Result;
use edtui::clipboard::ClipboardTrait;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::{App, AppSnapshot, MessageState, MessageType, Mode, Phase, UiMessageEvent};

impl App {
    pub(crate) fn handle_runtime_event(&mut self, runtime_event: Event) -> Result<()> {
        match runtime_event {
            Event::Paste(data)
                if self.phase == Phase::Input
                    && self.mode == Mode::Normal
                    && !self.show_background_tasks
                    && !self.ui_state.show_help
                    && self.viewing_task.is_none() =>
            {
                self.handle_runtime_paste(data);
            }
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.handle_runtime_key(key);
            }
            _ => {}
        }

        Ok(())
    }

    fn handle_runtime_paste(&mut self, data: String) {
        if self.vim_mode_enabled {
            let current_text = self.vim_input_editor.get_text_content();
            let cursor = self.vim_input_editor.state.cursor;

            let lines: Vec<&str> = current_text.lines().collect();
            let mut byte_pos = 0;
            for (row_idx, line) in lines.iter().enumerate() {
                if row_idx < cursor.row {
                    byte_pos += line.len() + 1;
                } else if row_idx == cursor.row {
                    byte_pos += cursor.col.min(line.len());
                    break;
                }
            }

            let mut new_text = current_text;
            new_text.insert_str(byte_pos, &data);
            self.vim_input_editor
                .set_text_content_preserving_mode(&new_text);

            let new_byte_pos = byte_pos + data.len();
            let lines: Vec<&str> = new_text.lines().collect();
            let mut remaining = new_byte_pos;
            let mut new_row = 0;
            let mut new_col = 0;
            for (row_idx, line) in lines.iter().enumerate() {
                let line_len = line.len();
                if remaining <= line_len {
                    new_row = row_idx;
                    new_col = remaining;
                    break;
                }
                remaining = remaining.saturating_sub(line_len + 1);
                new_row = row_idx + 1;
            }

            self.vim_input_editor.state.cursor.row = new_row;
            self.vim_input_editor.state.cursor.col = new_col;
            self.sync_vim_input();
        } else {
            let index = self.byte_index();
            self.input.insert_str(index, &data);
            self.character_index += data.chars().count();
            self.input_modified = true;
            self.update_autocomplete();
        }
    }

    fn handle_runtime_key(&mut self, key: KeyEvent) {
        if std::env::var("NITE_DEBUG_KEYS").ok().as_deref() == Some("1") {
            eprintln!(
                "[NITE KEY] code={:?} modifiers={:?} kind={:?} state={:?}",
                key.code, key.modifiers, key.kind, key.state
            );
        }

        match self.mode {
            Mode::Normal => self.handle_runtime_key_normal(key),
            Mode::Navigation | Mode::Visual | Mode::Search => {
                self.handle_runtime_key_navigation_visual_search(key)
            }
            Mode::Command => self.handle_runtime_key_command(key),
            Mode::SessionWindow => self.handle_runtime_key_session_window(key),
        }
    }

    fn handle_runtime_key_normal(&mut self, key: KeyEvent) {
        if self.dispatch_panel_key_from_runtime(key) {
            return;
        }

        if self.vim_mode_enabled && key.code == KeyCode::Esc {
            let vim_mode = self.vim_input_editor.get_mode();
            let is_in_normal_mode = matches!(vim_mode, edtui::EditorMode::Normal);
            if !is_in_normal_mode {
                self.vim_input_editor.handle_event(Event::Key(key));
                self.sync_vim_input();
                return;
            }
        }

        if key.code == KeyCode::Esc
            && (self.agent_state.agent_processing || self.thinking_indicator_active)
        {
            if let Some((current_summary, token_count, chunk_count)) =
                self.thinking_current_summary.take()
            {
                if let Some(last_msg) = self.messages.last()
                    && matches!(
                        UiMessageEvent::parse(last_msg),
                        Some(UiMessageEvent::ThinkingAnimation)
                    )
                {
                    self.messages.pop();
                    self.message_types.pop();
                    if !self.message_states.is_empty() {
                        self.message_states.pop();
                    }
                    self.thinking_indicator_active = false;
                }

                self.messages.push(Self::format_thinking_tree_line(
                    current_summary,
                    token_count,
                    chunk_count,
                    true,
                ));
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            } else if let Some(last_msg) = self.messages.last()
                && matches!(
                    UiMessageEvent::parse(last_msg),
                    Some(UiMessageEvent::ThinkingAnimation)
                )
            {
                self.messages.pop();
                self.message_types.pop();
                if !self.message_states.is_empty() {
                    self.message_states.pop();
                }
                self.thinking_indicator_active = false;
            }

            self.agent_state.agent_interrupted = true;
            if let Some(tx) = &self.agent_tx {
                let _ = tx.send(AgentMessage::Cancel);
            }

            if let Some(last_state) = self.message_states.last_mut()
                && matches!(last_state, MessageState::Queued)
            {
                *last_state = MessageState::Interrupted;
            }

            self.messages.push("● Interrupted".to_string());
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);

            self.messages
                .push(" ⎿ What should Nite do instead?".to_string());
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);

            self.ensure_generation_stats_marker();

            self.is_thinking = false;
            self.thinking_indicator_active = false;
            self.thinking_start_time = None;
            self.thinking_token_count = 0;
            self.thinking_position = 0;
            self.agent_state.agent_processing = false;
            return;
        }

        if self.survey.is_active()
            && let KeyCode::Char(c) = key.code
            && self.try_handle_survey_number_input(c)
        {
            return;
        }

        if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('w') {
            if self.mode == Mode::SessionWindow {
                self.leave_alt_w_view();
            } else {
                self.enter_alt_w_view();
            }
            self.cached_mode_content = None;
            return;
        }

        if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('n') {
            let mut snapshot = None;
            if let Some(prefix) = self.expanded_sub_agent.clone()
                && let Some(context) = self.sub_agent_contexts.get(&prefix)
            {
                snapshot = Some(context.to_snapshot());
            }

            if snapshot.is_none() {
                let elapsed_secs = if let Some(start_time) = self.thinking_start_time {
                    start_time.elapsed().as_secs()
                } else {
                    0
                };

                let (snapshot_messages, snapshot_types) = if self.show_summary_history {
                    let overlay_messages = self.summary_history_virtual_messages();
                    let overlay_types = vec![MessageType::Agent; overlay_messages.len()];
                    (overlay_messages, overlay_types)
                } else {
                    (self.messages.clone(), self.message_types.clone())
                };

                snapshot = Some(AppSnapshot {
                    messages: snapshot_messages,
                    message_types: snapshot_types,
                    thinking_indicator_active: self.thinking_indicator_active,
                    thinking_elapsed_secs: elapsed_secs,
                    thinking_token_count: self.thinking_token_count,
                    thinking_current_summary: self.thinking_current_summary.clone(),
                    thinking_position: self.thinking_position,
                    thinking_loader_frame: self.thinking_loader_frame,
                    thinking_current_word: self.thinking_current_word.clone(),
                    generation_stats: self.generation_stats.clone(),
                });
            }

            self.nav_snapshot = snapshot;
            self.mode = Mode::Navigation;
            self.nav_needs_init = true;
            self.nav_scroll_offset = 0;
            return;
        }

        if self.vim_mode_enabled && self.phase == Phase::Input && !self.show_background_tasks {
            let handled = match key.code {
                KeyCode::Char(c) => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'c' {
                        false
                    } else {
                        self.vim_input_editor.handle_event(Event::Key(key));
                        self.sync_vim_input();
                        self.update_autocomplete();
                        true
                    }
                }
                KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::Left
                | KeyCode::Right => {
                    self.vim_input_editor.handle_event(Event::Key(key));
                    self.sync_vim_input();
                    self.update_autocomplete();
                    true
                }
                _ => false,
            };
            if handled {
                return;
            }
        }

        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.ui_state.show_help {
                    self.ui_state.show_help = false;
                    self.messages.push(" ⎿ help dialog dismissed".to_string());
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                } else if self.viewing_task.is_some() {
                    self.viewing_task = None;
                    self.messages.push(" ⎿ shell viewer dismissed".to_string());
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                } else if self.show_background_tasks {
                    self.show_background_tasks = false;
                    self.messages.push(" ⎿ shells dialog dismissed".to_string());
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                } else if self.ui_state.show_resume {
                    self.ui_state.show_resume = false;
                    self.messages.push(" ⎿ resume dialog dismissed".to_string());
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                } else if self.show_rewind {
                    self.show_rewind = false;
                    self.messages.push(" ⎿ rewind dialog dismissed".to_string());
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                } else if let Some(idx) = self.editing_queue_index.take() {
                    if idx < self.queued_messages.len() {
                        self.queued_messages.remove(idx);
                    }
                    self.input.clear();
                    self.character_index = 0;
                    self.input_modified = false;
                } else if !self.queued_messages.is_empty() && self.input.is_empty() {
                    self.queued_messages.pop();
                } else if self.input.is_empty() {
                    if let Some(last_press) = self.ctrl_c_pressed {
                        if last_press.elapsed().as_millis() < 1000 {
                            self.persistence_state.save_pending = true;
                            self.exit = true;
                        } else {
                            self.ctrl_c_pressed = Some(Instant::now());
                        }
                    } else {
                        self.ctrl_c_pressed = Some(Instant::now());
                    }
                } else {
                    self.input.clear();
                    self.character_index = 0;
                    self.input_modified = false;
                    if self.vim_mode_enabled {
                        self.sync_input_to_vim();
                    }
                }
            }
            KeyCode::Esc if self.phase == Phase::Input && self.viewing_task.is_some() => {
                self.viewing_task = None;
                self.messages.push(" ⎿ shell viewer dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            KeyCode::Enter if self.phase == Phase::Input && self.viewing_task.is_some() => {
                self.viewing_task = None;
                self.messages.push(" ⎿ shell viewer dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            KeyCode::Char(' ') if self.phase == Phase::Input && self.viewing_task.is_some() => {
                self.viewing_task = None;
                self.messages.push(" ⎿ shell viewer dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            KeyCode::Char('k') if self.phase == Phase::Input && self.viewing_task.is_some() => {
                if let Some((session_id, _, _, _)) = self.viewing_task.take() {
                    self.background_tasks
                        .retain(|(sid, _, _, _)| sid != &session_id);
                    std::thread::spawn(move || {
                        let rt = tokio::runtime::Runtime::new().unwrap();
                        rt.block_on(async {
                            let _ = agent_core::kill_shell_session(session_id).await;
                        });
                    });
                }
            }
            KeyCode::Esc if self.phase == Phase::Input && self.show_todos => {
                self.show_todos = false;
                self.messages.push(" ⎿ todos dialog dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            KeyCode::Esc if self.phase == Phase::Input && self.show_background_tasks => {
                self.show_background_tasks = false;
                self.messages.push(" ⎿ shells dialog dismissed".to_string());
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
            }
            KeyCode::Up if self.phase == Phase::Input && self.show_background_tasks => {
                if !self.background_tasks.is_empty() && self.background_tasks_selected > 0 {
                    self.background_tasks_selected -= 1;
                }
            }
            KeyCode::Down if self.phase == Phase::Input && self.show_background_tasks => {
                if !self.background_tasks.is_empty()
                    && self.background_tasks_selected < self.background_tasks.len() - 1
                {
                    self.background_tasks_selected += 1;
                }
            }
            KeyCode::Char('k') if self.phase == Phase::Input && self.show_background_tasks => {
                if !self.background_tasks.is_empty()
                    && self.background_tasks_selected < self.background_tasks.len()
                {
                    let (session_id, _command, _log_file, _start_time) =
                        self.background_tasks.remove(self.background_tasks_selected);
                    if self.background_tasks_selected >= self.background_tasks.len()
                        && self.background_tasks_selected > 0
                    {
                        self.background_tasks_selected -= 1;
                    }
                    std::thread::spawn(move || {
                        let rt = tokio::runtime::Runtime::new().unwrap();
                        rt.block_on(async {
                            let _ = agent_core::kill_shell_session(session_id).await;
                        });
                    });
                }
            }
            KeyCode::Enter if self.phase == Phase::Input && self.show_background_tasks => {
                if !self.background_tasks.is_empty()
                    && self.background_tasks_selected < self.background_tasks.len()
                {
                    let task = &self.background_tasks[self.background_tasks_selected];
                    self.viewing_task =
                        Some((task.0.clone(), task.1.clone(), task.2.clone(), task.3));
                    self.show_background_tasks = false;
                }
            }
            KeyCode::Esc if self.phase == Phase::Input && self.autocomplete_active => {
                self.clear_autocomplete();
            }
            KeyCode::Tab if self.phase == Phase::Input && self.autocomplete_active => {
                self.apply_autocomplete_selection();
            }
            KeyCode::Enter
                if self.phase == Phase::Input
                    && !self.show_background_tasks
                    && self.viewing_task.is_none() =>
            {
                if !self.autocomplete_active || !self.apply_autocomplete_selection() {
                    self.submit_message();
                }
            }
            KeyCode::Char(to_insert)
                if self.phase == Phase::Input && !self.show_background_tasks =>
            {
                self.handle_input_char_key(key, to_insert);
            }
            KeyCode::Backspace if self.phase == Phase::Input && !self.show_background_tasks => {
                self.handle_input_backspace_key(key);
            }
            KeyCode::Left if self.phase == Phase::Input && !self.show_background_tasks => {
                if !self.vim_mode_enabled {
                    self.move_cursor_left();
                }
            }
            KeyCode::Right if self.phase == Phase::Input && !self.show_background_tasks => {
                if !self.vim_mode_enabled {
                    self.move_cursor_right();
                }
            }
            KeyCode::Up if self.phase == Phase::Input => {
                self.handle_input_up_key();
            }
            KeyCode::Down if self.phase == Phase::Input => {
                self.handle_input_down_key();
            }
            _ => {}
        }
    }

    fn handle_runtime_key_navigation_visual_search(&mut self, key: KeyEvent) {
        if self.mode == Mode::Navigation && key.code == KeyCode::Char('q') {
            self.mode = Mode::Normal;
            self.nav_snapshot = None;
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);
            return;
        }

        if self.mode == Mode::Navigation
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('c')
        {
            self.mode = Mode::Normal;
            self.nav_snapshot = None;
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);
            return;
        }

        if self.mode == Mode::Navigation && key.code == KeyCode::Char(':') {
            self.mode = Mode::Command;
            self.command_input.clear();
            self.cached_mode_content = None;
            return;
        }

        let old_clipboard_content = self.editor.state.clip.get_text();
        let old_selection = self.editor.state.selection.clone();
        let old_cursor = self.editor.state.cursor;
        self.editor.handle_event(Event::Key(key));

        let new_clipboard_content = self.editor.state.clip.get_text();
        if new_clipboard_content != old_clipboard_content && !new_clipboard_content.is_empty() {
            if let Some(sel) = old_selection {
                self.flash_highlight = Some((sel, std::time::Instant::now()));
            } else {
                let line_selection = edtui::state::selection::Selection::new(
                    edtui::Index2::new(old_cursor.row, 0),
                    edtui::Index2::new(
                        old_cursor.row,
                        self.editor
                            .state
                            .lines
                            .len_col(old_cursor.row)
                            .unwrap_or(0)
                            .saturating_sub(1),
                    ),
                );
                self.flash_highlight = Some((line_selection, std::time::Instant::now()));
            }
        }

        self.mode = match self.editor.get_mode() {
            edtui::EditorMode::Normal => Mode::Navigation,
            edtui::EditorMode::Visual => Mode::Visual,
            edtui::EditorMode::Search => Mode::Search,
            edtui::EditorMode::Insert => Mode::Navigation,
        };
        self.cached_mode_content = None;
    }

    fn handle_runtime_key_command(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Navigation;
                self.command_input.clear();
                self.cached_mode_content = None;
            }
            KeyCode::Enter => {
                if let Ok(line_num) = self.command_input.trim().parse::<usize>()
                    && line_num > 0
                {
                    let current_col = self.editor.state.cursor.col;
                    let target_row = line_num.saturating_sub(1);
                    let max_row = self.editor.state.lines.len().saturating_sub(1);
                    self.editor.state.cursor.row = target_row.min(max_row);
                    let line_len = self
                        .editor
                        .state
                        .lines
                        .len_col(self.editor.state.cursor.row)
                        .unwrap_or(0);
                    self.editor.state.cursor.col =
                        current_col.min(line_len.saturating_sub(1).max(0));
                }
                self.mode = Mode::Navigation;
                self.command_input.clear();
                self.cached_mode_content = None;
            }
            KeyCode::Char(c) => {
                self.command_input.push(c);
                self.cached_mode_content = None;
            }
            KeyCode::Backspace => {
                self.command_input.pop();
                self.cached_mode_content = None;
            }
            _ => {}
        }
    }

    fn handle_runtime_key_session_window(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => {
                self.leave_alt_w_view();
                self.cached_mode_content = None;
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.leave_alt_w_view();
                self.cached_mode_content = None;
            }
            KeyCode::Up => {
                self.session_manager.previous_session();
            }
            KeyCode::Down => {
                self.session_manager.next_session();
            }
            KeyCode::Enter => {
                if let Some(session) = self.session_manager.get_selected_session() {
                    if let Some(prefix) = session.prefix.clone() {
                        if self.sub_agent_contexts.contains_key(&prefix) {
                            self.expanded_sub_agent = Some(prefix.clone());
                            self.expanded_sub_agent_before_alt_w = None;
                            self.mode_before_sub_agent = None;
                            self.mode = Mode::Normal;
                            self.cached_mode_content = None;
                        } else {
                            self.status_message =
                                Some(format!("No activity yet for: {}", session.name));
                        }
                    } else {
                        self.expanded_sub_agent = None;
                        self.expanded_sub_agent_before_alt_w = None;
                        self.leave_alt_w_view();
                        self.cached_mode_content = None;
                    }
                }
            }
            KeyCode::Char('d') => {
                let session_info = self
                    .session_manager
                    .get_selected_session()
                    .map(|s| (s.name.clone(), s.group.clone()));
                if let Some((name, group)) = session_info {
                    if group.as_deref() == Some("orchestrator") {
                        self.status_message =
                            Some("Cannot detach orchestrator sessions".to_string());
                    } else {
                        self.session_manager.toggle_detach();
                        let badge = self
                            .session_manager
                            .get_selected_status_badge()
                            .unwrap_or("");
                        self.status_message = Some(format!("Session {} {}", name, badge));
                    }
                }
            }
            KeyCode::Char('x') => {
                let is_orchestrator = self
                    .session_manager
                    .get_selected_session()
                    .map(|s| s.group.as_deref() == Some("orchestrator"))
                    .unwrap_or(false);
                if is_orchestrator {
                    self.status_message = Some("Cannot kill orchestrator sessions".to_string());
                } else if let Some(name) = self.session_manager.kill_selected() {
                    self.status_message = Some(format!("Killed session: {}", name));
                }
            }
            KeyCode::Esc => {
                if self.expanded_sub_agent.is_some() {
                    self.expanded_sub_agent = None;
                }
            }
            _ => {}
        }
    }
}
