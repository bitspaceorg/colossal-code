use agent_core::StepStatus;
use ratatui::style::{Color, Style};
use unicode_width::UnicodeWidthChar;

use crate::app::{SessionRole, StepToolCallEntry, ToolCallStatus};

pub(crate) fn format_tool_label(entry: &StepToolCallEntry, available: usize) -> String {
    let role_prefix = match entry.role {
        SessionRole::Implementor => "",
        SessionRole::Summarizer => "[Summarize] ",
        SessionRole::Verifier => "[Verifier] ",
        SessionRole::Merge => "[Merge] ",
    };
    let mut label = format!("{}{}", role_prefix, entry.label);
    if let Some(branch) = entry.worktree_branch.as_deref()
        && !branch.is_empty()
    {
        label.push_str(&format!(" [{}]", branch));
    }
    if let Some(path) = entry.worktree_path.as_deref()
        && !path.is_empty()
    {
        label.push_str(&format!(" ({})", path));
    }
    trim_to_width(&label, available)
}

pub(crate) fn compose_prefix(parent: &str, index: &str) -> String {
    if parent.is_empty() {
        index.to_string()
    } else {
        format!("{}.{}", parent, index)
    }
}

pub(crate) fn step_status_icon(status: StepStatus) -> &'static str {
    match status {
        StepStatus::Pending => "○",
        StepStatus::InProgress => "◐",
        StepStatus::Completed => "●",
        StepStatus::Failed => "✗",
    }
}

pub(crate) fn tool_status_icon(status: ToolCallStatus) -> &'static str {
    match status {
        ToolCallStatus::Started => "◐",
        ToolCallStatus::Completed => "●",
        ToolCallStatus::Error => "✗",
    }
}

pub(crate) fn style_for_step(status: StepStatus) -> Style {
    match status {
        StepStatus::Pending => Style::default().fg(Color::DarkGray),
        StepStatus::InProgress => Style::default().fg(Color::Yellow),
        StepStatus::Completed => Style::default().fg(Color::Green),
        StepStatus::Failed => Style::default().fg(Color::Red),
    }
}

pub(crate) fn style_for_tool(status: ToolCallStatus) -> Style {
    match status {
        ToolCallStatus::Started => Style::default().fg(Color::Yellow),
        ToolCallStatus::Completed => Style::default().fg(Color::Green),
        ToolCallStatus::Error => Style::default().fg(Color::Red),
    }
}

pub(crate) fn trim_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(1);
        if width + ch_width > max_width {
            result.push('…');
            break;
        }
        result.push(ch);
        width += ch_width;
    }
    if result.is_empty() {
        text.chars().take(max_width).collect()
    } else {
        result
    }
}
