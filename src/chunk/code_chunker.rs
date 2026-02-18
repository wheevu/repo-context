//! Code-aware chunking.

use crate::chunk::line_chunker::LineChunker;
use crate::domain::{Chunk, FileInfo};
use crate::utils::{estimate_tokens, stable_hash};
use tree_sitter::{Language, Parser};

pub struct CodeChunker;

pub fn supported_tree_sitter_languages() -> &'static [&'static str] {
    &["python", "rust", "javascript", "typescript", "go"]
}

impl Default for CodeChunker {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeChunker {
    pub fn new() -> Self {
        Self
    }

    pub fn chunk(
        &self,
        file_info: &FileInfo,
        content: &str,
        max_tokens: usize,
        overlap_tokens: usize,
    ) -> Vec<Chunk> {
        if let Some(chunks) = chunk_with_tree_sitter(file_info, content, max_tokens, overlap_tokens)
        {
            if !chunks.is_empty() {
                return chunks;
            }
        }

        let lines: Vec<&str> = content.split_inclusive('\n').collect();
        if lines.is_empty() {
            return Vec::new();
        }

        let boundaries = find_definition_boundaries(&lines, &file_info.language);
        if boundaries.len() <= 1 {
            return LineChunker::new().chunk(file_info, content, max_tokens, overlap_tokens);
        }

        let mut chunks = Vec::new();
        let line_chunker = LineChunker::new();

        for window in boundaries.windows(2) {
            let start = window[0];
            let end = window[1];
            if end <= start || start >= lines.len() {
                continue;
            }

            let section_content = lines[start..end.min(lines.len())].join("");
            if section_content.trim().is_empty() {
                continue;
            }

            if estimate_tokens(&section_content) <= max_tokens {
                chunks.push(Chunk {
                    id: stable_hash(&section_content, &file_info.relative_path, start + 1, end),
                    path: file_info.relative_path.clone(),
                    language: file_info.language.clone(),
                    start_line: start + 1,
                    end_line: end,
                    token_estimate: estimate_tokens(&section_content),
                    content: section_content,
                    priority: file_info.priority,
                    tags: file_info.tags.clone(),
                });
            } else {
                let nested =
                    line_chunker.chunk(file_info, &section_content, max_tokens, overlap_tokens);
                for mut chunk in nested {
                    chunk.start_line += start;
                    chunk.end_line += start;
                    chunk.id =
                        stable_hash(&chunk.content, &chunk.path, chunk.start_line, chunk.end_line);
                    chunks.push(chunk);
                }
            }
        }

        if chunks.is_empty() {
            return LineChunker::new().chunk(file_info, content, max_tokens, overlap_tokens);
        }

        chunks.sort_by(|a, b| a.start_line.cmp(&b.start_line));
        chunks
    }
}

fn chunk_with_tree_sitter(
    file_info: &FileInfo,
    content: &str,
    max_tokens: usize,
    overlap_tokens: usize,
) -> Option<Vec<Chunk>> {
    let (language, definition_kinds): (Language, &[&str]) = match file_info.language.as_str() {
        "python" => (
            tree_sitter_python::LANGUAGE.into(),
            &["function_definition", "class_definition", "decorated_definition"],
        ),
        "rust" => (
            tree_sitter_rust::LANGUAGE.into(),
            &["function_item", "impl_item", "struct_item", "enum_item", "trait_item", "mod_item"],
        ),
        "javascript" => (
            tree_sitter_javascript::LANGUAGE.into(),
            &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "lexical_declaration",
            ],
        ),
        "typescript" => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "interface_declaration",
                "type_alias_declaration",
                "lexical_declaration",
            ],
        ),
        "go" => (
            tree_sitter_go::LANGUAGE.into(),
            &[
                "function_declaration",
                "method_declaration",
                "type_declaration",
                "const_declaration",
                "var_declaration",
            ],
        ),
        _ => return None,
    };

    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;

    let tree = parser.parse(content, None)?;
    let root = tree.root_node();

    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    if lines.is_empty() {
        return Some(Vec::new());
    }

    let mut boundaries = vec![0usize];
    for i in 0..root.named_child_count() {
        if let Some(child) = root.named_child(i) {
            let kind = child.kind();
            if definition_kinds.contains(&kind) {
                let row = child.start_position().row;
                if row > 0 {
                    boundaries.push(row);
                }
            }
        }
    }
    boundaries.push(lines.len());
    boundaries.sort_unstable();
    boundaries.dedup();

    if boundaries.len() <= 2 {
        return Some(Vec::new());
    }

    Some(chunk_by_boundaries(file_info, &lines, &boundaries, max_tokens, overlap_tokens))
}

