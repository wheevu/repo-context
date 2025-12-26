"""
Configuration models and defaults for repo-to-prompt.

Uses Pydantic for validation and type safety.
"""

from __future__ import annotations

import hashlib
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path


# Current report schema version
REPORT_SCHEMA_VERSION = "1.0.0"


class OutputMode(str, Enum):
    """Output mode for the tool."""
    PROMPT = "prompt"
    RAG = "rag"
    BOTH = "both"


# Default file extensions to include
DEFAULT_INCLUDE_EXTENSIONS: set[str] = {
    # Python
    ".py", ".pyi", ".pyx",
    # JavaScript/TypeScript
    ".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs",
    # Go
    ".go",
    # Java/Kotlin
    ".java", ".kt", ".kts",
    # Rust
    ".rs",
    # C/C++
    ".c", ".h", ".cpp", ".hpp", ".cc", ".cxx",
    # C#
    ".cs",
    # Ruby
    ".rb",
    # PHP
    ".php",
    # Swift
    ".swift",
    # Scala
    ".scala",
    # Shell
    ".sh", ".bash", ".zsh",
    # Documentation
    ".md", ".rst", ".txt", ".adoc",
    # Config
    ".yaml", ".yml", ".toml", ".json", ".ini", ".cfg",
    # Web
    ".html", ".css", ".scss", ".less", ".vue", ".svelte",
    # SQL
    ".sql",
    # Misc
    ".dockerfile", ".graphql", ".proto",
}

# Default glob patterns to exclude
DEFAULT_EXCLUDE_GLOBS: set[str] = {
    # Build outputs
    "dist/**",
    "build/**",
    "out/**",
    "target/**",
    "bin/**",
    "obj/**",
    "_build/**",
    # Dependencies
    "node_modules/**",
    ".venv/**",
    "venv/**",
    "vendor/**",
    "__pycache__/**",
    ".tox/**",
    ".nox/**",
    ".eggs/**",
    "*.egg-info/**",
    # IDE/Editor
    ".idea/**",
    ".vscode/**",
    ".vs/**",
    "*.swp",
    "*.swo",
    # Version control
    ".git/**",
    ".svn/**",
    ".hg/**",
    # Cache
    ".cache/**",
    ".pytest_cache/**",
    ".mypy_cache/**",
    ".ruff_cache/**",
    "*.pyc",
    # Generated/Lock files (deprioritized but not fully excluded)
    # "package-lock.json",  # handled by ranker
    # "yarn.lock",
    # "poetry.lock",
    # Coverage
    "coverage/**",
    ".coverage",
    "htmlcov/**",
    # Misc
    ".DS_Store",
    "Thumbs.db",
    "*.min.js",
    "*.min.css",
    "*.bundle.js",
    "*.map",
}

# Files that indicate project type and entrypoints
ENTRYPOINT_FILES: dict[str, list[str]] = {
    "python": [
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "requirements.txt",
        "Pipfile",
        "poetry.lock",
        "__main__.py",
        "main.py",
        "app.py",
        "cli.py",
    ],
    "javascript": [
        "package.json",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "index.js",
        "index.ts",
        "main.js",
        "main.ts",
        "app.js",
        "app.ts",
    ],
    "go": [
        "go.mod",
        "go.sum",
        "main.go",
        "cmd/**/*.go",
    ],
    "java": [
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
    ],
    "rust": [
        "Cargo.toml",
        "Cargo.lock",
        "main.rs",
        "lib.rs",
    ],
    "ruby": [
        "Gemfile",
        "Gemfile.lock",
        "Rakefile",
        "*.gemspec",
    ],
}

# Important documentation files
IMPORTANT_DOC_FILES: set[str] = {
    "README.md",
    "README.rst",
    "README.txt",
    "README",
    "CONTRIBUTING.md",
    "CHANGELOG.md",
    "HISTORY.md",
    "docs/index.md",
    "docs/README.md",
    "documentation/index.md",
}

# Configuration files with high importance
IMPORTANT_CONFIG_FILES: set[str] = {
    "pyproject.toml",
    "package.json",
    "tsconfig.json",
    "go.mod",
    "Cargo.toml",
    "pom.xml",
    "build.gradle",
    "Makefile",
    "Dockerfile",
    "docker-compose.yml",
    "docker-compose.yaml",
    ".env.example",
    "tox.ini",
    "setup.cfg",
}


