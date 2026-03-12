use ratatui::{symbols, symbols::border::Set};

pub const MESSAGE_BORDER_SET: Set = symbols::border::Set {
    top_left: "╭",
    top_right: "╮",
    bottom_left: "╰",
    bottom_right: "╯",
    vertical_left: "│",
    vertical_right: "│",
    horizontal_top: "─",
    horizontal_bottom: "─",
};

pub const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/clear", "clear conversation history and free up context"),
    ("/exit", "exit the repl"),
    (
        "/export",
        "export the current conversation to a file or clipboard",
    ),
    (
        "/fork",
        "fork (copy) a saved conversation as a new conversation",
    ),
    ("/help", "show help information and available commands"),
    ("/model", "set the ai model for colossal code"),
    ("/resume", "resume a conversation"),
    (
        "/review",
        "review code changes. options: -t <all|committed|uncommitted>, --base <branch>, --base-commit <commit>, --no-tool",
    ),
    (
        "/rewind",
        "restore the code and/or conversation to a previous point",
    ),
    (
        "/safety",
        "configure safety mode (yolo/regular/readonly) and permissions",
    ),
    ("/shells", "list and manage background shell sessions"),
    ("/status", "show tool statuses"),
    (
        "/stats",
        "show the total token count and duration of the current session",
    ),
    (
        "/summarize",
        "summarize conversation to reduce context. optional: /summarize [custom instructions]",
    ),
    (
        "/autosummarize",
        "show or set the auto-summarize trigger percent (percent of context used)",
    ),
    ("/todos", "list current todo items"),
    ("/vim", "toggle between vim and normal editing modes"),
    (
        "/spec",
        "show current spec or load a new spec. usage: /spec [path|goal]",
    ),
    (
        "/spec split",
        "split a step into sub-steps. usage: /spec split <index>",
    ),
    (
        "/spec status",
        "show detailed spec status as JSON (steps + history)",
    ),
    ("/spec abort", "abort the current orchestrator run"),
];

pub const MAX_COMPACTION_HISTORY: usize = 10;
pub const SUMMARY_BANNER_PREFIX: &str = "[SUMMARY_BANNER]";
pub const AUTO_SUMMARIZE_THRESHOLD_CONFIG_KEY: &str = "auto-summarize-threshold";
pub const AUTO_SUMMARIZE_THRESHOLD_VERSION_KEY: &str = "auto-summarize-threshold-version";
pub const DEFAULT_AUTO_SUMMARIZE_THRESHOLD: f32 = 85.0;
pub const LEGACY_AUTO_SUMMARIZE_THRESHOLD: f32 = 15.0;
pub const AUTO_SUMMARIZE_THRESHOLD_VERSION: u32 = 2;
pub const MIN_AUTO_SUMMARIZE_THRESHOLD: f32 = 5.0;
pub const MAX_AUTO_SUMMARIZE_THRESHOLD: f32 = 99.0;
pub const COMPACTION_HISTORY_RESERVE_TOKENS: usize = 1024;
pub const DEFAULT_COMPACTION_HISTORY_BUDGET: usize = 6000;
pub const MIN_COMPACTION_HISTORY_BUDGET: usize = 1024;
pub const APPROX_CHARS_PER_TOKEN: usize = 4;
