//! File scanner implementation with gitignore support

use crate::domain::ProjectProfile;
use crate::domain::{FileDisposition, FileDispositionReason, FileInfo, ScanStats};
use crate::godot::{file_policy, FilePolicy};
use crate::utils::{is_binary_file, is_likely_minified, normalize_path};
use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const DEFAULT_SAMPLE_SIZE: usize = 8192;
const MAX_UNSEEN_FILES: usize = 50_000;

/// File scanner that discovers files in a repository while respecting gitignore rules.
pub struct FileScanner {
    root_path: PathBuf,
    include_extensions: Vec<String>,
    exclude_globs: Vec<String>,
    max_file_bytes: u64,
    respect_gitignore: bool,
    follow_symlinks: bool,
    skip_minified: bool,
    max_line_length: usize,
    stats: ScanStats,
    dispositions: Vec<FileDisposition>,
    godot_profile: bool,
}

impl FileScanner {
    /// Create a new FileScanner from a root path and config.
    pub fn from_config(root_path: PathBuf, config: &crate::domain::Config) -> Self {
        Self {
            root_path,
            include_extensions: config.include_extensions.iter().cloned().collect(),
            exclude_globs: config.exclude_globs.iter().cloned().collect(),
            max_file_bytes: config.max_file_bytes,
            respect_gitignore: config.respect_gitignore,
            follow_symlinks: config.follow_symlinks,
            skip_minified: config.skip_minified,
            max_line_length: 5000,
            stats: ScanStats::default(),
            dispositions: Vec::new(),
            godot_profile: config.profile == ProjectProfile::Godot,
        }
    }

    /// Create a new FileScanner with default settings.
    pub fn new(root_path: PathBuf) -> Self {
        Self {
            root_path,
            include_extensions: crate::domain::default_include_extensions()
                .iter()
                .map(|s| s.to_string())
                .collect(),
            exclude_globs: crate::domain::default_exclude_globs()
                .iter()
                .map(|s| s.to_string())
                .collect(),
            max_file_bytes: 1_048_576, // 1MB
            respect_gitignore: true,
            follow_symlinks: false,
            skip_minified: true,
            max_line_length: 5000,
            stats: ScanStats::default(),
            dispositions: Vec::new(),
            godot_profile: false,
        }
    }

    /// Set file extensions to include (e.g., ".rs", ".py")
    #[must_use]
    pub fn include_extensions(mut self, extensions: Vec<String>) -> Self {
        self.include_extensions = extensions;
        self
    }

    /// Set glob patterns to exclude
    #[must_use]
    pub fn exclude_globs(mut self, globs: Vec<String>) -> Self {
        self.exclude_globs = globs;
        self
    }

    /// Set maximum file size in bytes
    #[must_use]
    pub fn max_file_bytes(mut self, max_bytes: u64) -> Self {
        self.max_file_bytes = max_bytes;
        self
    }

    /// Set whether to respect gitignore files
    #[must_use]
    pub fn respect_gitignore(mut self, respect: bool) -> Self {
        self.respect_gitignore = respect;
        self
    }

    /// Set whether to follow symbolic links
    #[must_use]
    pub fn follow_symlinks(mut self, follow: bool) -> Self {
        self.follow_symlinks = follow;
        self
    }

    /// Set whether to skip minified files
    #[must_use]
    pub fn skip_minified(mut self, skip: bool) -> Self {
        self.skip_minified = skip;
        self
    }

    fn build_exclude_globset(&self) -> Result<GlobSet> {
        let mut builder = GlobSetBuilder::new();
        for pattern in &self.exclude_globs {
            match Glob::new(pattern) {
                Ok(glob) => {
                    builder.add(glob);
                }
                Err(e) => {
                    tracing::warn!("Invalid exclude glob pattern '{}': {}", pattern, e);
                }
            }
        }
        Ok(builder.build()?)
    }

    /// Check if a file extension should be included
    fn should_include_extension(&self, path: &Path) -> bool {
        if is_special_repo_file(path) {
            return true;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();

        // Handle files without extension but with known names
        if ext.is_empty() {
            let known_extensionless = [
                "makefile",
                "dockerfile",
                "rakefile",
                "gemfile",
                "procfile",
                "vagrantfile",
                "jenkinsfile",
            ];
            return known_extensionless.contains(&name.as_str());
        }

        // Add leading dot if not present for comparison
        let ext_with_dot = if ext.starts_with('.') { ext } else { format!(".{}", ext) };

        self.include_extensions.contains(&ext_with_dot)
    }

    /// Scan the repository and return list of FileInfo objects.
    ///
    /// Files are returned in deterministic sorted order by relative path.
    pub fn scan(&mut self) -> Result<Vec<FileInfo>> {
        self.stats = ScanStats::default();
        self.dispositions.clear();

        // Pre-allocate with reasonable capacity to avoid reallocations during growth
        let mut files: Vec<(PathBuf, String)> = Vec::with_capacity(1024);
        let exclude_globset = self.build_exclude_globset()?;
        // Resolve the root once; containment checks require it. When
        // resolution fails, degrade to rejecting every symlink entry in the
        // walk below rather than scanning unguarded.
        let canonical_root = match self.root_path.canonicalize() {
            Ok(path) => Some(path),
            Err(error) => {
                tracing::warn!(
                    "cannot canonicalize scan root {}: {}; symlink entries will be rejected closed",
                    self.root_path.display(),
                    error
                );
                None
            }
        };
        let rejected_symlink_dirs = Arc::new(Mutex::new(Vec::<PathBuf>::new()));

        // Directory filter function matching Python's _walk_files behavior
        let canonical_root_for_filter = canonical_root.clone();
        let rejected_symlink_dirs_for_filter = Arc::clone(&rejected_symlink_dirs);
        let dir_filter = move |entry: &ignore::DirEntry| -> bool {
            // `ignore` follows a directory symlink before invoking this filter.
            // Reject an escaping target here so its children are never visited.
            if let Some(canonical_root) = canonical_root_for_filter.as_deref() {
                if entry.path_is_symlink()
                    && entry.path().is_dir()
                    && !is_path_within_root(entry.path(), canonical_root)
                {
                    if let Ok(mut rejected) = rejected_symlink_dirs_for_filter.lock() {
                        rejected.push(entry.path().to_path_buf());
                    }
                    return false;
                }
            }

            if let Some(file_type) = entry.file_type() {
                if file_type.is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        // Skip known large directories unconditionally (Python lines 880-887)
                        if matches!(
                            name,
                            "node_modules" | "__pycache__" | ".git" | ".venv" | "venv" | "target"
                        ) {
                            return false;
                        }
                        // Skip hidden directories except .github (Python lines 875-877)
                        if name.starts_with('.') && name != ".github" {
                            return false;
                        }
                    }
                }
            }
            true
        };

