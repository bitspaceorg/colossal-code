use crate::{Chunk, Chunker, ChunkerError};
use tree_sitter::Parser;

pub struct RustChunker;

impl RustChunker {
    pub fn new() -> Self {
        Self
    }
}

impl Chunker for RustChunker {
    fn chunk_file(&self, file_path: &str, source_code: &str) -> Result<Vec<Chunk>, ChunkerError> {
        let mut parser = Parser::new();
        let language = tree_sitter::Language::from(tree_sitter_rust::LANGUAGE);
        parser
            .set_language(&language)
            .map_err(|e| ChunkerError::ParseError(e.to_string()))?;

        let tree = parser
            .parse(source_code, None)
            .ok_or_else(|| ChunkerError::ParseError("Failed to parse file".to_string()))?;

        let mut chunks = Vec::new();
        let root = tree.root_node();

        // Simple implementation: chunk by top-level items
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            let kind = child.kind();
            if matches!(
                kind,
                "function_item"
                    | "struct_item"
                    | "enum_item"
                    | "impl_item"
                    | "trait_item"
                    | "mod_item"
                    | "const_item"
                    | "static_item"
                    | "type_item"
            ) {
                let text = child
                    .utf8_text(source_code.as_bytes())
                    .unwrap_or("[invalid]")
                    .to_string();

                chunks.push(Chunk {
                    score: None,
                    start_byte: child.start_byte() as u64,
                    end_byte: child.end_byte() as u64,
                    source_code: text,
                    file_name: file_path.to_string(),
                    kind: kind.to_string(),
                });
            }
        }

        Ok(chunks)
    }

    fn supported_extensions(&self) -> &[&str] {
        &["rs"]
    }
}
