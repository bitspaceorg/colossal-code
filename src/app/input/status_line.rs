use color_eyre::Result;
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use std::{env, path::PathBuf, process::Command};

use crate::app::{App, Mode};

fn context_status_color_for_percent(percent_left: f32) -> Color {
    let _ = percent_left;
    Color::DarkGray
}

impl App {
    fn truncate_middle(value: &str, max_chars: usize) -> String {
        let chars: Vec<char> = value.chars().collect();
        if chars.len() <= max_chars {
            return value.to_string();
        }
        if max_chars <= 1 {
            return "…".to_string();
        }
        if max_chars <= 4 {
            return chars[..max_chars.saturating_sub(1)]
                .iter()
                .collect::<String>()
                + "…";
        }

        let keep_left = (max_chars - 1) / 2;
        let keep_right = max_chars - 1 - keep_left;
        let left = chars[..keep_left].iter().collect::<String>();
        let right = chars[chars.len() - keep_right..].iter().collect::<String>();
        format!("{left}…{right}")
    }

    fn workspace_root_for_status() -> Result<PathBuf> {
        if let Ok(raw) = env::var("NITE_WORKSPACE_ROOT") {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                let candidate = PathBuf::from(trimmed);
                return if candidate.is_absolute() {
                    Ok(candidate)
                } else {
                    env::current_dir()
                        .map(|cwd| cwd.join(candidate))
                        .map_err(|e| {
                            color_eyre::eyre::eyre!(
                                "Failed to resolve workspace root from current directory: {}",
                                e
                            )
                        })
                };
            }
        }

