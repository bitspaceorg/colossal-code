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
