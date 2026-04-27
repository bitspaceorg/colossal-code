use agent_core::set_workspace_root_override;
use color_eyre::Result;
use ratatui::{
    DefaultTerminal, Frame,
    crossterm::{
        event::DisableBracketedPaste, event::DisableMouseCapture, event::EnableBracketedPaste,
        event::EnableMouseCapture, execute,
    },
    layout::Constraint,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};
use std::time::{Duration, Instant};

use crate::app::render::tips_format::render_tip_line;
use crate::app::{App, Mode};

pub(crate) const TIPS: &[&str] = &[
    "Tips for getting started:",
    "1. Be specific for the best results.",
    "2. Edit .niterules file to customize your interactions with the agent.",
    "3. /help for more information.",
    "4. Press Alt+n to enter navigation mode (vim-style hjkl, gg, G).",
];

#[derive(Clone, Copy, PartialEq, PartialOrd)]
pub(crate) enum Phase {
    Ascii,
    Tips,
    Input,
}

pub(crate) fn tips() -> &'static [&'static str] {
    TIPS
}

pub(crate) fn poll_duration_for_phase(phase: Phase, is_busy: bool) -> Duration {
    match phase {
        Phase::Ascii | Phase::Tips => Duration::from_millis(30),
        Phase::Input => {
            if is_busy {
                Duration::from_millis(16)
            } else {
                Duration::from_millis(50)
            }
        }
    }
}

pub(crate) fn parse_arg_value(args: &[String], flag: &str) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            if let Some(value) = iter.next() {
                if !value.starts_with('-') {
                    return Some(value.clone());
                }
            }
        } else if let Some(stripped) = arg.strip_prefix(&format!("{}=", flag)) {
            return Some(stripped.to_string());
        }
    }
    None
}

pub(crate) async fn run() -> Result<()> {
    color_eyre::install()?;

    let args: Vec<String> = std::env::args().collect();
    let spec_arg = parse_arg_value(&args, "--spec");
    if let Some(workspace_root) = parse_arg_value(&args, "--workspace-root") {
        unsafe {
            std::env::set_var("NITE_WORKSPACE_ROOT", workspace_root.clone());
        }
        set_workspace_root_override(workspace_root);
    }

    let terminal = ratatui::init();
    execute!(std::io::stdout(), EnableBracketedPaste, EnableMouseCapture)?;

    let app_result = {
        let loading_handle = tokio::spawn(async {
            let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let mut frame_idx = 0;

            loop {
                print!("\r{} Loading model...", spinner_frames[frame_idx]);
                use std::io::Write;
                std::io::stdout().flush().unwrap();
                frame_idx = (frame_idx + 1) % spinner_frames.len();
                tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
            }
        });

        let mut app = App::new().await?;

        loading_handle.abort();
        print!("\r✓ Model loaded successfully!\n");

        if let Some(spec_path) = spec_arg {
            if let Err(e) = app.load_spec(&spec_path) {
                eprintln!("Warning: Failed to load spec: {}", e);
            }
        }

        app.run(terminal).await
    };

    let _ = execute!(
        std::io::stdout(),
        DisableBracketedPaste,
        DisableMouseCapture
    );
    ratatui::restore();
    app_result
}

