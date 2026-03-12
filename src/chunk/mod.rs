//! Content chunking strategies

use crate::domain::{Chunk, FileInfo};
#[cfg(test)]
use crate::utils::read_file_safe;
use crate::utils::{estimate_tokens, stable_hash};
use anyhow::Result;

use code_chunker::CodeChunker;
use line_chunker::LineChunker;
use markdown_chunker::MarkdownChunker;

pub mod code_chunker;
pub mod line_chunker;
pub mod markdown_chunker;

/// Chunk a file with default options (800 tokens, 120 overlap).
///
/// This is primarily used in tests. In production, use [`chunk_content`] with pre-read content.
#[cfg(test)]
#[allow(dead_code)]
pub fn chunk_file(file_info: &FileInfo) -> Result<Vec<Chunk>> {
    chunk_file_with_options(file_info, 800, 120)
}

/// Chunk a file with custom token limits.
///
/// This is primarily used in tests. In production, use [`chunk_content`] with pre-read content.
#[cfg(test)]
#[allow(dead_code)]
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

/// Coalesce small chunks with default max tokens (800).
///
/// This is primarily used in tests. In production, use [`coalesce_small_chunks_with_max`].
#[cfg(test)]
#[allow(dead_code)]
pub fn coalesce_small_chunks(chunks: Vec<Chunk>, _min_tokens: usize) -> Vec<Chunk> {
    coalesce_small_chunks_with_max(chunks, 200, 800)
}

/// Coalesce small chunks that are adjacent or overlap.
///
/// Merges chunks that are:
/// - From the same file
/// - Adjacent or overlapping (next starts within 1 line of previous end)
/// - At least one is below `min_tokens` AND combined size is below `max_tokens`
///
/// # Arguments
/// * `chunks` - Vector of chunks to process
/// * `min_tokens` - Minimum token threshold for coalescing
/// * `max_tokens` - Maximum combined token limit
///
/// # Returns
/// Vector of coalesced chunks
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

#[cfg(test)]
mod tests {
    use super::coalesce_small_chunks_with_max;
    use crate::domain::Chunk;
    use std::collections::BTreeSet;

    fn mk_chunk(
        id: &str,
        path: &str,
        start: usize,
        end: usize,
        content: &str,
        tokens: usize,
    ) -> Chunk {
        Chunk {
            id: id.to_string(),
            path: path.to_string(),
            language: "rust".to_string(),
            start_line: start,
            end_line: end,
            content: content.to_string(),
            priority: 0.5,
            tags: BTreeSet::new(),
            token_estimate: tokens,
        }
    }

    #[test]
    fn coalesce_merges_adjacent_small_chunks() {
        let chunks = vec![
            mk_chunk("a", "src/main.rs", 1, 3, "fn a() {}\n", 10),
            mk_chunk("b", "src/main.rs", 4, 6, "fn b() {}\n", 10),
        ];

        let merged = coalesce_small_chunks_with_max(chunks, 20, 100);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].start_line, 1);
        assert_eq!(merged[0].end_line, 6);
        assert!(merged[0].content.contains("fn a()"));
        assert!(merged[0].content.contains("fn b()"));
    }

    #[test]
    fn coalesce_does_not_merge_when_combined_exceeds_max() {
        let chunks = vec![
            mk_chunk("a", "src/main.rs", 1, 3, "fn a() {}\n", 60),
            mk_chunk("b", "src/main.rs", 4, 6, "fn b() {}\n", 60),
        ];

        let merged = coalesce_small_chunks_with_max(chunks, 80, 100);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn coalesce_produces_stable_ids_for_same_input() {
        let chunks = vec![
            mk_chunk("a", "src/main.rs", 1, 3, "fn a() {}\n", 10),
            mk_chunk("b", "src/main.rs", 4, 6, "fn b() {}\n", 10),
        ];

        let first = coalesce_small_chunks_with_max(chunks.clone(), 20, 100);
        let second = coalesce_small_chunks_with_max(chunks, 20, 100);
        assert_eq!(first[0].id, second[0].id);
    }
}
