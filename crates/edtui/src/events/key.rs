use crate::actions::cpaste::PasteOverSelection;
use crate::actions::delete::DeleteToEndOfLine;
use crate::actions::motion::{
    FindCharBackward, FindCharForward, MoveFullPageDown, MoveFullPageUp, MoveHalfPageDown,
    MoveParagraphDown, MoveParagraphUp, MoveSentenceBackward, MoveSentenceForward, MoveToFirstRow,
    MoveToLastRow, MoveToScreenBottom, MoveToScreenMiddle, MoveToScreenTop, MoveWORDBackward,
    MoveWORDForward, MoveWORDForwardToEnd, MoveWordBackwardToEndOfWord, TillCharBackward,
    TillCharForward,
};
use crate::actions::search::{SearchWordUnderCursor, StartSearch};
use crate::actions::{
    Action, Append, AppendCharToSearch, ChangeInnerBetween, ChangeInnerWord, ChangeSelection,
    Composed, CopyLine, CopySelection, DeleteChar, DeleteLine, DeleteSelection, Execute, FindNext,
    FindPrevious, InsertChar, JoinLineWithLineBelow, LineBreak, MoveBackward, MoveDown,
    MoveForward, MoveHalfPageUp, MoveToEndOfLine, MoveToFirst, MoveToLastNonBlank,
    MoveToMatchinBracket, MoveToStartOfLine, MoveUp, MoveWordBackward, MoveWordForward,
    MoveWordForwardToEndOfWord, Paste, Redo, RemoveChar, RemoveCharFromSearch, SearchHistoryNewer,
    SearchHistoryOlder, SelectInnerBetween, SelectInnerWord, SelectLine, StopSearch, SwitchMode,
    TriggerSearch, Undo,
};
use crate::{EditorMode, EditorState};
use ratatui::crossterm::event::{KeyCode, KeyEvent as CTKeyEvent, KeyModifiers};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub enum KeyEvent {
    Char(char),
    Down,
    Up,
    Right,
    Left,
    Enter,
    Esc,
    Backspace,
    Tab,
    Ctrl(char),
    None,
}

