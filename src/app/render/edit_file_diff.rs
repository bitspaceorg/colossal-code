use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line as RatatuiLine, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Widget},
};
use similar::{ChangeTag, TextDiff};
use std::path::Path;
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Style as SyntectStyle, Theme, ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
};
use unicode_width::UnicodeWidthChar;

const DIFF_BORDER_SET: symbols::border::Set = symbols::border::Set {
    top_left: "┌",
    top_right: "┐",
    bottom_left: "└",
    bottom_right: "┘",
    vertical_left: "│",
    vertical_right: "│",
    horizontal_top: "─",
    horizontal_bottom: "─",
};

const COLLAPSE_THRESHOLD_LINES: usize = 30;
#[derive(Clone)]
struct DiffLine {
    old_line: Option<usize>,
    new_line: Option<usize>,
    content: String,
    change_type: ChangeTag,
}

#[derive(Clone)]
struct DiffGroup {
    lines: Vec<DiffLine>,
    full_lines: Vec<DiffLine>,
    collapsed: bool,
}

#[derive(Clone)]
struct DiffRow {
    left: Option<DiffLine>,
    right: Option<DiffLine>,
}

#[derive(Clone, Copy)]
enum PaneSide {
    Left,
    Right,
}

impl DiffLine {
    fn format_line_number(&self, width: usize, pane_side: PaneSide) -> String {
        let selected = match pane_side {
            PaneSide::Left => self.old_line.or(self.new_line),
            PaneSide::Right => self.new_line.or(self.old_line),
        };

        match selected {
            Some(idx) => format!("{:>width$}", idx + 1, width = width),
            None => " ".repeat(width),
        }
    }
}

fn expand_tabs_in_segments(segments: &[(String, Style)], tab_width: usize) -> Vec<(String, Style)> {
    let mut out = Vec::with_capacity(segments.len());
    let mut col = 0;

    for (text, style) in segments {
        let mut expanded = String::new();
        for ch in text.chars() {
            if ch == '\t' {
                let spaces = tab_width - (col % tab_width);
                expanded.push_str(&" ".repeat(spaces));
                col += spaces;
            } else {
                expanded.push(ch);
                col += UnicodeWidthChar::width(ch).unwrap_or(0);
            }
        }
        out.push((expanded, *style));
    }

    out
}

fn wrap_styled_segments(segments: &[(String, Style)], max_width: usize) -> Vec<Vec<Span<'static>>> {
    if max_width == 0 {
        return vec![vec![Span::raw(String::new())]];
    }

    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    let mut line_spans: Vec<Span<'static>> = Vec::new();
    let mut line_width = 0;
    let mut run_text = String::new();
    let mut run_style = Style::default();
    let mut has_run = false;

    let flush_run = |line_spans: &mut Vec<Span<'static>>,
                     run_text: &mut String,
                     run_style: Style,
                     has_run: &mut bool| {
        if *has_run && !run_text.is_empty() {
            line_spans.push(Span::styled(std::mem::take(run_text), run_style));
        }
        *has_run = false;
    };

    for (segment_text, segment_style) in segments {
        for ch in segment_text.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);

            if line_width + w > max_width && line_width > 0 {
                flush_run(&mut line_spans, &mut run_text, run_style, &mut has_run);
                lines.push(std::mem::take(&mut line_spans));
                line_width = 0;
            }

            if w > max_width && line_width == 0 {
                if has_run && run_style != *segment_style {
                    flush_run(&mut line_spans, &mut run_text, run_style, &mut has_run);
                }
                if !has_run {
                    run_style = *segment_style;
                    has_run = true;
                }
                run_text.push(ch);
                flush_run(&mut line_spans, &mut run_text, run_style, &mut has_run);
                lines.push(std::mem::take(&mut line_spans));
                continue;
            }

            if has_run && run_style != *segment_style {
                flush_run(&mut line_spans, &mut run_text, run_style, &mut has_run);
            }
            if !has_run {
                run_style = *segment_style;
                has_run = true;
            }

            run_text.push(ch);
            line_width += w;
        }
    }

    flush_run(&mut line_spans, &mut run_text, run_style, &mut has_run);
    if !line_spans.is_empty() {
        lines.push(line_spans);
    }

    if lines.is_empty() {
        lines.push(vec![Span::raw(String::new())]);
    }

    lines
}

