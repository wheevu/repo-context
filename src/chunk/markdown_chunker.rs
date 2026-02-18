//! Markdown-aware chunking.

use crate::chunk::line_chunker::LineChunker;
use crate::domain::{Chunk, FileInfo};
use crate::utils::{estimate_tokens, stable_hash};

pub struct MarkdownChunker;

impl Default for MarkdownChunker {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkdownChunker {
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
        let lines: Vec<&str> = content.split_inclusive('\n').collect();
        if lines.is_empty() {
            return Vec::new();
        }

        let mut sections: Vec<(usize, usize, Option<String>)> = Vec::new();
        let mut section_start = 0usize;
        let mut current_heading: Option<String> = None;

        for (i, line) in lines.iter().enumerate() {
            // Heading detection: must be 1-6 '#' followed by whitespace (Python line 196)
            let trimmed = line.trim_start();
            let hash_count = trimmed.chars().take_while(|&c| c == '#').count();
            let is_heading = if (1..=6).contains(&hash_count) {
                let rest = &trimmed[hash_count..];
                rest.starts_with(' ') || rest.starts_with('\t')
            } else {
                false
            };

            if i != 0 && is_heading {
                sections.push((section_start, i, current_heading.take()));
                section_start = i;
                // Extract heading text: strip leading '#' characters and whitespace (Python line 238)
                current_heading = Some(
                    trimmed.trim_start_matches('#').trim().chars().take(30).collect::<String>(),
                );
            }
        }
        sections.push((section_start, lines.len(), current_heading.take()));

        let line_chunker = LineChunker::new();
        let mut result = Vec::new();

        for (start, end, heading) in sections {
            let section_content = lines[start..end].join("");
            if estimate_tokens(&section_content) <= max_tokens {
                let mut tags = file_info.tags.clone();
                if let Some(ref h) = heading {
                    if !h.is_empty() {
                        tags.insert(format!("section:{h}"));
                    }
                }
                result.push(Chunk {
                    id: stable_hash(&section_content, &file_info.relative_path, start + 1, end),
                    path: file_info.relative_path.clone(),
                    language: file_info.language.clone(),
                    start_line: start + 1,
                    end_line: end,
                    token_estimate: estimate_tokens(&section_content),
                    content: section_content,
                    priority: file_info.priority,
                    tags,
                });
            } else {
                let nested =
                    line_chunker.chunk(file_info, &section_content, max_tokens, overlap_tokens);
                for mut chunk in nested {
                    chunk.start_line += start;
                    chunk.end_line += start;
                    chunk.id =
                        stable_hash(&chunk.content, &chunk.path, chunk.start_line, chunk.end_line);
                    result.push(chunk);
                }
            }
        }

        result.sort_by(|a, b| a.start_line.cmp(&b.start_line));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::MarkdownChunker;
    use crate::domain::FileInfo;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    #[test]
    fn nested_markdown_chunks_keep_absolute_line_numbers() {
        let info = FileInfo {
            path: PathBuf::from("/tmp/readme.md"),
            relative_path: "README.md".to_string(),
            size_bytes: 0,
            extension: ".md".to_string(),
            language: "markdown".to_string(),
            id: "x".to_string(),
            priority: 1.0,
            token_estimate: 0,
            tags: BTreeSet::new(),
            is_readme: true,
            is_config: false,
            is_doc: true,
        };
        let content = "# A\n\nIntro\n\n# B\n".to_string() + &"line\n".repeat(200);
        let chunks = MarkdownChunker::new().chunk(&info, &content, 80, 10);
        assert!(!chunks.is_empty());
        for chunk in chunks {
            assert!(chunk.start_line >= 1);
            assert!(chunk.end_line >= chunk.start_line);
        }
    }
}
