use jagged::index::RowIndex;

use crate::actions::motion::CharacterClass;
use crate::{EditorMode, EditorState, Index2};

use super::Execute;

/// Command to append a single character to the search buffer and trigger a search.
#[derive(Clone, Debug, Copy)]
pub struct AppendCharToSearch(pub char);

impl Execute for AppendCharToSearch {
    /// Executes the command, appending the specified character to the search buffer
    /// and triggering a search based on the updated buffer.
    fn execute(&mut self, state: &mut EditorState) {
        state.search.push_char(self.0);
        state.search.trigger_search(&state.lines);
        if let Some(index) = state.search.find_first() {
            state.cursor = *index;
        }
    }
}

/// Command to remove the last character from the search buffer and trigger a search.
#[derive(Clone, Debug, Copy)]
pub struct RemoveCharFromSearch;

impl Execute for RemoveCharFromSearch {
    /// Executes the command, removing the last character from the search buffer
    /// and triggering a search based on the updated buffer.
    fn execute(&mut self, state: &mut EditorState) {
        state.search.remove_char();
        state.search.trigger_search(&state.lines);
    }
}

/// Command to find the first match of the search pattern behind the last cursor position.
#[derive(Clone, Debug)]
pub struct TriggerSearch;

impl Execute for TriggerSearch {
    /// Executes the command, finding the first match of the search pattern behind
    /// the last cursor position and setting the cursor to the found match.
    /// Switches to normal mode.
    fn execute(&mut self, state: &mut EditorState) {
        state.mode = EditorMode::Normal;
        if let Some(index) = state.search.find_first() {
            state.cursor = *index;
        }
    }
}

/// Command to find the next search match and update the cursor position.
#[derive(Clone, Debug)]
pub struct FindNext;

impl Execute for FindNext {
    /// Executes the command, finding the next search match and updating the cursor position.
    /// Switches to normal mode.
    fn execute(&mut self, state: &mut EditorState) {
        state.mode = EditorMode::Normal;
        if let Some(index) = state.search.find_next() {
            state.cursor = *index;
        }
    }
}

/// Command to find the previous search match and update the cursor position.
#[derive(Clone, Debug)]
pub struct FindPrevious;

impl Execute for FindPrevious {
    /// Executes the command, finding the previous search match and updating the cursor position.
    /// Switches to normal mode.
    fn execute(&mut self, state: &mut EditorState) {
        state.mode = EditorMode::Normal;
        if let Some(index) = state.search.find_previous() {
            state.cursor = *index;
        }
    }
}

/// Command to clear to start of the search and switch into search mode.
#[derive(Clone, Debug)]
pub struct StartSearch;

impl Execute for StartSearch {
    /// Executes the command, starting the search state and switching to search mode.
    fn execute(&mut self, state: &mut EditorState) {
        state.mode = EditorMode::Search;
        state.search.start(state.cursor);
    }
}
/// Command to clear the search state and switch to normal mode.
#[derive(Clone, Debug)]
pub struct StopSearch;

impl Execute for StopSearch {
    /// Executes the command, clearing the search pattern (but keeping matches highlighted) and switching to normal mode.
    fn execute(&mut self, state: &mut EditorState) {
        state.mode = EditorMode::Normal;
        state.search.clear_pattern();
        state.cursor = state.search.start_cursor;
    }
}

/// Search for the word under the cursor (* forward, # backward in vim)
#[derive(Clone, Debug)]
pub struct SearchWordUnderCursor {
    /// If true, search forward (like *); if false, search backward (like #)
    pub forward: bool,
}

impl Execute for SearchWordUnderCursor {
    fn execute(&mut self, state: &mut EditorState) {
        let row_index = state.cursor.row;
        let Some(line) = state.lines.get(RowIndex::new(row_index)) else {
            return;
        };

        let Some(len_col) = state.lines.len_col(state.cursor.row) else {
            return;
        };

        if len_col == 0 {
            return;
        }

        let col = state.cursor.col.min(len_col.saturating_sub(1));
        let start_char_class = CharacterClass::from(line.get(col));

        // Don't search on whitespace or punctuation that doesn't form a "word"
        if start_char_class != CharacterClass::Alphanumeric {
            return;
        }

        // Find word start
        let mut word_start = col;
        for c in (0..col).rev() {
            if CharacterClass::from(line.get(c)) != start_char_class {
                break;
            }
            word_start = c;
        }

        // Find word end
        let mut word_end = col;
        for c in (col + 1)..len_col {
            if CharacterClass::from(line.get(c)) != start_char_class {
                break;
            }
            word_end = c;
        }

        // Extract word
        let word: String = line[word_start..=word_end].iter().collect();

        // Start a fresh search anchored at the current cursor.
        state.search.start(state.cursor);
        state.search.pattern = word;
        state.search.trigger_search(&state.lines);

        let current_match = Index2::new(row_index, word_start);

        // Find the next/previous whole-word match relative to the current one.
        if self.forward {
            let selected = state
                .search
                .matches
                .iter()
                .position(|index| index > &current_match)
                .unwrap_or(0);
            state.search.selected_index = Some(selected);
            state.cursor = state.search.matches[selected];
        } else {
            let selected = state
                .search
                .matches
                .iter()
                .rposition(|index| index < &current_match)
                .unwrap_or_else(|| state.search.matches.len().saturating_sub(1));
            state.search.selected_index = Some(selected);
            state.cursor = state.search.matches[selected];
        }

        state.mode = EditorMode::Normal;
    }
}

#[cfg(test)]
mod tests {
    use crate::{Index2, Lines};

    use super::*;

    #[test]
    fn test_search_word_under_cursor_forward_skips_current_match() {
        let mut state = EditorState::new(Lines::from("foo bar foo"));
        state.cursor = Index2::new(0, 1);

        SearchWordUnderCursor { forward: true }.execute(&mut state);

        assert_eq!(state.search_pattern(), "foo");
        assert_eq!(state.cursor, Index2::new(0, 8));
    }

    #[test]
    fn test_search_word_under_cursor_backward_finds_previous_match() {
        let mut state = EditorState::new(Lines::from("foo bar foo"));
        state.cursor = Index2::new(0, 9);

        SearchWordUnderCursor { forward: false }.execute(&mut state);

        assert_eq!(state.search_pattern(), "foo");
        assert_eq!(state.cursor, Index2::new(0, 0));
    }
}
