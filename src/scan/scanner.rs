//! File scanner implementation with gitignore support

use crate::domain::{FileDisposition, FileDispositionReason, FileInfo, ScanStats};
use crate::utils::{is_binary_file, is_likely_minified, normalize_path};
use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const DEFAULT_SAMPLE_SIZE: usize = 8192;

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
}

impl FileScanner {
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

        // Directory filter function matching Python's _walk_files behavior
        let dir_filter = |entry: &ignore::DirEntry| -> bool {
            if let Some(file_type) = entry.file_type() {
                if file_type.is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        // Skip known large directories unconditionally (Python lines 880-887)
                        if matches!(
                            name,
                            "node_modules" | "__pycache__" | ".git" | ".venv" | "venv"
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

        // Collect all files
        for entry_result in walker {
            let entry = match entry_result {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();

            // Skip directories
            if path.is_dir() {
                continue;
            }

            // Count this file toward files_scanned (files only, not directories).
            self.stats.files_scanned += 1;
            self.stats.files_discovered += 1;

            // Get relative path
            let rel_path = match path.strip_prefix(&self.root_path) {
                Ok(p) => normalize_path(p.to_str().unwrap_or("")),
                Err(_) => continue,
            };

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

        // Sort by relative path for deterministic ordering
        files.sort_by(|a, b| a.1.cmp(&b.1));

        // Convert to FileInfo objects
        // Pre-allocate result with known capacity to avoid reallocations
        let mut result = Vec::with_capacity(files.len());
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

        // Add disposition records for files hidden from the ignore walker by gitignore or
        // directory filters so report inventory can reconcile with discovered files.
        self.record_unseen_files();

        self.stats.files_skipped = self.stats.files_skipped_size
            + self.stats.files_skipped_binary
            + self.stats.files_skipped_extension
            + self.stats.files_skipped_gitignore
            + self.stats.files_skipped_glob;

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

    fn record_unseen_files(&mut self) {
        let mut seen: BTreeSet<String> = self.dispositions.iter().map(|d| d.path.clone()).collect();
        let mut unseen = collect_regular_files(&self.root_path);
        unseen.sort_by(|a, b| a.0.cmp(&b.0));

        // Guard against excessive work on large repos. The second walk is
        // exhaustive by design to reconcile the inventory, but we cap the
        // number of entries processed to avoid long stalls.
        const MAX_UNSEEN: usize = 50_000;
        if unseen.len() > MAX_UNSEEN {
            tracing::warn!(
                "Too many unseen files ({}), capping disposition inventory at {}",
                unseen.len(),
                MAX_UNSEEN
            );
            unseen.truncate(MAX_UNSEEN);
        }

        for (rel_path, path) in unseen {
            if seen.contains(&rel_path) {
                continue;
            }
            let size = path.metadata().map(|m| m.len()).ok();
            self.stats.files_discovered += 1;
            if let Some(size) = size {
                self.stats.total_bytes_discovered += size;
            }
            if is_excluded_noise_path(&rel_path) {
                self.record_path(
                    &path,
                    rel_path.clone(),
                    FileDispositionReason::ExcludedNoiseDir,
                    size,
                );
            } else if self.respect_gitignore {
                self.stats.files_skipped_gitignore += 1;
                self.record_path(
                    &path,
                    rel_path.clone(),
                    FileDispositionReason::SkippedGitignore,
                    size,
                );
            } else {
                self.record_path(
                    &path,
                    rel_path.clone(),
                    FileDispositionReason::ExcludedNoiseDir,
                    size,
                );
            }
            seen.insert(rel_path);
        }
    }
}

fn collect_regular_files(root: &Path) -> Vec<(String, PathBuf)> {
    let dir_filter = |entry: &ignore::DirEntry| -> bool {
        if let Some(file_type) = entry.file_type() {
            if file_type.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if matches!(
                        name,
                        "node_modules"
                            | "__pycache__"
                            | ".git"
                            | ".venv"
                            | "venv"
                            | "target"
                            | "out"
                            | "dist"
                            | "build"
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

    let walker = WalkBuilder::new(root)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .hidden(false)
        .parents(false)
        .filter_entry(dir_filter)
        .build();

    let mut files = Vec::new();
    for entry in walker.flatten() {
        let path = entry.path();
        if entry.file_type().is_some_and(|ft| ft.is_file()) {
            if let Ok(rel) = path.strip_prefix(root) {
                files.push((normalize_path(rel.to_str().unwrap_or("")), path.to_path_buf()));
            }
        }
    }
    files
}

fn is_excluded_noise_path(rel_path: &str) -> bool {
    rel_path.split('/').any(|part| {
        matches!(
            part,
            "node_modules"
                | "__pycache__"
                | ".git"
                | ".venv"
                | "venv"
                | "target"
                | "out"
                | "dist"
                | "build"
        ) || (part.starts_with('.') && part != ".github")
    })
}

fn extension_with_dot(path: &Path) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    if !ext.is_empty() && !ext.starts_with('.') {
        format!(".{ext}")
    } else {
        ext
    }
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
