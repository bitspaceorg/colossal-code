//! The editors state
pub mod mode;
mod search;
pub mod selection;
mod undo;
mod view;

use self::search::SearchState;
use self::view::ViewState;
use self::{mode::EditorMode, selection::Selection, undo::Stack};
use crate::clipboard::{Clipboard, ClipboardTrait};
use crate::helper::max_col;
use crate::{Index2, Lines};

/// Represents the state of an editor.
#[derive(Clone)]
pub struct EditorState {
    /// The text in the editor.
    pub lines: Lines,

    /// The current cursor position in the editor.
    pub cursor: Index2,

    /// The mode of the editor (insert, visual or normal mode).
    pub mode: EditorMode,

    /// Represents the selection in the editor, if any.
    pub selection: Option<Selection>,

    /// Internal view state of the editor.
    pub view: ViewState,

    /// State holding the search results in search mode.
    pub search: SearchState,

    /// Stack for undo operations.
    pub undo: Stack,

    /// Stack for redo operations.
    pub redo: Stack,

    /// Clipboard for yank and paste operations.
    pub clip: Clipboard,

    /// Desired column for vertical movement. None means "end of line".
    pub(crate) desired_col: Option<usize>,
}

impl Default for EditorState {
    /// Creates a default `EditorState` with no text.
    fn default() -> Self {
        EditorState::new(Lines::default())
    }
}

impl EditorState {
    /// Creates a new editor state.
    ///
    /// # Example
    ///
    /// ```
    /// use edtui::{EditorState, Lines};
    ///
    /// let state = EditorState::new(Lines::from("First line\nSecond Line"));
    /// ```
    #[must_use]
    pub fn new(lines: Lines) -> EditorState {
        EditorState {
            lines,
            cursor: Index2::new(0, 0),
            mode: EditorMode::Normal,
            selection: None,
            view: ViewState::default(),
            search: SearchState::default(),
            undo: Stack::new(),
            redo: Stack::new(),
            clip: Clipboard::default(),
            desired_col: Some(0),
        }
    }

    /// Set a custom clipboard.
    pub fn set_clipboard(&mut self, clipboard: impl ClipboardTrait + 'static) {
        self.clip = Clipboard::new(clipboard);
    }

    /// Returns the current search pattern.
    #[must_use]
    pub fn search_pattern(&self) -> String {
        self.search.pattern.clone()
    }

    /// Sets the number of visible rows in the viewport.
    /// This is used for page up/down navigation (Ctrl+u/Ctrl+d).
    pub fn set_viewport_rows(&mut self, num_rows: usize) {
        self.view.num_rows = num_rows;
    }

    /// Returns the number of visible viewport rows.
    #[must_use]
    pub fn viewport_rows(&self) -> usize {
        self.view.num_rows
    }

    /// Sets the current vertical viewport offset.
    /// This lets external renderers keep screen-relative motions in sync.
    pub fn set_viewport_offset_y(&mut self, offset_y: usize) {
        self.view.viewport.y = offset_y;
    }

    /// Returns the current vertical viewport offset.
    #[must_use]
    pub fn viewport_offset_y(&self) -> usize {
        self.view.viewport.y
    }

    /// Returns the search matches.
    #[must_use]
    pub fn search_matches(&self) -> &[Index2] {
        &self.search.matches
    }

    /// Returns the selected search match index.
    #[must_use]
    pub fn search_selected_index(&self) -> Option<usize> {
        self.search.selected_index
    }

    /// Returns the cached pattern length for search highlighting.
    #[must_use]
    pub fn search_pattern_len(&self) -> usize {
        self.search.cached_pattern_len
    }

    /// Returns the desired column for vertical movement.
    #[must_use]
    pub fn desired_col(&self) -> Option<usize> {
        self.desired_col
    }

    /// Sets the desired column for vertical movement.
    pub fn set_desired_col(&mut self, col: Option<usize>) {
        self.desired_col = col;
    }

    /// Clamps the column of the cursor if the cursor is out of bounds.
    /// In normal or visual mode, clamps on `col = len() - 1`, in insert
    /// mode on `col = len()`.
    pub(crate) fn clamp_column(&mut self) {
        let max_col = max_col(&self.lines, &self.cursor, self.mode);
        self.cursor.col = self.cursor.col.min(max_col);
    }
}
