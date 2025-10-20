use std::path::PathBuf;

/// Configuration for markdown rendering, including citation rewriting settings.
#[derive(Clone, Debug)]
pub struct RendererConfig {
    /// URI scheme for file citations (e.g., "vscode", "file")
    /// When None, citations are not rewritten as clickable links
    pub file_opener_scheme: Option<String>,

    /// Current working directory for resolving relative file paths in citations
    pub cwd: PathBuf,

    /// Optional width for word wrapping
    pub width: Option<usize>,
}

impl RendererConfig {
    /// Create a new renderer config with the given current working directory
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            file_opener_scheme: None,
            cwd,
            width: None,
        }
    }

    /// Set the file opener scheme for clickable file citations
    pub fn with_scheme(mut self, scheme: impl Into<String>) -> Self {
        self.file_opener_scheme = Some(scheme.into());
        self
    }

    /// Set the width for word wrapping
    pub fn with_width(mut self, width: usize) -> Self {
        self.width = Some(width);
        self
    }
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            file_opener_scheme: None,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            width: None,
        }
    }
}
