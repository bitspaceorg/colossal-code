use std::time::Instant;

use agent_core::AgentMessage;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use crate::app::input::vim_context::{VimKeyContext, VimKeyResult};
use crate::app::input::vim_sync::RichEditor;
use crate::app::{App, AppSnapshot, MessageState, MessageType, Mode, Phase};

// ---------------------------------------------------------------------------
// Input-bar context – implements VimKeyContext for the read-write input editor
// ---------------------------------------------------------------------------

struct InputBarContext<'a> {
    editor: &'a mut RichEditor,
}

impl VimKeyContext for InputBarContext<'_> {
    fn editor(&mut self) -> &mut RichEditor {
        self.editor
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn exit_on_q(&self) -> bool {
        false
    }

    fn command_on_colon(&self) -> bool {
        false
    }

    fn supports_viewport_scroll(&self) -> bool {
        false // Input bar has no message viewport to scroll
    }

    fn supports_search(&self) -> bool {
        false // Input bar doesn't support vim search mode
    }
}

pub(crate) fn handle_runtime_key_normal(app: &mut App, key: KeyEvent) {
    if app.dispatch_panel_key_from_runtime(key) {
        return;
    }

    if app.vim_mode_enabled && key.code == KeyCode::Esc {
        let vim_mode = app.vim_input_editor.get_mode();
        let is_in_normal_mode = matches!(vim_mode, edtui::EditorMode::Normal);
        if !is_in_normal_mode {
            app.vim_input_editor.handle_event(Event::Key(key));
            app.sync_vim_input();
            return;
        }
    }

    if key.code == KeyCode::Esc
        && !app.show_background_tasks
        && app.viewing_task.is_none()
        && !app.show_todos
        && (app.agent_state.agent_processing || app.thinking_indicator_active)
    {
        let removed_thinking =
            App::remove_thinking_animation_placeholder(&mut app.messages, &mut app.message_types);
        if removed_thinking && !app.message_states.is_empty() {
            app.message_states.pop();
        }

        if let Some((current_summary, token_count, chunk_count)) =
            app.thinking_current_summary.take()
        {
            app.messages.push(App::format_thinking_tree_line(
                current_summary,
                token_count,
                chunk_count,
                true,
            ));
            app.message_types.push(MessageType::Agent);
            app.message_states.push(MessageState::Sent);
        }

        app.agent_state.agent_interrupted = true;
        if let Some(tx) = &app.agent_tx {
            let _ = tx.send(AgentMessage::Cancel);
        }

        if let Some(last_state) = app.message_states.last_mut()
            && matches!(last_state, MessageState::Queued)
        {
            *last_state = MessageState::Interrupted;
        }

        app.messages.push("● Interrupted".to_string());
        app.message_types.push(MessageType::Agent);
        app.message_states.push(MessageState::Sent);

        app.messages
            .push(" ⎿ What should Nite do instead?".to_string());
        app.message_types.push(MessageType::Agent);
        app.message_states.push(MessageState::Sent);

        app.safety_state.show_approval_prompt = false;
        app.safety_state.approval_prompt_content.clear();
        app.safety_state.show_sandbox_prompt = false;
        app.safety_state.sandbox_blocked_path.clear();

        app.is_thinking = false;
        app.thinking_indicator_active = false;
        app.thinking_start_time = None;
        app.thinking_token_count = 0;
        app.thinking_position = 0;
        app.agent_state.agent_processing = false;
        return;
    }

    if app.survey.is_active()
        && let KeyCode::Char(c) = key.code
        && app.try_handle_survey_number_input(c)
    {
        return;
    }

    if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('w') {
        if app.mode == Mode::SessionWindow {
            app.leave_alt_w_view();
        } else {
            app.enter_alt_w_view();
        }
        app.cached_mode_content = None;
        return;
    }

    if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('n') {
        let mut snapshot = None;
        if let Some(prefix) = app.expanded_sub_agent.clone()
            && let Some(context) = app.sub_agent_contexts.get(&prefix)
        {
            snapshot = Some(context.to_snapshot());
        }

        if snapshot.is_none() {
            let elapsed_secs = if let Some(start_time) = app.thinking_start_time {
                start_time.elapsed().as_secs()
            } else {
                0
            };

            let (snapshot_messages, snapshot_types) = if app.show_summary_history {
                let overlay_messages = app.summary_history_virtual_messages();
                let overlay_types = vec![MessageType::Agent; overlay_messages.len()];
                (overlay_messages, overlay_types)
            } else {
                (app.messages.clone(), app.message_types.clone())
            };

            snapshot = Some(AppSnapshot {
                messages: snapshot_messages,
                message_types: snapshot_types,
                thinking_indicator_active: app.thinking_indicator_active,
                thinking_elapsed_secs: elapsed_secs,
                thinking_token_count: app.thinking_token_count,
                thinking_current_summary: app.thinking_current_summary.clone(),
                thinking_position: app.thinking_position,
                thinking_loader_frame: app.thinking_loader_frame,
                thinking_current_word: app.thinking_current_word.clone(),
                generation_stats: app.generation_stats.clone(),
            });
        }

        app.nav_snapshot = snapshot;
        app.mode = Mode::Navigation;
        app.nav_needs_init = true;
        app.nav_scroll_offset = 0;
        app.vim_processor.reset();
        return;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char('r')
        && app.expand_visible_edit_file_diff()
    {
        return;
    }

    let prompt_active = app.show_queue_choice
        || app.safety_state.show_approval_prompt
        || app.safety_state.show_sandbox_prompt;

    let vim_insert_mode = app.vim_mode_enabled
        && matches!(app.vim_input_editor.get_mode(), edtui::EditorMode::Insert);

    // --- Shared vim processor for the input bar ---
    // When vim mode is active and the editor is in Normal/Visual/Search mode,
    // route ALL keys through the shared processor so every vim motion,
    // command, and multi-key sequence works identically to nav mode.
    if app.vim_mode_enabled
        && app.phase == Phase::Input
        && !app.show_background_tasks
        && (!prompt_active || !vim_insert_mode)
    {
        let vim_mode = app.vim_input_editor.get_mode();

        // In Insert mode, only forward specific keys to edtui;
        // the rest fall through to the main match for app-level handling.
        if vim_mode == edtui::EditorMode::Insert {
            let handled = match key.code {
                KeyCode::Char(c) => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'c' {
                        false
                    } else {
                        app.vim_input_editor.handle_event(Event::Key(key));
                        app.sync_vim_input();
                        app.update_autocomplete();
                        true
                    }
                }
                KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::Left
                | KeyCode::Right => {
                    app.vim_input_editor.handle_event(Event::Key(key));
                    app.sync_vim_input();
                    app.update_autocomplete();
                    true
                }
                _ => false,
            };
            if handled {
                return;
            }
        } else {
            // Normal, Visual, or Search mode – use the shared processor
            let mut ctx = InputBarContext {
                editor: &mut app.vim_input_editor,
            };
            let result = app.vim_processor.process_key(&mut ctx, key);

            match result {
                VimKeyResult::Handled | VimKeyResult::ModeChanged(_) => {
                    app.sync_vim_input();
                    app.update_autocomplete();
                    return;
                }
                VimKeyResult::ClipboardChanged { .. } => {
                    app.sync_vim_input();
                    app.update_autocomplete();
                    return;
                }
                VimKeyResult::ViewportScroll(_) => {
                    // Input bar doesn't support viewport scroll, but
                    // the processor won't emit this (supports_viewport_scroll=false)
                    return;
                }
                VimKeyResult::ExitRequested | VimKeyResult::CommandRequested => {
                    // Input bar doesn't support these
                    return;
                }
                VimKeyResult::Unhandled => {
                    // Fall through to the main match block
                }
            }
        }
    }

    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.ui_state.show_help {
                app.ui_state.show_help = false;
                push_agent_notice(app, " ⎿ help dialog dismissed");
            } else if app.viewing_task.is_some() {
                dismiss_task_viewer(app);
            } else if app.show_background_tasks {
                app.show_background_tasks = false;
                push_agent_notice(app, " ⎿ shells dialog dismissed");
            } else if app.ui_state.show_resume {
                app.ui_state.show_resume = false;
                push_agent_notice(app, " ⎿ resume dialog dismissed");
            } else if app.show_rewind {
                app.show_rewind = false;
                push_agent_notice(app, " ⎿ rewind dialog dismissed");
            } else if let Some(idx) = app.editing_queue_index.take() {
                if idx < app.queued_messages.len() {
                    app.queued_messages.remove(idx);
                }
                app.input.clear();
                app.character_index = 0;
                app.input_modified = false;
            } else if !app.queued_messages.is_empty() && app.input.is_empty() {
                app.queued_messages.pop();
            } else if app.input.is_empty() {
                if let Some(last_press) = app.ctrl_c_pressed {
                    if last_press.elapsed().as_millis() < 1000 {
                        app.persistence_state.save_pending = true;
                        app.exit = true;
                    } else {
                        app.ctrl_c_pressed = Some(Instant::now());
                    }
                } else {
                    app.ctrl_c_pressed = Some(Instant::now());
                }
            } else {
                app.input.clear();
                app.character_index = 0;
                app.input_modified = false;
                if app.vim_mode_enabled {
                    app.sync_input_to_vim();
                }
            }
        }
        KeyCode::Esc if app.phase == Phase::Input && app.autocomplete_active => {
            app.clear_autocomplete();
        }
        KeyCode::Tab if app.phase == Phase::Input && app.autocomplete_active => {
            app.apply_autocomplete_selection();
        }
        KeyCode::Enter
            if app.phase == Phase::Input
                && !app.show_background_tasks
                && app.viewing_task.is_none() =>
        {
            if !app.autocomplete_active || !app.apply_autocomplete_selection() {
                app.submit_message();
            }
        }
        KeyCode::Char(to_insert) if app.phase == Phase::Input && !app.show_background_tasks => {
            app.handle_input_char_key(key, to_insert);
        }
        KeyCode::Backspace if app.phase == Phase::Input && !app.show_background_tasks => {
            app.handle_input_backspace_key(key);
        }
        KeyCode::Left if app.phase == Phase::Input && !app.show_background_tasks => {
            if !app.vim_mode_enabled {
                app.move_cursor_left();
            }
        }
        KeyCode::Right if app.phase == Phase::Input && !app.show_background_tasks => {
            if !app.vim_mode_enabled {
                app.move_cursor_right();
            }
        }
        KeyCode::Up if app.phase == Phase::Input => {
            app.handle_input_up_key();
        }
        KeyCode::Down if app.phase == Phase::Input => {
            app.handle_input_down_key();
        }
        _ => {}
    }
}

fn push_agent_notice(app: &mut App, message: &str) {
    app.messages.push(message.to_string());
    app.message_types.push(MessageType::Agent);
    app.message_states.push(MessageState::Sent);
}

fn dismiss_task_viewer(app: &mut App) {
    app.viewing_task = None;
    push_agent_notice(app, " ⎿ shell viewer dismissed");
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex as StdMutex, OnceLock};

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::handle_runtime_key_normal;
    use crate::app::{App, Phase};

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
    async fn esc_closes_shells_panel_before_interrupting_generation() {
        let _lock = env_test_lock();
        let _backend = EnvVarGuard::set("NITE_BACKEND_MODE", "none");
        let mut app = App::new().await.expect("create app");
        app.phase = Phase::Input;
        app.connect.show_connect_modal = false;
        app.show_background_tasks = true;
        app.agent_state.agent_processing = true;

        handle_runtime_key_normal(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(!app.show_background_tasks, "Esc should close /shells first");
        assert!(
            !app.agent_state.agent_interrupted,
            "closing /shells should not interrupt generation"
        );
    }
}