impl App {
    pub(crate) fn create_title_lines() -> Vec<Line<'static>> {
        let ascii_art = r"__     _________  __   ____  ___________   __     _________  ___  ____
\ \   / ___/ __ \/ /  / __ \/ __/ __/ _ | / /    / ___/ __ \/ _ \/ __/
 > > / /__/ /_/ / /__/ /_/ /\ \_\ \/ __ |/ /__  / /__/ /_/ / // / _/  
/_/  \___/\____/____/\____/___/___/_/ |_/____/  \___/\____/____/___/  
";
        let colors = [Color::Cyan, Color::Blue, Color::Magenta, Color::Red];
        ascii_art
            .lines()
            .map(|line| {
                let spans: Vec<Span> = line
                    .chars()
                    .enumerate()
                    .map(|(i, ch)| {
                        let color = colors[i % colors.len()];
                        Span::styled(
                            ch.to_string(),
                            Style::default().fg(color).add_modifier(Modifier::BOLD),
                        )
                    })
                    .collect();
                Line::from(spans)
            })
            .collect()
    }

    pub(crate) fn advance_startup_phase(&mut self) {
        match self.phase {
            Phase::Ascii => {
                if self.last_update.elapsed() >= Duration::from_nanos(800) {
                    let mut animation_complete = false;
                    let mut current_line = 0;
                    let mut found_incomplete = false;
                    for (i, line) in self.title_lines.iter().enumerate() {
                        if self.visible_chars[i] < line.width() {
                            current_line = i;
                            found_incomplete = true;
                            break;
                        }
                    }
                    if found_incomplete {
                        self.visible_chars[current_line] += 10;
                        if self.visible_chars[current_line] > self.title_lines[current_line].width()
                        {
                            self.visible_chars[current_line] =
                                self.title_lines[current_line].width();
                        }
                        self.last_update = Instant::now();
                        if self
                            .visible_chars
                            .iter()
                            .zip(self.title_lines.iter())
                            .all(|(visible, line)| *visible >= line.width())
                        {
                            animation_complete = true;
                        }
                    } else {
                        animation_complete = true;
                    }
                    if animation_complete && self.last_update.elapsed() >= Duration::from_nanos(900)
                    {
                        self.phase = Phase::Tips;
                        self.visible_tips = 0;
                        self.last_update = Instant::now();
                    }
                }
            }
            Phase::Tips => {
                if self.last_update.elapsed() >= Duration::from_millis(30) {
                    if self.visible_tips < TIPS.len() {
                        self.visible_tips += 1;
                        self.last_update = Instant::now();
                    } else if self.last_update.elapsed() >= Duration::from_millis(30) {
                        self.phase = Phase::Input;
                    }
                }
            }
            Phase::Input => {}
        }
    }

    pub(crate) fn startup_poll_duration(&self) -> Duration {
        poll_duration_for_phase(
            self.phase,
            self.agent_state.agent_processing || self.thinking_indicator_active,
        )
    }

    pub(crate) fn clear_startup_screen_if_ready(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> Result<()> {
        if !self.initial_screen_cleared && self.phase == Phase::Input {
            terminal.clear()?;
            self.initial_screen_cleared = true;
        }
        Ok(())
    }

    pub(crate) fn startup_layout_constraints(
        &self,
        render_area: ratatui::layout::Rect,
    ) -> Vec<Constraint> {
        match self.phase {
            Phase::Ascii => vec![
                Constraint::Length(self.title_lines.len() as u16),
                Constraint::Min(1),
                Constraint::Length(1),
            ],
            Phase::Tips => vec![
                Constraint::Length(self.title_lines.len() as u16),
                Constraint::Length(1),
                Constraint::Length(TIPS.len() as u16),
                Constraint::Min(1),
                Constraint::Length(1),
            ],
            Phase::Input => {
                let input_height = match self.mode {
                    Mode::Normal => {
                        let prompt_width = 4u16;
                        let indent_width = 4u16;
                        let max_width = render_area.width.saturating_sub(4);
                        let content_str = if !self.input_modified && self.input.is_empty() {
                            "Type your message or @/ to give suggestions for what tools to use."
                        } else {
                            self.input.as_str()
                        };
                        let mut lines_needed = 1u16;
                        let mut current_width = prompt_width;
                        for c in content_str.chars() {
                            if c == '\n' {
                                lines_needed += 1;
                                current_width = indent_width;
                                continue;
                            }

                            let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1) as u16;
                            if current_width + cw > max_width {
                                lines_needed += 1;
                                current_width = indent_width + cw;
                            } else {
                                current_width += cw;
                            }
                        }
                        lines_needed.clamp(1, 4) + 3
                    }
                    _ => 4u16,
                };
                let queue_choice_height = if self.show_queue_choice { 2 } else { 0 };
                let approval_prompt_height = if self.safety_state.show_approval_prompt {
                    2
                } else {
                    0
                };
                let isolated_changes_prompt_height = if self.isolated_changes.prompt_open {
                    2
                } else {
                    0
                };
                let sandbox_prompt_height = if self.safety_state.show_sandbox_prompt {
                    2
                } else {
                    0
                };
                let survey_height = self.survey.get_height();
                let autocomplete_height = if self.autocomplete_active && self.mode == Mode::Normal {
                    self.autocomplete_suggestions.len().min(10) as u16
                } else {
                    0
                };
                let background_tasks_height = if self.show_background_tasks {
                    10
                } else if self.viewing_task.is_some() {
                    20
                } else {
                    0
                };
                let help_height = if self.ui_state.show_help { 25 } else { 0 };
                let resume_height = if self.ui_state.show_resume { 25 } else { 0 };
                let rewind_height = if self.show_rewind { 25 } else { 0 };
                let isolated_conflicts_height = if self.isolated_changes.show_conflicts_panel {
                    16
                } else {
                    0
                };
                let todos_height = if self.show_todos {
                    self.todos_panel_height()
                } else {
                    0
                };
                let model_selection_height = if self.show_model_selection { 20 } else { 0 };
                let mut constraints_vec = vec![
                    Constraint::Length(self.title_lines.len() as u16),
                    Constraint::Length(1),
                ];

                constraints_vec.push(Constraint::Min(1));

                if queue_choice_height > 0 {
                    constraints_vec.push(Constraint::Length(queue_choice_height));
                }
                if approval_prompt_height > 0 {
                    constraints_vec.push(Constraint::Length(approval_prompt_height));
                }
                if isolated_changes_prompt_height > 0 {
                    constraints_vec.push(Constraint::Length(isolated_changes_prompt_height));
                }
                if sandbox_prompt_height > 0 {
                    constraints_vec.push(Constraint::Length(sandbox_prompt_height));
                }
                if survey_height > 0 {
                    constraints_vec.push(Constraint::Length(survey_height));
                }
                constraints_vec.push(Constraint::Length(1));

                constraints_vec.push(Constraint::Length(input_height));

                if autocomplete_height > 0 {
                    constraints_vec.push(Constraint::Length(autocomplete_height));
                }

                if background_tasks_height > 0 {
                    constraints_vec.push(Constraint::Length(background_tasks_height));
                }

                if help_height > 0 {
                    constraints_vec.push(Constraint::Length(help_height));
                }

                if resume_height > 0 {
                    constraints_vec.push(Constraint::Length(resume_height));
                }

                if rewind_height > 0 {
                    constraints_vec.push(Constraint::Length(rewind_height));
                }

                if isolated_conflicts_height > 0 {
                    constraints_vec.push(Constraint::Length(isolated_conflicts_height));
                }

                if todos_height > 0 {
                    constraints_vec.push(Constraint::Length(todos_height));
                }

                if model_selection_height > 0 {
                    constraints_vec.push(Constraint::Length(model_selection_height));
                }

                constraints_vec.push(Constraint::Length(1));

                constraints_vec
            }
        }
    }

    pub(crate) fn render_startup_chrome(&self, frame: &mut Frame, areas: &[ratatui::layout::Rect]) {
        if self.phase >= Phase::Ascii {
            let title_text: Vec<Line> = self
                .title_lines
                .iter()
                .enumerate()
                .map(|(i, line)| {
                    let visible_chars = self.visible_chars[i];
                    let spans: Vec<Span> = line.spans.iter().take(visible_chars).cloned().collect();
                    Line::from(spans)
                })
                .collect();
            let title =
                Paragraph::new(Text::from(title_text)).style(Style::default().fg(Color::White));
            frame.render_widget(title, areas[0]);
        }
        if self.phase == Phase::Tips && areas.len() > 2 {
            let gap = Paragraph::new(Line::from(" "));
            frame.render_widget(gap, areas[1]);

            let tips = self.render_tips();
            let tips_paragraph = Paragraph::new(tips).style(Style::default().fg(Color::Gray));
            frame.render_widget(tips_paragraph, areas[2]);
        }
        if self.phase == Phase::Input && areas.len() > 2 {
            let gap = Paragraph::new(Line::from(" "));
            frame.render_widget(gap, areas[1]);
        }
    }

    pub(crate) fn render_tips(&self) -> Vec<Line<'_>> {
        TIPS.iter()
            .take(self.visible_tips)
            .map(|&tip| render_tip_line(tip, Color::Blue))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{Phase, parse_arg_value, poll_duration_for_phase};

    #[test]
    fn parses_split_and_equals_style_flags() {
        let args = vec![
            "nite".to_string(),
            "--spec".to_string(),
            "roadmap.md".to_string(),
            "--workspace-root=/tmp/project".to_string(),
        ];

        assert_eq!(
            parse_arg_value(&args, "--spec"),
            Some("roadmap.md".to_string())
        );
        assert_eq!(
            parse_arg_value(&args, "--workspace-root"),
            Some("/tmp/project".to_string())
        );
    }

    #[test]
    fn ignores_missing_or_flag_like_values() {
        let args = vec![
            "nite".to_string(),
            "--spec".to_string(),
            "--workspace-root".to_string(),
            "--other".to_string(),
        ];

        assert_eq!(parse_arg_value(&args, "--spec"), None);
        assert_eq!(parse_arg_value(&args, "--workspace-root"), None);
        assert_eq!(parse_arg_value(&args, "--missing"), None);
    }

    #[test]
    fn startup_poll_duration_switches_by_phase_and_activity() {
        assert_eq!(
            poll_duration_for_phase(Phase::Ascii, false),
            std::time::Duration::from_millis(30)
        );
        assert_eq!(
            poll_duration_for_phase(Phase::Input, true),
            std::time::Duration::from_millis(16)
        );
        assert_eq!(
            poll_duration_for_phase(Phase::Input, false),
            std::time::Duration::from_millis(50)
        );
    }
}