impl From<CTKeyEvent> for KeyEvent {
    fn from(key: CTKeyEvent) -> Self {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Char(c) => KeyEvent::Ctrl(c),
                _ => KeyEvent::None,
            };
        }

        match key.code {
            KeyCode::Char(c) => KeyEvent::Char(c),
            KeyCode::Enter => KeyEvent::Enter,
            KeyCode::Down => KeyEvent::Down,
            KeyCode::Up => KeyEvent::Up,
            KeyCode::Right => KeyEvent::Right,
            KeyCode::Left => KeyEvent::Left,
            KeyCode::Esc => KeyEvent::Esc,
            KeyCode::Backspace => KeyEvent::Backspace,
            KeyCode::Tab => KeyEvent::Tab,
            _ => KeyEvent::None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct KeyEventHandler {
    lookup: Vec<KeyEvent>,
    register: HashMap<KeyEventRegister, Action>,
    /// Pending find/till operation waiting for a character
    pending_find: Option<(PendingFind, usize)>,
    /// Last completed find/till operation for `;` and `,` repeat
    last_find: Option<(PendingFind, char)>,
    pending_mark: Option<PendingMark>,
    count_prefix: Option<usize>,
    marks: HashMap<char, crate::Index2>,
    jump_back_exact: Option<crate::Index2>,
    jump_back_line: Option<crate::Index2>,
}

#[derive(Clone, Debug, Copy)]
enum PendingFind {
    FindForward,
    FindBackward,
    TillForward,
    TillBackward,
}

#[derive(Clone, Debug, Copy)]
enum PendingMark {
    Set,
    JumpLine,
    JumpExact,
}

impl Default for KeyEventHandler {
    #[allow(clippy::too_many_lines)]
    fn default() -> Self {
        let register: HashMap<KeyEventRegister, Action> = HashMap::from([
            // Go into normal mode
            (
                KeyEventRegister::i(vec![KeyEvent::Esc]),
                SwitchMode(EditorMode::Normal).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Esc]),
                SwitchMode(EditorMode::Normal).into(),
            ),
            // Go into visual mode
            (
                KeyEventRegister::n(vec![KeyEvent::Char('v')]),
                SwitchMode(EditorMode::Visual).into(),
            ),
            // Go into insert mode
            (
                KeyEventRegister::n(vec![KeyEvent::Char('i')]),
                SwitchMode(EditorMode::Insert).into(),
            ),
            // Go into insert mode after cursor (append)
            (
                KeyEventRegister::n(vec![KeyEvent::Char('a')]),
                Append.into(),
            ),
            // Go into insert mode at end of line
            (
                KeyEventRegister::n(vec![KeyEvent::Char('A')]),
                Composed(vec![
                    MoveToEndOfLine().into(),
                    SwitchMode(EditorMode::Insert).into(),
                ])
                .into(),
            ),
            // 'o' creates a newline AFTER current line and enters insert mode
            (
                KeyEventRegister::n(vec![KeyEvent::Char('o')]),
                Composed(vec![
                    SwitchMode(EditorMode::Insert).into(),
                    MoveToEndOfLine().into(),
                    LineBreak(1).into(),
                ])
                .into(),
            ),
            // 'O' creates a newline BEFORE current line and enters insert mode
            (
                KeyEventRegister::n(vec![KeyEvent::Char('O')]),
                Composed(vec![
                    SwitchMode(EditorMode::Insert).into(),
                    MoveToFirst().into(),
                    LineBreak(1).into(),
                ])
                .into(),
            ),
            // Goes into search mode and starts of a new search.
            (
                KeyEventRegister::n(vec![KeyEvent::Char('/')]),
                StartSearch { forward: true }.into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('?')]),
                StartSearch { forward: false }.into(),
            ),
            // Trigger initial search
            (
                KeyEventRegister::s(vec![KeyEvent::Enter]),
                TriggerSearch.into(),
            ),
            // Find next
            (
                KeyEventRegister::n(vec![KeyEvent::Char('n')]),
                FindNext.into(),
            ),
            // Find previous
            (
                KeyEventRegister::n(vec![KeyEvent::Char('N')]),
                FindPrevious.into(),
            ),
            // Clear search
            (KeyEventRegister::s(vec![KeyEvent::Esc]), StopSearch.into()),
            // Delete last character from search
            (
                KeyEventRegister::s(vec![KeyEvent::Backspace]),
                RemoveCharFromSearch.into(),
            ),
            (
                KeyEventRegister::s(vec![KeyEvent::Up]),
                SearchHistoryOlder.into(),
            ),
            (
                KeyEventRegister::s(vec![KeyEvent::Down]),
                SearchHistoryNewer.into(),
            ),
            // Page up/down with Ctrl+d and Ctrl+u (in all modes)
            (
                KeyEventRegister::n(vec![KeyEvent::Ctrl('d')]),
                MoveHalfPageDown().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Ctrl('u')]),
                MoveHalfPageUp().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Ctrl('d')]),
                MoveHalfPageDown().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Ctrl('u')]),
                MoveHalfPageUp().into(),
            ),
            (
                KeyEventRegister::i(vec![KeyEvent::Ctrl('d')]),
                MoveHalfPageDown().into(),
            ),
            (
                KeyEventRegister::i(vec![KeyEvent::Ctrl('u')]),
                MoveHalfPageUp().into(),
            ),
            (
                KeyEventRegister::s(vec![KeyEvent::Ctrl('d')]),
                MoveHalfPageDown().into(),
            ),
            (
                KeyEventRegister::s(vec![KeyEvent::Ctrl('u')]),
                MoveHalfPageUp().into(),
            ),
            // Full page up/down with Ctrl+f and Ctrl+b
            (
                KeyEventRegister::n(vec![KeyEvent::Ctrl('f')]),
                MoveFullPageDown().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Ctrl('b')]),
                MoveFullPageUp().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Ctrl('f')]),
                MoveFullPageDown().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Ctrl('b')]),
                MoveFullPageUp().into(),
            ),
            (
                KeyEventRegister::s(vec![KeyEvent::Ctrl('f')]),
                MoveFullPageDown().into(),
            ),
            (
                KeyEventRegister::s(vec![KeyEvent::Ctrl('b')]),
                MoveFullPageUp().into(),
            ),
            // Screen-relative cursor positioning (H, M, L)
            (
                KeyEventRegister::n(vec![KeyEvent::Char('H')]),
                MoveToScreenTop().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('M')]),
                MoveToScreenMiddle().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('L')]),
                MoveToScreenBottom().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('H')]),
                MoveToScreenTop().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('M')]),
                MoveToScreenMiddle().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('L')]),
                MoveToScreenBottom().into(),
            ),
            // Move cursor forward
            (
                KeyEventRegister::n(vec![KeyEvent::Char('l')]),
                MoveForward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('l')]),
                MoveForward(1).into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Right]),
                MoveForward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Right]),
                MoveForward(1).into(),
            ),
            (
                KeyEventRegister::i(vec![KeyEvent::Right]),
                MoveForward(1).into(),
            ),
            // Move cursor backward
            (
                KeyEventRegister::n(vec![KeyEvent::Char('h')]),
                MoveBackward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('h')]),
                MoveBackward(1).into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Left]),
                MoveBackward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Left]),
                MoveBackward(1).into(),
            ),
            (
                KeyEventRegister::i(vec![KeyEvent::Left]),
                MoveBackward(1).into(),
            ),
            // Move cursor up
            (
                KeyEventRegister::n(vec![KeyEvent::Char('k')]),
                MoveUp(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('k')]),
                MoveUp(1).into(),
            ),
            (KeyEventRegister::n(vec![KeyEvent::Up]), MoveUp(1).into()),
            (KeyEventRegister::v(vec![KeyEvent::Up]), MoveUp(1).into()),
            // Don't bind Up in Insert mode - let it fall through to history navigation
            // (KeyEventRegister::i(vec![KeyEvent::Up]), MoveUp(1).into()),
            // Move cursor down
            (
                KeyEventRegister::n(vec![KeyEvent::Char('j')]),
                MoveDown(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('j')]),
                MoveDown(1).into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Down]),
                MoveDown(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Down]),
                MoveDown(1).into(),
            ),
            // Don't bind Down in Insert mode - let it fall through to history navigation
            // (
            //     KeyEventRegister::i(vec![KeyEvent::Down]),
            //     MoveDown(1).into(),
            // ),
            // Move one word forward/backward
            (
                KeyEventRegister::n(vec![KeyEvent::Char('w')]),
                MoveWordForward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('w')]),
                MoveWordForward(1).into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('e')]),
                MoveWordForwardToEndOfWord(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('e')]),
                MoveWordForwardToEndOfWord(1).into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('b')]),
                MoveWordBackward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('b')]),
                MoveWordBackward(1).into(),
            ),
            // WORD motions (whitespace-delimited) W, B, E
            (
                KeyEventRegister::n(vec![KeyEvent::Char('W')]),
                MoveWORDForward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('W')]),
                MoveWORDForward(1).into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('E')]),
                MoveWORDForwardToEnd(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('E')]),
                MoveWORDForwardToEnd(1).into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('B')]),
                MoveWORDBackward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('B')]),
                MoveWORDBackward(1).into(),
            ),
            // ge - move backward to end of previous word
            (
                KeyEventRegister::n(vec![KeyEvent::Char('g'), KeyEvent::Char('e')]),
                MoveWordBackwardToEndOfWord(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('g'), KeyEvent::Char('e')]),
                MoveWordBackwardToEndOfWord(1).into(),
            ),
            // Paragraph motions { and }
            (
                KeyEventRegister::n(vec![KeyEvent::Char('{')]),
                MoveParagraphUp().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('}')]),
                MoveParagraphDown().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('{')]),
                MoveParagraphUp().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('}')]),
                MoveParagraphDown().into(),
            ),
            // Word under cursor search (* and #)
            (
                KeyEventRegister::n(vec![KeyEvent::Char('*')]),
                SearchWordUnderCursor { forward: true }.into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('#')]),
                SearchWordUnderCursor { forward: false }.into(),
            ),
            // Move cursor to start/first/last position
            (
                KeyEventRegister::n(vec![KeyEvent::Char('0')]),
                MoveToStartOfLine().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('_')]),
                MoveToFirst().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('^')]),
                MoveToFirst().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('$')]),
                MoveToEndOfLine().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('0')]),
                MoveToStartOfLine().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('_')]),
                MoveToFirst().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('^')]),
                MoveToFirst().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('$')]),
                MoveToEndOfLine().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('g'), KeyEvent::Char('_')]),
                MoveToLastNonBlank().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('g'), KeyEvent::Char('_')]),
                MoveToLastNonBlank().into(),
            ),
            // Move cursor to start/last row in the buffer
            (
                KeyEventRegister::n(vec![KeyEvent::Char('g'), KeyEvent::Char('g')]),
                MoveToFirstRow().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('g'), KeyEvent::Char('g')]),
                MoveToFirstRow().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('G')]),
                MoveToLastRow().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('G')]),
                MoveToLastRow().into(),
            ),
            // Move cursor to the next opening/closing bracket.
            (
                KeyEventRegister::n(vec![KeyEvent::Char('%')]),
                MoveToMatchinBracket().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('%')]),
                MoveToMatchinBracket().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('(')]),
                MoveSentenceBackward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('(')]),
                MoveSentenceBackward(1).into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char(')')]),
                MoveSentenceForward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char(')')]),
                MoveSentenceForward(1).into(),
            ),
            // Select inner word between delimiters
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char('w')]),
                SelectInnerWord.into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char('"')]),
                SelectInnerBetween::new('"', '"').into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char('\'')]),
                SelectInnerBetween::new('\'', '\'').into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char('(')]),
                SelectInnerBetween::new('(', ')').into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char(')')]),
                SelectInnerBetween::new('(', ')').into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char('{')]),
                SelectInnerBetween::new('{', '}').into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char('}')]),
                SelectInnerBetween::new('{', '}').into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char('[')]),
                SelectInnerBetween::new('[', ']').into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char(']')]),
                SelectInnerBetween::new('[', ']').into(),
            ),
            // Select the line
            (
                KeyEventRegister::n(vec![KeyEvent::Char('V')]),
                SelectLine.into(),
            ),
            // Select the line in visual mode (V in visual mode switches to visual line)
            (
                KeyEventRegister::v(vec![KeyEvent::Char('V')]),
                SelectLine.into(),
            ),
            // Copy
            (
                KeyEventRegister::v(vec![KeyEvent::Char('y')]),
                CopySelection.into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('y'), KeyEvent::Char('y')]),
                CopyLine.into(),
            ),
            // Delete line with dd
            (
                KeyEventRegister::n(vec![KeyEvent::Char('d'), KeyEvent::Char('d')]),
                DeleteLine(1).into(),
            ),
            // Delete character under cursor with x
            (
                KeyEventRegister::n(vec![KeyEvent::Char('x')]),
                RemoveChar(1).into(),
            ),
            // Delete to end of line with D
            (
                KeyEventRegister::n(vec![KeyEvent::Char('D')]),
                DeleteToEndOfLine.into(),
            ),
            // Delete word with dw
            (
                KeyEventRegister::n(vec![KeyEvent::Char('d'), KeyEvent::Char('w')]),
                Composed(vec![
                    SelectInnerWord.into(),
                    MoveWordForward(1).into(),
                    DeleteSelection.into(),
                ])
                .into(),
            ),
            // Delete inner word with diw
            (
                KeyEventRegister::n(vec![
                    KeyEvent::Char('d'),
                    KeyEvent::Char('i'),
                    KeyEvent::Char('w'),
                ]),
                Composed(vec![SelectInnerWord.into(), DeleteSelection.into()]).into(),
            ),
            // Paste
            (KeyEventRegister::n(vec![KeyEvent::Char('p')]), Paste.into()),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('p')]),
                PasteOverSelection.into(),
            ),
            // Undo with u
            (KeyEventRegister::n(vec![KeyEvent::Char('u')]), Undo.into()),
            // Redo with Ctrl+R
            (KeyEventRegister::n(vec![KeyEvent::Ctrl('r')]), Redo.into()),
            // Insert at first non-blank character with I
            (
                KeyEventRegister::n(vec![KeyEvent::Char('I')]),
                Composed(vec![
                    MoveToFirst().into(),
                    SwitchMode(EditorMode::Insert).into(),
                ])
                .into(),
            ),
            // Join lines with J
            (
                KeyEventRegister::n(vec![KeyEvent::Char('J')]),
                JoinLineWithLineBelow.into(),
            ),
            // Change inner word with ciw
            (
                KeyEventRegister::n(vec![
                    KeyEvent::Char('c'),
                    KeyEvent::Char('i'),
                    KeyEvent::Char('w'),
                ]),
                ChangeInnerWord.into(),
            ),
            // Change inner double quotes with ci"
            (
                KeyEventRegister::n(vec![
                    KeyEvent::Char('c'),
                    KeyEvent::Char('i'),
                    KeyEvent::Char('"'),
                ]),
                ChangeInnerBetween::new('"', '"').into(),
            ),
            // Change inner single quotes with ci'
            (
                KeyEventRegister::n(vec![
                    KeyEvent::Char('c'),
                    KeyEvent::Char('i'),
                    KeyEvent::Char('\''),
                ]),
                ChangeInnerBetween::new('\'', '\'').into(),
            ),
            // Change inner parentheses with ci(
            (
                KeyEventRegister::n(vec![
                    KeyEvent::Char('c'),
                    KeyEvent::Char('i'),
                    KeyEvent::Char('('),
                ]),
                ChangeInnerBetween::new('(', ')').into(),
            ),
            // Change inner parentheses with ci)
            (
                KeyEventRegister::n(vec![
                    KeyEvent::Char('c'),
                    KeyEvent::Char('i'),
                    KeyEvent::Char(')'),
                ]),
                ChangeInnerBetween::new('(', ')').into(),
            ),
            // Change inner braces with ci{
            (
                KeyEventRegister::n(vec![
                    KeyEvent::Char('c'),
                    KeyEvent::Char('i'),
                    KeyEvent::Char('{'),
                ]),
                ChangeInnerBetween::new('{', '}').into(),
            ),
            // Change inner braces with ci}
            (
                KeyEventRegister::n(vec![
                    KeyEvent::Char('c'),
                    KeyEvent::Char('i'),
                    KeyEvent::Char('}'),
                ]),
                ChangeInnerBetween::new('{', '}').into(),
            ),
            // Change inner brackets with ci[
            (
                KeyEventRegister::n(vec![
                    KeyEvent::Char('c'),
                    KeyEvent::Char('i'),
                    KeyEvent::Char('['),
                ]),
                ChangeInnerBetween::new('[', ']').into(),
            ),
            // Change inner brackets with ci]
            (
                KeyEventRegister::n(vec![
                    KeyEvent::Char('c'),
                    KeyEvent::Char('i'),
                    KeyEvent::Char(']'),
                ]),
                ChangeInnerBetween::new('[', ']').into(),
            ),
            // Delete selection in visual mode with d
            (
                KeyEventRegister::v(vec![KeyEvent::Char('d')]),
                DeleteSelection.into(),
            ),
            // Delete selection in visual mode with x
            (
                KeyEventRegister::v(vec![KeyEvent::Char('x')]),
                DeleteSelection.into(),
            ),
            // Change selection in visual mode with c
            (
                KeyEventRegister::v(vec![KeyEvent::Char('c')]),
                ChangeSelection.into(),
            ),
            // Join lines in visual mode with J
            (
                KeyEventRegister::v(vec![KeyEvent::Char('J')]),
                JoinLineWithLineBelow.into(),
            ),
        ]);

        Self {
            lookup: Vec::new(),
            register,
            pending_find: None,
            last_find: None,
            pending_mark: None,
            count_prefix: None,
            marks: HashMap::new(),
            jump_back_exact: None,
            jump_back_line: None,
        }
    }
}

