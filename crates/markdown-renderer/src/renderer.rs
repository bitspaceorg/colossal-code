use crate::citation::rewrite_file_citations_with_scheme;
use crate::line_utils::line_to_static;
use crate::wrapping::word_wrap_line;
use crate::wrapping::RtOptions;
use pulldown_cmark::CodeBlockKind;
use pulldown_cmark::CowStr;
use pulldown_cmark::Event;
use pulldown_cmark::HeadingLevel;
use pulldown_cmark::Options;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use pulldown_cmark::TagEnd;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use std::path::Path;

#[derive(Clone, Debug)]
struct IndentContext {
    prefix: Vec<Span<'static>>,
    marker: Option<Vec<Span<'static>>>,
    is_list: bool,
}

impl IndentContext {
    fn new(prefix: Vec<Span<'static>>, marker: Option<Vec<Span<'static>>>, is_list: bool) -> Self {
        Self {
            prefix,
            marker,
            is_list,
        }
    }
}

pub fn render_markdown_text(input: &str) -> Text<'static> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(input, options);
    let mut w = Writer::new(parser, None, None, None);
    w.run();
    w.text
}

pub(crate) fn render_markdown_text_with_citations(
    input: &str,
    width: Option<usize>,
    scheme: Option<&str>,
    cwd: &Path,
) -> Text<'static> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(input, options);
    let mut w = Writer::new(
        parser,
        scheme.map(str::to_string),
        Some(cwd.to_path_buf()),
        width,
    );
    w.run();
    w.text
}

struct Writer<'a, I>
where
    I: Iterator<Item = Event<'a>>,
{
    iter: I,
    text: Text<'static>,
    inline_styles: Vec<Style>,
    indent_stack: Vec<IndentContext>,
    list_indices: Vec<Option<u64>>,
    link: Option<String>,
    needs_newline: bool,
    pending_marker_line: bool,
    in_paragraph: bool,
    scheme: Option<String>,
    cwd: Option<std::path::PathBuf>,
    in_code_block: bool,
    wrap_width: Option<usize>,
    current_line_content: Option<Line<'static>>,
    current_initial_indent: Vec<Span<'static>>,
    current_subsequent_indent: Vec<Span<'static>>,
    current_line_style: Style,
    current_line_in_code_block: bool,
    // Table state
    in_table: bool,
    table_rows: Vec<Vec<String>>,
    current_table_row: Vec<String>,
    current_cell_content: String,
    is_table_header: bool,
}

