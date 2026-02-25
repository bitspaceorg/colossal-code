use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

pub mod javascript;
pub mod python;
pub mod rust;
pub mod typescript;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Chunk {
    /// Relevance score (optional, used for search results)
    pub score: Option<f32>,
    /// Start byte position in the file
    pub start_byte: u64,
    /// End byte position in the file
    pub end_byte: u64,
    /// The actual source code content
    pub source_code: String,
    /// File name or path
    pub file_name: String,
    /// Type of chunk (e.g., "function_definition", "class_definition", "import")
    pub kind: String,
}

#[derive(Error, Debug)]
pub enum ChunkerError {
    #[error("Failed to read file: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Failed to parse file: {0}")]
    ParseError(String),

    #[error("Unsupported file type: {0}")]
    UnsupportedFileType(String),
}

/// Trait for language-specific chunkers
pub trait Chunker: Send + Sync {
    /// Parse a file into semantic chunks
    fn chunk_file(&self, file_path: &str, source_code: &str) -> Result<Vec<Chunk>, ChunkerError>;

    /// Parse a specific region of a file into chunks
    fn chunk_region(
        &self,
        file_path: &str,
        source_code: &str,
        start_byte: usize,
        end_byte: usize,
    ) -> Result<Vec<Chunk>, ChunkerError> {
        // Default implementation: chunk the whole file and filter by byte range
        let chunks = self.chunk_file(file_path, source_code)?;
        Ok(chunks
            .into_iter()
            .filter(|chunk| {
                // Include chunk if it overlaps with the region
                chunk.start_byte < end_byte as u64 && chunk.end_byte > start_byte as u64
            })
            .collect())
    }

    /// Get the file extensions this chunker supports
    fn supported_extensions(&self) -> &[&str];
}

/// Factory for creating chunkers based on file extension
pub struct ChunkerFactory;

impl ChunkerFactory {
    /// Get the appropriate chunker for a file based on its extension
    pub fn get_chunker(file_path: &Path) -> Result<Box<dyn Chunker>, ChunkerError> {
        let extension = file_path
            .extension()
            .and_then(|s| s.to_str())
            .ok_or_else(|| ChunkerError::UnsupportedFileType("No file extension".to_string()))?;

        match extension {
            "py" => Ok(Box::new(python::PythonChunker::new())),
            "rs" => Ok(Box::new(rust::RustChunker::new())),
            "js" | "jsx" => Ok(Box::new(javascript::JavaScriptChunker::new())),
            "ts" | "tsx" => Ok(Box::new(typescript::TypeScriptChunker::new())),
            _ => Err(ChunkerError::UnsupportedFileType(extension.to_string())),
        }
    }

    /// Check if a file type is supported
    pub fn is_supported(file_path: &Path) -> bool {
        file_path
            .extension()
            .and_then(|s| s.to_str())
            .map(|ext| matches!(ext, "py" | "rs" | "js" | "jsx" | "ts" | "tsx"))
            .unwrap_or(false)
    }

    /// Get all supported extensions
    pub fn supported_extensions() -> Vec<&'static str> {
        vec!["py", "rs", "js", "jsx", "ts", "tsx"]
    }
}

/// Chunk a file using the appropriate chunker
pub fn chunk_file(file_path: &str) -> Result<Vec<Chunk>, ChunkerError> {
    let path = Path::new(file_path);
    let chunker = ChunkerFactory::get_chunker(path)?;
    let source_code = std::fs::read_to_string(file_path)?;
    chunker.chunk_file(file_path, &source_code)
}

/// Chunk a specific region of a file
pub fn chunk_region(
    file_path: &str,
    start_byte: usize,
    end_byte: usize,
) -> Result<Vec<Chunk>, ChunkerError> {
    let path = Path::new(file_path);
    let chunker = ChunkerFactory::get_chunker(path)?;
    let source_code = std::fs::read_to_string(file_path)?;
    chunker.chunk_region(file_path, &source_code, start_byte, end_byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supported_extensions() {
        assert!(ChunkerFactory::is_supported(Path::new("test.py")));
        assert!(ChunkerFactory::is_supported(Path::new("test.rs")));
        assert!(ChunkerFactory::is_supported(Path::new("test.js")));
        assert!(ChunkerFactory::is_supported(Path::new("test.ts")));
        assert!(!ChunkerFactory::is_supported(Path::new("test.txt")));
    }
}