impl KeyEventHandler {
    /// Creates a new `EditorInput`.
    #[must_use]
    pub fn new(register: HashMap<KeyEventRegister, Action>) -> Self {
        Self {
            lookup: Vec::new(),
            register,
            pending_find: None,
            last_find: None,
            pending_mark: None,
            count_prefix: None,
            marks: HashMap::new(),
            jump_back_exact: None,
            jump_back_line: None,
        }
    }

    /// Insert a new callback to the registry
    pub fn insert<T>(&mut self, key: KeyEventRegister, action: T)
    where
        T: Into<Action>,
    {
        self.register.insert(key, action.into());
    }

    /// Extents the register with the contents of an iterator
    pub fn extend<T, U>(&mut self, iter: T)
    where
        U: Into<Action>,
        T: IntoIterator<Item = (KeyEventRegister, U)>,
    {
        self.register
            .extend(iter.into_iter().map(|(k, v)| (k, v.into())));
    }

    /// Returns an action for a specific register key, if present.
    /// Returns an action only if there is an exact match. If there
    /// are multiple matches or an inexact match, the specified key
    /// is appended to the lookup vector.
    /// If there is an exact match or if none of the keys in the registry
    /// starts with the current sequence, the lookup sequence is reset.
    #[must_use]
    fn get(&mut self, c: KeyEvent, mode: EditorMode) -> Option<Action> {
        self.lookup.push(c);
        let key = KeyEventRegister::new(self.lookup.clone(), mode);

        match self
            .register
            .keys()
            .filter(|k| k.mode == key.mode && k.keys.starts_with(&key.keys))
            .count()
        {
            0 => {
                self.lookup.clear();
                None
            }
            1 => self.register.get(&key).map(|action| {
                self.lookup.clear();
                action.clone()
            }),
            _ => None,
        }
    }
}

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct KeyEventRegister {
    keys: Vec<KeyEvent>,
    mode: EditorMode,
}

