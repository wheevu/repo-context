"""File scanner module for repo-to-prompt.

Discovers files in a repository, respects .gitignore, and filters by extension/size.
Uses pathspec with GitWildMatchPattern for correct .gitignore semantics.
Supports concurrent file I/O with thread pool for improved performance.
"""

from __future__ import annotations

import fnmatch
import os
import subprocess
from collections import defaultdict
from collections.abc import Generator
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from typing import NamedTuple

import pathspec
from pathspec.patterns import GitWildMatchPattern

from .config import (
    DEFAULT_EXCLUDE_GLOBS,
    DEFAULT_INCLUDE_EXTENSIONS,
    FileInfo,
    ScanStats,
    get_language,
)
from .utils import is_binary_file, is_likely_minified, normalize_path


class FileCacheKey(NamedTuple):
    """Cache key based on path, mtime, and size."""
    path: str
    mtime_ns: int
    size: int


class FileCache:
    """
    Simple cache for file metadata and computed values.
    
    Cache keys are based on (path, mtime_ns, size) for freshness detection.
    """
    
    def __init__(self):
        self._binary_cache: dict[FileCacheKey, bool] = {}
        self._minified_cache: dict[FileCacheKey, bool] = {}
        self._token_cache: dict[FileCacheKey, int] = {}
        self._hash_cache: dict[FileCacheKey, str] = {}
    
    def _get_key(self, path: Path) -> FileCacheKey | None:
        """Get cache key for a file, or None if stat fails."""
        try:
            stat = path.stat()
            return FileCacheKey(str(path), stat.st_mtime_ns, stat.st_size)
        except OSError:
            return None
    
    def get_binary(self, path: Path) -> bool | None:
        """Get cached binary check result."""
        key = self._get_key(path)
        if key:
            return self._binary_cache.get(key)
        return None
    
    def set_binary(self, path: Path, is_binary: bool) -> None:
        """Cache binary check result."""
        key = self._get_key(path)
        if key:
            self._binary_cache[key] = is_binary
    
    def get_minified(self, path: Path) -> bool | None:
        """Get cached minified check result."""
        key = self._get_key(path)
        if key:
            return self._minified_cache.get(key)
        return None
    
    def set_minified(self, path: Path, is_minified: bool) -> None:
        """Cache minified check result."""
        key = self._get_key(path)
        if key:
            self._minified_cache[key] = is_minified
    
    def get_tokens(self, path: Path) -> int | None:
        """Get cached token count."""
        key = self._get_key(path)
        if key:
            return self._token_cache.get(key)
        return None
    
    def set_tokens(self, path: Path, tokens: int) -> None:
        """Cache token count."""
        key = self._get_key(path)
        if key:
            self._token_cache[key] = tokens
    
    def get_hash(self, path: Path) -> str | None:
        """Get cached content hash."""
        key = self._get_key(path)
        if key:
            return self._hash_cache.get(key)
        return None
    
    def set_hash(self, path: Path, content_hash: str) -> None:
        """Cache content hash."""
        key = self._get_key(path)
        if key:
            self._hash_cache[key] = content_hash
    
    def clear(self) -> None:
        """Clear all caches."""
        self._binary_cache.clear()
        self._minified_cache.clear()
        self._token_cache.clear()
        self._hash_cache.clear()
    
    def stats(self) -> dict[str, int]:
        """Get cache statistics."""
        return {
            "binary_entries": len(self._binary_cache),
            "minified_entries": len(self._minified_cache),
            "token_entries": len(self._token_cache),
            "hash_entries": len(self._hash_cache),
        }


# Global cache instance (can be replaced for testing)
_file_cache: FileCache | None = None


def get_file_cache() -> FileCache:
    """Get or create the global file cache."""
    global _file_cache
    if _file_cache is None:
        _file_cache = FileCache()
    return _file_cache


def clear_file_cache() -> None:
    """Clear the global file cache."""
    global _file_cache
    if _file_cache:
        _file_cache.clear()


