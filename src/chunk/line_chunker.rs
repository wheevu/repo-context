//! Line-based chunking.

use crate::domain::{Chunk, FileInfo};
use crate::utils::{estimate_tokens, stable_hash};

pub struct LineChunker;

impl Default for LineChunker {
    fn default() -> Self {
        Self::new()
    }
}

impl LineChunker {
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

        let total_tokens = estimate_tokens(content).max(1);
        let avg_tokens_per_line = (total_tokens / lines.len()).max(1);
        let target_lines = (max_tokens / avg_tokens_per_line).max(1);
        let overlap_lines = overlap_tokens / avg_tokens_per_line;

        let mut chunks = Vec::new();
        let mut start = 0usize;

        while start < lines.len() {
            let mut end = (start + target_lines).min(lines.len());

            if end < lines.len() {
                let window_start = start + ((target_lines as f64 * 0.8) as usize);
                let search_start = window_start.min(end);
                let search_end = (end + 10).min(lines.len());
                if let Some(boundary) = find_boundary(&lines, search_start, search_end) {
                    end = boundary;
                }
            }

            if end <= start {
                end = (start + 1).min(lines.len());
            }

            let chunk_content = lines[start..end].join("");
            if chunk_content.trim().is_empty() {
                start = end;
                continue;
            }

            let chunk = Chunk {
                id: stable_hash(&chunk_content, &file_info.relative_path, start + 1, end),
                path: file_info.relative_path.clone(),
                language: file_info.language.clone(),
                start_line: start + 1,
                end_line: end,
                token_estimate: estimate_tokens(&chunk_content),
                content: chunk_content,
                priority: file_info.priority,
                tags: file_info.tags.clone(),
            };
            chunks.push(chunk);

            let next_start = end.saturating_sub(overlap_lines);
            if next_start <= start {
                start = end;
            } else {
                start = next_start;
            }
        }

        chunks
    }
}

fn find_boundary(lines: &[&str], start: usize, end: usize) -> Option<usize> {
    let mut best_idx: Option<usize> = None;
    let mut best_weight: u32 = 0;

    for (idx, line) in lines.iter().enumerate().take(end).skip(start) {
        let trimmed = line.trim_start();
        let weight: u32 = if trimmed.starts_with("class ") {
            4
        } else if trimmed.starts_with("def ")
            || trimmed.starts_with("fn ")
            || trimmed.starts_with("pub fn ")
            || trimmed.starts_with("async fn ")
            || trimmed.starts_with("pub async fn ")
        {
            3
        } else if line.trim().is_empty() {
            1
        } else {
            0
        };

        if weight > best_weight {
            best_weight = weight;
            best_idx = Some(idx);
        }
    }

    best_idx
}