type RegisterCB = fn(&mut EditorState);

#[derive(Clone, Debug)]
struct RegisterVal(pub fn(&mut EditorState));

impl KeyEventRegister {
    pub fn new<T>(key: T, mode: EditorMode) -> Self
    where
        T: Into<Vec<KeyEvent>>,
    {
        Self {
            keys: key.into(),
            mode,
        }
    }

    pub fn n<T>(key: T) -> Self
    where
        T: Into<Vec<KeyEvent>>,
    {
        Self::new(key.into(), EditorMode::Normal)
    }

    pub fn v<T>(key: T) -> Self
    where
        T: Into<Vec<KeyEvent>>,
    {
        Self::new(key.into(), EditorMode::Visual)
    }

    pub fn i<T>(key: T) -> Self
    where
        T: Into<Vec<KeyEvent>>,
    {
        Self::new(key.into(), EditorMode::Insert)
    }

    pub fn s<T>(key: T) -> Self
    where
        T: Into<Vec<KeyEvent>>,
    {
        Self::new(key.into(), EditorMode::Search)
    }
}

impl KeyEventHandler {
    fn clear_pending_prefixes(&mut self) {
        self.count_prefix = None;
    }

    fn execute_find(
        &mut self,
        pending: PendingFind,
        count: usize,
        ch: char,
        state: &mut EditorState,
    ) {
        for _ in 0..count.max(1) {
            match pending {
                PendingFind::FindForward => FindCharForward { ch }.execute(state),
                PendingFind::FindBackward => FindCharBackward { ch }.execute(state),
                PendingFind::TillForward => TillCharForward { ch }.execute(state),
                PendingFind::TillBackward => TillCharBackward { ch }.execute(state),
            }
        }
        self.last_find = Some((pending, ch));
    }

