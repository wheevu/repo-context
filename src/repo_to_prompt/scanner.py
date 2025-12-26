"""
File scanner module for repo-to-prompt.

Discovers files in a repository, respects .gitignore, and filters by extension/size.
"""

from __future__ import annotations

import fnmatch
import os
from collections import defaultdict
from pathlib import Path
from typing import Generator, Optional

import pathspec

from .config import (
    DEFAULT_EXCLUDE_GLOBS,
    DEFAULT_INCLUDE_EXTENSIONS,
    FileInfo,
    ScanStats,
    get_language,
)
from .utils import is_binary_file, normalize_path


class GitIgnoreParser:
    """
    Parser for .gitignore files.
    
    Supports nested .gitignore files in subdirectories.
    """
    
    def __init__(self, root_path: Path):
        """
        Initialize the parser.
        
        Args:
            root_path: Root directory of the repository
        """
        self.root_path = root_path.resolve()
        self._specs: dict[Path, pathspec.PathSpec] = {}
        self._load_gitignores()
    
    def _load_gitignores(self) -> None:
        """Load all .gitignore files in the repository."""
        # Load root .gitignore
        root_gitignore = self.root_path / ".gitignore"
        if root_gitignore.exists():
            self._load_gitignore_file(root_gitignore, self.root_path)
        
        # Load nested .gitignore files
        for gitignore_path in self.root_path.rglob(".gitignore"):
            if gitignore_path != root_gitignore:
                self._load_gitignore_file(gitignore_path, gitignore_path.parent)
    
    def _load_gitignore_file(self, gitignore_path: Path, base_path: Path) -> None:
        """Load a single .gitignore file."""
        try:
            with open(gitignore_path, "r", encoding="utf-8", errors="replace") as f:
                patterns = f.read().splitlines()
            
            # Filter out comments and empty lines
            patterns = [
                p.strip() for p in patterns
                if p.strip() and not p.strip().startswith("#")
            ]
            
            if patterns:
                self._specs[base_path] = pathspec.PathSpec.from_lines(
                    pathspec.patterns.GitWildMatchPattern,
                    patterns
                )
        except Exception:
            pass  # Ignore unreadable .gitignore files
    
    def is_ignored(self, file_path: Path) -> bool:
        """
        Check if a file is ignored by .gitignore.
        
        Args:
            file_path: Absolute path to the file
            
        Returns:
            True if the file should be ignored
        """
        file_path = file_path.resolve()
        
        # Check each .gitignore from most specific to least
        for base_path, spec in sorted(
            self._specs.items(),
            key=lambda x: len(x[0].parts),
            reverse=True
        ):
            try:
                rel_path = file_path.relative_to(base_path)
                # Check both the file path and any parent directories
                if spec.match_file(str(rel_path)):
                    return True
                # Also check with trailing slash for directories
                if file_path.is_dir() and spec.match_file(str(rel_path) + "/"):
                    return True
            except ValueError:
                continue  # File is not under this base path
        
        return False


class FileScanner:
    """
    Scans a repository for files to include.
    
    Handles filtering by extension, size, gitignore, and custom patterns.
    """
    
    def __init__(
        self,
        root_path: Path,
        include_extensions: Optional[set[str]] = None,
        exclude_globs: Optional[set[str]] = None,
        max_file_bytes: int = 1_048_576,
        respect_gitignore: bool = True,
    ):
        """
        Initialize the scanner.
        
        Args:
            root_path: Root directory to scan
            include_extensions: File extensions to include
            exclude_globs: Glob patterns to exclude
            max_file_bytes: Maximum file size in bytes
            respect_gitignore: Whether to respect .gitignore files
        """
        self.root_path = root_path.resolve()
        self.include_extensions = include_extensions or DEFAULT_INCLUDE_EXTENSIONS.copy()
        self.exclude_globs = exclude_globs or DEFAULT_EXCLUDE_GLOBS.copy()
        self.max_file_bytes = max_file_bytes
        self.respect_gitignore = respect_gitignore
        
        # Initialize gitignore parser
        self._gitignore: Optional[GitIgnoreParser] = None
        if respect_gitignore:
            self._gitignore = GitIgnoreParser(root_path)
        
        # Compile exclude patterns
        self._exclude_patterns = [
            pattern for pattern in self.exclude_globs
        ]
        
        # Statistics tracking
        self.stats = ScanStats()
        self._ignored_pattern_counts: dict[str, int] = defaultdict(int)
    
    def _matches_exclude_glob(self, rel_path: str) -> Optional[str]:
        """
        Check if a path matches any exclude glob pattern.
        
        Returns the matching pattern or None.
        """
        rel_path_normalized = normalize_path(rel_path)
        
        for pattern in self._exclude_patterns:
            # Handle directory patterns
            if pattern.endswith("/**"):
                dir_pattern = pattern[:-3]
                if rel_path_normalized.startswith(dir_pattern + "/") or rel_path_normalized == dir_pattern:
                    return pattern
            elif fnmatch.fnmatch(rel_path_normalized, pattern):
                return pattern
            elif fnmatch.fnmatch(Path(rel_path_normalized).name, pattern):
                return pattern
        
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
        
        Yields:
            FileInfo objects for each included file
        """
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
            
            # Check if binary
            if is_binary_file(file_path):
                self.stats.files_skipped_binary += 1
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
        
        # Finalize stats
        self.stats.top_ignored_patterns = dict(self._ignored_pattern_counts)
    
    def _walk_files(self) -> Generator[Path, None, None]:
        """
        Walk the repository and yield file paths.
        
        Uses os.scandir for efficiency.
        """
        dirs_to_process = [self.root_path]
        
        while dirs_to_process:
            current_dir = dirs_to_process.pop()
            
            try:
                with os.scandir(current_dir) as entries:
                    entries_list = sorted(entries, key=lambda e: e.name)
                    
                    for entry in entries_list:
                        try:
                            if entry.is_symlink():
                                continue
                            
                            entry_path = Path(entry.path)
                            
                            if entry.is_dir():
                                # Skip hidden directories (except root .github, etc.)
                                if entry.name.startswith(".") and entry.name not in {".github"}:
                                    continue
                                
                                # Quick check for obvious excludes
                                if entry.name in {"node_modules", "__pycache__", ".git", ".venv", "venv"}:
                                    continue
                                
                                dirs_to_process.append(entry_path)
                            
                            elif entry.is_file():
                                yield entry_path
                        
                        except (OSError, PermissionError):
                            continue
            
            except (OSError, PermissionError):
                continue


def scan_repository(
    root_path: Path,
    include_extensions: Optional[set[str]] = None,
    exclude_globs: Optional[set[str]] = None,
    max_file_bytes: int = 1_048_576,
    respect_gitignore: bool = True,
) -> tuple[list[FileInfo], ScanStats]:
    """
    Convenience function to scan a repository.
    
    Returns:
        Tuple of (list of FileInfo, ScanStats)
    """
    scanner = FileScanner(
        root_path=root_path,
        include_extensions=include_extensions,
        exclude_globs=exclude_globs,
        max_file_bytes=max_file_bytes,
        respect_gitignore=respect_gitignore,
    )
    
    files = list(scanner.scan())
    return files, scanner.stats


def generate_tree(
    root_path: Path,
    max_depth: int = 4,
    include_files: bool = True,
    files_to_highlight: Optional[set[str]] = None,
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