class GitIgnoreParser:
    """
    Parser for .gitignore files.

    Uses Git as the source of truth when available (via `git check-ignore`),
    falling back to pathspec with GitWildMatchPattern for non-git directories.
    This ensures correct handling of negations, directory rules, and edge cases.
    """

    def __init__(self, root_path: Path, use_git_check: bool = True):
        """
        Initialize the parser.

        Args:
            root_path: Root directory of the repository
            use_git_check: Whether to use `git check-ignore` when available
        """
        self.root_path = root_path.resolve()
        self._specs: dict[Path, pathspec.PathSpec] = {}
        self._use_git = use_git_check and self._is_git_repo()
        self._git_ignore_cache: dict[str, bool] = {}

        # Always load pathspec as fallback
        self._load_gitignores()

    def _is_git_repo(self) -> bool:
        """Check if the root path is inside a git repository."""
        try:
            result = subprocess.run(
                ["git", "rev-parse", "--git-dir"],
                cwd=self.root_path,
                capture_output=True,
                timeout=5,
            )
            return result.returncode == 0
        except (subprocess.TimeoutExpired, FileNotFoundError, OSError):
            return False

    def _git_check_ignore(self, file_path: Path) -> bool | None:
        """
        Use git check-ignore to check if a file is ignored.

        Returns:
            True if ignored, False if not ignored, None if git check failed
        """
        try:
            rel_path = file_path.relative_to(self.root_path)
            rel_path_str = str(rel_path)

            # Check cache first
            if rel_path_str in self._git_ignore_cache:
                return self._git_ignore_cache[rel_path_str]

            result = subprocess.run(
                ["git", "check-ignore", "-q", rel_path_str],
                cwd=self.root_path,
                capture_output=True,
                timeout=2,
            )
            # Exit code 0 = ignored, 1 = not ignored, 128 = error
            is_ignored = result.returncode == 0
            self._git_ignore_cache[rel_path_str] = is_ignored
            return is_ignored
        except (subprocess.TimeoutExpired, FileNotFoundError, OSError, ValueError):
            return None

    def _load_gitignores(self) -> None:
        """Load all .gitignore files in the repository."""
        # Load root .gitignore
        root_gitignore = self.root_path / ".gitignore"
        if root_gitignore.exists():
            self._load_gitignore_file(root_gitignore, self.root_path)

        # Load nested .gitignore files (only if not using git check)
        if not self._use_git:
            for gitignore_path in self.root_path.rglob(".gitignore"):
                if gitignore_path != root_gitignore:
                    self._load_gitignore_file(gitignore_path, gitignore_path.parent)

    def _load_gitignore_file(self, gitignore_path: Path, base_path: Path) -> None:
        """Load a single .gitignore file using pathspec with GitWildMatchPattern."""
        try:
            with open(gitignore_path, encoding="utf-8", errors="replace") as f:
                patterns = f.read().splitlines()

            # Filter out comments and empty lines, preserve negation patterns
            patterns = [
                p.strip() for p in patterns
                if p.strip() and not p.strip().startswith("#")
            ]

            if patterns:
                self._specs[base_path] = pathspec.PathSpec.from_lines(
                    GitWildMatchPattern,
                    patterns
                )
        except Exception:
            pass  # Ignore unreadable .gitignore files

    def is_ignored(self, file_path: Path) -> bool:
        """
        Check if a file is ignored by .gitignore.

        Uses git check-ignore when available for accurate results,
        falls back to pathspec matching otherwise.

        Args:
            file_path: Absolute path to the file

        Returns:
            True if the file should be ignored
        """
        file_path = file_path.resolve()

        # Try git check-ignore first (most accurate)
        if self._use_git:
            git_result = self._git_check_ignore(file_path)
            if git_result is not None:
                return git_result

        # Fall back to pathspec matching
        # Check each .gitignore from most specific to least
        for base_path, spec in sorted(
            self._specs.items(),
            key=lambda x: len(x[0].parts),
            reverse=True
        ):
            try:
                rel_path = file_path.relative_to(base_path)
                rel_path_str = str(rel_path)

                # pathspec handles negations correctly when patterns are loaded properly
                if spec.match_file(rel_path_str):
                    return True
                # Also check with trailing slash for directories
                if file_path.is_dir() and spec.match_file(rel_path_str + "/"):
                    return True
            except ValueError:
                continue  # File is not under this base path

        return False

    def clear_cache(self) -> None:
        """Clear the git ignore cache."""
        self._git_ignore_cache.clear()