fn syntect_to_ratatui_style(style: SyntectStyle) -> Style {
    let mut out = Style::default().fg(Color::Rgb(
        style.foreground.r,
        style.foreground.g,
        style.foreground.b,
    ));

    if style.font_style.contains(FontStyle::BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        out = out.add_modifier(Modifier::UNDERLINED);
    }

    out
}

fn darker_gutter_from_line_bg(_line_bg: Color, _fallback: Color) -> Color {
    Color::Rgb(14, 14, 14)
}

struct SyntaxHighlighter {
    syntax_set: SyntaxSet,
    theme: Theme,
    syntax_extension: Option<String>,
}

impl SyntaxHighlighter {
    const SUPPORTED_THEMES: [&'static str; 4] = [
        "base16-ocean.dark",
        "InspiredGitHub",
        "Solarized (dark)",
        "Solarized (light)",
    ];

    fn new(file_path: &str, theme_name: Option<&str>) -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let theme_set = ThemeSet::load_defaults();
        let selected_theme = theme_name.filter(|name| Self::SUPPORTED_THEMES.contains(name));
        let theme = selected_theme
            .and_then(|name| theme_set.themes.get(name).cloned())
            .or_else(|| theme_set.themes.get("InspiredGitHub").cloned())
            .or_else(|| theme_set.themes.values().next().cloned())
            .unwrap_or_default();
        let syntax_extension = Path::new(file_path)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(ToOwned::to_owned);

        Self {
            syntax_set,
            theme,
            syntax_extension,
        }
    }

    fn syntax(&self) -> &SyntaxReference {
        if let Some(ext) = &self.syntax_extension
            && let Some(syntax) = self.syntax_set.find_syntax_by_extension(ext)
        {
            return syntax;
        }
        self.syntax_set.find_syntax_plain_text()
    }

    fn highlight_file_lines(&self, content: &str) -> Vec<Vec<(String, Style)>> {
        let mut highlighter = HighlightLines::new(self.syntax(), &self.theme);
        let mut lines = Vec::new();

        for raw_line in content.split_inclusive('\n') {
            let has_trailing_newline = raw_line.ends_with('\n');
            let mut highlighted = match highlighter.highlight_line(raw_line, &self.syntax_set) {
                Ok(regions) => regions
                    .into_iter()
                    .map(|(style, text)| (text.to_string(), syntect_to_ratatui_style(style)))
                    .collect::<Vec<(String, Style)>>(),
                Err(_) => vec![(raw_line.to_string(), Style::default())],
            };

            if has_trailing_newline {
                if let Some((last_text, _)) = highlighted.last_mut()
                    && last_text.ends_with('\n')
                {
                    last_text.pop();
                }
                highlighted.retain(|(text, _)| !text.is_empty());
            }

            if highlighted.is_empty() {
                highlighted.push((String::new(), Style::default()));
            }

            lines.push(highlighted);
        }

        lines
    }
}

#[derive(Clone)]
struct RenderedPair {
    left_line: RatatuiLine<'static>,
    left_style: Style,
    right_line: RatatuiLine<'static>,
    right_style: Style,
}

#[derive(Clone, Copy)]
struct DiffPalette {
    delete_fg: Color,
    delete_bg: Color,
    insert_fg: Color,
    insert_bg: Color,
    changed_gutter_bg: Color,
    delete_sign_fg: Color,
    insert_sign_fg: Color,
}

impl Default for DiffPalette {
    fn default() -> Self {
        Self::from_theme_name(None)
    }
}

impl DiffPalette {
    fn from_theme_name(theme_name: Option<&str>) -> Self {
        match theme_name {
            None | Some("InspiredGitHub") | Some("Solarized (light)") => Self {
                delete_fg: Color::White,
                delete_bg: Color::Rgb(28, 28, 28),
                insert_fg: Color::White,
                insert_bg: Color::Rgb(28, 28, 28),
                changed_gutter_bg: Color::Rgb(22, 22, 22),
                delete_sign_fg: Color::Rgb(255, 120, 120),
                insert_sign_fg: Color::Rgb(120, 235, 120),
            },
            _ => Self {
                delete_fg: Color::White,
                delete_bg: Color::Rgb(60, 20, 20),
                insert_fg: Color::White,
                insert_bg: Color::Rgb(20, 60, 20),
                changed_gutter_bg: Color::Rgb(14, 14, 14),
                delete_sign_fg: Color::Red,
                insert_sign_fg: Color::Green,
            },
        }
    }
}