    fn apply_count(action: Action, count: usize) -> Action {
        match action {
            Action::MoveForward(_) => MoveForward(count).into(),
            Action::MoveBackward(_) => MoveBackward(count).into(),
            Action::MoveUp(_) => MoveUp(count).into(),
            Action::MoveDown(_) => MoveDown(count).into(),
            Action::MoveWordForward(_) => MoveWordForward(count).into(),
            Action::MoveWordForwardToEndOfWord(_) => MoveWordForwardToEndOfWord(count).into(),
            Action::MoveWordBackward(_) => MoveWordBackward(count).into(),
            Action::MoveHalfPageDown(_) => {
                Composed((0..count).map(|_| MoveHalfPageDown().into()).collect()).into()
            }
            Action::MoveHalfPageUp(_) => {
                Composed((0..count).map(|_| MoveHalfPageUp().into()).collect()).into()
            }
            Action::MoveFullPageDown(_) => {
                Composed((0..count).map(|_| MoveFullPageDown().into()).collect()).into()
            }
            Action::MoveFullPageUp(_) => {
                Composed((0..count).map(|_| MoveFullPageUp().into()).collect()).into()
            }
            Action::MoveParagraphDown(_) => MoveParagraphDown().into(),
            Action::MoveParagraphUp(_) => MoveParagraphUp().into(),
            Action::MoveWORDForward(_) => MoveWORDForward(count).into(),
            Action::MoveWORDBackward(_) => MoveWORDBackward(count).into(),
            Action::MoveWORDForwardToEnd(_) => MoveWORDForwardToEnd(count).into(),
            Action::MoveWordBackwardToEndOfWord(_) => MoveWordBackwardToEndOfWord(count).into(),
            Action::MoveSentenceForward(_) => MoveSentenceForward(count).into(),
            Action::MoveSentenceBackward(_) => MoveSentenceBackward(count).into(),
            Action::FindNext(_) => {
                let mut actions = Vec::with_capacity(count);
                for _ in 0..count {
                    actions.push(FindNext.into());
                }
                Composed(actions).into()
            }
            Action::FindPrevious(_) => {
                let mut actions = Vec::with_capacity(count);
                for _ in 0..count {
                    actions.push(FindPrevious.into());
                }
                Composed(actions).into()
            }
            other => other,
        }
    }

    fn jump_to_mark(&mut self, mark: crate::Index2, linewise: bool, state: &mut EditorState) {
        let current = state.cursor;
        self.jump_back_exact = Some(current);
        self.jump_back_line = Some(current);

        if linewise {
            state.cursor.row = mark.row.min(state.lines.len().saturating_sub(1));
            state.cursor.col = 0;
            MoveToFirst().execute(state);
        } else {
            state.cursor = mark;
            state.clamp_column();
            state.set_desired_col(Some(state.cursor.col));
            if state.mode == EditorMode::Visual {
                crate::state::selection::set_selection(&mut state.selection, state.cursor);
            }
        }
    }