class FileScanner:
    """
    Scans a repository for files to include.

    Handles filtering by extension, size, gitignore, and custom patterns.
    Provides options for symlinks, encoding handling, and minified file detection.
    Supports concurrent file I/O for improved performance on large repos.
    """

    def __init__(
        self,
        root_path: Path,
        include_extensions: set[str] | None = None,
        exclude_globs: set[str] | None = None,
        max_file_bytes: int = 1_048_576,
        respect_gitignore: bool = True,
        follow_symlinks: bool = False,
        skip_minified: bool = True,
        max_line_length: int = 5000,
        max_workers: int | None = None,
        use_cache: bool = True,
    ):
        """
        Initialize the scanner.

        Args:
            root_path: Root directory to scan
            include_extensions: File extensions to include (default: common code extensions)
            exclude_globs: Glob patterns to exclude (default: common build/cache dirs)
            max_file_bytes: Maximum file size in bytes (default: 1MB)
            respect_gitignore: Whether to respect .gitignore files (default: True)
            follow_symlinks: Whether to follow symbolic links (default: False)
            skip_minified: Whether to skip minified/generated files (default: True)
            max_line_length: Maximum line length before treating as minified (default: 5000)
            max_workers: Maximum number of worker threads (default: min(32, os.cpu_count() + 4))
            use_cache: Whether to use file cache for binary/minified checks (default: True)
        """
        self.root_path = root_path.resolve()
        self.include_extensions = include_extensions if include_extensions is not None else DEFAULT_INCLUDE_EXTENSIONS.copy()
        self.exclude_globs = exclude_globs if exclude_globs is not None else DEFAULT_EXCLUDE_GLOBS.copy()
        self.max_file_bytes = max_file_bytes
        self.respect_gitignore = respect_gitignore
        self.follow_symlinks = follow_symlinks
        self.skip_minified = skip_minified
        self.max_line_length = max_line_length
        self.max_workers = max_workers
        self.use_cache = use_cache
        
        # Get cache
        self._cache = get_file_cache() if use_cache else None

        # Initialize gitignore parser
        self._gitignore: GitIgnoreParser | None = None
        if respect_gitignore:
            self._gitignore = GitIgnoreParser(root_path)

        # Compile exclude patterns using pathspec for consistency
        self._exclude_spec = pathspec.PathSpec.from_lines(
            GitWildMatchPattern,
            list(self.exclude_globs)
        )

        # Statistics tracking
        self.stats = ScanStats()
        self._ignored_pattern_counts: dict[str, int] = defaultdict(int)

    def _matches_exclude_glob(self, rel_path: str) -> str | None:
        """
        Check if a path matches any exclude glob pattern using pathspec.

        Returns the matching pattern or None.
        """
        rel_path_normalized = normalize_path(rel_path)

        # Use pathspec for matching (handles all gitignore-style patterns correctly)
        if self._exclude_spec.match_file(rel_path_normalized):
            # Find which pattern matched for statistics
            for pattern in self.exclude_globs:
                single_spec = pathspec.PathSpec.from_lines(GitWildMatchPattern, [pattern])
                if single_spec.match_file(rel_path_normalized):
                    return pattern
            return "exclude_glob"

        return None

    def _should_include_extension(self, file_path: Path) -> bool:
        """Check if file extension should be included."""
        ext = file_path.suffix.lower()
        name = file_path.name.lower()

        # Handle files without extension but with known names
        if not ext:
            known_extensionless = {
                "makefile", "dockerfile", "rakefile", "gemfile",
                "procfile", "vagrantfile", "jenkinsfile",
            }
            return name in known_extensionless

        return ext in self.include_extensions

    def scan(self) -> Generator[FileInfo, None, None]:
        """
        Scan the repository and yield file information.

        Files are yielded in deterministic sorted order (by relative path).

        Yields:
            FileInfo objects for each included file
        """
        # Collect all files first, then sort for deterministic ordering
        all_files: list[tuple[Path, str]] = []

        for file_path in self._walk_files():
            self.stats.files_scanned += 1

            # Get relative path
            try:
                rel_path = str(file_path.relative_to(self.root_path))
            except ValueError:
                continue

            rel_path = normalize_path(rel_path)

            # Check exclude globs
            matching_pattern = self._matches_exclude_glob(rel_path)
            if matching_pattern:
                self.stats.files_skipped_glob += 1
                self._ignored_pattern_counts[matching_pattern] += 1
                continue

            # Check gitignore
            if self._gitignore and self._gitignore.is_ignored(file_path):
                self.stats.files_skipped_gitignore += 1
                continue

            # Check extension
            if not self._should_include_extension(file_path):
                self.stats.files_skipped_extension += 1
                continue

            # Check file size
            try:
                size = file_path.stat().st_size
                self.stats.total_bytes_scanned += size
            except OSError:
                continue

            if size > self.max_file_bytes:
                self.stats.files_skipped_size += 1
                continue

            # Check if binary (with caching)
            is_binary = None
            if self._cache:
                is_binary = self._cache.get_binary(file_path)
            
            if is_binary is None:
                is_binary = is_binary_file(file_path)
                if self._cache:
                    self._cache.set_binary(file_path, is_binary)
            
            if is_binary:
                self.stats.files_skipped_binary += 1
                continue

            # Check for minified files (with caching)
            if self.skip_minified:
                is_minified = None
                if self._cache:
                    is_minified = self._cache.get_minified(file_path)
                
                if is_minified is None:
                    is_minified = is_likely_minified(file_path, self.max_line_length)
                    if self._cache:
                        self._cache.set_minified(file_path, is_minified)
                
                if is_minified:
                    self.stats.files_skipped_glob += 1
                    self._ignored_pattern_counts["(minified)"] += 1
                    continue

            all_files.append((file_path, rel_path))

        # Sort by relative path for deterministic ordering
        all_files.sort(key=lambda x: x[1])

        # Now yield FileInfo objects in sorted order
        for file_path, rel_path in all_files:
            try:
                size = file_path.stat().st_size
            except OSError:
                continue

            # Get language
            ext = file_path.suffix.lower()
            language = get_language(ext, file_path.name)

            # Track language stats
            self.stats.languages_detected[language] = (
                self.stats.languages_detected.get(language, 0) + 1
            )

            # Create FileInfo
            file_info = FileInfo(
                path=file_path,
                relative_path=rel_path,
                size_bytes=size,
                extension=ext,
                language=language,
            )

            self.stats.files_included += 1
            self.stats.total_bytes_included += size

            yield file_info

        # Finalize stats (sort for determinism)
        self.stats.top_ignored_patterns = dict(
            sorted(self._ignored_pattern_counts.items(), key=lambda x: (-x[1], x[0]))
        )
    
    def scan_concurrent(self, progress_callback: callable | None = None) -> list[FileInfo]:
        """
        Scan the repository using concurrent file I/O.
        
        This method is faster for large repositories with many files,
        as it parallelizes binary/minified checks across threads.
        
        Args:
            progress_callback: Optional callback(current, total) for progress updates
            
        Returns:
            List of FileInfo objects sorted by relative path
        """
        # Phase 1: Collect all candidate paths (fast, single-threaded)
        candidates: list[tuple[Path, str, int]] = []
        
        for file_path in self._walk_files():
            self.stats.files_scanned += 1
            
            try:
                rel_path = str(file_path.relative_to(self.root_path))
            except ValueError:
                continue
            
            rel_path = normalize_path(rel_path)
            
            # Check exclude globs
            matching_pattern = self._matches_exclude_glob(rel_path)
            if matching_pattern:
                self.stats.files_skipped_glob += 1
                self._ignored_pattern_counts[matching_pattern] += 1
                continue
            
            # Check gitignore
            if self._gitignore and self._gitignore.is_ignored(file_path):
                self.stats.files_skipped_gitignore += 1
                continue
            
            # Check extension
            if not self._should_include_extension(file_path):
                self.stats.files_skipped_extension += 1
                continue
            
            # Check file size
            try:
                size = file_path.stat().st_size
                self.stats.total_bytes_scanned += size
            except OSError:
                continue
            
            if size > self.max_file_bytes:
                self.stats.files_skipped_size += 1
                continue
            
            candidates.append((file_path, rel_path, size))
        
        # Phase 2: Check binary/minified concurrently
        total = len(candidates)
        results: list[tuple[Path, str, int] | None] = [None] * total
        
        def check_file(idx: int, file_path: Path, rel_path: str, size: int) -> tuple[int, tuple[Path, str, int] | None]:
            """Check a single file for binary/minified status."""
            # Check if binary (with caching)
            is_binary = None
            if self._cache:
                is_binary = self._cache.get_binary(file_path)
            
            if is_binary is None:
                is_binary = is_binary_file(file_path)
                if self._cache:
                    self._cache.set_binary(file_path, is_binary)
            
            if is_binary:
                return (idx, None, "binary")
            
            # Check for minified files (with caching)
            if self.skip_minified:
                is_minified = None
                if self._cache:
                    is_minified = self._cache.get_minified(file_path)
                
                if is_minified is None:
                    is_minified = is_likely_minified(file_path, self.max_line_length)
                    if self._cache:
                        self._cache.set_minified(file_path, is_minified)
                
                if is_minified:
                    return (idx, None, "minified")
            
            return (idx, (file_path, rel_path, size), None)
        
        # Determine number of workers
        if self.max_workers is None:
            max_workers = min(32, (os.cpu_count() or 1) + 4)
        else:
            max_workers = self.max_workers
        
        # Use thread pool for I/O-bound checks
        valid_files: list[tuple[Path, str, int]] = []
        completed = 0
        
        with ThreadPoolExecutor(max_workers=max_workers) as executor:
            futures = {
                executor.submit(check_file, i, *candidate): i
                for i, candidate in enumerate(candidates)
            }
            
            for future in as_completed(futures):
                completed += 1
                if progress_callback:
                    progress_callback(completed, total)
                
                try:
                    idx, result, skip_reason = future.result()
                    if result is not None:
                        valid_files.append(result)
                    elif skip_reason == "binary":
                        self.stats.files_skipped_binary += 1
                    elif skip_reason == "minified":
                        self.stats.files_skipped_glob += 1
                        self._ignored_pattern_counts["(minified)"] += 1
                except Exception:
                    pass
        
        # Sort by relative path for deterministic ordering
        valid_files.sort(key=lambda x: x[1])
        
        # Phase 3: Create FileInfo objects
        file_infos: list[FileInfo] = []
        for file_path, rel_path, size in valid_files:
            ext = file_path.suffix.lower()
            language = get_language(ext, file_path.name)
            
            self.stats.languages_detected[language] = (
                self.stats.languages_detected.get(language, 0) + 1
            )
            
            file_info = FileInfo(
                path=file_path,
                relative_path=rel_path,
                size_bytes=size,
                extension=ext,
                language=language,
            )
            
            self.stats.files_included += 1
            self.stats.total_bytes_included += size
            file_infos.append(file_info)
        
        # Finalize stats
        self.stats.top_ignored_patterns = dict(
            sorted(self._ignored_pattern_counts.items(), key=lambda x: (-x[1], x[0]))
        )
        
        return file_infos

    def _walk_files(self) -> Generator[Path, None, None]:
        """
        Walk the repository and yield file paths.

        Uses os.scandir for efficiency. Respects follow_symlinks setting.
        Directories are processed in sorted order for deterministic traversal.
        """
        dirs_to_process = [self.root_path]

        while dirs_to_process:
            # Pop from end but we'll sort entries, ensuring deterministic order
            current_dir = dirs_to_process.pop()

            try:
                with os.scandir(current_dir) as entries:
                    # Sort entries for deterministic traversal
                    entries_list = sorted(entries, key=lambda e: e.name)

                    # Collect directories to add in reverse sorted order
                    # (so when we pop, we get them in sorted order)
                    dirs_to_add = []

                    for entry in entries_list:
                        try:
                            # Handle symlinks based on setting
                            if entry.is_symlink():
                                if not self.follow_symlinks:
                                    continue
                                # If following symlinks, check if it points to a valid target
                                try:
                                    resolved = Path(entry.path).resolve()
                                    if not resolved.exists():
                                        continue
                                except (OSError, RuntimeError):
                                    continue  # Broken or circular symlink

                            # Use original path (not resolved) to preserve symlink path in output
                            entry_path = Path(entry.path)

                            if entry.is_dir(follow_symlinks=self.follow_symlinks):
                                # Skip hidden directories (except root .github, etc.)
                                if entry.name.startswith(".") and entry.name not in {".github"}:
                                    continue

                                # Quick check for obvious excludes
                                if entry.name in {"node_modules", "__pycache__", ".git", ".venv", "venv"}:
                                    continue

                                dirs_to_add.append(entry_path)

                            elif entry.is_file(follow_symlinks=self.follow_symlinks):
                                yield entry_path

                        except (OSError, PermissionError):
                            continue

                    # Add directories in reverse order so pop() returns them sorted
                    for d in reversed(dirs_to_add):
                        dirs_to_process.append(d)

            except (OSError, PermissionError):
                continue