struct DiffRenderApp {
    diff_groups: Vec<DiffGroup>,
    has_collapsed_groups: bool,
    line_number_width: usize,
    palette: DiffPalette,
    old_highlighted_lines: Vec<Vec<(String, Style)>>,
    new_highlighted_lines: Vec<Vec<(String, Style)>>,
}

impl DiffRenderApp {
    fn new_from_content(old: &str, new: &str, file_path: &str, theme_name: Option<&str>) -> Self {
        let diff = TextDiff::from_lines(old, new);

        let mut all_lines = Vec::new();
        for op in diff.ops() {
            for change in diff.iter_changes(op) {
                let mut content = change.value().to_string();
                if content.ends_with('\n') {
                    content.pop();
                }
                all_lines.push(DiffLine {
                    old_line: change.old_index(),
                    new_line: change.new_index(),
                    content,
                    change_type: change.tag(),
                });
            }
        }

        let first_change = all_lines
            .iter()
            .position(|l| l.change_type != ChangeTag::Equal);
        let last_change = all_lines
            .iter()
            .rposition(|l| l.change_type != ChangeTag::Equal);

        let lines = match (first_change, last_change) {
            (Some(first), Some(last)) => {
                let start = first.saturating_sub(3);
                let end = (last + 3 + 1).min(all_lines.len());
                all_lines[start..end].to_vec()
            }
            _ => all_lines,
        };

        let collapsed = lines.len() > COLLAPSE_THRESHOLD_LINES;
        let diff_groups = vec![DiffGroup {
            lines: lines.clone(),
            full_lines: lines,
            collapsed,
        }];
        let has_collapsed_groups = diff_groups.iter().any(|g| g.collapsed);
        let max_line_number = diff_groups
            .iter()
            .flat_map(|group| group.full_lines.iter())
            .flat_map(|line| [line.old_line, line.new_line])
            .flatten()
            .map(|idx| idx + 1)
            .max()
            .unwrap_or(0);
        let line_number_width = max_line_number.to_string().len().max(4);

        let syntax_highlighter = SyntaxHighlighter::new(file_path, theme_name);
        let old_highlighted_lines = syntax_highlighter.highlight_file_lines(old);
        let new_highlighted_lines = syntax_highlighter.highlight_file_lines(new);

        Self {
            diff_groups,
            has_collapsed_groups,
            line_number_width,
            palette: DiffPalette::from_theme_name(theme_name),
            old_highlighted_lines,
            new_highlighted_lines,
        }
    }

    fn highlighted_segments_for(
        &self,
        diff_line: &DiffLine,
        pane_side: PaneSide,
    ) -> Vec<(String, Style)> {
        let primary_index = match pane_side {
            PaneSide::Left => diff_line.old_line,
            PaneSide::Right => diff_line.new_line,
        };
        let fallback_index = match pane_side {
            PaneSide::Left => diff_line.new_line,
            PaneSide::Right => diff_line.old_line,
        };

        let primary = match pane_side {
            PaneSide::Left => &self.old_highlighted_lines,
            PaneSide::Right => &self.new_highlighted_lines,
        };
        let fallback = match pane_side {
            PaneSide::Left => &self.new_highlighted_lines,
            PaneSide::Right => &self.old_highlighted_lines,
        };

        if let Some(idx) = primary_index
            && let Some(line) = primary.get(idx)
        {
            return line.clone();
        }
        if let Some(idx) = fallback_index
            && let Some(line) = fallback.get(idx)
        {
            return line.clone();
        }

        vec![(diff_line.content.clone(), Style::default())]
    }

    fn build_rows(lines: &[DiffLine]) -> Vec<DiffRow> {
        let mut rows = Vec::new();
        let mut i = 0;

        while i < lines.len() {
            if lines[i].change_type == ChangeTag::Equal {
                let line = lines[i].clone();
                rows.push(DiffRow {
                    left: Some(line.clone()),
                    right: Some(line),
                });
                i += 1;
                continue;
            }

            let mut deletes = Vec::new();
            let mut inserts = Vec::new();
            while i < lines.len() && lines[i].change_type != ChangeTag::Equal {
                match lines[i].change_type {
                    ChangeTag::Delete => deletes.push(lines[i].clone()),
                    ChangeTag::Insert => inserts.push(lines[i].clone()),
                    ChangeTag::Equal => {}
                }
                i += 1;
            }

            let pair_count = deletes.len().max(inserts.len());
            for idx in 0..pair_count {
                rows.push(DiffRow {
                    left: deletes.get(idx).cloned(),
                    right: inserts.get(idx).cloned(),
                });
            }
        }

        rows
    }
}