    pub(crate) fn on_event<T>(&mut self, key: T, state: &mut EditorState)
    where
        T: Into<KeyEvent> + Copy,
    {
        let mode = state.mode;
        let key_event = key.into();
        let count_mode = mode == EditorMode::Normal || mode == EditorMode::Visual;

        if let Some(pending) = self.pending_mark.take() {
            if let KeyEvent::Char(ch) = key_event {
                match pending {
                    PendingMark::Set => {
                        self.marks.insert(ch, state.cursor);
                    }
                    PendingMark::JumpLine => {
                        let destination = if ch == '\'' {
                            self.jump_back_line
                        } else {
                            self.marks.get(&ch).copied()
                        };
                        if let Some(mark) = destination {
                            self.jump_to_mark(mark, true, state);
                        }
                    }
                    PendingMark::JumpExact => {
                        let destination = if ch == '`' {
                            self.jump_back_exact
                        } else {
                            self.marks.get(&ch).copied()
                        };
                        if let Some(mark) = destination {
                            self.jump_to_mark(mark, false, state);
                        }
                    }
                }
            }
            self.clear_pending_prefixes();
            return;
        }

        if count_mode {
            if let KeyEvent::Char(c) = key_event {
                if let Some(digit) = c.to_digit(10) {
                    if digit > 0 || self.count_prefix.is_some() {
                        let current = self.count_prefix.unwrap_or(0);
                        self.count_prefix =
                            Some(current.saturating_mul(10).saturating_add(digit as usize));
                        return;
                    }
                }
            }
        }

        // Handle pending find/till operation
        if let Some((pending, count)) = self.pending_find.take() {
            if let KeyEvent::Char(ch) = key_event {
                self.execute_find(pending, count, ch, state);
            }
            self.clear_pending_prefixes();
            return;
        }

        match key_event {
            // Always insert characters in insert mode
            KeyEvent::Char(c) if mode == EditorMode::Insert => InsertChar(c).execute(state),
            KeyEvent::Tab if mode == EditorMode::Insert => InsertChar('\t').execute(state),
            KeyEvent::Backspace if mode == EditorMode::Insert => DeleteChar(1).execute(state),
            // Always add characters to search in search mode
            KeyEvent::Char(c) if mode == EditorMode::Search => AppendCharToSearch(c).execute(state),
            KeyEvent::Up if mode == EditorMode::Search => {
                let mut action = SearchHistoryOlder;
                action.execute(state);
            }
            KeyEvent::Down if mode == EditorMode::Search => {
                let mut action = SearchHistoryNewer;
                action.execute(state);
            }
            // Repeat last find in same direction (;)
            KeyEvent::Char(';') if mode == EditorMode::Normal || mode == EditorMode::Visual => {
                if let Some((pending, ch)) = self.last_find {
                    self.execute_find(pending, self.count_prefix.unwrap_or(1), ch, state);
                }
                self.clear_pending_prefixes();
            }
            // Repeat last find in opposite direction (,)
            KeyEvent::Char(',') if mode == EditorMode::Normal || mode == EditorMode::Visual => {
                if let Some((pending, ch)) = self.last_find {
                    let reverse = match pending {
                        PendingFind::FindForward => PendingFind::FindBackward,
                        PendingFind::FindBackward => PendingFind::FindForward,
                        PendingFind::TillForward => PendingFind::TillBackward,
                        PendingFind::TillBackward => PendingFind::TillForward,
                    };
                    self.execute_find(reverse, self.count_prefix.unwrap_or(1), ch, state);
                }
                self.clear_pending_prefixes();
            }
            KeyEvent::Char('m') if count_mode => {
                self.pending_mark = Some(PendingMark::Set);
            }
            KeyEvent::Char('\'') if count_mode => {
                self.pending_mark = Some(PendingMark::JumpLine);
            }
            KeyEvent::Char('`') if count_mode => {
                self.pending_mark = Some(PendingMark::JumpExact);
            }
            // Handle f/F/t/T keys in Normal and Visual modes
            KeyEvent::Char('f') if mode == EditorMode::Normal || mode == EditorMode::Visual => {
                self.pending_find =
                    Some((PendingFind::FindForward, self.count_prefix.unwrap_or(1)));
            }
            KeyEvent::Char('F') if mode == EditorMode::Normal || mode == EditorMode::Visual => {
                self.pending_find =
                    Some((PendingFind::FindBackward, self.count_prefix.unwrap_or(1)));
            }
            KeyEvent::Char('t') if mode == EditorMode::Normal || mode == EditorMode::Visual => {
                self.pending_find =
                    Some((PendingFind::TillForward, self.count_prefix.unwrap_or(1)));
            }
            KeyEvent::Char('T') if mode == EditorMode::Normal || mode == EditorMode::Visual => {
                self.pending_find =
                    Some((PendingFind::TillBackward, self.count_prefix.unwrap_or(1)));
            }
            // Else lookup an action from the register
            _ => {
                if let Some(mut action) = self.get(key_event, mode) {
                    if let Some(count) = self.count_prefix.take() {
                        action = Self::apply_count(action, count);
                    }
                    action.execute(state);
                } else if self.lookup.is_empty() {
                    self.clear_pending_prefixes();
                }
            }
        }
    }
}