@dataclass
class Config:
    """Main configuration for repo-to-prompt."""

    # Input source (one must be set)
    path: Path | None = None
    repo_url: str | None = None
    ref: str | None = None  # branch/tag/sha for GitHub repos

    # Filtering options
    include_extensions: set[str] = field(default_factory=lambda: DEFAULT_INCLUDE_EXTENSIONS.copy())
    exclude_globs: set[str] = field(default_factory=lambda: DEFAULT_EXCLUDE_GLOBS.copy())
    max_file_bytes: int = 1_048_576  # 1 MB
    max_total_bytes: int = 20_000_000  # 20 MB
    respect_gitignore: bool = True

    # Chunking options
    chunk_tokens: int = 800
    chunk_overlap: int = 120

    # Output options
    mode: OutputMode = OutputMode.BOTH
    output_dir: Path = field(default_factory=lambda: Path("./out"))

    # Tree options
    tree_depth: int = 4

    # Redaction
    redact_secrets: bool = True

    # Entrypoints
    entrypoints_auto: bool = True
    entrypoints: list[str] = field(default_factory=list)

    def __post_init__(self) -> None:
        """Validate configuration after initialization."""
        if self.path is None and self.repo_url is None:
            raise ValueError("Either --path or --repo must be specified")

        if self.path is not None and self.repo_url is not None:
            raise ValueError("Cannot specify both --path and --repo")

        if self.path is not None:
            self.path = Path(self.path).resolve()
            if not self.path.exists():
                raise ValueError(f"Path does not exist: {self.path}")
            if not self.path.is_dir():
                raise ValueError(f"Path is not a directory: {self.path}")

        # Ensure output dir is absolute
        self.output_dir = Path(self.output_dir).resolve()

        # Normalize extensions to include leading dot
        self.include_extensions = {
            ext if ext.startswith(".") else f".{ext}"
            for ext in self.include_extensions
        }


@dataclass
class FileInfo:
    """Information about a scanned file."""

    path: Path  # Absolute path
    relative_path: str  # Relative to repo root
    size_bytes: int
    extension: str
    language: str
    priority: float = 0.5
    tags: list[str] = field(default_factory=list)
    token_estimate: int = 0  # Estimated tokens in file

    @property
    def id(self) -> str:
        """Generate a stable, deterministic file ID based on path."""
        # Use SHA256 of relative path for deterministic ID
        hash_input = self.relative_path.encode("utf-8")
        return hashlib.sha256(hash_input).hexdigest()[:16]

    @property
    def is_readme(self) -> bool:
        """Check if file is a README."""
        name_lower = self.path.name.lower()
        return name_lower.startswith("readme")

    @property
    def is_config(self) -> bool:
        """Check if file is a config file."""
        return self.relative_path in IMPORTANT_CONFIG_FILES or self.path.name in IMPORTANT_CONFIG_FILES

    @property
    def is_doc(self) -> bool:
        """Check if file is documentation."""
        return (
            self.is_readme
            or self.extension in {".md", ".rst", ".txt", ".adoc"}
            or "docs/" in self.relative_path.lower()
            or "documentation/" in self.relative_path.lower()
        )

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization."""
        return {
            "id": self.id,
            "path": self.relative_path,
            "extension": self.extension,
            "language": self.language,
            "priority": round(self.priority, 3),
            "size_bytes": self.size_bytes,
            "tags": sorted(self.tags),
            "token_estimate": self.token_estimate,
        }


@dataclass
class Chunk:
    """A chunk of content from a file."""

    id: str  # Stable hash-based ID
    path: str  # Relative file path
    language: str
    start_line: int
    end_line: int
    content: str
    priority: float
    tags: list[str] = field(default_factory=list)
    token_estimate: int = 0

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization."""
        return {
            "id": self.id,
            "path": self.path,
            "lang": self.language,
            "start_line": self.start_line,
            "end_line": self.end_line,
            "content": self.content,
            "priority": round(self.priority, 3),
            "tags": sorted(self.tags),  # Sort for determinism
        }