        // Build walker with gitignore support using the `ignore` crate
        let mut builder = WalkBuilder::new(&self.root_path);
        builder
            .git_ignore(self.respect_gitignore)
            .git_global(self.respect_gitignore)
            .git_exclude(self.respect_gitignore)
            .follow_links(self.follow_symlinks)
            .hidden(false) // Don't automatically skip hidden files
            .parents(true) // Read .gitignore files from parent directories
            .filter_entry(dir_filter);

        let walker = builder.build();

        // Set when any symlink entry was observed; used to decide whether the
        // canonical-path dedup pass below can be skipped entirely.
        let mut observed_symlink = false;

        // Collect all files
        for entry_result in walker {
            let entry = match entry_result {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();
            if entry.path_is_symlink() {
                observed_symlink = true;
            }

            // Skip directories.
            if path.is_dir() {
                // A directory symlink that is not followed would otherwise
                // vanish silently from the inventory. Record it so the report
                // can reconcile what it skipped. Followed links (follow=true)
                // are walked, so their children are accounted for normally.
                if entry.path_is_symlink() && !self.follow_symlinks {
                    let rel_path = path
                        .strip_prefix(&self.root_path)
                        .ok()
                        .and_then(|p| p.to_str())
                        .map(normalize_path)
                        .unwrap_or_default();
                    if !rel_path.is_empty() {
                        self.record_symlink_skip(path, rel_path, true);
                    }
                }
                continue;
            }

            // Count this file toward files_scanned (files only, not directories).
            self.stats.files_scanned += 1;
            self.stats.files_discovered += 1;

            // Get relative path
            let rel_path = match path.strip_prefix(&self.root_path) {
                Ok(p) => p.to_str(),
                Err(_) => continue,
            };
            // Paths that are not valid UTF-8 cannot be represented in output
            // artifacts; recording an empty string would collide with other
            // files and corrupt IDs and dispositions.
            let Some(rel_path) = rel_path else {
                self.stats.files_skipped_encoding += 1;
                self.record_path(
                    path,
                    normalize_path(&path.to_string_lossy()),
                    FileDispositionReason::SkippedEncoding,
                    None,
                );
                continue;
            };
            let rel_path = normalize_path(rel_path);

            // Walkdir yields symlink entries even when links are not followed
            // (it only refuses to descend into them), and metadata() below
            // follows the link to read the target. Reject any symlink whose
            // fully resolved target escapes the canonical root. This runs
            // unconditionally so an external file cannot leak into the pack
            // under the default follow_symlinks=false configuration. When the
            // root could not be canonicalized, every symlink is rejected
            // closed. Entries under a followed link report themselves as
            // symlinks too, so this check also catches a file reached through
            // an ancestor link whose directory entry looked safe.
            if entry.path_is_symlink() {
                let within_root = canonical_root
                    .as_deref()
                    .map(|root| is_path_within_root(path, root))
                    .unwrap_or(false);
                if !within_root {
                    self.record_symlink_skip(path, rel_path, false);
                    continue;
                }
            }

            let metadata = match path.metadata() {
                Ok(m) => m,
                Err(_) => {
                    self.record_path(
                        path,
                        rel_path,
                        FileDispositionReason::ErrorReadingMetadata,
                        None,
                    );
                    continue;
                }
            };

            let size = metadata.len();
            self.stats.total_bytes_scanned += size;
            self.stats.total_bytes_discovered += size;

            // Check explicit exclude globs
            if exclude_globset.is_match(&rel_path) {
                self.stats.files_skipped_glob += 1;
                self.record_path(path, rel_path, FileDispositionReason::SkippedGlob, Some(size));
                continue;
            }

            if self.godot_profile && file_policy(path) == FilePolicy::InventoryOnly {
                self.stats.files_inventory_only += 1;
                self.record_path(path, rel_path, FileDispositionReason::InventoryOnly, Some(size));
                continue;
            }

            // Check extension
            if !self.should_include_extension(path) {
                self.stats.files_skipped_extension += 1;
                self.record_path(
                    path,
                    rel_path,
                    FileDispositionReason::SkippedExtension,
                    Some(size),
                );
                continue;
            }

            if size > self.max_file_bytes {
                self.stats.files_skipped_size += 1;
                self.record_path(path, rel_path, FileDispositionReason::SkippedSize, Some(size));
                continue;
            }

            // Check if binary
            if is_binary_file(path, DEFAULT_SAMPLE_SIZE) {
                self.stats.files_skipped_binary += 1;
                self.record_path(path, rel_path, FileDispositionReason::SkippedBinary, Some(size));
                continue;
            }

            // Check if minified
            if self.skip_minified && is_likely_minified(path, self.max_line_length) {
                self.stats.files_skipped_minified += 1;
                self.record_path(
                    path,
                    rel_path,
                    FileDispositionReason::SkippedMinified,
                    Some(size),
                );
                continue;
            }

            files.push((path.to_path_buf(), rel_path));
        }

        // Directory symlinks are filtered before the walker descends into them,
        // so record those skipped entries after iteration has completed.
        let mut rejected_symlink_dirs =
            rejected_symlink_dirs.lock().map(|paths| paths.clone()).unwrap_or_default();
        rejected_symlink_dirs.sort();
        rejected_symlink_dirs.dedup();
        for path in rejected_symlink_dirs {
            let rel_path = match path.strip_prefix(&self.root_path) {
                Ok(path) => normalize_path(&path.to_string_lossy()),
                Err(_) => continue,
            };
            if rel_path.is_empty() {
                continue;
            }
            self.record_symlink_skip(&path, rel_path, true);
        }

        // Sort by relative path for deterministic ordering
        files.sort_by(|a, b| a.1.cmp(&b.1));

        // Convert to FileInfo objects
        // Pre-allocate result with known capacity to avoid reallocations
        let mut result = Vec::with_capacity(files.len());
        // Canonical paths claimed by each included file. When the walk
        // observed symlinks, the same content can be reachable through two
        // paths (a real directory and a symlinked alias); it must be included
        // exactly once or tokens, chunks, and stats are inflated.
        let mut canonical_paths: HashMap<PathBuf, String> = HashMap::new();
        for (path, rel_path) in files {
            let metadata = match path.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            let size = metadata.len();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            let ext_with_dot =
                if !ext.is_empty() && !ext.starts_with('.') { format!(".{}", ext) } else { ext };

            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let language = crate::domain::get_language(&ext_with_dot, filename);

            if observed_symlink {
                let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
                if let Some(original) = canonical_paths.get(&canonical) {
                    self.stats.files_skipped_symlink += 1;
                    let mut disposition = FileDisposition::new(
                        rel_path.clone(),
                        FileDispositionReason::SkippedSymlink,
                    );
                    disposition.size_bytes = Some(size);
                    disposition.extension = ext_with_dot.clone();
                    disposition.language = language.clone();
                    disposition.notes =
                        Some(format!("duplicate content, already included via {original}"));
                    self.dispositions.push(disposition);
                    continue;
                }
                canonical_paths.insert(canonical, rel_path.clone());
            }

            // Generate stable ID: SHA-256 of relative path, first 16 hex chars (matches Python)
            let id = {
                let hash = Sha256::digest(rel_path.as_bytes());
                format!("{:x}", hash)[..16].to_string()
            };

            // Update language stats
            *self.stats.languages_detected.entry(language.clone()).or_insert(0) += 1;

            let file_info = FileInfo {
                path: path.clone(),
                relative_path: rel_path.clone(),
                size_bytes: size,
                extension: ext_with_dot,
                language: language.clone(),
                id,
                priority: 0.5,         // Default priority, will be set by ranker
                token_estimate: 0,     // Will be calculated later
                tags: BTreeSet::new(), // Will be populated by ranker
                is_readme: false,      // Will be detected by ranker
                is_config: false,      // Will be detected by ranker
                is_doc: false,         // Will be detected by ranker
            };

            self.stats.files_included += 1;
            self.stats.total_bytes_included += size;
            self.stats.candidate_files += 1;
            self.stats.total_bytes_candidates += size;
            self.record_file(&file_info, FileDispositionReason::IncludedFull);

            result.push(file_info);
        }

        // Add disposition records for files hidden from the ignore walker by
        // gitignore so report inventory can reconcile with discovered files.
        // With gitignore disabled the walker yields every regular file, and
        // the reconciliation traversal applies the same directory filter, so
        // it could never discover anything; skip it entirely.
        if self.respect_gitignore {
            self.record_unseen_files();
        }

        self.stats.files_skipped = self.stats.files_skipped_size
            + self.stats.files_skipped_binary
            + self.stats.files_skipped_extension
            + self.stats.files_skipped_gitignore
            + self.stats.files_skipped_glob
            + self.stats.files_skipped_minified
            + self.stats.files_skipped_symlink
            + self.stats.files_skipped_encoding
            + self.stats.files_inventory_only;

        Ok(result)
    }