impl KeyEventHandler {
    #[allow(clippy::too_many_lines)]
    pub fn create_readonly_handler() -> Self {
        let register = std::collections::HashMap::from([
            // Go into visual mode
            (
                KeyEventRegister::n(vec![KeyEvent::Char('v')]),
                SwitchMode(EditorMode::Visual).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Esc]),
                SwitchMode(EditorMode::Normal).into(),
            ),
            // Goes into search mode and starts of a new search.
            (
                KeyEventRegister::n(vec![KeyEvent::Char('/')]),
                StartSearch { forward: true }.into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('?')]),
                StartSearch { forward: false }.into(),
            ),
            // Trigger initial search
            (
                KeyEventRegister::s(vec![KeyEvent::Enter]),
                TriggerSearch.into(),
            ),
            // Find next
            (
                KeyEventRegister::n(vec![KeyEvent::Char('n')]),
                FindNext.into(),
            ),
            // Find previous
            (
                KeyEventRegister::n(vec![KeyEvent::Char('N')]),
                FindPrevious.into(),
            ),
            // Clear search
            (KeyEventRegister::s(vec![KeyEvent::Esc]), StopSearch.into()),
            // Delete last character from search
            (
                KeyEventRegister::s(vec![KeyEvent::Backspace]),
                RemoveCharFromSearch.into(),
            ),
            (
                KeyEventRegister::s(vec![KeyEvent::Up]),
                SearchHistoryOlder.into(),
            ),
            (
                KeyEventRegister::s(vec![KeyEvent::Down]),
                SearchHistoryNewer.into(),
            ),
            // Page up/down with Ctrl+d and Ctrl+u (in all modes)
            (
                KeyEventRegister::n(vec![KeyEvent::Ctrl('d')]),
                MoveHalfPageDown().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Ctrl('u')]),
                MoveHalfPageUp().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Ctrl('d')]),
                MoveHalfPageDown().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Ctrl('u')]),
                MoveHalfPageUp().into(),
            ),
            (
                KeyEventRegister::s(vec![KeyEvent::Ctrl('d')]),
                MoveHalfPageDown().into(),
            ),
            (
                KeyEventRegister::s(vec![KeyEvent::Ctrl('u')]),
                MoveHalfPageUp().into(),
            ),
            // Full page up/down with Ctrl+f and Ctrl+b
            (
                KeyEventRegister::n(vec![KeyEvent::Ctrl('f')]),
                MoveFullPageDown().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Ctrl('b')]),
                MoveFullPageUp().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Ctrl('f')]),
                MoveFullPageDown().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Ctrl('b')]),
                MoveFullPageUp().into(),
            ),
            (
                KeyEventRegister::s(vec![KeyEvent::Ctrl('f')]),
                MoveFullPageDown().into(),
            ),
            (
                KeyEventRegister::s(vec![KeyEvent::Ctrl('b')]),
                MoveFullPageUp().into(),
            ),
            // Screen-relative cursor positioning (H, M, L)
            (
                KeyEventRegister::n(vec![KeyEvent::Char('H')]),
                MoveToScreenTop().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('M')]),
                MoveToScreenMiddle().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('L')]),
                MoveToScreenBottom().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('H')]),
                MoveToScreenTop().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('M')]),
                MoveToScreenMiddle().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('L')]),
                MoveToScreenBottom().into(),
            ),
            // Move cursor forward
            (
                KeyEventRegister::n(vec![KeyEvent::Char('l')]),
                MoveForward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('l')]),
                MoveForward(1).into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Right]),
                MoveForward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Right]),
                MoveForward(1).into(),
            ),
            // Move cursor backward
            (
                KeyEventRegister::n(vec![KeyEvent::Char('h')]),
                MoveBackward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('h')]),
                MoveBackward(1).into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Left]),
                MoveBackward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Left]),
                MoveBackward(1).into(),
            ),
            // Move cursor up
            (
                KeyEventRegister::n(vec![KeyEvent::Char('k')]),
                MoveUp(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('k')]),
                MoveUp(1).into(),
            ),
            (KeyEventRegister::n(vec![KeyEvent::Up]), MoveUp(1).into()),
            (KeyEventRegister::v(vec![KeyEvent::Up]), MoveUp(1).into()),
            // Move cursor down
            (
                KeyEventRegister::n(vec![KeyEvent::Char('j')]),
                MoveDown(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('j')]),
                MoveDown(1).into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Down]),
                MoveDown(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Down]),
                MoveDown(1).into(),
            ),
            // Move one word forward/backward
            (
                KeyEventRegister::n(vec![KeyEvent::Char('w')]),
                MoveWordForward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('w')]),
                MoveWordForward(1).into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('e')]),
                MoveWordForwardToEndOfWord(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('e')]),
                MoveWordForwardToEndOfWord(1).into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('b')]),
                MoveWordBackward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('b')]),
                MoveWordBackward(1).into(),
            ),
            // WORD motions (whitespace-delimited) W, B, E
            (
                KeyEventRegister::n(vec![KeyEvent::Char('W')]),
                MoveWORDForward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('W')]),
                MoveWORDForward(1).into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('E')]),
                MoveWORDForwardToEnd(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('E')]),
                MoveWORDForwardToEnd(1).into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('B')]),
                MoveWORDBackward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('B')]),
                MoveWORDBackward(1).into(),
            ),
            // ge - move backward to end of previous word
            (
                KeyEventRegister::n(vec![KeyEvent::Char('g'), KeyEvent::Char('e')]),
                MoveWordBackwardToEndOfWord(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('g'), KeyEvent::Char('e')]),
                MoveWordBackwardToEndOfWord(1).into(),
            ),
            // Paragraph motions { and }
            (
                KeyEventRegister::n(vec![KeyEvent::Char('{')]),
                MoveParagraphUp().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('}')]),
                MoveParagraphDown().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('{')]),
                MoveParagraphUp().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('}')]),
                MoveParagraphDown().into(),
            ),
            // Word under cursor search (* and #)
            (
                KeyEventRegister::n(vec![KeyEvent::Char('*')]),
                SearchWordUnderCursor { forward: true }.into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('#')]),
                SearchWordUnderCursor { forward: false }.into(),
            ),
            // Move cursor to start/first/last position
            (
                KeyEventRegister::n(vec![KeyEvent::Char('0')]),
                MoveToStartOfLine().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('0')]),
                MoveToStartOfLine().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('_')]),
                MoveToFirst().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('^')]),
                MoveToFirst().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('_')]),
                MoveToFirst().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('^')]),
                MoveToFirst().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('$')]),
                MoveToEndOfLine().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('$')]),
                MoveToEndOfLine().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('g'), KeyEvent::Char('_')]),
                MoveToLastNonBlank().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('g'), KeyEvent::Char('_')]),
                MoveToLastNonBlank().into(),
            ),
            // Move cursor to start/last row in the buffer
            (
                KeyEventRegister::n(vec![KeyEvent::Char('g'), KeyEvent::Char('g')]),
                MoveToFirstRow().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('g'), KeyEvent::Char('g')]),
                MoveToFirstRow().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('G')]),
                MoveToLastRow().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('G')]),
                MoveToLastRow().into(),
            ),
            // Move cursor to the next opening/closing bracket.
            (
                KeyEventRegister::n(vec![KeyEvent::Char('%')]),
                MoveToMatchinBracket().into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('%')]),
                MoveToMatchinBracket().into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('(')]),
                MoveSentenceBackward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('(')]),
                MoveSentenceBackward(1).into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char(')')]),
                MoveSentenceForward(1).into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char(')')]),
                MoveSentenceForward(1).into(),
            ),
            // Copy
            (
                KeyEventRegister::v(vec![KeyEvent::Char('y')]),
                CopySelection.into(),
            ),
            (
                KeyEventRegister::n(vec![KeyEvent::Char('y'), KeyEvent::Char('y')]),
                CopyLine.into(),
            ),
            // Select inner word between delimiters (in visual mode only)
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char('w')]),
                SelectInnerWord.into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char('"')]),
                SelectInnerBetween::new('"', '"').into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char('\'')]),
                SelectInnerBetween::new('\'', '\'').into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char('(')]),
                SelectInnerBetween::new('(', ')').into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char(')')]),
                SelectInnerBetween::new('(', ')').into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char('{')]),
                SelectInnerBetween::new('{', '}').into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char('}')]),
                SelectInnerBetween::new('{', '}').into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char('[')]),
                SelectInnerBetween::new('[', ']').into(),
            ),
            (
                KeyEventRegister::v(vec![KeyEvent::Char('i'), KeyEvent::Char(']')]),
                SelectInnerBetween::new('[', ']').into(),
            ),
            // Select line
            (
                KeyEventRegister::n(vec![KeyEvent::Char('V')]),
                SelectLine.into(),
            ),
            // Select the line in visual mode (V in visual mode switches to visual line)
            (
                KeyEventRegister::v(vec![KeyEvent::Char('V')]),
                SelectLine.into(),
            ),
        ]);

        Self {
            lookup: Vec::new(),
            register,
            pending_find: None,
            last_find: None,
            pending_mark: None,
            count_prefix: None,
            marks: HashMap::new(),
            jump_back_exact: None,
            jump_back_line: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{EditorState, Index2, Lines};

    use super::{KeyEvent, KeyEventHandler};

    #[test]
    fn test_count_prefix_moves_cursor() {
        let mut handler = KeyEventHandler::default();
        let mut state = EditorState::new(Lines::from("a\nb\nc\nd\n"));

        handler.on_event(KeyEvent::Char('3'), &mut state);
        handler.on_event(KeyEvent::Char('j'), &mut state);

        assert_eq!(state.cursor.row, 3);
    }

    #[test]
    fn test_backward_search_with_question_mark() {
        let mut handler = KeyEventHandler::default();
        let mut state = EditorState::new(Lines::from("foo bar foo"));
        state.cursor = Index2::new(0, 7);

        handler.on_event(KeyEvent::Char('?'), &mut state);
        handler.on_event(KeyEvent::Char('f'), &mut state);
        handler.on_event(KeyEvent::Char('o'), &mut state);
        handler.on_event(KeyEvent::Char('o'), &mut state);
        handler.on_event(KeyEvent::Enter, &mut state);

        assert_eq!(state.cursor, Index2::new(0, 0));
    }

    #[test]
    fn test_mark_jump_and_jump_back() {
        let mut handler = KeyEventHandler::default();
        let mut state = EditorState::new(Lines::from("  alpha\n  beta\n"));
        state.cursor = Index2::new(0, 3);

        handler.on_event(KeyEvent::Char('m'), &mut state);
        handler.on_event(KeyEvent::Char('a'), &mut state);

        state.cursor = Index2::new(1, 4);
        handler.on_event(KeyEvent::Char('\''), &mut state);
        handler.on_event(KeyEvent::Char('a'), &mut state);
        assert_eq!(state.cursor, Index2::new(0, 2));

        handler.on_event(KeyEvent::Char('\''), &mut state);
        handler.on_event(KeyEvent::Char('\''), &mut state);
        assert_eq!(state.cursor.row, 1);
    }
}
