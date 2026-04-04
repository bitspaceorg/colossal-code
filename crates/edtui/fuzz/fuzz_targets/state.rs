#![no_main]

use edtui::{EditorEventHandler, EditorState, events::KeyEvent};
use libfuzzer_sys::fuzz_target;

// run: cargo fuzz run state -- -rss_limit_mb=8192
fuzz_target!(|data: Vec<KeyEvent>| {
    let mut state = EditorState::default();
    let mut input = EditorEventHandler::default();
    for key in data {
        input.on_key_event(key, &mut state)
    }
});