fn render_pane_lines(
    app: &DiffRenderApp,
    diff_line: &DiffLine,
    pane_side: PaneSide,
    content_width: usize,
) -> Vec<(RatatuiLine<'static>, Style)> {
    let line_num = diff_line.format_line_number(app.line_number_width, pane_side);
    let (sign, style, line_style) = match diff_line.change_type {
        ChangeTag::Delete => (
            "-",
            Style::default().fg(app.palette.delete_fg),
            Style::default().bg(app.palette.delete_bg),
        ),
        ChangeTag::Insert => (
            "+",
            Style::default().fg(app.palette.insert_fg),
            Style::default().bg(app.palette.insert_bg),
        ),
        ChangeTag::Equal => (" ", Style::default().fg(Color::Gray), Style::default()),
    };

    let line_bg = line_style.bg.unwrap_or(Color::Reset);
    let highlighted_segments = app.highlighted_segments_for(diff_line, pane_side);
    let expanded_segments = expand_tabs_in_segments(&highlighted_segments, 4);
    let styled_segments: Vec<(String, Style)> = expanded_segments
        .into_iter()
        .map(|(text, token_style)| (text, style.patch(token_style).bg(line_bg)))
        .collect();
    let wrapped_content = wrap_styled_segments(&styled_segments, content_width);
    let gutter_bg = match diff_line.change_type {
        ChangeTag::Delete | ChangeTag::Insert => {
            darker_gutter_from_line_bg(line_bg, app.palette.changed_gutter_bg)
        }
        ChangeTag::Equal => line_bg,
    };
    let sign_style = match diff_line.change_type {
        ChangeTag::Delete => Style::default().fg(app.palette.delete_sign_fg),
        ChangeTag::Insert => Style::default().fg(app.palette.insert_sign_fg),
        ChangeTag::Equal => style,
    };

    let continuation_num = " ".repeat(app.line_number_width + 1);
    let continuation_sign = "  ".to_string();

    wrapped_content
        .into_iter()
        .enumerate()
        .map(|(idx, content_spans)| {
            let num_cell = if idx == 0 {
                format!("{} ", line_num)
            } else {
                continuation_num.clone()
            };
            let sign_cell = if idx == 0 {
                format!("{} ", sign)
            } else {
                continuation_sign.clone()
            };

            let mut spans = vec![
                Span::styled(num_cell, Style::default().fg(Color::DarkGray).bg(gutter_bg)),
                Span::styled(
                    sign_cell,
                    sign_style.add_modifier(Modifier::BOLD).bg(gutter_bg),
                ),
            ];
            spans.extend(content_spans);

            (RatatuiLine::from(spans), line_style)
        })
        .collect::<Vec<(RatatuiLine<'static>, Style)>>()
}

fn build_rendered_pairs(
    app: &DiffRenderApp,
    all_rows: &[DiffRow],
    content_width: usize,
) -> Vec<RenderedPair> {
    let blank_render_line = || (RatatuiLine::from(""), Style::default());
    let mut rendered_pairs = Vec::new();

    for row in all_rows {
        let left_lines = match &row.left {
            Some(line) => render_pane_lines(app, line, PaneSide::Left, content_width),
            None => vec![blank_render_line()],
        };
        let right_lines = match &row.right {
            Some(line) => render_pane_lines(app, line, PaneSide::Right, content_width),
            None => vec![blank_render_line()],
        };

        let row_height = left_lines.len().max(right_lines.len());
        for idx in 0..row_height {
            let (left_line, left_style) = left_lines
                .get(idx)
                .cloned()
                .unwrap_or_else(blank_render_line);
            let (right_line, right_style) = right_lines
                .get(idx)
                .cloned()
                .unwrap_or_else(blank_render_line);

            rendered_pairs.push(RenderedPair {
                left_line,
                left_style,
                right_line,
                right_style,
            });
        }
    }

    rendered_pairs
}

fn diff_pane_content_width(line_number_width: usize) -> usize {
    let block_area = Rect::new(0, 0, 120, 3);
    let inner_area = Rect::new(
        block_area.x + 1,
        block_area.y + 1,
        block_area.width.saturating_sub(2),
        block_area.height.saturating_sub(2),
    );
    let pane_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner_area);
    let pane_width = pane_chunks[0].width as usize;
    let gutter_width = line_number_width + 1 + 2;

    pane_width.saturating_sub(gutter_width)
}

