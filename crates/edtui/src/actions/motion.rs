use std::cmp::min;

use crate::{
    helper::{find_matching_bracket, skip_empty_lines},
    state::selection::set_selection,
};
use jagged::Index2;

use super::Execute;
use crate::{
    EditorMode, EditorState,
    helper::{max_col, max_col_normal, skip_whitespace, skip_whitespace_rev},
};

#[derive(Clone, Debug, Copy)]
pub struct MoveForward(pub usize);

impl Execute for MoveForward {
    fn execute(&mut self, state: &mut EditorState) {
        for _ in 0..self.0 {
            if state.cursor.col >= max_col(&state.lines, &state.cursor, state.mode) {
                break;
            }
            state.cursor.col += 1;
        }
        // Reset desired column on horizontal movement
        state.desired_col = Some(state.cursor.col);

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

#[derive(Clone, Debug, Copy)]
pub struct MoveBackward(pub usize);

impl Execute for MoveBackward {
    fn execute(&mut self, state: &mut EditorState) {
        for _ in 0..self.0 {
            if state.cursor.col == 0 {
                break;
            }
            let max_col = max_col(&state.lines, &state.cursor, state.mode);
            if state.cursor.col > max_col {
                state.cursor.col = max_col;
            }
            state.cursor.col = state.cursor.col.saturating_sub(1);
        }
        // Reset desired column on horizontal movement
        state.desired_col = Some(state.cursor.col);

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

#[derive(Clone, Debug, Copy)]
pub struct MoveUp(pub usize);

impl Execute for MoveUp {
    fn execute(&mut self, state: &mut EditorState) {
        for _ in 0..self.0 {
            if state.cursor.row == 0 {
                break;
            }
            state.cursor.row = state.cursor.row.saturating_sub(1);
        }

        // Apply desired column behavior
        if let Some(desired) = state.desired_col {
            let line_len = state.lines.len_col(state.cursor.row).unwrap_or(0);
            state.cursor.col = desired.min(line_len.saturating_sub(1).max(0));
        } else {
            // desired_col is None, go to end of line
            state.cursor.col = max_col(&state.lines, &state.cursor, state.mode);
        }

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

#[derive(Clone, Debug, Copy)]
pub struct MoveDown(pub usize);

impl Execute for MoveDown {
    fn execute(&mut self, state: &mut EditorState) {
        for _ in 0..self.0 {
            if state.cursor.row >= state.lines.len().saturating_sub(1) {
                break;
            }
            state.cursor.row += 1;
        }

        // Apply desired column behavior
        if let Some(desired) = state.desired_col {
            let line_len = state.lines.len_col(state.cursor.row).unwrap_or(0);
            state.cursor.col = desired.min(line_len.saturating_sub(1).max(0));
        } else {
            // desired_col is None, go to end of line
            state.cursor.col = max_col(&state.lines, &state.cursor, state.mode);
        }

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

/// Move one word forward. Breaks on the first character that is not of
/// the same class as the initial character or breaks on line ending.
/// Furthermore, after the first break, whitespaces are skipped.
#[derive(Clone, Debug, Copy)]
pub struct MoveWordForward(pub usize);

impl Execute for MoveWordForward {
    fn execute(&mut self, state: &mut EditorState) {
        if state.lines.is_empty() {
            return;
        }

        state.clamp_column();

        for _ in 0..self.0 {
            move_word_forward(state);
        }

        // Update desired column after word movement
        state.desired_col = Some(state.cursor.col);

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

fn move_word_forward(state: &mut EditorState) {
    let start_index = match (
        state.lines.is_last_col(state.cursor),
        state.lines.is_last_row(state.cursor),
    ) {
        (true, true) => return,
        (true, false) => {
            state.cursor = Index2::new(state.cursor.row.saturating_add(1), 0);
            return;
        }
        _ => Index2::new(state.cursor.row, state.cursor.col.saturating_add(1)),
    };
    let start_char_class = CharacterClass::from(state.lines.get(start_index));

    for (next_char, index) in state.lines.iter().from(start_index) {
        state.cursor = index;
        if CharacterClass::from(next_char) != start_char_class {
            break;
        }
    }

    skip_whitespace(&state.lines, &mut state.cursor);
}

/// Move one word forward to the end of the word.
#[derive(Clone, Debug, Copy)]
pub struct MoveWordForwardToEndOfWord(pub usize);
impl Execute for MoveWordForwardToEndOfWord {
    fn execute(&mut self, state: &mut EditorState) {
        if state.lines.is_empty() {
            return;
        }

        state.clamp_column();

        for _ in 0..self.0 {
            move_word_forward_to_end_of_word(state);
        }

        // Update desired column after word movement
        state.desired_col = Some(state.cursor.col);

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

fn move_word_forward_to_end_of_word(state: &mut EditorState) {
    let mut start_index = match (
        state.lines.is_last_col(state.cursor),
        state.lines.is_last_row(state.cursor),
    ) {
        (true, true) => return,
        (true, false) => Index2::new(state.cursor.row.saturating_add(1), 0),
        _ => Index2::new(state.cursor.row, state.cursor.col.saturating_add(1)),
    };
    skip_empty_lines(&state.lines, &mut start_index.row);
    skip_whitespace(&state.lines, &mut start_index);
    let start_char_class = CharacterClass::from(state.lines.get(start_index));

    for (next_char, index) in state.lines.iter().from(start_index) {
        // Break loop if characters don't belong to the same class
        if CharacterClass::from(next_char) != start_char_class {
            break;
        }
        state.cursor = index;

        // Break loop if it reaches the end of the line
        if state.lines.is_last_col(index) {
            break;
        }
    }
}

/// Move one word forward. Breaks on the first character that is not of
/// the same class as the initial character or breaks on line starts.
/// Skips whitespaces if necessary.
#[derive(Clone, Debug, Copy)]
pub struct MoveWordBackward(pub usize);

impl Execute for MoveWordBackward {
    fn execute(&mut self, state: &mut EditorState) {
        if state.lines.is_empty() {
            return;
        }

        let max_col = max_col(&state.lines, &state.cursor, state.mode);
        if state.cursor.col > max_col {
            state.cursor.col = max_col;
        }

        for _ in 0..self.0 {
            move_word_backward(state);
        }

        // Update desired column after word movement
        state.desired_col = Some(state.cursor.col);

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

fn move_word_backward(state: &mut EditorState) {
    let mut start_index = state.cursor;
    if start_index.row == 0 && start_index.col == 0 {
        return;
    }

    if start_index.col == 0 {
        state.cursor.row = start_index.row.saturating_sub(1);
        state.cursor.col = state.lines.last_col_index(state.cursor.row);
        return;
    }

    start_index.col = start_index.col.saturating_sub(1);
    skip_whitespace_rev(&state.lines, &mut start_index);
    let start_char_class = CharacterClass::from(state.lines.get(start_index));

    for (next_char, i) in state.lines.iter().from(start_index).rev() {
        // Break loop if it reaches the start of the line
        if i.col == 0 {
            start_index = i;
            break;
        }
        // Break loop if characters don't belong to the same class
        if CharacterClass::from(next_char) != start_char_class {
            break;
        }
        start_index = i;
    }

    state.cursor = start_index;
}

// Move the cursor to the start of the line.
#[derive(Clone, Debug, Copy)]
pub struct MoveToStartOfLine();

impl Execute for MoveToStartOfLine {
    fn execute(&mut self, state: &mut EditorState) {
        state.cursor.col = 0;
        // Update desired column on horizontal movement
        state.desired_col = Some(0);

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}
// move to the first non-whitespace character in the line.
#[derive(Clone, Debug, Copy)]
pub struct MoveToFirst();

impl Execute for MoveToFirst {
    fn execute(&mut self, state: &mut EditorState) {
        state.cursor.col = 0;
        skip_whitespace(&state.lines, &mut state.cursor);
        // Update desired column on horizontal movement
        state.desired_col = Some(state.cursor.col);

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

// Move the cursor to the end of the line.
#[derive(Clone, Debug, Copy)]
pub struct MoveToEndOfLine();

impl Execute for MoveToEndOfLine {
    fn execute(&mut self, state: &mut EditorState) {
        state.cursor.col = max_col(&state.lines, &state.cursor, state.mode);
        // Set desired_col to None to stick to end of line
        state.desired_col = None;

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

// Move the cursor to the start of the buffer.
#[derive(Clone, Debug, Copy)]
pub struct MoveToFirstRow();

impl Execute for MoveToFirstRow {
    fn execute(&mut self, state: &mut EditorState) {
        state.cursor.row = 0;

        // Apply desired column behavior
        if let Some(desired) = state.desired_col {
            let line_len = state.lines.len_col(0).unwrap_or(0);
            state.cursor.col = desired.min(line_len.saturating_sub(1).max(0));
        } else {
            // desired_col is None, go to end of line
            state.cursor.col = max_col(&state.lines, &state.cursor, state.mode);
        }

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

// Move the cursor to the end of the buffer.
#[derive(Clone, Debug, Copy)]
pub struct MoveToLastRow();

impl Execute for MoveToLastRow {
    fn execute(&mut self, state: &mut EditorState) {
        let last_row = state.lines.len().saturating_sub(1);
        state.cursor.row = last_row;

        // Apply desired column behavior
        if let Some(desired) = state.desired_col {
            let line_len = state.lines.len_col(last_row).unwrap_or(0);
            state.cursor.col = desired.min(line_len.saturating_sub(1).max(0));
        } else {
            // desired_col is None, go to end of line
            state.cursor.col = max_col(&state.lines, &state.cursor, state.mode);
        }

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

// Move the cursor to the closing bracket.
#[derive(Clone, Debug, Copy)]
pub struct MoveToMatchinBracket();

impl Execute for MoveToMatchinBracket {
    fn execute(&mut self, state: &mut EditorState) {
        let max_col = max_col_normal(&state.lines, &state.cursor);
        let index = Index2::new(state.cursor.row, state.cursor.col.min(max_col));
        if let Some(index) = find_matching_bracket(&state.lines, index) {
            state.cursor = index;
            if state.mode == EditorMode::Visual {
                set_selection(&mut state.selection, state.cursor);
            }
        };
    }
}

#[derive(Clone, Debug, Copy)]
pub struct MoveHalfPageDown();

impl Execute for MoveHalfPageDown {
    fn execute(&mut self, state: &mut EditorState) {
        let jump_rows = state.view.num_rows / 2;
        state.cursor.row = min(state.cursor.row + jump_rows, state.lines.last_row_index());

        // Apply desired column behavior
        if let Some(desired) = state.desired_col {
            let line_len = state.lines.len_col(state.cursor.row).unwrap_or(0);
            state.cursor.col = desired.min(line_len.saturating_sub(1).max(0));
        } else {
            // desired_col is None, go to end of line
            state.cursor.col = max_col(&state.lines, &state.cursor, state.mode);
        }

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

#[derive(Clone, Debug, Copy)]
pub struct MoveHalfPageUp();

impl Execute for MoveHalfPageUp {
    fn execute(&mut self, state: &mut EditorState) {
        let jump_rows = state.view.num_rows / 2;
        state.cursor.row = state.cursor.row.saturating_sub(jump_rows);

        // Apply desired column behavior
        if let Some(desired) = state.desired_col {
            let line_len = state.lines.len_col(state.cursor.row).unwrap_or(0);
            state.cursor.col = desired.min(line_len.saturating_sub(1).max(0));
        } else {
            // desired_col is None, go to end of line
            state.cursor.col = max_col(&state.lines, &state.cursor, state.mode);
        }

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

/// Move a full page down (Ctrl-f in vim)
/// Moves cursor down by num_rows - 2 lines (keeping 2 lines of overlap like vim)
#[derive(Clone, Debug, Copy)]
pub struct MoveFullPageDown();

impl Execute for MoveFullPageDown {
    fn execute(&mut self, state: &mut EditorState) {
        let jump_rows = state.view.num_rows.saturating_sub(2).max(1);
        state.cursor.row = min(state.cursor.row + jump_rows, state.lines.last_row_index());

        // Apply desired column behavior
        if let Some(desired) = state.desired_col {
            let line_len = state.lines.len_col(state.cursor.row).unwrap_or(0);
            state.cursor.col = desired.min(line_len.saturating_sub(1).max(0));
        } else {
            state.cursor.col = max_col(&state.lines, &state.cursor, state.mode);
        }

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

/// Move a full page up (Ctrl-b in vim)
/// Moves cursor up by num_rows - 2 lines (keeping 2 lines of overlap like vim)
#[derive(Clone, Debug, Copy)]
pub struct MoveFullPageUp();

impl Execute for MoveFullPageUp {
    fn execute(&mut self, state: &mut EditorState) {
        let jump_rows = state.view.num_rows.saturating_sub(2).max(1);
        state.cursor.row = state.cursor.row.saturating_sub(jump_rows);

        // Apply desired column behavior
        if let Some(desired) = state.desired_col {
            let line_len = state.lines.len_col(state.cursor.row).unwrap_or(0);
            state.cursor.col = desired.min(line_len.saturating_sub(1).max(0));
        } else {
            state.cursor.col = max_col(&state.lines, &state.cursor, state.mode);
        }

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

/// Move cursor to top of visible screen (H in vim)
#[derive(Clone, Debug, Copy)]
pub struct MoveToScreenTop();

impl Execute for MoveToScreenTop {
    fn execute(&mut self, state: &mut EditorState) {
        let top_row = state.view.viewport.y;
        state.cursor.row = top_row.min(state.lines.last_row_index());
        state.cursor.col = 0;
        skip_whitespace(&state.lines, &mut state.cursor);
        state.desired_col = Some(state.cursor.col);

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

/// Move cursor to middle of visible screen (M in vim)
#[derive(Clone, Debug, Copy)]
pub struct MoveToScreenMiddle();

impl Execute for MoveToScreenMiddle {
    fn execute(&mut self, state: &mut EditorState) {
        let top_row = state.view.viewport.y;
        let middle_row = top_row + state.view.num_rows / 2;
        state.cursor.row = middle_row.min(state.lines.last_row_index());
        state.cursor.col = 0;
        skip_whitespace(&state.lines, &mut state.cursor);
        state.desired_col = Some(state.cursor.col);

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

/// Move cursor to bottom of visible screen (L in vim)
#[derive(Clone, Debug, Copy)]
pub struct MoveToScreenBottom();

impl Execute for MoveToScreenBottom {
    fn execute(&mut self, state: &mut EditorState) {
        let top_row = state.view.viewport.y;
        let bottom_row = top_row + state.view.num_rows.saturating_sub(1);
        state.cursor.row = bottom_row.min(state.lines.last_row_index());
        state.cursor.col = 0;
        skip_whitespace(&state.lines, &mut state.cursor);
        state.desired_col = Some(state.cursor.col);

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

/// Move to next paragraph boundary (} in vim)
/// A paragraph boundary is an empty line (or start/end of buffer)
#[derive(Clone, Debug, Copy)]
pub struct MoveParagraphDown();

impl Execute for MoveParagraphDown {
    fn execute(&mut self, state: &mut EditorState) {
        let last_row = state.lines.last_row_index();
        let start_row = state.cursor.row;

        // Skip current blank lines
        let mut row = start_row + 1;
        while row <= last_row {
            if let Some(line) = state.lines.get(jagged::index::RowIndex::new(row)) {
                if !line.is_empty() {
                    break;
                }
            }
            row += 1;
        }

        // Now find the next blank line (or end of buffer)
        while row <= last_row {
            if let Some(line) = state.lines.get(jagged::index::RowIndex::new(row)) {
                if line.is_empty() {
                    break;
                }
            }
            row += 1;
        }

        state.cursor.row = row.min(last_row);
        state.cursor.col = 0;
        state.desired_col = Some(0);

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

/// Move to previous paragraph boundary ({ in vim)
#[derive(Clone, Debug, Copy)]
pub struct MoveParagraphUp();

impl Execute for MoveParagraphUp {
    fn execute(&mut self, state: &mut EditorState) {
        if state.cursor.row == 0 {
            state.cursor.col = 0;
            state.desired_col = Some(0);
            if state.mode == EditorMode::Visual {
                set_selection(&mut state.selection, state.cursor);
            }
            return;
        }

        let start_row = state.cursor.row;

        // Skip current blank lines
        let mut row = start_row.saturating_sub(1);
        while row > 0 {
            if let Some(line) = state.lines.get(jagged::index::RowIndex::new(row)) {
                if !line.is_empty() {
                    break;
                }
            }
            row = row.saturating_sub(1);
        }

        // Now find the previous blank line (or start of buffer)
        while row > 0 {
            if let Some(line) = state.lines.get(jagged::index::RowIndex::new(row)) {
                if line.is_empty() {
                    break;
                }
            }
            row = row.saturating_sub(1);
        }

        state.cursor.row = row;
        state.cursor.col = 0;
        state.desired_col = Some(0);

        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

/// Move one WORD forward (W in vim) - whitespace-delimited words
#[derive(Clone, Debug, Copy)]
pub struct MoveWORDForward(pub usize);

impl Execute for MoveWORDForward {
    fn execute(&mut self, state: &mut EditorState) {
        if state.lines.is_empty() {
            return;
        }
        state.clamp_column();

        for _ in 0..self.0 {
            move_word_forward_big(state);
        }

        state.desired_col = Some(state.cursor.col);
        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

fn move_word_forward_big(state: &mut EditorState) {
    let start_index = match (
        state.lines.is_last_col(state.cursor),
        state.lines.is_last_row(state.cursor),
    ) {
        (true, true) => return,
        (true, false) => {
            state.cursor = Index2::new(state.cursor.row.saturating_add(1), 0);
            // Skip whitespace on the new line
            skip_whitespace(&state.lines, &mut state.cursor);
            return;
        }
        _ => Index2::new(state.cursor.row, state.cursor.col.saturating_add(1)),
    };

    // Scan forward past non-whitespace
    let mut found_ws = false;
    for (next_char, index) in state.lines.iter().from(start_index) {
        if let Some(&ch) = next_char {
            if ch.is_ascii_whitespace() {
                found_ws = true;
            } else if found_ws {
                state.cursor = index;
                return;
            }
        } else {
            // End of line boundary acts like whitespace
            found_ws = true;
        }
        state.cursor = index;
    }
}

/// Move one WORD backward (B in vim) - whitespace-delimited words
#[derive(Clone, Debug, Copy)]
pub struct MoveWORDBackward(pub usize);

impl Execute for MoveWORDBackward {
    fn execute(&mut self, state: &mut EditorState) {
        if state.lines.is_empty() {
            return;
        }
        let max_col = max_col(&state.lines, &state.cursor, state.mode);
        if state.cursor.col > max_col {
            state.cursor.col = max_col;
        }

        for _ in 0..self.0 {
            move_word_backward_big(state);
        }

        state.desired_col = Some(state.cursor.col);
        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

fn move_word_backward_big(state: &mut EditorState) {
    let mut start_index = state.cursor;
    if start_index.row == 0 && start_index.col == 0 {
        return;
    }

    if start_index.col == 0 {
        state.cursor.row = start_index.row.saturating_sub(1);
        state.cursor.col = state.lines.last_col_index(state.cursor.row);
        return;
    }

    start_index.col = start_index.col.saturating_sub(1);

    // Skip whitespace backward
    let mut found_nonws = false;
    let mut result = start_index;
    for (next_char, i) in state.lines.iter().from(start_index).rev() {
        if let Some(&ch) = next_char {
            if ch.is_ascii_whitespace() {
                if found_nonws {
                    break;
                }
            } else {
                found_nonws = true;
                result = i;
            }
        }
        if i.col == 0 {
            if found_nonws {
                result = i;
            }
            break;
        }
    }

    state.cursor = result;
}

/// Move one WORD forward to end of WORD (E in vim) - whitespace-delimited
#[derive(Clone, Debug, Copy)]
pub struct MoveWORDForwardToEnd(pub usize);

impl Execute for MoveWORDForwardToEnd {
    fn execute(&mut self, state: &mut EditorState) {
        if state.lines.is_empty() {
            return;
        }
        state.clamp_column();

        for _ in 0..self.0 {
            move_word_forward_to_end_big(state);
        }

        state.desired_col = Some(state.cursor.col);
        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

fn move_word_forward_to_end_big(state: &mut EditorState) {
    let mut start_index = match (
        state.lines.is_last_col(state.cursor),
        state.lines.is_last_row(state.cursor),
    ) {
        (true, true) => return,
        (true, false) => Index2::new(state.cursor.row.saturating_add(1), 0),
        _ => Index2::new(state.cursor.row, state.cursor.col.saturating_add(1)),
    };

    // Skip whitespace and empty lines
    skip_empty_lines(&state.lines, &mut start_index.row);
    skip_whitespace(&state.lines, &mut start_index);

    // Scan forward to find end of WORD (next whitespace or end of line)
    for (next_char, index) in state.lines.iter().from(start_index) {
        if let Some(&ch) = next_char {
            if ch.is_ascii_whitespace() {
                break;
            }
        }
        state.cursor = index;
        if state.lines.is_last_col(index) {
            break;
        }
    }
}

/// Move backward to end of previous word (ge in vim)
#[derive(Clone, Debug, Copy)]
pub struct MoveWordBackwardToEndOfWord(pub usize);

impl Execute for MoveWordBackwardToEndOfWord {
    fn execute(&mut self, state: &mut EditorState) {
        if state.lines.is_empty() {
            return;
        }
        let max_col = max_col(&state.lines, &state.cursor, state.mode);
        if state.cursor.col > max_col {
            state.cursor.col = max_col;
        }

        for _ in 0..self.0 {
            move_word_backward_to_end_of_word(state);
        }

        state.desired_col = Some(state.cursor.col);
        if state.mode == EditorMode::Visual {
            set_selection(&mut state.selection, state.cursor);
        }
    }
}

fn move_word_backward_to_end_of_word(state: &mut EditorState) {
    if state.cursor.row == 0 && state.cursor.col == 0 {
        return;
    }

    let mut start_index = state.cursor;
    if start_index.col == 0 {
        start_index.row = start_index.row.saturating_sub(1);
        start_index.col = state.lines.last_col_index(start_index.row);
    } else {
        start_index.col = start_index.col.saturating_sub(1);
    }

    let mut line = match state
        .lines
        .get(jagged::index::RowIndex::new(start_index.row))
    {
        Some(line) => line,
        None => return,
    };

    // If we start on a word, first step left past that word so `ge` reaches the previous one.
    while start_index.col > 0 {
        let class = CharacterClass::from(line.get(start_index.col));
        if class == CharacterClass::Whitespace {
            break;
        }
        let prev_col = start_index.col.saturating_sub(1);
        if CharacterClass::from(line.get(prev_col)) != class {
            start_index.col = prev_col;
            break;
        }
        start_index.col = prev_col;
    }

    skip_whitespace_rev(&state.lines, &mut start_index);

    // Move right to the end of the previous word if we stopped on its start.
    line = match state
        .lines
        .get(jagged::index::RowIndex::new(start_index.row))
    {
        Some(line) => line,
        None => return,
    };
    let class = CharacterClass::from(line.get(start_index.col));
    while start_index.col + 1 < line.len() {
        let next_col = start_index.col + 1;
        if CharacterClass::from(line.get(next_col)) != class {
            break;
        }
        start_index.col = next_col;
    }

    state.cursor = start_index;
}

/// Find character forward on current line (f in vim)
#[derive(Clone, Debug, Copy)]
pub struct FindCharForward {
    pub ch: char,
}

impl Execute for FindCharForward {
    fn execute(&mut self, state: &mut EditorState) {
        let row_index = state.cursor.row;
        let Some(line) = state.lines.get(jagged::index::RowIndex::new(row_index)) else {
            return;
        };

        let start_col = state.cursor.col;

        // Search forward from current position (exclusive)
        for col in (start_col + 1)..line.len() {
            if let Some(&ch) = line.get(col) {
                if ch == self.ch {
                    state.cursor.col = col;
                    state.set_desired_col(None);

                    if state.mode == EditorMode::Visual {
                        set_selection(&mut state.selection, state.cursor);
                    }
                    return;
                }
            }
        }
    }
}

/// Find character backward on current line (F in vim)
#[derive(Clone, Debug, Copy)]
pub struct FindCharBackward {
    pub ch: char,
}

impl Execute for FindCharBackward {
    fn execute(&mut self, state: &mut EditorState) {
        let row_index = state.cursor.row;
        let Some(line) = state.lines.get(jagged::index::RowIndex::new(row_index)) else {
            return;
        };

        let start_col = state.cursor.col;

        // Search backward from current position (exclusive)
        if start_col > 0 {
            for col in (0..start_col).rev() {
                if let Some(&ch) = line.get(col) {
                    if ch == self.ch {
                        state.cursor.col = col;
                        state.set_desired_col(None);

                        if state.mode == EditorMode::Visual {
                            set_selection(&mut state.selection, state.cursor);
                        }
                        return;
                    }
                }
            }
        }
    }
}

/// Till character forward on current line (t in vim) - stops before the character
#[derive(Clone, Debug, Copy)]
pub struct TillCharForward {
    pub ch: char,
}

impl Execute for TillCharForward {
    fn execute(&mut self, state: &mut EditorState) {
        let row_index = state.cursor.row;
        let Some(line) = state.lines.get(jagged::index::RowIndex::new(row_index)) else {
            return;
        };

        let start_col = state.cursor.col;

        // Search forward from current position (exclusive)
        for col in (start_col + 1)..line.len() {
            if let Some(&ch) = line.get(col) {
                if ch == self.ch {
                    // Stop one character before the match
                    state.cursor.col = col.saturating_sub(1);
                    state.set_desired_col(None);

                    if state.mode == EditorMode::Visual {
                        set_selection(&mut state.selection, state.cursor);
                    }
                    return;
                }
            }
        }
    }
}

/// Till character backward on current line (T in vim) - stops before the character
#[derive(Clone, Debug, Copy)]
pub struct TillCharBackward {
    pub ch: char,
}

impl Execute for TillCharBackward {
    fn execute(&mut self, state: &mut EditorState) {
        let row_index = state.cursor.row;
        let Some(line) = state.lines.get(jagged::index::RowIndex::new(row_index)) else {
            return;
        };

        let start_col = state.cursor.col;

        // Search backward from current position (exclusive)
        if start_col > 0 {
            for col in (0..start_col).rev() {
                if let Some(&ch) = line.get(col) {
                    if ch == self.ch {
                        // Stop one character after the match (towards cursor)
                        if col + 1 < line.len() {
                            state.cursor.col = col + 1;
                        } else {
                            state.cursor.col = col;
                        }
                        state.set_desired_col(None);

                        if state.mode == EditorMode::Visual {
                            set_selection(&mut state.selection, state.cursor);
                        }
                        return;
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Eq)]
pub enum CharacterClass {
    Unknown,
    Alphanumeric,
    Punctuation,
    Whitespace,
}

impl From<&char> for CharacterClass {
    fn from(value: &char) -> Self {
        if value.is_ascii_alphanumeric() {
            return Self::Alphanumeric;
        }
        if value.is_ascii_punctuation() {
            return Self::Punctuation;
        }
        if value.is_ascii_whitespace() {
            return Self::Whitespace;
        }
        Self::Unknown
    }
}

impl From<Option<&char>> for CharacterClass {
    fn from(value: Option<&char>) -> Self {
        value.map_or(CharacterClass::Unknown, Self::from)
    }
}

impl PartialEq for CharacterClass {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (CharacterClass::Unknown, _) | (_, CharacterClass::Unknown) => false,
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Index2, Lines};

    use super::*;
    fn test_state() -> EditorState {
        EditorState::new(Lines::from("Hello World!\n\n123."))
    }

    #[test]
    fn test_move_forward() {
        let mut state = test_state();

        MoveForward(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 1));

        MoveForward(10).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 11));

        MoveForward(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 11));
    }

    #[test]
    fn test_move_backward() {
        let mut state = test_state();
        state.cursor = Index2::new(0, 11);

        MoveBackward(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 10));

        MoveBackward(10).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 0));

        MoveBackward(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 0));
    }

    #[test]
    fn test_move_down() {
        let mut state = test_state();
        state.cursor = Index2::new(0, 6);

        MoveDown(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(1, 6));

        MoveDown(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(2, 6));

        MoveDown(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(2, 6));
    }

    #[test]
    fn test_move_up() {
        let mut state = test_state();
        state.cursor = Index2::new(2, 2);

        MoveUp(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(1, 2));

        MoveUp(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 2));

        MoveUp(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 2));
    }

    #[test]
    fn test_move_word_forward() {
        let mut state = test_state();

        MoveWordForward(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 6));

        MoveWordForward(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 11));

        MoveWordForward(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(1, 0));

        MoveWordForward(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(2, 0));

        MoveWordForward(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(2, 3));
    }

    #[test]
    fn test_move_word_forward_out_of_bounds() {
        let mut state = test_state();

        state.cursor = Index2::new(0, 99);
        MoveWordForward(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(1, 0));
    }

    #[test]
    fn test_move_word_forward_to_end_of_word() {
        let mut state = test_state();

        MoveWordForwardToEndOfWord(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 4));

        MoveWordForwardToEndOfWord(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 10));

        MoveWordForwardToEndOfWord(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 11));

        MoveWordForwardToEndOfWord(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(2, 2));

        MoveWordForwardToEndOfWord(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(2, 3));

        MoveWordForwardToEndOfWord(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(2, 3));
    }

    #[test]
    fn test_move_word_backward() {
        let mut state = test_state();
        state.cursor = Index2::new(2, 3);

        MoveWordBackward(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(2, 0));

        MoveWordBackward(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(1, 0));

        MoveWordBackward(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 11));

        MoveWordBackward(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 6));

        MoveWordBackward(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 0));

        MoveWordBackward(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 0));
    }

    #[test]
    fn test_move_to_start() {
        let mut state = test_state();
        state.cursor = Index2::new(0, 2);

        MoveToStartOfLine().execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 0));
    }

    #[test]
    fn test_move_to_end() {
        let mut state = test_state();
        state.cursor = Index2::new(0, 2);

        MoveToEndOfLine().execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 11));
    }

    #[test]
    fn test_move_to_first() {
        let mut state = EditorState::new(Lines::from(" Hello"));
        state.cursor = Index2::new(0, 3);

        MoveToFirst().execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 1));
    }

    #[test]
    fn test_move_word_backward_to_end_of_word() {
        let mut state = EditorState::new(Lines::from("alpha beta gamma"));
        state.cursor = Index2::new(0, 11);

        MoveWordBackwardToEndOfWord(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 9));

        MoveWordBackwardToEndOfWord(1).execute(&mut state);
        assert_eq!(state.cursor, Index2::new(0, 4));
    }
}