def scan_repository(
    root_path: Path,
    include_extensions: set[str] | None = None,
    exclude_globs: set[str] | None = None,
    max_file_bytes: int = 1_048_576,
    respect_gitignore: bool = True,
    follow_symlinks: bool = False,
    skip_minified: bool = True,
) -> tuple[list[FileInfo], ScanStats]:
    """
    Scan a repository for files matching the given criteria.

    This function provides deterministic output: files are always returned
    in the same sorted order for the same input.

    Args:
        root_path: Root directory to scan
        include_extensions: File extensions to include (default: common code extensions)
        exclude_globs: Glob patterns to exclude (default: common build/cache dirs)
        max_file_bytes: Maximum file size in bytes (default: 1MB)
        respect_gitignore: Whether to respect .gitignore files (default: True)
        follow_symlinks: Whether to follow symbolic links (default: False)
        skip_minified: Whether to skip minified/generated files (default: True)

    Returns:
        Tuple of (list of FileInfo sorted by relative_path, ScanStats)
    """
    scanner = FileScanner(
        root_path=root_path,
        include_extensions=include_extensions,
        exclude_globs=exclude_globs,
        max_file_bytes=max_file_bytes,
        respect_gitignore=respect_gitignore,
        follow_symlinks=follow_symlinks,
        skip_minified=skip_minified,
    )

    files = list(scanner.scan())
    return files, scanner.stats