impl<'a, I> Writer<'a, I>
where
    I: Iterator<Item = Event<'a>>,
{
    fn new(
        iter: I,
        scheme: Option<String>,
        cwd: Option<std::path::PathBuf>,
        wrap_width: Option<usize>,
    ) -> Self {
        Self {
            iter,
            text: Text::default(),
            inline_styles: Vec::new(),
            indent_stack: Vec::new(),
            list_indices: Vec::new(),
            link: None,
            needs_newline: false,
            pending_marker_line: false,
            in_paragraph: false,
            scheme,
            cwd,
            in_code_block: false,
            wrap_width,
            current_line_content: None,
            current_initial_indent: Vec::new(),
            current_subsequent_indent: Vec::new(),
            current_line_style: Style::default(),
            current_line_in_code_block: false,
            // Initialize table state
            in_table: false,
            table_rows: Vec::new(),
            current_table_row: Vec::new(),
            current_cell_content: String::new(),
            is_table_header: false,
        }
    }

    fn run(&mut self) {
        while let Some(ev) = self.iter.next() {
            self.handle_event(ev);
        }
        self.flush_current_line();
    }

    fn handle_event(&mut self, event: Event<'a>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.text(text),
            Event::Code(code) => self.code(code),
            Event::SoftBreak => self.soft_break(),
            Event::HardBreak => self.hard_break(),
            Event::Rule => {
                self.flush_current_line();
                if !self.text.lines.is_empty() {
                    self.push_blank_line();
                }
                self.push_line(Line::from("———"));
                self.needs_newline = true;
            }
            Event::Html(html) => self.html(html, false),
            Event::InlineHtml(html) => self.html(html, true),
            Event::FootnoteReference(_) => {}
            Event::TaskListMarker(checked) => {
                // Render task list checkboxes
                let checkbox = if checked { "[✓] " } else { "[ ] " };
                self.push_span(Span::raw(checkbox));
            }
            Event::InlineMath(_) => {}
            Event::DisplayMath(_) => {}
        }
    }

    fn start_tag(&mut self, tag: Tag<'a>) {
        match tag {
            Tag::Paragraph => self.start_paragraph(),
            Tag::Heading { level, .. } => self.start_heading(level),
            Tag::BlockQuote(_) => self.start_blockquote(),
            Tag::CodeBlock(kind) => {
                let indent = match kind {
                    CodeBlockKind::Fenced(_) => None,
                    CodeBlockKind::Indented => Some(Span::from(" ".repeat(4))),
                };
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) => Some(lang.to_string()),
                    CodeBlockKind::Indented => None,
                };
                self.start_codeblock(lang, indent)
            }
            Tag::List(start) => self.start_list(start),
            Tag::Item => self.start_item(),
            Tag::Emphasis => self.push_inline_style(Style::new().italic()),
            Tag::Strong => self.push_inline_style(Style::new().bold()),
            Tag::Strikethrough => self.push_inline_style(Style::new().crossed_out()),
            Tag::Link { dest_url, .. } => self.push_link(dest_url.to_string()),
            Tag::Table(_) => self.start_table(),
            Tag::TableHead => self.start_table_head(),
            Tag::TableRow => self.start_table_row(),
            Tag::TableCell => self.start_table_cell(),
            Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::Image { .. }
            | Tag::MetadataBlock(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.end_paragraph(),
            TagEnd::Heading(_) => self.end_heading(),
            TagEnd::BlockQuote(_) => self.end_blockquote(),
            TagEnd::CodeBlock => self.end_codeblock(),
            TagEnd::List(_) => self.end_list(),
            TagEnd::Item => {
                self.indent_stack.pop();
                self.pending_marker_line = false;
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => self.pop_inline_style(),
            TagEnd::Link => self.pop_link(),
            TagEnd::Table => self.end_table(),
            TagEnd::TableHead => self.end_table_head(),
            TagEnd::TableRow => self.end_table_row(),
            TagEnd::TableCell => self.end_table_cell(),
            TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::Image
            | TagEnd::MetadataBlock(_)
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition => {}
        }
    }

    fn start_paragraph(&mut self) {
        if self.needs_newline {
            self.push_blank_line();
        }
        self.push_line(Line::default());
        self.needs_newline = false;
        self.in_paragraph = true;
    }

    fn end_paragraph(&mut self) {
        self.needs_newline = true;
        self.in_paragraph = false;
        self.pending_marker_line = false;
    }

    fn start_heading(&mut self, level: HeadingLevel) {
        if self.needs_newline {
            self.push_line(Line::default());
            self.needs_newline = false;
        }
        let heading_style = match level {
            HeadingLevel::H1 => Style::new().bold().underlined(),
            HeadingLevel::H2 => Style::new().bold(),
            HeadingLevel::H3 => Style::new().bold().italic(),
            HeadingLevel::H4 => Style::new().italic(),
            HeadingLevel::H5 => Style::new().italic(),
            HeadingLevel::H6 => Style::new().italic(),
        };
        let content = format!("{} ", "#".repeat(level as usize));
        self.push_line(Line::from(vec![Span::styled(content, heading_style)]));
        self.push_inline_style(heading_style);
        self.needs_newline = false;
    }

    fn end_heading(&mut self) {
        self.needs_newline = true;
        self.pop_inline_style();
    }

    fn start_blockquote(&mut self) {
        if self.needs_newline {
            self.push_blank_line();
            self.needs_newline = false;
        }
        self.indent_stack
            .push(IndentContext::new(vec![Span::from("> ")], None, false));
    }

    fn end_blockquote(&mut self) {
        self.indent_stack.pop();
        self.needs_newline = true;
    }

    fn text(&mut self, text: CowStr<'a>) {
        // If we're in a table, accumulate cell content
        if self.in_table {
            self.current_cell_content.push_str(&text);
            return;
        }

        if self.pending_marker_line {
            self.push_line(Line::default());
        }
        self.pending_marker_line = false;
        if self.in_code_block && !self.needs_newline {
            let has_content = self
                .current_line_content
                .as_ref()
                .map(|line| !line.spans.is_empty())
                .unwrap_or_else(|| {
                    self.text
                        .lines
                        .last()
                        .map(|line| !line.spans.is_empty())
                        .unwrap_or(false)
                });
            if has_content {
                self.push_line(Line::default());
            }
        }
        for (i, line) in text.lines().enumerate() {
            if self.needs_newline {
                self.push_line(Line::default());
                self.needs_newline = false;
            }
            if i > 0 {
                self.push_line(Line::default());
            }
            let mut content = line.to_string();
            if !self.in_code_block {
                if let (Some(scheme), Some(cwd)) = (&self.scheme, &self.cwd) {
                    let cow =
                        rewrite_file_citations_with_scheme(&content, Some(scheme.as_str()), cwd);
                    if let std::borrow::Cow::Owned(s) = cow {
                        content = s;
                    }
                }
            }
            let span = Span::styled(
                content,
                self.inline_styles.last().copied().unwrap_or_default(),
            );
            self.push_span(span);
        }
        self.needs_newline = false;
    }

    fn code(&mut self, code: CowStr<'a>) {
        // If we're in a table, accumulate cell content
        if self.in_table {
            self.current_cell_content.push_str(&code);
            return;
        }

        if self.pending_marker_line {
            self.push_line(Line::default());
            self.pending_marker_line = false;
        }
        let span = Span::from(code.into_string()).dim();
        self.push_span(span);
    }

    fn html(&mut self, html: CowStr<'a>, inline: bool) {
        self.pending_marker_line = false;
        for (i, line) in html.lines().enumerate() {
            if self.needs_newline {
                self.push_line(Line::default());
                self.needs_newline = false;
            }
            if i > 0 {
                self.push_line(Line::default());
            }
            let style = self.inline_styles.last().copied().unwrap_or_default();
            self.push_span(Span::styled(line.to_string(), style));
        }
        self.needs_newline = !inline;
    }

    fn hard_break(&mut self) {
        self.push_line(Line::default());
    }

    fn soft_break(&mut self) {
        self.push_line(Line::default());
    }

    fn start_list(&mut self, index: Option<u64>) {
        if self.list_indices.is_empty() && self.needs_newline {
            self.push_line(Line::default());
        }
        self.list_indices.push(index);
    }

    fn end_list(&mut self) {
        self.list_indices.pop();
        self.needs_newline = true;
    }

    fn start_item(&mut self) {
        self.pending_marker_line = true;
        let depth = self.list_indices.len();
        let is_ordered = self
            .list_indices
            .last()
            .map(Option::is_some)
            .unwrap_or(false);
        let width = depth * 4 - 3;
        let marker = if let Some(last_index) = self.list_indices.last_mut() {
            match last_index {
                None => Some(vec![Span::from(" ".repeat(width - 1) + "- ")]),
                Some(index) => {
                    *index += 1;
                    Some(vec![format!("{:width$}. ", *index - 1).light_blue()])
                }
            }
        } else {
            None
        };
        let indent_prefix = if depth == 0 {
            Vec::new()
        } else {
            let indent_len = if is_ordered { width + 2 } else { width + 1 };
            vec![Span::from(" ".repeat(indent_len))]
        };
        self.indent_stack
            .push(IndentContext::new(indent_prefix, marker, true));
        self.needs_newline = false;
    }

    fn start_codeblock(&mut self, _lang: Option<String>, indent: Option<Span<'static>>) {
        self.flush_current_line();
        if !self.text.lines.is_empty() {
            self.push_blank_line();
        }
        self.in_code_block = true;
        self.indent_stack.push(IndentContext::new(
            vec![indent.unwrap_or_default()],
            None,
            false,
        ));
        self.needs_newline = true;
    }

    fn end_codeblock(&mut self) {
        self.needs_newline = true;
        self.in_code_block = false;
        self.indent_stack.pop();
    }

    fn push_inline_style(&mut self, style: Style) {
        let current = self.inline_styles.last().copied().unwrap_or_default();
        let merged = current.patch(style);
        self.inline_styles.push(merged);
    }

    fn pop_inline_style(&mut self) {
        self.inline_styles.pop();
    }

    fn push_link(&mut self, dest_url: String) {
        self.link = Some(dest_url);
    }

    fn pop_link(&mut self) {
        if let Some(link) = self.link.take() {
            self.push_span(" (".into());
            self.push_span(link.cyan().underlined());
            self.push_span(")".into());
        }
    }

    fn flush_current_line(&mut self) {
        if let Some(line) = self.current_line_content.take() {
            let style = self.current_line_style;
            // NB we don't wrap code in code blocks, in order to preserve whitespace for copy/paste.
            if !self.current_line_in_code_block {
                if let Some(width) = self.wrap_width {
                    let opts = RtOptions::new(width)
                        .initial_indent(self.current_initial_indent.clone().into())
                        .subsequent_indent(self.current_subsequent_indent.clone().into());
                    for wrapped in word_wrap_line(&line, opts) {
                        let owned = line_to_static(&wrapped).style(style);
                        self.text.lines.push(owned);
                    }
                } else {
                    let mut spans = self.current_initial_indent.clone();
                    let mut line = line;
                    spans.append(&mut line.spans);
                    self.text.lines.push(Line::from_iter(spans).style(style));
                }
            } else {
                let mut spans = self.current_initial_indent.clone();
                let mut line = line;
                spans.append(&mut line.spans);
                self.text.lines.push(Line::from_iter(spans).style(style));
            }
            self.current_initial_indent.clear();
            self.current_subsequent_indent.clear();
            self.current_line_in_code_block = false;
        }
    }

    fn push_line(&mut self, line: Line<'static>) {
        self.flush_current_line();
        let blockquote_active = self
            .indent_stack
            .iter()
            .any(|ctx| ctx.prefix.iter().any(|s| s.content.contains('>')));
        let style = if blockquote_active {
            Style::new().green()
        } else {
            line.style
        };
        let was_pending = self.pending_marker_line;

        self.current_initial_indent = self.prefix_spans(was_pending);
        self.current_subsequent_indent = self.prefix_spans(false);
        self.current_line_style = style;
        self.current_line_content = Some(line);
        self.current_line_in_code_block = self.in_code_block;

        self.pending_marker_line = false;
    }

    fn push_span(&mut self, span: Span<'static>) {
        if let Some(line) = self.current_line_content.as_mut() {
            line.push_span(span);
        } else {
            self.push_line(Line::from(vec![span]));
        }
    }

    fn push_blank_line(&mut self) {
        self.flush_current_line();
        if self.indent_stack.iter().all(|ctx| ctx.is_list) {
            self.text.lines.push(Line::default());
        } else {
            self.push_line(Line::default());
            self.flush_current_line();
        }
    }

    fn prefix_spans(&self, pending_marker_line: bool) -> Vec<Span<'static>> {
        let mut prefix: Vec<Span<'static>> = Vec::new();
        let last_marker_index = if pending_marker_line {
            self.indent_stack
                .iter()
                .enumerate()
                .rev()
                .find_map(|(i, ctx)| if ctx.marker.is_some() { Some(i) } else { None })
        } else {
            None
        };
        let last_list_index = self.indent_stack.iter().rposition(|ctx| ctx.is_list);

        for (i, ctx) in self.indent_stack.iter().enumerate() {
            if pending_marker_line {
                if Some(i) == last_marker_index {
                    if let Some(marker) = &ctx.marker {
                        prefix.extend(marker.iter().cloned());
                        continue;
                    }
                }
                if ctx.is_list && last_marker_index.is_some_and(|idx| idx > i) {
                    continue;
                }
            } else if ctx.is_list && Some(i) != last_list_index {
                continue;
            }
            prefix.extend(ctx.prefix.iter().cloned());
        }

        prefix
    }

    // Table handling methods
    fn start_table(&mut self) {
        self.flush_current_line();
        self.in_table = true;
        self.table_rows.clear();
        if self.needs_newline {
            self.push_blank_line();
        }
    }

    fn start_table_head(&mut self) {
        self.is_table_header = true;
    }

    fn start_table_row(&mut self) {
        self.current_table_row.clear();
    }

    fn start_table_cell(&mut self) {
        self.current_cell_content.clear();
    }

    fn end_table_cell(&mut self) {
        self.current_table_row
            .push(self.current_cell_content.clone());
        self.current_cell_content.clear();
    }

    fn end_table_row(&mut self) {
        if !self.current_table_row.is_empty() {
            self.table_rows.push(self.current_table_row.clone());
        }
        self.current_table_row.clear();
    }

    fn end_table_head(&mut self) {
        self.is_table_header = false;
    }

    fn end_table(&mut self) {
        self.flush_current_line();
        self.render_table();
        self.in_table = false;
        self.table_rows.clear();
        self.needs_newline = true;
    }

    fn render_table(&mut self) {
        if self.table_rows.is_empty() {
            return;
        }

        // Clone the rows to avoid borrow checker issues
        let rows = self.table_rows.clone();

        // Calculate column widths
        let num_cols = rows.iter().map(|row| row.len()).max().unwrap_or(0);
        let mut col_widths = vec![0; num_cols];

        for row in &rows {
            for (i, cell) in row.iter().enumerate() {
                col_widths[i] = col_widths[i].max(cell.chars().count());
            }
        }

        // Check if table exceeds available width and adjust if needed
        if let Some(max_width) = self.wrap_width {
            // Calculate total table width: borders + separators + cell content
            // Format: "│ cell1 │ cell2 │ cell3 │"
            let separators_width = (num_cols - 1) * 3; // " │ " between columns
            let borders_width = 4; // "│ " at start and " │" at end
            let total_width: usize =
                col_widths.iter().sum::<usize>() + separators_width + borders_width;

            if total_width > max_width {
                // Table is too wide, need to constrain column widths
                let available_for_cells =
                    max_width.saturating_sub(separators_width + borders_width);

                // Distribute available width proportionally, but ensure minimum width of 3 chars
                let total_desired: usize = col_widths.iter().sum();
                if total_desired > 0 {
                    for width in &mut col_widths {
                        let proportion = (*width as f64) / (total_desired as f64);
                        let new_width =
                            ((available_for_cells as f64) * proportion).floor() as usize;
                        *width = new_width.max(3); // Minimum 3 chars per column
                    }
                }
            }
        }

        // Render table rows
        for (row_idx, row) in rows.iter().enumerate() {
            let mut line_spans = Vec::new();
            line_spans.push(Span::raw("│ "));

            for (col_idx, cell) in row.iter().enumerate() {
                let width = col_widths[col_idx];

                // Truncate cell content if it's longer than allocated width
                // Use char-based truncation to avoid UTF-8 boundary issues
                let cell_content = if cell.chars().count() > width {
                    if width > 3 {
                        // Truncate and add ellipsis
                        let truncate_at = width.saturating_sub(3);
                        let truncated: String = cell.chars().take(truncate_at).collect();
                        format!("{}...", truncated)
                    } else {
                        // Too narrow for ellipsis, just truncate
                        cell.chars().take(width).collect()
                    }
                } else {
                    cell.clone()
                };

                let padding = " ".repeat(width.saturating_sub(cell_content.chars().count()));

                // Style header row differently
                if row_idx == 0 {
                    line_spans.push(Span::styled(cell_content, Style::new().bold()));
                } else {
                    line_spans.push(Span::raw(cell_content));
                }
                line_spans.push(Span::raw(padding));

                if col_idx < row.len() - 1 {
                    line_spans.push(Span::raw(" │ "));
                }
            }

            line_spans.push(Span::raw(" │"));
            self.push_line(Line::from(line_spans));

            // Add separator after header row
            if row_idx == 0 {
                let mut separator_spans = Vec::new();
                separator_spans.push(Span::raw("├─"));

                for (col_idx, &width) in col_widths.iter().enumerate() {
                    separator_spans.push(Span::raw("─".repeat(width)));
                    if col_idx < col_widths.len() - 1 {
                        separator_spans.push(Span::raw("─┼─"));
                    }
                }

                separator_spans.push(Span::raw("─┤"));
                self.push_line(Line::from(separator_spans));
            }
        }
    }
}