fn chunk_by_boundaries(
    file_info: &FileInfo,
    lines: &[&str],
    boundaries: &[usize],
    max_tokens: usize,
    overlap_tokens: usize,
) -> Vec<Chunk> {
    let line_chunker = LineChunker::new();
    let mut chunks = Vec::new();

    for window in boundaries.windows(2) {
        let start = window[0];
        let end = window[1];
        if end <= start || start >= lines.len() {
            continue;
        }

        let section_content = lines[start..end.min(lines.len())].join("");
        if section_content.trim().is_empty() {
            continue;
        }

        if estimate_tokens(&section_content) <= max_tokens {
            chunks.push(Chunk {
                id: stable_hash(&section_content, &file_info.relative_path, start + 1, end),
                path: file_info.relative_path.clone(),
                language: file_info.language.clone(),
                start_line: start + 1,
                end_line: end,
                token_estimate: estimate_tokens(&section_content),
                content: section_content,
                priority: file_info.priority,
                tags: file_info.tags.clone(),
            });
        } else {
            let nested =
                line_chunker.chunk(file_info, &section_content, max_tokens, overlap_tokens);
            for mut chunk in nested {
                chunk.start_line += start;
                chunk.end_line += start;
                chunk.id =
                    stable_hash(&chunk.content, &chunk.path, chunk.start_line, chunk.end_line);
                chunks.push(chunk);
            }
        }
    }

    chunks.sort_by(|a, b| a.start_line.cmp(&b.start_line));
    chunks
}

fn find_definition_boundaries(lines: &[&str], language: &str) -> Vec<usize> {
    let mut boundaries = vec![0usize];

    for (idx, line) in lines.iter().enumerate() {
        if idx == 0 {
            continue;
        }

        let trimmed = line.trim_start();
        let is_boundary = match language {
            "python" => {
                trimmed.starts_with("def ")
                    || trimmed.starts_with("class ")
                    || trimmed.starts_with("async def ")
            }
            "rust" => {
                trimmed.starts_with("fn ")
                    || trimmed.starts_with("pub fn ")
                    || trimmed.starts_with("impl ")
                    || trimmed.starts_with("struct ")
                    || trimmed.starts_with("enum ")
                    || trimmed.starts_with("trait ")
            }
            "javascript" | "typescript" => {
                trimmed.starts_with("function ")
                    || trimmed.starts_with("export function ")
                    || trimmed.starts_with("export const ")
                    || trimmed.starts_with("class ")
                    || trimmed.starts_with("export class ")
                    || trimmed.starts_with("interface ")
                    || trimmed.starts_with("type ")
            }
            "go" => {
                trimmed.starts_with("func ")
                    || trimmed.starts_with("type ")
                    || trimmed.starts_with("var ")
                    || trimmed.starts_with("const ")
            }
            _ => {
                trimmed.starts_with("def ")
                    || trimmed.starts_with("class ")
                    || trimmed.starts_with("fn ")
                    || trimmed.starts_with("function ")
            }
        };

        if is_boundary {
            boundaries.push(idx);
        }
    }

    boundaries.push(lines.len());
    boundaries.dedup();
    boundaries
}

#[cfg(test)]
mod tests {
    use super::CodeChunker;
    use crate::domain::FileInfo;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    #[test]
    fn code_chunker_splits_at_definitions() {
        let info = FileInfo {
            path: PathBuf::from("/tmp/main.py"),
            relative_path: "main.py".to_string(),
            size_bytes: 0,
            extension: ".py".to_string(),
            language: "python".to_string(),
            id: "x".to_string(),
            priority: 0.8,
            token_estimate: 0,
            tags: BTreeSet::new(),
            is_readme: false,
            is_config: false,
            is_doc: false,
        };

        let content = "def a():\n    pass\n\ndef b():\n    pass\n\ndef c():\n    pass\n";
        let chunks = CodeChunker::new().chunk(&info, content, 20, 0);
        assert!(!chunks.is_empty());
        assert!(chunks.len() >= 2);
        assert!(chunks[0].start_line >= 1);
    }

    #[test]
    fn code_chunker_supports_rust_tree_sitter() {
        let info = FileInfo {
            path: PathBuf::from("/tmp/main.rs"),
            relative_path: "main.rs".to_string(),
            size_bytes: 0,
            extension: ".rs".to_string(),
            language: "rust".to_string(),
            id: "x".to_string(),
            priority: 0.8,
            token_estimate: 0,
            tags: BTreeSet::new(),
            is_readme: false,
            is_config: false,
            is_doc: false,
        };

        let content = "struct S;\nfn a() {}\nimpl S { fn b(&self) {} }\nfn c() {}\n";
        let chunks = CodeChunker::new().chunk(&info, content, 20, 0);
        assert!(!chunks.is_empty());
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn code_chunker_supports_go_tree_sitter() {
        let info = FileInfo {
            path: PathBuf::from("/tmp/main.go"),
            relative_path: "main.go".to_string(),
            size_bytes: 0,
            extension: ".go".to_string(),
            language: "go".to_string(),
            id: "x".to_string(),
            priority: 0.8,
            token_estimate: 0,
            tags: BTreeSet::new(),
            is_readme: false,
            is_config: false,
            is_doc: false,
        };

        let content = "package main\n\nfunc a() {}\n\nfunc b() {}\n\nfunc main() {}\n";
        let chunks = CodeChunker::new().chunk(&info, content, 20, 0);
        assert!(!chunks.is_empty());
        assert!(chunks.len() >= 2);
    }
}
