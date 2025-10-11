use crate::{Chunk, Chunker, ChunkerError};
use tree_sitter::{Node, Parser};

pub struct PythonChunker;

impl PythonChunker {
    pub fn new() -> Self {
        Self
    }

    fn is_docstring_expression(node: Node) -> bool {
        if node.kind() != "expression_statement" {
            return false;
        }
        if let Some(first) = node.child(0) {
            return first.kind() == "string";
        }
        false
    }

    fn collect_chunks<'a>(
        node: Node<'a>,
        source_code: &str,
        file_name: &str,
        chunks: &mut Vec<Chunk>,
    ) {
        let mut current_chunk = String::new();
        let mut start_byte: Option<usize> = None;
        let mut end_byte = 0usize;
        let mut current_kind = String::new();
        let mut is_new_chunk = true;

        let mut comment_buffer = String::new();
        let mut comment_start_byte: Option<usize> = None;

        let mut doc_buffer = String::new();
        let mut doc_start_byte: Option<usize> = None;

        let mut cursor = node.walk();
        let children = node.children(&mut cursor).collect::<Vec<_>>();
        let mut i = 0;

        while i < children.len() {
            let child = children[i];
            let kind = child.kind();
            let text = child
                .utf8_text(source_code.as_bytes())
                .unwrap_or("[invalid]")
                .to_string();

            if kind == "comment" {
                comment_buffer = text.clone() + "\n";
                comment_start_byte = Some(child.start_byte());
                i += 1;
                continue;
            }

            let this_is_doc = Self::is_docstring_expression(child);
            if this_is_doc {
                doc_buffer = text.clone() + "\n";
                doc_start_byte = Some(child.start_byte());
                i += 1;
                continue;
            }

            let is_import = kind == "import_statement" || kind == "import_from_statement";
            let is_expression = kind == "expression_statement";

            if !current_chunk.is_empty()
                && !(is_import && current_kind == "import_group"
                    || (is_expression && current_kind == "expression_group"))
            {
                chunks.push(Chunk {
                    score: None,
                    start_byte: start_byte.unwrap().try_into().unwrap(),
                    end_byte: end_byte.try_into().unwrap(),
                    source_code: current_chunk.trim_end().to_string(),
                    file_name: file_name.to_string(),
                    kind: current_kind.clone(),
                });
                current_chunk.clear();
                is_new_chunk = true;
            }

            if is_new_chunk {
                let earliest_start = match (comment_start_byte, doc_start_byte) {
                    (Some(c), Some(d)) => Some(c.min(d)),
                    (Some(c), None) => Some(c),
                    (None, Some(d)) => Some(d),
                    _ => Some(child.start_byte()),
                };
                start_byte = earliest_start;
                current_kind = if is_import {
                    "import_group".to_string()
                } else if is_expression {
                    "expression_group".to_string()
                } else {
                    kind.to_string()
                };
                is_new_chunk = false;

                let mut trivia: Vec<(usize, String)> = Vec::new();
                if let Some(csb) = comment_start_byte {
                    if !comment_buffer.is_empty() {
                        trivia.push((csb, comment_buffer.clone()));
                    }
                }
                if let Some(dsb) = doc_start_byte {
                    if !doc_buffer.is_empty() {
                        trivia.push((dsb, doc_buffer.clone()));
                    }
                }
                trivia.sort_by_key(|(start, _)| *start);
                for (_, txt) in trivia {
                    current_chunk.push_str(&txt);
                }

                comment_buffer.clear();
                comment_start_byte = None;
                doc_buffer.clear();
                doc_start_byte = None;
            }

            current_chunk.push_str(&text);
            current_chunk.push('\n');
            end_byte = child.end_byte();

            if i + 1 < children.len() {
                let next = children[i + 1];
                let next_kind = next.kind();
                let next_is_import =
                    next_kind == "import_statement" || next_kind == "import_from_statement";
                let next_is_expression = next_kind == "expression_statement";
                let next_is_comment = next_kind == "comment";
                let next_is_doc = Self::is_docstring_expression(next);

                if ((is_import && next_is_import) || (is_expression && next_is_expression))
                    && !next_is_comment
                    && !next_is_doc
                {
                    i += 1;
                    continue;
                }
            }

            if !current_chunk.is_empty() {
                chunks.push(Chunk {
                    score: None,
                    start_byte: start_byte.unwrap().try_into().unwrap(),
                    end_byte: end_byte.try_into().unwrap(),
                    source_code: current_chunk.trim_end().to_string(),
                    file_name: file_name.to_string(),
                    kind: current_kind.clone(),
                });
                current_chunk.clear();
                is_new_chunk = true;
            }

            i += 1;
        }

        if !comment_buffer.is_empty() {
            chunks.push(Chunk {
                score: None,
                start_byte: comment_start_byte.unwrap().try_into().unwrap(),
                end_byte: (comment_start_byte.unwrap() + comment_buffer.len() - 1)
                    .try_into()
                    .unwrap(),
                source_code: comment_buffer.trim_end().to_string(),
                file_name: file_name.to_string(),
                kind: "comment".to_string(),
            });
        }
        if !doc_buffer.is_empty() {
            chunks.push(Chunk {
                score: None,
                start_byte: doc_start_byte.unwrap().try_into().unwrap(),
                end_byte: (doc_start_byte.unwrap() + doc_buffer.len() - 1)
                    .try_into()
                    .unwrap(),
                source_code: doc_buffer.trim_end().to_string(),
                file_name: file_name.to_string(),
                kind: "docstring".to_string(),
            });
        }
    }
}

impl Chunker for PythonChunker {
    fn chunk_file(&self, file_path: &str, source_code: &str) -> Result<Vec<Chunk>, ChunkerError> {
        let mut parser = Parser::new();
        let language = tree_sitter::Language::from(tree_sitter_python::LANGUAGE);
        parser
            .set_language(&language)
            .map_err(|e| ChunkerError::ParseError(e.to_string()))?;

        let tree = parser
            .parse(source_code, None)
            .ok_or_else(|| ChunkerError::ParseError("Failed to parse file".to_string()))?;

        let mut chunks = Vec::new();
        Self::collect_chunks(tree.root_node(), source_code, file_path, &mut chunks);
        Ok(chunks)
    }

    fn supported_extensions(&self) -> &[&str] {
        &["py"]
    }
}
