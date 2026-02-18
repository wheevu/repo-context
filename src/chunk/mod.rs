//! Content chunking strategies

use crate::domain::{Chunk, FileInfo};
use crate::utils::{estimate_tokens, read_file_safe, stable_hash};
use anyhow::Result;

use code_chunker::CodeChunker;
use line_chunker::LineChunker;
use markdown_chunker::MarkdownChunker;

pub mod code_chunker;
pub mod line_chunker;
pub mod markdown_chunker;

#[allow(dead_code)]
pub fn chunk_file(file_info: &FileInfo) -> Result<Vec<Chunk>> {
    chunk_file_with_options(file_info, 800, 120)
}

pub fn chunk_file_with_options(
    file_info: &FileInfo,
    max_tokens: usize,
    overlap_tokens: usize,
) -> Result<Vec<Chunk>> {
    let (content, _encoding) = read_file_safe(&file_info.path, None, None)?;
    chunk_content(file_info, &content, max_tokens, overlap_tokens)
}

/// Chunk pre-loaded (and optionally pre-redacted) content.  Callers that want
/// to redact before chunking should read the file, apply the redactor, then
/// call this instead of `chunk_file_with_options`.
pub fn chunk_content(
    file_info: &FileInfo,
    content: &str,
    max_tokens: usize,
    overlap_tokens: usize,
) -> Result<Vec<Chunk>> {
    let chunker_kind = chunker_for_language(&file_info.language);
    let chunks = match chunker_kind {
        ChunkerKind::Markdown => {
            MarkdownChunker::new().chunk(file_info, content, max_tokens, overlap_tokens)
        }
        ChunkerKind::Code => {
            CodeChunker::new().chunk(file_info, content, max_tokens, overlap_tokens)
        }
        ChunkerKind::Line => {
            LineChunker::new().chunk(file_info, content, max_tokens, overlap_tokens)
        }
    };

    if !chunks.is_empty() {
        return Ok(chunks);
    }

    let line_count = content.lines().count().max(1);
    let token_estimate = estimate_tokens(content);
    let id = stable_hash(content, &file_info.relative_path, 1, line_count);

    Ok(vec![Chunk {
        id,
        path: file_info.relative_path.clone(),
        language: file_info.language.clone(),
        start_line: 1,
        end_line: line_count,
        content: content.to_string(),
        priority: file_info.priority,
        tags: file_info.tags.clone(),
        token_estimate,
    }])
}

#[allow(dead_code)]
pub fn coalesce_small_chunks(chunks: Vec<Chunk>, _min_tokens: usize) -> Vec<Chunk> {
    coalesce_small_chunks_with_max(chunks, 200, 800)
}

pub fn coalesce_small_chunks_with_max(
    chunks: Vec<Chunk>,
    min_tokens: usize,
    max_tokens: usize,
) -> Vec<Chunk> {
    if chunks.is_empty() {
        return Vec::new();
    }

    let mut sorted = chunks;
    sorted.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.start_line.cmp(&b.start_line)));

    let mut result: Vec<Chunk> = Vec::new();

    for chunk in sorted {
        if let Some(last) = result.last_mut() {
            if last.path == chunk.path && chunk.start_line <= last.end_line + 1 {
                let combined_tokens = last.token_estimate + chunk.token_estimate;
                let can_merge = (last.token_estimate < min_tokens
                    || chunk.token_estimate < min_tokens)
                    && combined_tokens <= max_tokens;

                if can_merge {
                    let merged_content = merge_chunk_content(last, &chunk);
                    let merged_tags = last.tags.union(&chunk.tags).cloned().collect();
                    last.end_line = chunk.end_line;
                    last.content = merged_content.clone();
                    last.priority = last.priority.max(chunk.priority);
                    last.tags = merged_tags;
                    last.token_estimate = estimate_tokens(&merged_content);
                    last.id =
                        stable_hash(&merged_content, &last.path, last.start_line, last.end_line);
                    continue;
                }
            }
        }

        result.push(chunk);
    }

    result
}

fn merge_chunk_content(current: &Chunk, next: &Chunk) -> String {
    if next.start_line > current.end_line {
        // No overlap: simple concatenation
        format!("{}{}", current.content, next.content)
    } else {
        // Overlapping: keep non-overlapping prefix of current + all of next.
        // overlap_start = number of lines in current that precede the overlap region.
        let overlap_start = next.start_line.saturating_sub(current.start_line);
        let current_lines: Vec<&str> = current.content.split_inclusive('\n').collect();
        let prefix = if overlap_start < current_lines.len() {
            current_lines[..overlap_start].join("")
        } else {
            current.content.clone()
        };
        format!("{}{}", prefix, next.content)
    }
}

enum ChunkerKind {
    Code,
    Markdown,
    Line,
}

fn chunker_for_language(language: &str) -> ChunkerKind {
    match language {
        "markdown" | "restructuredtext" | "asciidoc" => ChunkerKind::Markdown,
        "python" | "javascript" | "typescript" | "go" | "java" | "rust" | "c" | "cpp"
        | "csharp" | "ruby" | "php" | "swift" | "kotlin" | "scala" => ChunkerKind::Code,
        _ => ChunkerKind::Line,
    }
}