@dataclass
class ScanStats:
    """Statistics from scanning a repository."""

    files_scanned: int = 0
    files_included: int = 0
    files_skipped_size: int = 0
    files_skipped_binary: int = 0
    files_skipped_extension: int = 0
    files_skipped_gitignore: int = 0
    files_skipped_glob: int = 0
    files_dropped_budget: int = 0  # Files dropped due to token budget
    total_bytes_scanned: int = 0
    total_bytes_included: int = 0
    total_tokens_estimated: int = 0  # Total token estimate
    chunks_created: int = 0
    processing_time_seconds: float = 0.0
    top_ignored_patterns: dict[str, int] = field(default_factory=dict)
    languages_detected: dict[str, int] = field(default_factory=dict)
    dropped_files: list[dict] = field(default_factory=list)  # Files dropped from budget
    redaction_counts: dict[str, int] = field(default_factory=dict)  # Redaction stats
    top_ranked_files: list[dict] = field(default_factory=list)  # Top N files by priority

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization.

        Output is deterministic: all dicts are sorted by key for stable JSON.
        """
        result = {
            "chunks_created": self.chunks_created,
            "files_dropped_budget": self.files_dropped_budget,
            "files_included": self.files_included,
            "files_scanned": self.files_scanned,
            "files_skipped": {
                "binary": self.files_skipped_binary,
                "extension": self.files_skipped_extension,
                "gitignore": self.files_skipped_gitignore,
                "glob": self.files_skipped_glob,
                "size": self.files_skipped_size,
            },
            "languages_detected": dict(
                sorted(self.languages_detected.items(), key=lambda x: (-x[1], x[0]))
            ),
            "processing_time_seconds": round(self.processing_time_seconds, 3),
            "top_ignored_patterns": dict(
                sorted(self.top_ignored_patterns.items(), key=lambda x: (-x[1], x[0]))[:10]
            ),
            "total_bytes_included": self.total_bytes_included,
            "total_bytes_scanned": self.total_bytes_scanned,
            "total_tokens_estimated": self.total_tokens_estimated,
        }
        
        # Include redaction counts if any
        if self.redaction_counts:
            result["redaction_counts"] = dict(
                sorted(self.redaction_counts.items(), key=lambda x: (-x[1], x[0]))
            )
        
        # Include top ranked files if set
        if self.top_ranked_files:
            result["top_ranked_files"] = self.top_ranked_files
        
        # Include dropped files summary if any
        if self.dropped_files:
            result["dropped_files"] = self.dropped_files
        
        return result


# Language detection by extension
EXTENSION_TO_LANGUAGE: dict[str, str] = {
    ".py": "python",
    ".pyi": "python",
    ".pyx": "python",
    ".js": "javascript",
    ".jsx": "javascript",
    ".mjs": "javascript",
    ".cjs": "javascript",
    ".ts": "typescript",
    ".tsx": "typescript",
    ".go": "go",
    ".java": "java",
    ".kt": "kotlin",
    ".kts": "kotlin",
    ".rs": "rust",
    ".c": "c",
    ".h": "c",
    ".cpp": "cpp",
    ".hpp": "cpp",
    ".cc": "cpp",
    ".cxx": "cpp",
    ".cs": "csharp",
    ".rb": "ruby",
    ".php": "php",
    ".swift": "swift",
    ".scala": "scala",
    ".sh": "bash",
    ".bash": "bash",
    ".zsh": "zsh",
    ".md": "markdown",
    ".rst": "restructuredtext",
    ".txt": "text",
    ".adoc": "asciidoc",
    ".yaml": "yaml",
    ".yml": "yaml",
    ".toml": "toml",
    ".json": "json",
    ".ini": "ini",
    ".cfg": "ini",
    ".html": "html",
    ".css": "css",
    ".scss": "scss",
    ".less": "less",
    ".vue": "vue",
    ".svelte": "svelte",
    ".sql": "sql",
    ".dockerfile": "dockerfile",
    ".graphql": "graphql",
    ".proto": "protobuf",
}


def get_language(extension: str, filename: str = "") -> str:
    """Get language from file extension or filename."""
    ext_lower = extension.lower()
    if ext_lower in EXTENSION_TO_LANGUAGE:
        return EXTENSION_TO_LANGUAGE[ext_lower]

    # Handle special filenames
    name_lower = filename.lower()
    if name_lower == "dockerfile":
        return "dockerfile"
    if name_lower == "makefile":
        return "makefile"
    if name_lower == "rakefile":
        return "ruby"
    if name_lower.endswith("rc") and ext_lower == "":
        return "shell"

    return "text"
