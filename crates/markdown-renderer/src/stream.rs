use ratatui::text::Line;

use crate::config::RendererConfig;
use crate::line_utils;
use crate::renderer;

/// Newline-gated accumulator that renders markdown and commits only fully
/// completed logical lines.
pub struct MarkdownStreamCollector {
    buffer: String,
    committed_line_count: usize,
    width: Option<usize>,
}

impl MarkdownStreamCollector {
    pub fn new(width: Option<usize>) -> Self {
        Self {
            buffer: String::new(),
            committed_line_count: 0,
            width,
        }
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.committed_line_count = 0;
    }

    pub fn push_delta(&mut self, delta: &str) {
        tracing::trace!("push_delta: {delta:?}");
        self.buffer.push_str(delta);
    }

    /// Render the full buffer and return only the newly completed logical lines
    /// since the last commit. When the buffer does not end with a newline, the
    /// final rendered line is considered incomplete and is not emitted.
    pub fn commit_complete_lines(&mut self, config: &RendererConfig) -> Vec<Line<'static>> {
        let source = self.buffer.clone();
        let last_newline_idx = source.rfind('\n');
        let source = if let Some(last_newline_idx) = last_newline_idx {
            source[..=last_newline_idx].to_string()
        } else {
            return Vec::new();
        };

        let text = renderer::render_markdown_text_with_citations(
            &source,
            self.width,
            config.file_opener_scheme.as_deref(),
            &config.cwd,
        );
        let rendered = text.lines;

        let mut complete_line_count = rendered.len();
        if complete_line_count > 0
            && line_utils::is_blank_line_spaces_only(&rendered[complete_line_count - 1])
        {
            complete_line_count -= 1;
        }

        if self.committed_line_count >= complete_line_count {
            return Vec::new();
        }

        let out_slice = &rendered[self.committed_line_count..complete_line_count];

        let out = out_slice.to_vec();
        self.committed_line_count = complete_line_count;
        out
    }

    /// Finalize the stream: emit all remaining lines beyond the last commit.
    /// If the buffer does not end with a newline, a temporary one is appended
    /// for rendering.
    pub fn finalize_and_drain(&mut self, config: &RendererConfig) -> Vec<Line<'static>> {
        let raw_buffer = self.buffer.clone();
        let mut source: String = raw_buffer.clone();
        if !source.ends_with('\n') {
            source.push('\n');
        }
        tracing::debug!(
            raw_len = raw_buffer.len(),
            source_len = source.len(),
            "markdown finalize (raw length: {}, rendered length: {})",
            raw_buffer.len(),
            source.len()
        );
        tracing::trace!("markdown finalize (raw source):\n---\n{source}\n---");

        let text = renderer::render_markdown_text_with_citations(
            &source,
            self.width,
            config.file_opener_scheme.as_deref(),
            &config.cwd,
        );
        let rendered = text.lines;

        let out = if self.committed_line_count >= rendered.len() {
            Vec::new()
        } else {
            rendered[self.committed_line_count..].to_vec()
        };

        // Reset collector state for next stream.
        self.clear();
        out
    }
}