    /// Get scanning statistics
    pub fn stats(&self) -> &ScanStats {
        &self.stats
    }

    /// Get a complete disposition inventory for files observed by the scanner.
    pub fn dispositions(&self) -> &[FileDisposition] {
        &self.dispositions
    }

    fn record_path(
        &mut self,
        path: &Path,
        rel_path: String,
        reason: FileDispositionReason,
        size: Option<u64>,
    ) {
        let ext = extension_with_dot(path);
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let language = crate::domain::get_language(&ext, filename);
        let mut disposition = FileDisposition::new(rel_path, reason);
        disposition.size_bytes = size;
        disposition.extension = ext;
        disposition.language = language;
        self.dispositions.push(disposition);
    }

    fn record_file(&mut self, file: &FileInfo, reason: FileDispositionReason) {
        let mut disposition = FileDisposition::new(file.relative_path.clone(), reason);
        disposition.size_bytes = Some(file.size_bytes);
        disposition.extension = file.extension.clone();
        disposition.language = file.language.clone();
        disposition.priority = Some(file.priority);
        self.dispositions.push(disposition);
    }

    fn record_symlink_skip(&mut self, path: &Path, rel_path: String, count_discovered: bool) {
        if count_discovered {
            self.stats.files_discovered += 1;
        }
        self.stats.files_skipped_symlink += 1;
        let size = std::fs::symlink_metadata(path).map(|metadata| metadata.len()).ok();
        self.record_path(path, rel_path, FileDispositionReason::SkippedSymlink, size);
    }

