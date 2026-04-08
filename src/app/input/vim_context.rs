//! Shared vim key processing trait and processor.
//!
//! Both nav mode (read-only message viewer) and the input bar (read-write editor)
//! use edtui for vim-style key handling. This module defines the shared contract
//! (`VimKeyContext`) and the unified key processor (`process_vim_key`) so that
//! every vim motion, command, and multi-key sequence is available in both contexts
//! without duplicating routing logic.

use edtui::Index2;
use edtui::clipboard::ClipboardTrait;
use edtui::state::selection::Selection;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use super::vim_sync::RichEditor;

// ---------------------------------------------------------------------------
// Result enum – tells the caller what happened so it can apply side effects
// ---------------------------------------------------------------------------

/// Outcome of a vim key press processed by the shared processor.
pub(crate) enum VimKeyResult {
    /// Key was consumed by the vim engine – nothing special to report.
    Handled,
    /// Key was not a vim key – caller should handle it (Enter, Ctrl+C, etc.).
    Unhandled,
    /// The edtui editor mode changed (e.g. Normal→Visual, Normal→Search).
    ModeChanged(edtui::EditorMode),
    /// A yank/copy occurred – the clipboard content changed.
    ClipboardChanged {
        old_cursor: Index2,
        old_selection: Option<Selection>,
    },
    /// A viewport scroll was requested via `zz`, `zt`, or `zb`.
    ViewportScroll(ViewportScrollKind),
    /// The user pressed `q` to exit the vim context.
    ExitRequested,
    /// The user pressed `:` to enter command mode.
    CommandRequested,
}

/// Viewport scroll kinds from `z` commands.
#[derive(Clone, Copy)]
pub(crate) enum ViewportScrollKind {
    Center,
    Top,
    Bottom,
}

// ---------------------------------------------------------------------------
// Trait – the behavioral contract that each vim context must implement
// ---------------------------------------------------------------------------

/// Defines the behavioral contract for a vim-enabled editing context.
///
/// Both the nav mode (read-only) and input bar (read-write) implement this
/// trait so the shared `process_vim_key` processor can drive them uniformly.
pub(crate) trait VimKeyContext {
    /// Returns a mutable reference to the underlying edtui editor.
    fn editor(&mut self) -> &mut RichEditor;

    /// Whether this context is read-only (nav mode = true, input bar = false).
    fn is_read_only(&self) -> bool;

    /// Whether the `q` key should exit this vim context.
    fn exit_on_q(&self) -> bool;

    /// Whether `:` should enter command mode.
    fn command_on_colon(&self) -> bool;

    /// Whether `zz`/`zt`/`zb` viewport scroll commands are supported.
    fn supports_viewport_scroll(&self) -> bool;

    /// Whether `/` search mode is supported. Nav mode supports it; the input
    /// bar does not, so `/` in Normal mode should be swallowed there.
    fn supports_search(&self) -> bool;
}

// ---------------------------------------------------------------------------
// Shared processor – owns pending-z state, drives edtui, returns result
// ---------------------------------------------------------------------------

/// Persistent state for the shared vim key processor.
///
/// Lives on `App` and is shared by both nav and input bar contexts.
/// Owns multi-key sequence state that transcends a single key press.
pub(crate) struct VimKeyProcessor {
    /// Pending `z` viewport command (`zz`, `zt`, `zb`).
    pending_z: bool,
}

impl Default for VimKeyProcessor {
    fn default() -> Self {
        Self { pending_z: false }
    }
}

impl VimKeyProcessor {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Reset any pending multi-key state (e.g. when exiting a mode).
    pub(crate) fn reset(&mut self) {
        self.pending_z = false;
    }

    /// Returns whether a `z` command is pending.
    pub(crate) fn is_pending_z(&self) -> bool {
        self.pending_z
    }