fn render_diff_buffer(rendered_pairs: &[RenderedPair], has_collapsed_groups: bool) -> Buffer {
    let border_color = Color::DarkGray;
    let max_visible_lines = 30usize;
    let viewport_height = rendered_pairs.len().min(max_visible_lines).max(1);
    let box_height = viewport_height as u16 + 2;
    let total_height = box_height + if has_collapsed_groups { 1 } else { 0 };
    let area = Rect::new(0, 0, 120, total_height);
    let box_area = Rect::new(0, 0, 120, box_height);
    let mut buffer = Buffer::empty(area);

    let diff_block = Block::default()
        .borders(Borders::ALL)
        .border_set(DIFF_BORDER_SET)
        .border_style(Style::default().fg(border_color));
    let inner_area = diff_block.inner(box_area);
    diff_block.render(box_area, &mut buffer);

    let pane_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner_area);

    let visible_pairs = rendered_pairs
        .iter()
        .take(viewport_height)
        .collect::<Vec<&RenderedPair>>();
    let mut left_items = Vec::with_capacity(visible_pairs.len());
    let mut right_items = Vec::with_capacity(visible_pairs.len());

    for pair in visible_pairs {
        left_items.push(ListItem::new(pair.left_line.clone()).style(pair.left_style));
        right_items.push(ListItem::new(pair.right_line.clone()).style(pair.right_style));
    }

    List::new(left_items).render(pane_chunks[0], &mut buffer);
    List::new(right_items).render(pane_chunks[1], &mut buffer);

    if has_collapsed_groups {
        let hidden_lines = rendered_pairs.len().saturating_sub(max_visible_lines);
        let info_area = Rect::new(0, box_height, 120, 1);
        let info_msg = format!(
            "... {} more lines hidden. Press Ctrl+r to expand",
            hidden_lines
        );
        Paragraph::new(info_msg)
            .style(
                Style::default()
                    .fg(border_color)
                    .add_modifier(Modifier::ITALIC),
            )
            .render(info_area, &mut buffer);
    }

    buffer
}

fn buffer_row_to_line(
    buffer: &Buffer,
    row: u16,
    connector_prefix: &Span<'static>,
) -> RatatuiLine<'static> {
    let mut spans = vec![connector_prefix.clone()];
    let mut run_text = String::new();
    let mut run_style: Option<Style> = None;
    let mut row_end = buffer.area.right();

    while row_end > buffer.area.left() {
        let cell = &buffer[(row_end - 1, row)];
        if cell.symbol().is_empty() {
            row_end -= 1;
            continue;
        }
        if cell.symbol() == " " && cell.style() == Style::default() {
            row_end -= 1;
            continue;
        }
        break;
    }

    for x in buffer.area.left()..row_end {
        let cell = &buffer[(x, row)];
        let symbol = cell.symbol();
        if symbol.is_empty() {
            continue;
        }

        let style = cell.style();
        if run_style != Some(style) {
            if let Some(style) = run_style.take() {
                spans.push(Span::styled(std::mem::take(&mut run_text), style));
            }
            run_style = Some(style);
        }
        run_text.push_str(symbol);
    }

    if let Some(style) = run_style {
        spans.push(Span::styled(run_text, style));
    }

    RatatuiLine::from(spans)
}