def generate_tree(
    root_path: Path,
    max_depth: int = 4,
    include_files: bool = True,
    files_to_highlight: set[str] | None = None,
) -> str:
    """
    Generate a directory tree representation.

    Args:
        root_path: Root directory
        max_depth: Maximum depth to display
        include_files: Whether to include files in the tree
        files_to_highlight: Set of relative paths to highlight

    Returns:
        String representation of the directory tree
    """
    files_to_highlight = files_to_highlight or set()
    lines = [root_path.name + "/"]

    def _walk(path: Path, prefix: str, depth: int) -> None:
        if depth > max_depth:
            return

        try:
            entries = sorted(os.scandir(path), key=lambda e: (not e.is_dir(), e.name))
        except (OSError, PermissionError):
            return

        # Filter out hidden and common ignored directories
        filtered_entries = []
        for entry in entries:
            if entry.name.startswith(".") and entry.name not in {".github", ".env.example"}:
                continue
            if entry.is_dir() and entry.name in {
                "node_modules", "__pycache__", ".git", "venv", ".venv",
                "dist", "build", "target", ".tox", ".eggs"
            }:
                continue
            filtered_entries.append(entry)

        for i, entry in enumerate(filtered_entries):
            is_last = i == len(filtered_entries) - 1
            connector = "└── " if is_last else "├── "

            entry_path = Path(entry.path)

            try:
                rel_path = str(entry_path.relative_to(root_path))
            except ValueError:
                rel_path = entry.name

            # Mark important files
            marker = ""
            if normalize_path(rel_path) in files_to_highlight:
                marker = " ⭐"

            if entry.is_dir():
                lines.append(f"{prefix}{connector}{entry.name}/{marker}")
                extension = "    " if is_last else "│   "
                _walk(entry_path, prefix + extension, depth + 1)
            elif include_files:
                lines.append(f"{prefix}{connector}{entry.name}{marker}")

    _walk(root_path, "", 1)
    return "\n".join(lines)