    /// Process a key event through the shared vim pipeline.
    ///
    /// This is the single entry point for all vim key handling. It:
    /// 1. Checks for exit keys (`q`, `Ctrl+C`) if the context supports them
    /// 2. Checks for command mode entry (`:`)
    /// 3. Handles pending `z` viewport commands
    /// 4. Forwards the key to edtui
    /// 5. Detects clipboard changes (yank operations)
    /// 6. Detects mode changes
    /// 7. Returns a `VimKeyResult` so the caller can apply side effects
    pub(crate) fn process_key<C: VimKeyContext>(
        &mut self,
        ctx: &mut C,
        key: KeyEvent,
    ) -> VimKeyResult {
        let editor_mode = ctx.editor().get_mode();
        let is_search = matches!(editor_mode, edtui::EditorMode::Search);

        // --- Exit keys (q, Ctrl+C) ---
        if ctx.exit_on_q() && !is_search {
            if key.code == KeyCode::Char('q') {
                self.reset();
                return VimKeyResult::ExitRequested;
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                self.reset();
                return VimKeyResult::ExitRequested;
            }
        }

        // --- Command mode entry ---
        if ctx.command_on_colon() && !is_search && key.code == KeyCode::Char(':') {
            return VimKeyResult::CommandRequested;
        }

        // --- Pending z viewport commands ---
        if ctx.supports_viewport_scroll() && !is_search {
            if self.pending_z {
                self.pending_z = false;
                let scroll_kind = match key.code {
                    KeyCode::Char('z') => Some(ViewportScrollKind::Center),
                    KeyCode::Char('t') => Some(ViewportScrollKind::Top),
                    KeyCode::Char('b') => Some(ViewportScrollKind::Bottom),
                    _ => None, // Not a valid z-command follow-up, fall through
                };
                if let Some(kind) = scroll_kind {
                    return VimKeyResult::ViewportScroll(kind);
                }
                // Invalid follow-up – fall through to normal processing
            }

            if key.code == KeyCode::Char('z')
                && !is_search
                && matches!(
                    editor_mode,
                    edtui::EditorMode::Normal | edtui::EditorMode::Visual
                )
            {
                self.pending_z = true;
                return VimKeyResult::Handled;
            }
        }

        // --- Keys that should NOT be forwarded to edtui ---
        // These are app-level keys that the caller handles directly.
        match key.code {
            KeyCode::Enter if !is_search => return VimKeyResult::Unhandled,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return VimKeyResult::Unhandled;
            }
            // Block `/` in Normal mode when the context doesn't support search
            // (e.g. the input bar). Without this, edtui enters Search mode
            // which the input bar can't render or exit properly.
            KeyCode::Char('/')
                if !ctx.supports_search() && matches!(editor_mode, edtui::EditorMode::Normal) =>
            {
                return VimKeyResult::Handled;
            }
            _ => {}
        }

        // --- Forward to edtui ---
        let old_clipboard = ctx.editor().state.clip.get_text();
        let old_selection = ctx.editor().state.selection.clone();
        let old_cursor = ctx.editor().state.cursor;
        let old_mode = ctx.editor().get_mode();

        ctx.editor().handle_event(Event::Key(key));

        let new_mode = ctx.editor().get_mode();
        let new_clipboard = ctx.editor().state.clip.get_text();

        // If read-only context entered Insert mode, force back to Normal
        if ctx.is_read_only() && new_mode == edtui::EditorMode::Insert {
            ctx.editor().state.mode = edtui::EditorMode::Normal;
        }

        // --- Detect clipboard changes (yank) ---
        if new_clipboard != old_clipboard && !new_clipboard.is_empty() {
            return VimKeyResult::ClipboardChanged {
                old_cursor,
                old_selection,
            };
        }

        // --- Detect mode changes ---
        let final_mode = ctx.editor().get_mode();
        if final_mode != old_mode {
            return VimKeyResult::ModeChanged(final_mode);
        }

        VimKeyResult::Handled
    }
}