pub(crate) fn render_edit_file_diff_lines(
    old_string: &str,
    new_string: &str,
    file_path: &str,
    max_width: usize,
    connector_prefix: Span<'static>,
) -> Vec<RatatuiLine<'static>> {
    let app = DiffRenderApp::new_from_content(old_string, new_string, file_path, None);
    let mut all_rows = Vec::new();
    for group in &app.diff_groups {
        let group_rows = DiffRenderApp::build_rows(&group.lines);
        all_rows.extend(group_rows);
    }

    let _ = max_width;
    let cell_content_width = diff_pane_content_width(app.line_number_width);

    let rendered_pairs = build_rendered_pairs(&app, &all_rows, cell_content_width);
    let buffer = render_diff_buffer(&rendered_pairs, app.has_collapsed_groups);

    (buffer.area.top()..buffer.area.bottom())
        .map(|row| buffer_row_to_line(&buffer, row, &connector_prefix))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Cell;
    use similar::ChangeTag;

    #[test]
    fn rendered_diff_rows_keep_consistent_width() {
        let lines = render_edit_file_diff_lines(
            "fn old_name() {\n    println!(\"old\");\n}\n",
            "fn new_name() {\n    println!(\"new\");\n}\n",
            "src/main.rs",
            100,
            Span::raw(""),
        );

        let widths = lines.iter().map(RatatuiLine::width).collect::<Vec<_>>();

        assert!(!widths.is_empty());
        assert!(widths.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn collapsed_diff_renders_bottom_border_before_hidden_lines_message() {
        let old_string = (0..40)
            .map(|idx| format!("old line {idx}\n"))
            .collect::<String>();
        let new_string = (0..40)
            .map(|idx| format!("new line {idx}\n"))
            .collect::<String>();

        let lines = render_edit_file_diff_lines(
            &old_string,
            &new_string,
            "src/main.rs",
            100,
            Span::raw(""),
        );

        assert_eq!(lines.len(), 33);
        assert_eq!(lines[31].to_string(), format!("└{}┘", "─".repeat(118)));
        assert_eq!(
            lines[32].to_string().trim_end(),
            "... 10 more lines hidden. Press Ctrl+r to expand"
        );
    }

    #[test]
    fn short_diff_does_not_pad_to_full_viewport_height() {
        let lines =
            render_edit_file_diff_lines("", "line 1\nline 2\n", "src/main.rs", 100, Span::raw(""));

        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].to_string(), format!("┌{}┐", "─".repeat(118)));
        assert_eq!(lines[3].to_string(), format!("└{}┘", "─".repeat(118)));
    }

    #[test]
    fn changed_rows_keep_distinct_gutter_and_content_backgrounds() {
        let app = DiffRenderApp::new_from_content("old\n", "new\n", "src/main.rs", None);
        let diff_line = DiffLine {
            old_line: Some(0),
            new_line: None,
            content: "old".to_string(),
            change_type: ChangeTag::Delete,
        };

        let rendered = render_pane_lines(&app, &diff_line, PaneSide::Left, 20);
        let line = &rendered[0].0;

        assert!(line.spans.len() >= 3);
        assert_eq!(line.spans[0].style.bg, Some(Color::Rgb(14, 14, 14)));
        assert_eq!(line.spans[1].style.bg, Some(Color::Rgb(14, 14, 14)));
        assert_eq!(line.spans[2].style.bg, Some(Color::Rgb(28, 28, 28)));
    }

    #[test]
    fn buffer_render_matches_widget_row_fill_behavior() {
        let app = DiffRenderApp::new_from_content("", "new\n", "src/main.rs", None);
        let rows = DiffRenderApp::build_rows(&app.diff_groups[0].lines);
        let rendered_pairs =
            build_rendered_pairs(&app, &rows, diff_pane_content_width(app.line_number_width));
        let buffer = render_diff_buffer(&rendered_pairs, app.has_collapsed_groups);

        assert_eq!(buffer[(2, 1)].bg, Color::Reset);
        assert_ne!(buffer[(61, 1)].bg, Color::Reset);
    }

    #[test]
    fn blank_side_has_no_background_fill() {
        let app = DiffRenderApp::new_from_content("", "new\n", "src/main.rs", None);
        let rows = DiffRenderApp::build_rows(&app.diff_groups[0].lines);
        let rendered_pairs =
            build_rendered_pairs(&app, &rows, diff_pane_content_width(app.line_number_width));
        let buffer = render_diff_buffer(&rendered_pairs, app.has_collapsed_groups);

        let left_blank_cells = (1..59)
            .map(|x| buffer[(x, 1)].clone())
            .collect::<Vec<Cell>>();
        assert!(left_blank_cells.iter().all(|cell| cell.bg == Color::Reset));
    }

    #[test]
    fn paragraph_render_preserves_right_pane_background_fill() {
        let lines =
            render_edit_file_diff_lines("", "line 1\nline 2\n", "src/main.rs", 100, Span::raw(""));

        let area = Rect::new(0, 0, lines[0].width() as u16, lines.len() as u16);
        let mut buffer = Buffer::empty(area);
        Paragraph::new(ratatui::text::Text::from(lines)).render(area, &mut buffer);

        assert_eq!(buffer[(10, 1)].bg, Color::Reset);
        assert_ne!(buffer[(80, 1)].bg, Color::Reset);
    }
}