    /// Record dispositions for files the gitignore-filtered walker never
    /// yielded, so the report inventory can reconcile with the tree contents.
    ///
    /// The reconciliation traversal applies the same hidden/noise directory
    /// filter as the main walker, so every file it finds was hidden
    /// exclusively by gitignore rules and is classified accordingly.
    fn record_unseen_files(&mut self) {
        let seen: BTreeSet<String> = self.dispositions.iter().map(|d| d.path.clone()).collect();
        let inventory = collect_regular_files(&self.root_path, &seen, MAX_UNSEEN_FILES);
        self.stats.unseen_files_examined = inventory.examined;
        self.stats.unseen_files_reconciled = inventory.files.len();
        self.stats.unseen_inventory_truncated = inventory.truncated;
        self.stats.unseen_inventory_errors = inventory.errors;

        if inventory.truncated {
            tracing::warn!(
                "Unseen-file disposition inventory reached its cap of {} entries",
                MAX_UNSEEN_FILES
            );
        }
        if inventory.errors > 0 {
            tracing::warn!(
                "Unseen-file disposition inventory encountered {} traversal errors",
                inventory.errors
            );
        }

        for (rel_path, path) in inventory.files {
            let size = path.metadata().map(|m| m.len()).ok();
            self.stats.files_discovered += 1;
            if let Some(size) = size {
                self.stats.total_bytes_discovered += size;
            }
            self.stats.files_skipped_gitignore += 1;
            self.record_path(
                &path,
                rel_path.clone(),
                FileDispositionReason::SkippedGitignore,
                size,
            );
        }
    }
}

struct UnseenFileInventory {
    files: Vec<(String, PathBuf)>,
    examined: usize,
    truncated: bool,
    errors: usize,
}

fn collect_regular_files(
    root: &Path,
    already_seen: &BTreeSet<String>,
    max_files: usize,
) -> UnseenFileInventory {
    let dir_filter = |entry: &ignore::DirEntry| -> bool {
        if let Some(file_type) = entry.file_type() {
            if file_type.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if matches!(
                        name,
                        "node_modules" | "__pycache__" | ".git" | ".venv" | "venv" | "target"
                    ) {
                        return false;
                    }
                    if name.starts_with('.') && name != ".github" {
                        return false;
                    }
                }
            }
        }
        true
    };

    let mut builder = WalkBuilder::new(root);
    builder
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .hidden(false)
        .parents(false)
        .filter_entry(dir_filter)
        .sort_by_file_path(|a, b| a.cmp(b));
    let walker = builder.build();

    let mut seen = already_seen.clone();
    let mut files = Vec::with_capacity(max_files.min(1024));
    let mut examined = 0;
    let mut truncated = false;
    let mut errors = 0;
    for entry_result in walker {
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(_) => {
                errors += 1;
                continue;
            }
        };
        let path = entry.path();
        if !entry.file_type().is_some_and(|file_type| file_type.is_file()) {
            continue;
        }
        examined += 1;
        let rel_path = match path.strip_prefix(root) {
            Ok(rel_path) => match rel_path.to_str() {
                Some(rel_path) => normalize_path(rel_path),
                // Non-UTF-8 names cannot be represented in the disposition
                // inventory; skip them rather than emitting an empty path.
                None => continue,
            },
            Err(_) => {
                errors += 1;
                continue;
            }
        };
        if !seen.insert(rel_path.clone()) {
            continue;
        }
        files.push((rel_path, path.to_path_buf()));
        // The cap bounds the inventory size (unseen files found), not the
        // number of files walked: a repository with nothing unseen must not
        // report a truncated inventory.
        if files.len() >= max_files {
            truncated = true;
            break;
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    UnseenFileInventory { files, examined, truncated, errors }
}

fn extension_with_dot(path: &Path) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    if !ext.is_empty() && !ext.starts_with('.') {
        format!(".{ext}")
    } else {
        ext
    }
}

/// Returns `true` when the fully resolved path stays within the canonical root.
/// Resolution failures are rejected closed because the target cannot be proven safe.
fn is_path_within_root(path: &Path, canonical_root: &Path) -> bool {
    path.canonicalize()
        .map(|canonical_path| canonical_path.starts_with(canonical_root))
        .unwrap_or(false)
}

