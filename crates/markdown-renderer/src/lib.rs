//! A terminal-based markdown renderer with streaming support.
//!
//! This library provides markdown rendering capabilities optimized for terminal UIs,
//! with support for:
//! - Streaming markdown rendering (accumulate and render incrementally)
//! - Word wrapping with configurable width
//! - File citation rewriting for clickable links
//! - Full pulldown-cmark markdown support
//! - Ratatui-based styled text output

mod citation;
mod config;
mod line_utils;
mod renderer;
mod stream;
mod wrapping;

pub use config::RendererConfig;
pub use renderer::render_markdown_text;
pub use stream::MarkdownStreamCollector;

use ratatui::text::Line;
use std::path::Path;

/// Render markdown text and append the resulting lines to the output vector.
///
/// # Arguments
/// * `markdown_source` - The markdown text to render
/// * `width` - Optional width for word wrapping
/// * `lines` - Output vector to append rendered lines to
/// * `config` - Renderer configuration including citation settings
pub fn append_markdown(
    markdown_source: &str,
    width: Option<usize>,
    lines: &mut Vec<Line<'static>>,
    config: &RendererConfig,
) {
    let rendered = renderer::render_markdown_text_with_citations(
        markdown_source,
        width,
        config.file_opener_scheme.as_deref(),
        &config.cwd,
    );
    line_utils::push_owned_lines(&rendered.lines, lines);
}

/// Render markdown text and append the resulting lines with custom settings.
///
/// # Arguments
/// * `markdown_source` - The markdown text to render
/// * `width` - Optional width for word wrapping
/// * `lines` - Output vector to append rendered lines to
/// * `scheme` - Optional URI scheme for file citations (e.g., "vscode")
/// * `cwd` - Current working directory for resolving relative paths
pub fn append_markdown_with_settings(
    markdown_source: &str,
    width: Option<usize>,
    lines: &mut Vec<Line<'static>>,
    scheme: Option<&str>,
    cwd: &Path,
) {
    let rendered = renderer::render_markdown_text_with_citations(
        markdown_source,
        width,
        scheme,
        cwd,
    );
    line_utils::push_owned_lines(&rendered.lines, lines);
}