        env::current_dir()
            .map_err(|e| color_eyre::eyre::eyre!("Failed to get current directory: {}", e))
    }

    pub(crate) fn compute_status_left_initial() -> Result<Line<'static>> {
        Self::compute_status_left_impl(false, edtui::EditorMode::Normal, None)
    }

    pub(crate) fn compute_status_left(&self) -> Result<Line<'static>> {
        let mode = self.vim_input_editor.get_mode();
        Self::compute_status_left_impl(self.vim_mode_enabled, mode, None)
    }

    fn compute_status_left_impl(
        vim_mode_enabled: bool,
        vim_input_mode: edtui::EditorMode,
        max_width: Option<usize>,
    ) -> Result<Line<'static>> {
        let current_dir = Self::workspace_root_for_status()?;
        let dir_string = current_dir.to_string_lossy().to_string();
        let home_dir = env::var("HOME").unwrap_or_else(|_| "/home".to_string());
        let display_path = if dir_string.starts_with(&home_dir) {
            dir_string.replacen(&home_dir, "~", 1)
        } else {
            dir_string
        };
        let mut git_dir = current_dir.clone();
        let mut git_info = String::new();
        loop {
            if git_dir.join(".git").exists() {
                let head_path = git_dir.join(".git").join("HEAD");
                if let Ok(head_content) = std::fs::read_to_string(&head_path) {
                    if head_content.starts_with("ref: refs/heads/") {
                        let branch = head_content.trim_start_matches("ref: refs/heads/").trim();
                        git_info = format!(" ({branch}");
                        let git_status = Command::new("git")
                            .arg("status")
                            .arg("--porcelain")
                            .current_dir(&git_dir)
                            .output();
                        if let Ok(output) = git_status
                            && !output.stdout.is_empty()
                        {
                            git_info.push('*');
                        }
                        git_info.push(')');
                    } else {
                        git_info = " (git)".to_string();
                    }
                } else {
                    git_info = " (git)".to_string();
                }
                break;
            }
            if !git_dir.pop() {
                break;
            }
        }
        let mode_str = if vim_mode_enabled {
            match vim_input_mode {
                edtui::EditorMode::Normal => Some("[NORMAL]"),
                edtui::EditorMode::Insert => Some("[INSERT]"),
                edtui::EditorMode::Visual { .. } => Some("[VISUAL]"),
                edtui::EditorMode::Search => None,
            }
        } else {
            None
        };

        let reserved_width =
            mode_str.map_or(0, |mode| mode.chars().count() + 1) + git_info.chars().count();
        let display_path = max_width
            .map(|width| width.saturating_sub(reserved_width))
            .filter(|available| *available > 0)
            .map(|available| Self::truncate_middle(&display_path, available))
            .unwrap_or(display_path);

        let mut spans = Vec::new();

        if let Some(mode) = mode_str {
            spans.push(Span::styled(mode, Style::default().fg(Color::DarkGray)));
            spans.push(Span::raw(" "));
        }

        spans.push(Span::styled(display_path, Style::default().fg(Color::Blue)));

        if !git_info.is_empty() {
            spans.push(Span::styled(git_info, Style::default().fg(Color::DarkGray)));
        }

        Ok(Line::from(spans))
    }

    fn context_status_span(&self) -> Span<'static> {
        if let Some(limit) = self.current_context_tokens {
            if limit > 0 {
                if let Some(stats) = self.get_generation_stats() {
                    let used = stats.prompt_tokens.saturating_add(stats.completion_tokens);
                    let remaining = limit.saturating_sub(used);
                    let percent_left = (remaining as f32 / limit as f32 * 100.0).clamp(0.0, 100.0);
                    let text = format!(
                        "({:.0}% context left · auto {})",
                        percent_left,
                        self.auto_summarize_hint()
                    );
                    let color = self.context_status_color(percent_left);
                    return Span::styled(text, Style::default().fg(color));
                }

                if self.agent_state.agent_processing {
                    let streaming_tokens =
                        self.streaming_completion_tokens + self.thinking_token_count;
                    if streaming_tokens > 0 {
                        let prompt_tokens = self
                            .nav_snapshot
                            .as_ref()
                            .and_then(|s| s.generation_stats.as_ref())
                            .map(|s| s.prompt_tokens)
                            .unwrap_or(0);
                        let used = prompt_tokens.saturating_add(streaming_tokens);
                        let remaining = limit.saturating_sub(used);
                        let percent_left =
                            (remaining as f32 / limit as f32 * 100.0).clamp(0.0, 100.0);
                        let text = format!(
                            "(~{:.0}% context left · auto {})",
                            percent_left,
                            self.auto_summarize_hint()
                        );
                        let color = self.context_status_color(percent_left);
                        return Span::styled(text, Style::default().fg(color));
                    }
                }

                return Span::styled(
                    format!("(100% context left · auto {})", self.auto_summarize_hint()),
                    Style::default().fg(Color::DarkGray),
                );
            }
        }
        Span::styled(
            format!("(context unknown · auto {})", self.auto_summarize_hint()),
            Style::default().fg(Color::DarkGray),
        )
    }

    pub(crate) fn auto_summarize_hint(&self) -> String {
        let used = Self::clamp_auto_summarize_threshold(self.auto_summarize_threshold);
        let left = (100.0 - used).max(0.0);
        format!("≥{:.0}% used (~≤{:.0}% left)", used, left)
    }

    fn context_status_color(&self, percent_left: f32) -> Color {
        context_status_color_for_percent(percent_left)
    }

    fn shortcuts_status_line(&self) -> Line<'static> {
        let mut spans = Vec::new();

        if self.current_model_supports_reasoning() {
            spans.push(Span::styled("ctrl+e", Style::default().fg(Color::DarkGray)));
            spans.push(Span::raw(" "));
            spans.push(Span::styled("effort", Style::default().fg(Color::White)));
            spans.push(Span::styled(" • ", Style::default().fg(Color::Gray)));
        }

        spans.push(Span::styled(
            "shift + tab",
            Style::default().fg(Color::DarkGray),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled("modes", Style::default().fg(Color::White)));

        Line::from(spans)
    }

    pub(crate) fn render_status_bar(
        &self,
        frame: &mut Frame,
        status_area: ratatui::layout::Rect,
        mode: Mode,
        cursor_row: usize,
        cursor_col: usize,
        scroll_offset: usize,
    ) {
        let center_text = match mode {
            Mode::Navigation | Mode::Visual | Mode::Search | Mode::SessionWindow => {
                let (mode_name, mode_color) = match mode {
                    Mode::Navigation => ("NAV MODE", Color::Yellow),
                    Mode::Visual => ("VISUAL MODE", Color::Magenta),
                    Mode::Search => ("SEARCH MODE", Color::Cyan),
                    Mode::SessionWindow => ("SESSION WINDOW", Color::Blue),
                    _ => ("", Color::White),
                };
                vec![
                    Span::styled(
                        format!("{} - Cursor: ({}, {}) ", mode_name, cursor_col, cursor_row),
                        Style::default().fg(mode_color),
                    ),
                    Span::styled(
                        format!("Scroll: {}", scroll_offset),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]
            }
            Mode::Command => {
                vec![
                    Span::styled("CMD MODE ", Style::default().fg(Color::Green)),
                    Span::styled(
                        format!("Scroll: {}", scroll_offset),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]
            }
            Mode::Normal => {
                if self.safety_state.sandbox_enabled {
                    vec![
                        Span::styled("sandbox ", Style::default().fg(Color::Green)),
                        Span::styled("(ctrl + s to cycle)", Style::default().fg(Color::DarkGray)),
                    ]
                } else {
                    vec![
                        Span::styled("no sandbox ", Style::default().fg(Color::Red)),
                        Span::styled("(ctrl + s to cycle)", Style::default().fg(Color::DarkGray)),
                    ]
                }
            }
        };
        let center_line = Line::from(center_text);
        let center_width = center_line.width() as u16;
        let right_line = self.shortcuts_status_line();
        let right_width = right_line.width() as u16;
        let max_left_width = status_area
            .width
            .saturating_sub(center_width)
            .saturating_sub(right_width)
            .saturating_sub(6)
            .max(12);
        let status_left = Self::compute_status_left_impl(
            self.vim_mode_enabled,
            self.vim_input_editor.get_mode(),
            Some(max_left_width as usize),
        )
        .unwrap_or_else(|_| self.status_left.clone());
        let directory_width = status_left.width().min(max_left_width as usize) as u16;
        let horizontal = Layout::horizontal([
            Constraint::Length(1),
            Constraint::Length(directory_width),
            Constraint::Min(1),
            Constraint::Length(center_width),
            Constraint::Min(1),
            Constraint::Length(right_width),
            Constraint::Length(1),
        ])
        .flex(ratatui::layout::Flex::SpaceBetween);
        let [_, left_area, _, center_area, _, right_area, _] = horizontal.areas(status_area);

        let directory = Paragraph::new(status_left).left_aligned();
        frame.render_widget(directory, left_area);
        let centered_area = Self::center_horizontal(center_area, center_width);
        let sandbox = Paragraph::new(center_line);
        frame.render_widget(sandbox, centered_area);
        let shortcuts = Paragraph::new(right_line).right_aligned();
        frame.render_widget(shortcuts, right_area);
    }
}

#[cfg(test)]
mod tests {
    use super::context_status_color_for_percent;
    use ratatui::style::Color;

    #[test]
    fn context_status_color_uses_expected_thresholds() {
        assert_eq!(context_status_color_for_percent(8.0), Color::DarkGray);
        assert_eq!(context_status_color_for_percent(20.0), Color::DarkGray);
        assert_eq!(context_status_color_for_percent(55.0), Color::DarkGray);
    }
}