/// Returns true when a repository metadata/config file should bypass extension filtering.
pub fn is_special_repo_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
    let special = [
        "readme",
        "changelog",
        "history",
        "contributing",
        "security",
        "code_of_conduct",
        "license",
        "notice",
        "authors",
        "maintainers",
        "agents.md",
        "claude.md",
        "design.md",
        "architecture.md",
        "codeowners",
        "makefile",
        "dockerfile",
        "containerfile",
        "docker-compose.yml",
        "docker-compose.yaml",
        "justfile",
        "taskfile.yml",
        "taskfile.yaml",
        "procfile",
        ".env.example",
        ".env.sample",
        ".env.template",
        "cargo.lock",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "poetry.lock",
        "uv.lock",
        "pipfile.lock",
        "go.sum",
        "gemfile.lock",
    ];
    special.contains(&name.as_str())
        || special.iter().any(|prefix| name.starts_with(&format!("{prefix}.")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_scanner_basic() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test files
        fs::write(root.join("test.rs"), "fn main() {}").unwrap();
        fs::write(root.join("test.py"), "print('hello')").unwrap();
        fs::write(root.join("test.txt"), "text file").unwrap();

        let mut scanner = FileScanner::new(root.to_path_buf());
        let files = scanner.scan().unwrap();

        // Should find .rs and .py files (default extensions)
        assert!(files.iter().any(|f| f.relative_path.ends_with("test.rs")));
        assert!(files.iter().any(|f| f.relative_path.ends_with("test.py")));

        // Files should be sorted by relative path
        for i in 1..files.len() {
            assert!(files[i - 1].relative_path <= files[i].relative_path);
        }
    }

    #[test]
    fn scanner_includes_csv_and_jsonl_as_line_oriented_text() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        fs::write(root.join("review.csv"), "claim,status\nalpha,supported\n").unwrap();
        fs::write(root.join("cases.jsonl"), "{\"id\":1}\n{\"id\":2}\n").unwrap();

        let mut scanner = FileScanner::new(root.to_path_buf()).respect_gitignore(false);
        let files = scanner.scan().unwrap();

        assert!(files
            .iter()
            .any(|file| { file.relative_path == "review.csv" && file.language == "csv" }));
        assert!(files
            .iter()
            .any(|file| { file.relative_path == "cases.jsonl" && file.language == "jsonl" }));
    }

    #[test]
    fn scanner_excludes_nested_build_and_browser_artifacts() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        for directory in ["web/dist", "web/test-results", "web/playwright-report"] {
            fs::create_dir_all(root.join(directory)).unwrap();
            fs::write(root.join(directory).join("artifact.json"), "{}\n").unwrap();
        }
        fs::create_dir_all(root.join("web/src")).unwrap();
        fs::write(root.join("web/src/app.json"), "{}\n").unwrap();

        let mut scanner = FileScanner::new(root.to_path_buf()).respect_gitignore(false);
        let files = scanner.scan().unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, "web/src/app.json");
    }

    #[test]
    fn configured_exclude_globs_can_include_artifact_named_directories() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        for directory in
            ["build", "web/out", "web/dist", "web/test-results", "web/playwright-report"]
        {
            fs::create_dir_all(root.join(directory)).unwrap();
            fs::write(root.join(directory).join("source.rs"), "pub fn value() {}\n").unwrap();
        }

        let mut scanner =
            FileScanner::new(root.to_path_buf()).exclude_globs(Vec::new()).respect_gitignore(false);
        let files = scanner.scan().unwrap();
        let paths: Vec<&str> = files.iter().map(|file| file.relative_path.as_str()).collect();

        assert_eq!(
            paths,
            vec![
                "build/source.rs",
                "web/dist/source.rs",
                "web/out/source.rs",
                "web/playwright-report/source.rs",
                "web/test-results/source.rs",
            ]
        );
    }

    #[test]
    fn godot_profile_includes_text_and_inventories_generated_and_binary_files() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::create_dir_all(root.join(".godot/editor")).unwrap();
        fs::write(root.join("project.godot"), "config_version=5\n").unwrap();
        fs::write(root.join("scripts/player.gd"), "extends Node\n").unwrap();
        fs::write(root.join("scripts/player.gd.uid"), "uid://abc\n").unwrap();
        fs::write(root.join("assets/player.png.import"), "[remap]\n").unwrap();
        fs::write(root.join("assets/player.png"), [0_u8, 1, 2, 0, 3]).unwrap();
        fs::write(root.join(".godot/editor/cache.cfg"), "generated=true\n").unwrap();

        let mut config = crate::domain::Config::default();
        crate::godot::resolve_profile(&mut config, root);
        let mut scanner = FileScanner::from_config(root.to_path_buf(), &config);
        let files = scanner.scan().unwrap();

        assert!(files.iter().any(|file| file.relative_path == "project.godot"));
        assert!(files.iter().any(|file| file.relative_path == "scripts/player.gd"));
        assert!(!files.iter().any(|file| file.relative_path.ends_with(".uid")));
        assert_eq!(scanner.stats().files_inventory_only, 3);
        assert!(scanner.dispositions().iter().any(|item| {
            item.path == "assets/player.png"
                && item.reason == FileDispositionReason::InventoryOnly
                && item.language == "image_asset"
        }));
        assert!(!scanner.dispositions().iter().any(|item| item.path.starts_with(".godot/")));
    }

    #[test]
    fn test_scanner_respects_size_limit() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create a large file
        fs::write(root.join("large.rs"), "a".repeat(2_000_000)).unwrap();
        fs::write(root.join("small.rs"), "fn main() {}").unwrap();

        let mut scanner = FileScanner::new(root.to_path_buf()).max_file_bytes(1_000_000);
        let files = scanner.scan().unwrap();

        // Should only find small file
        assert_eq!(files.len(), 1);
        assert!(files[0].relative_path.ends_with("small.rs"));
    }

    #[test]
    fn test_scanner_extension_filtering() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        fs::write(root.join("test.rs"), "fn main() {}").unwrap();
        fs::write(root.join("test.txt"), "text file").unwrap();

        let mut scanner =
            FileScanner::new(root.to_path_buf()).include_extensions(vec![".rs".to_string()]);
        let files = scanner.scan().unwrap();

        // Should only find .rs file
        assert_eq!(files.len(), 1);
        assert!(files[0].relative_path.ends_with("test.rs"));
    }

    // --- Test 9: Hidden dirs skipped except .github ---
    #[test]
    fn test_hidden_dirs_skipped_except_github() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Hidden dirs — should be skipped
        fs::create_dir_all(root.join(".cache")).unwrap();
        fs::write(root.join(".cache/a.py"), "# hidden cache").unwrap();

        fs::create_dir_all(root.join(".vscode")).unwrap();
        fs::write(root.join(".vscode/b.py"), "# hidden vscode").unwrap();

        // .github — should be included
        fs::create_dir_all(root.join(".github/workflows")).unwrap();
        fs::write(root.join(".github/workflows/c.yml"), "on: push").unwrap();

        let mut scanner = FileScanner::new(root.to_path_buf())
            .include_extensions(vec![".py".to_string(), ".yml".to_string()])
            .respect_gitignore(false);
        let files = scanner.scan().unwrap();

        let paths: Vec<&str> = files.iter().map(|f| f.relative_path.as_str()).collect();
        // Only the .github file should be present
        assert!(
            paths.iter().any(|p| p.contains(".github")),
            "expected .github/workflows/c.yml to be included, got: {:?}",
            paths
        );
        assert!(
            !paths.iter().any(|p| p.contains(".cache")),
            ".cache should be excluded, got: {:?}",
            paths
        );
        assert!(
            !paths.iter().any(|p| p.contains(".vscode")),
            ".vscode should be excluded, got: {:?}",
            paths
        );
    }

    // --- Test 10: Noise dirs (node_modules, __pycache__, .git, .venv, venv) skipped ---
    #[test]
    fn test_noise_dirs_skipped() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create noise directories with files in them
        for noise_dir in &["node_modules", "__pycache__", ".venv", "venv"] {
            fs::create_dir_all(root.join(noise_dir)).unwrap();
            fs::write(root.join(noise_dir).join("file.py"), "# noise").unwrap();
        }
        // .git is a special case — WalkBuilder may already handle it, but we filter it too
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/config"), "# git config").unwrap();

        // A legitimate file at the root
        fs::write(root.join("main.py"), "print('hello')").unwrap();

        let mut scanner = FileScanner::new(root.to_path_buf())
            .include_extensions(vec![".py".to_string()])
            .respect_gitignore(false);
        let files = scanner.scan().unwrap();

        let paths: Vec<&str> = files.iter().map(|f| f.relative_path.as_str()).collect();
        assert_eq!(files.len(), 1, "only main.py should be found, got: {:?}", paths);
        assert!(files[0].relative_path.ends_with("main.py"));
    }

    // --- Test 11: files_scanned stat counts correctly ---
    #[test]
    fn test_stats_files_scanned_correct() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // 3 .rs files — all should be scanned
        fs::write(root.join("a.rs"), "fn a() {}").unwrap();
        fs::write(root.join("b.rs"), "fn b() {}").unwrap();
        fs::write(root.join("c.rs"), "fn c() {}").unwrap();
        // 1 .txt file — filtered by extension, but still counted toward files_scanned
        fs::write(root.join("notes.txt"), "text").unwrap();

        let mut scanner = FileScanner::new(root.to_path_buf())
            .include_extensions(vec![".rs".to_string()])
            .respect_gitignore(false);
        let files = scanner.scan().unwrap();
        let stats = scanner.stats();

        // 3 .rs files included
        assert_eq!(files.len(), 3, "should include 3 .rs files");
        // files_scanned = total files visited (4: 3 rs + 1 txt)
        assert_eq!(stats.files_scanned, 4, "files_scanned should count all visited files");
        // files_included = only the .rs ones
        assert_eq!(stats.files_included, 3, "files_included should be 3");
    }

    #[cfg(unix)]
    #[test]
    fn follow_symlinks_rejects_outside_directory_targets_with_distinct_accounting() {
        let root_dir = TempDir::new().unwrap();
        let outside_dir = TempDir::new().unwrap();
        fs::write(outside_dir.path().join("secret.rs"), "password=12345\n").unwrap();
        std::os::unix::fs::symlink(outside_dir.path(), root_dir.path().join("linked")).unwrap();

        let mut scanner = FileScanner::new(root_dir.path().to_path_buf())
            .include_extensions(vec![".rs".to_string()])
            .exclude_globs(Vec::new())
            .respect_gitignore(false)
            .follow_symlinks(true);
        let files = scanner.scan().unwrap();

        assert!(files.is_empty(), "outside directory contents must not be scanned");
        assert_eq!(scanner.stats().files_skipped_symlink, 1);
        assert_eq!(scanner.stats().files_skipped, 1);
        assert_eq!(scanner.stats().files_discovered, 1);
        assert!(scanner.dispositions().iter().any(|disposition| {
            disposition.path == "linked"
                && disposition.reason == FileDispositionReason::SkippedSymlink
        }));
    }

    #[cfg(unix)]
    #[test]
    fn follow_symlinks_rejects_outside_file_targets_with_distinct_disposition() {
        let root_dir = TempDir::new().unwrap();
        let outside_dir = TempDir::new().unwrap();
        let outside_file = outside_dir.path().join("secret.rs");
        fs::write(&outside_file, "password=12345\n").unwrap();
        std::os::unix::fs::symlink(&outside_file, root_dir.path().join("secret.rs")).unwrap();

        let mut scanner = FileScanner::new(root_dir.path().to_path_buf())
            .include_extensions(vec![".rs".to_string()])
            .exclude_globs(Vec::new())
            .respect_gitignore(false)
            .follow_symlinks(true);
        let files = scanner.scan().unwrap();

        assert!(files.is_empty(), "outside file contents must not be scanned");
        assert_eq!(scanner.stats().files_skipped_symlink, 1);
        assert!(scanner.dispositions().iter().any(|disposition| {
            disposition.path == "secret.rs"
                && disposition.reason == FileDispositionReason::SkippedSymlink
        }));
    }

    #[cfg(unix)]
    #[test]
    fn follow_symlinks_allows_directory_targets_inside_repository() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        fs::create_dir_all(root.join("real")).unwrap();
        fs::write(root.join("real/inside.rs"), "fn inside() {}\n").unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("linked")).unwrap();

        let mut scanner = FileScanner::new(root.to_path_buf())
            .include_extensions(vec![".rs".to_string()])
            .exclude_globs(Vec::new())
            .respect_gitignore(false)
            .follow_symlinks(true);
        let files = scanner.scan().unwrap();

        // The inside directory symlink is followed, but the content is
        // reachable through both real/ and linked/; it is included exactly
        // once and the duplicate path is recorded as skipped.
        assert_eq!(files.len(), 1, "content reachable through two paths must be included once");
        assert!(files.iter().any(|file| file.relative_path == "linked/inside.rs"));
        assert_eq!(scanner.stats().files_skipped_symlink, 1);
        assert!(scanner.dispositions().iter().any(|disposition| {
            disposition.path == "real/inside.rs"
                && disposition.reason == FileDispositionReason::SkippedSymlink
                && disposition
                    .notes
                    .as_deref()
                    .is_some_and(|notes| notes.contains("linked/inside.rs"))
        }));
    }

    #[cfg(unix)]
    #[test]
    fn default_scanner_rejects_external_file_symlinks() {
        let root_dir = TempDir::new().unwrap();
        let outside_dir = TempDir::new().unwrap();
        fs::write(outside_dir.path().join("secret.rs"), "password=12345\n").unwrap();
        std::os::unix::fs::symlink(
            outside_dir.path().join("secret.rs"),
            root_dir.path().join("linked.rs"),
        )
        .unwrap();

        // follow_symlinks stays at its default (false): the default
        // configuration must not read the target of a symlink that escapes
        // the repository, even though walkdir yields the symlink entry.
        let mut scanner = FileScanner::new(root_dir.path().to_path_buf())
            .include_extensions(vec![".rs".to_string()])
            .exclude_globs(Vec::new())
            .respect_gitignore(false);
        let files = scanner.scan().unwrap();

        assert!(files.is_empty(), "external file contents must not be scanned");
        assert_eq!(scanner.stats().files_skipped_symlink, 1);
        assert_eq!(scanner.stats().files_discovered, 1);
        assert!(scanner.dispositions().iter().any(|disposition| {
            disposition.path == "linked.rs"
                && disposition.reason == FileDispositionReason::SkippedSymlink
        }));
    }

    #[cfg(unix)]
    #[test]
    fn default_scanner_records_unfollowed_directory_symlinks() {
        let root_dir = TempDir::new().unwrap();
        fs::create_dir_all(root_dir.path().join("real")).unwrap();
        fs::write(root_dir.path().join("real/inside.rs"), "fn inside() {}\n").unwrap();
        std::os::unix::fs::symlink(root_dir.path().join("real"), root_dir.path().join("linked"))
            .unwrap();

        let mut scanner = FileScanner::new(root_dir.path().to_path_buf())
            .include_extensions(vec![".rs".to_string()])
            .exclude_globs(Vec::new())
            .respect_gitignore(false);
        let files = scanner.scan().unwrap();

        // Only the real file is scanned; the unfollowed alias directory is
        // recorded instead of dropping silently out of the inventory.
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, "real/inside.rs");
        assert_eq!(scanner.stats().files_skipped_symlink, 1);
        assert_eq!(scanner.stats().files_discovered, 2);
        assert!(scanner.dispositions().iter().any(|disposition| {
            disposition.path == "linked"
                && disposition.reason == FileDispositionReason::SkippedSymlink
        }));
    }

    #[test]
    fn unseen_inventory_is_complete_when_nothing_is_unseen() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let mut seen = BTreeSet::new();
        for name in ["a.rs", "b.rs", "c.rs"] {
            fs::write(root.join(name), "fn value() {}\n").unwrap();
            seen.insert(name.to_string());
        }

        // Cap (2) is below the file count (3), yet everything was already
        // seen: the inventory must not claim truncation.
        let inventory = collect_regular_files(root, &seen, 2);

        assert!(inventory.files.is_empty());
        assert_eq!(inventory.examined, 3);
        assert!(!inventory.truncated);
        assert_eq!(inventory.errors, 0);
    }

    #[test]
    fn gitignore_disabled_skips_unseen_reconciliation_walk() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let git_init = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root)
            .status()
            .unwrap();
        assert!(git_init.success(), "git init should create the temporary repository");
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        fs::create_dir_all(root.join("ignored")).unwrap();
        fs::write(root.join("ignored/hidden.rs"), "fn hidden() {}\n").unwrap();
        fs::write(root.join("visible.rs"), "fn visible() {}\n").unwrap();

        let mut scanner = FileScanner::new(root.to_path_buf())
            .include_extensions(vec![".rs".to_string()])
            .exclude_globs(Vec::new())
            .respect_gitignore(false);
        let files = scanner.scan().unwrap();
        let stats = scanner.stats();

        // With gitignore disabled the main walker yields every file, so the
        // unseen reconciliation traversal has nothing to find and must not
        // run at all.
        assert!(files.iter().any(|file| file.relative_path == "ignored/hidden.rs"));
        assert_eq!(stats.files_skipped_gitignore, 0);
        assert_eq!(stats.unseen_files_examined, 0);
        assert_eq!(stats.unseen_files_reconciled, 0);
        assert!(!stats.unseen_inventory_truncated);
    }

    #[test]
    fn unseen_inventory_stops_at_cap_and_reports_truncation() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        for name in ["a.rs", "b.rs", "c.rs"] {
            fs::write(root.join(name), "fn value() {}\n").unwrap();
        }

        let inventory = collect_regular_files(root, &BTreeSet::new(), 2);

        assert_eq!(inventory.files.len(), 2);
        assert_eq!(
            inventory.files.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(),
            vec!["a.rs", "b.rs"]
        );
        assert_eq!(inventory.examined, 2);
        assert!(inventory.truncated);
        assert_eq!(inventory.errors, 0);
    }

    #[test]
    fn unseen_inventory_stats_expose_reconciliation_status() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let git_init = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root)
            .status()
            .unwrap();
        assert!(git_init.success(), "git init should create the temporary repository");
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        fs::create_dir_all(root.join("ignored")).unwrap();
        fs::write(root.join("ignored/hidden.rs"), "fn hidden() {}\n").unwrap();
        fs::write(root.join("visible.rs"), "fn visible() {}\n").unwrap();

        let mut scanner = FileScanner::new(root.to_path_buf())
            .include_extensions(vec![".rs".to_string()])
            .exclude_globs(Vec::new());
        let files = scanner.scan().unwrap();
        let stats = scanner.stats();

        assert!(files.iter().any(|file| file.relative_path == "visible.rs"));
        assert!(stats.unseen_files_examined >= stats.unseen_files_reconciled);
        assert_eq!(stats.unseen_files_reconciled, 1);
        assert!(!stats.unseen_inventory_truncated);
        assert_eq!(stats.unseen_inventory_errors, 0);
        assert_eq!(
            stats.to_report_value()["unseen_inventory"]["files_reconciled"],
            serde_json::json!(1)
        );
        assert_eq!(
            stats.to_report_value()["unseen_inventory"]["complete"],
            serde_json::json!(true)
        );
        assert!(scanner.dispositions().iter().any(|disposition| {
            disposition.path == "ignored/hidden.rs"
                && disposition.reason == FileDispositionReason::SkippedGitignore
        }));
    }

    // macOS kernels reject non-UTF-8 file names outright, so the fixture can
    // only be created on platforms that accept arbitrary bytes in names.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn non_utf8_file_names_are_skipped_with_a_disposition() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        fs::write(root.join("plain.rs"), "fn plain() {}\n").unwrap();
        let weird = root.join(OsString::from_vec(b"caf\xe9.rs".to_vec()));
        fs::write(&weird, "fn weird() {}\n").unwrap();

        let mut scanner = FileScanner::new(root.to_path_buf())
            .include_extensions(vec![".rs".to_string()])
            .respect_gitignore(false);
        let files = scanner.scan().unwrap();
        let stats = scanner.stats();

        // Only the representable file is included; the non-UTF-8 file must
        // not silently become an empty relative path.
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, "plain.rs");
        assert_eq!(stats.files_skipped_encoding, 1);
        assert!(scanner.dispositions().iter().any(|disposition| {
            disposition.path.contains('�')
                && disposition.reason == FileDispositionReason::SkippedEncoding
        }));
        assert!(!scanner.dispositions().iter().any(|disposition| disposition.path.is_empty()));
    }

    #[test]
    fn test_special_extensionless_and_dotfiles_have_dispositions() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        fs::write(root.join("Makefile"), "build:\n\tcargo build\n").unwrap();
        fs::write(root.join(".env.example"), "TOKEN=example\n").unwrap();
        fs::write(root.join("image.bin"), [0, 159, 146, 150]).unwrap();

        let mut scanner = FileScanner::new(root.to_path_buf())
            .include_extensions(vec![".rs".to_string()])
            .respect_gitignore(false);
        let files = scanner.scan().unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.relative_path.as_str()).collect();

        assert!(paths.contains(&"Makefile"));
        assert!(paths.contains(&".env.example"));
        assert_eq!(scanner.dispositions().len(), 3);
        assert!(scanner
            .dispositions()
            .iter()
            .any(|d| d.path == "image.bin" && d.reason == FileDispositionReason::SkippedExtension));
    }
}
