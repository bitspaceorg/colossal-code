use jagged::index::RowIndex;

use super::{delete::delete_selection, motion::CharacterClass, Execute};
use crate::{
    clipboard::ClipboardTrait, state::selection::Selection, EditorMode, EditorState, Index2, Lines,
};

/// Selects text between specified delimiter characters.
///
/// It searches for the first occurrence of a delimiter character in the text to
/// define the start of the selection, and the next occurrence of any of the delimiter
/// characters to define the end of the selection.
#[derive(Clone, Debug, Copy)]
pub struct SelectInnerBetween {
    opening: char,
    closing: char,
}

impl SelectInnerBetween {
    #[must_use]
    pub fn new(opening: char, closing: char) -> Self {
        Self { opening, closing }
    }
}

impl Execute for SelectInnerBetween {
    fn execute(&mut self, state: &mut EditorState) {
        if let Some(selection) = select_between(
            &state.lines,
            state.cursor,
            |(ch, _)| *ch == self.opening,
            |(ch, _)| *ch == self.closing,
            |(_, _)| false,
            |(_, _)| false,
        ) {
            state.selection = Some(selection);
            state.mode = EditorMode::Visual;
        }
    }
}

#[derive(Clone, Debug, Copy)]
pub struct SelectInnerWord;

impl Execute for SelectInnerWord {
    fn execute(&mut self, state: &mut EditorState) {
        let row_index = state.cursor.row;
        let Some(line) = state.lines.get(RowIndex::new(row_index)) else {
            return;
        };

        let Some(len_col) = state.lines.len_col(state.cursor.row) else {
            return;
        };

        let max_col_index = len_col.saturating_sub(1);

        let start_col = state.cursor.col;
        let start_char_class = CharacterClass::from(line.get(start_col));

        let opening_predicate =
            |(ch, _): (&char, usize)| CharacterClass::from(ch) != start_char_class.clone();
        let closing_predicate =
            |(ch, _): (&char, usize)| CharacterClass::from(ch) != start_char_class.clone();

        if let Some(selection) = select_between(
            &state.lines,
            state.cursor,
            opening_predicate,
            closing_predicate,
            |(_, col)| col == 0,
            |(_, col)| col == max_col_index,
        ) {
            state.selection = Some(selection);
        }
    }
}

fn select_between(
    lines: &Lines,
    cursor: Index2,
    opening_predicate_excl: impl Fn((&char, usize)) -> bool,
    closing_predicate_excl: impl Fn((&char, usize)) -> bool,
    opening_predicate_incl: impl Fn((&char, usize)) -> bool,
    closing_predicate_incl: impl Fn((&char, usize)) -> bool,
) -> Option<Selection> {
    let len_col = lines.len_col(cursor.row)?;
    if cursor.col >= len_col {
        return None;
    }

    let row_index = cursor.row;
    let line = lines.get(RowIndex::new(row_index))?;

    let start_col = cursor.col;

    let mut opening: Option<usize> = None;
    for col in (0..=start_col).rev() {
        if let Some(ch) = line.get(col) {
            if opening_predicate_excl((ch, col)) {
                // Found boundary - opening is the next column (to the right)
                opening = Some(col + 1);
                break;
            }
            if opening_predicate_incl((ch, col)) {
                opening = Some(col);
                break;
            }
        }
    }
    // If we didn't find an opening boundary, start from column 0
    if opening.is_none() {
        opening = Some(0);
    }

    let mut closing: Option<usize> = None;
    for col in start_col..len_col {
        if let Some(ch) = line.get(col) {
            if closing_predicate_excl((ch, col)) {
                // Found boundary - closing is the previous column (to the left)
                closing = Some(col.saturating_sub(1));
                break;
            }
            if closing_predicate_incl((ch, col)) {
                closing = Some(col);
                break;
            }
        }
    }
    // If we didn't find a closing boundary, end at last column
    if closing.is_none() {
        closing = Some(len_col.saturating_sub(1));
    }

    if let (Some(opening), Some(closing)) = (opening, closing) {
        let selection = Selection::new(
            Index2::new(row_index, opening),
            Index2::new(row_index, closing),
        );
        Some(selection)
    } else {
        None
    }
}

#[derive(Clone, Debug, Copy)]
pub struct ChangeInnerWord;

impl Execute for ChangeInnerWord {
    fn execute(&mut self, state: &mut EditorState) {
        SelectInnerWord.execute(state);
        if let Some(selection) = state.selection.take() {
            state.capture();
            let deleted = delete_selection(state, &selection);
            state.clip.set_text(deleted.into());
            state.mode = EditorMode::Insert;
        }
    }
}

#[derive(Clone, Debug, Copy)]
pub struct ChangeInnerBetween {
    opening: char,
    closing: char,
}

impl ChangeInnerBetween {
    #[must_use]
    pub fn new(opening: char, closing: char) -> Self {
        Self { opening, closing }
    }
}

impl Execute for ChangeInnerBetween {
    fn execute(&mut self, state: &mut EditorState) {
        SelectInnerBetween::new(self.opening, self.closing).execute(state);
        if let Some(selection) = state.selection.take() {
            state.capture();
            let deleted = delete_selection(state, &selection);
            state.clip.set_text(deleted.into());
            state.mode = EditorMode::Insert;
        }
    }
}

#[derive(Clone, Debug, Copy)]
pub struct SelectLine;

impl Execute for SelectLine {
    fn execute(&mut self, state: &mut EditorState) {
        let row = state.cursor.row;
        if let Some(len_col) = state.lines.len_col(row) {
            let start = Index2::new(row, 0);
            let end = if len_col > 0 {
                Index2::new(row, len_col - 1)
            } else {
                Index2::new(row, 0) // For empty lines, select the 0th position
            };
            state.selection = Some(Selection::new(start, end).line_mode());
            state.mode = EditorMode::Visual;
        }
    }
}

#[derive(Clone, Debug, Copy)]
pub struct ChangeSelection;
impl Execute for ChangeSelection {
    fn execute(&mut self, state: &mut EditorState) {
        if let Some(selection) = state.selection.take() {
            state.capture();
            let deleted = delete_selection(state, &selection);
            state.clip.set_text(deleted.into());
            state.mode = EditorMode::Insert;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::state::selection::Selection;
    use crate::Index2;
    use crate::Lines;

    use super::*;
    fn test_state() -> EditorState {
        EditorState::new(Lines::from("Hello World!\n\n123."))
    }

    #[test]
    fn test_select_line() {
        let mut state = test_state();

        state.cursor = Index2::new(0, 4);
        SelectLine.execute(&mut state);

        let want = Some(Selection::new(Index2::new(0, 0), Index2::new(0, 11)).line_mode());
        assert_eq!(state.selection, want);
        assert_eq!(state.mode, EditorMode::Visual);
        assert_eq!(state.cursor, Index2::new(0, 4));
    }

    #[test]
    fn test_select_inner_between() {
        let lines = Lines::from("\"Hello\" World");
        let mut state = EditorState::new(lines);
        state.cursor = Index2::new(0, 1);

        SelectInnerBetween::new('"', '"').execute(&mut state);

        let want = Selection::new(Index2::new(0, 1), Index2::new(0, 5));
        assert_eq!(state.selection.unwrap(), want);
    }

    #[test]
    fn test_select_inner_word() {
        let lines = Lines::from("Hello World");
        let mut state = EditorState::new(lines);
        state.cursor = Index2::new(0, 1);

        SelectInnerWord.execute(&mut state);

        let want = Selection::new(Index2::new(0, 0), Index2::new(0, 4));
        assert_eq!(state.selection.unwrap(), want);
    }
}
