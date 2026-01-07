"""
Configuration file loader for repo-to-prompt.

Supports loading configuration from:
- repo-to-prompt.toml / .repo-to-prompt.toml
- r2p.yml / .r2p.yml / r2p.yaml / .r2p.yaml

CLI flags override config file values.
"""

from __future__ import annotations

import importlib
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from repo_to_prompt.redactor import RedactionConfig

# Optional runtime modules (loaded via importlib); typed as Any to avoid stub issues.
tomllib: Any | None
yaml: Any | None

# Optional imports for config file parsing
try:
    import tomllib as _tomllib  # Python 3.11+
except ImportError:
    try:
        _tomllib = importlib.import_module("tomli")
    except ImportError:
        _tomllib = None
tomllib = _tomllib

try:
    yaml = importlib.import_module("yaml")
except ImportError:
    yaml = None


# Config file search order (first found wins)
CONFIG_FILE_NAMES = [
    "repo-to-prompt.toml",
    ".repo-to-prompt.toml",
    "r2p.toml",
    ".r2p.toml",
    "r2p.yml",
    ".r2p.yml",
    "r2p.yaml",
    ".r2p.yaml",
]


@dataclass
class RankingWeights:
    """Custom ranking weights for file prioritization.

    Values are intended to be floats in the range [0.0, 1.0], but the loader is permissive
    and will coerce ints/floats from config files. Unknown keys are ignored for forwards
    compatibility.
    """

    readme: float = 1.0
    main_doc: float = 0.95
    config: float = 0.90
    entrypoint: float = 0.85
    core_source: float = 0.75
    api_definition: float = 0.80
    test: float = 0.50
    example: float = 0.60
    generated: float = 0.20
    lock_file: float = 0.15
    vendored: float = 0.10
    default: float = 0.50

    def to_dict(self) -> dict[str, float]:
        """Convert weights to a plain dictionary.

        Returns:
            Mapping of weight category name to weight value.
        """
        return {
            "readme": self.readme,
            "main_doc": self.main_doc,
            "config": self.config,
            "entrypoint": self.entrypoint,
            "core_source": self.core_source,
            "api_definition": self.api_definition,
            "test": self.test,
            "example": self.example,
            "generated": self.generated,
            "lock_file": self.lock_file,
            "vendored": self.vendored,
            "default": self.default,
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> RankingWeights:
        """Create a `RankingWeights` instance from config data.

        Args:
            data: Mapping of weight names to numeric values.

        Returns:
            A `RankingWeights` instance with provided numeric fields overridden.
        """
        weights = cls()
        for key, value in data.items():
            if hasattr(weights, key) and isinstance(value, (int, float)):
                setattr(weights, key, float(value))
        return weights


@dataclass
class ProjectConfig:
    """
    Project-level configuration loaded from config files.

    All fields are optional - CLI flags will override any values set here.
    """

    # File filtering
    include_extensions: set[str] | None = None
    exclude_globs: set[str] | None = None
    max_file_bytes: int | None = None
    max_total_bytes: int | None = None
    follow_symlinks: bool | None = None
    skip_minified: bool | None = None

    # Token budget
    max_tokens: int | None = None

    # Chunking options
    chunk_tokens: int | None = None
    chunk_overlap: int | None = None
    min_chunk_tokens: int | None = None

    # Output options
    output_dir: Path | None = None
    mode: str | None = None  # "prompt", "rag", "both"

    # Behavior options
    respect_gitignore: bool | None = None
    redact_secrets: bool | None = None
    tree_depth: int | None = None

    # Ranking weights
    ranking_weights: RankingWeights = field(default_factory=RankingWeights)

    # Redaction config (loaded from [redaction] section)
    redaction_config: dict[str, Any] = field(default_factory=dict)

    # Source file path (for debugging)
    _config_file: Path | None = field(default=None, repr=False)

    def get_redaction_config(self) -> RedactionConfig:
        """Build a `RedactionConfig` object from stored redaction config data.

        Returns:
            A `RedactionConfig` instance.
        """
        from repo_to_prompt.redactor import RedactionConfig

        return RedactionConfig.from_dict(self.redaction_config)

    def to_dict(self) -> dict[str, Any]:
        """Convert to a JSON-serializable dictionary.

        Output is deterministic: lists are sorted and dict keys are sorted to produce stable
        diffs and repeatable `report.json`.

        Returns:
            A dict representation of the configuration.
        """
        result: dict[str, Any] = {}

        if self.include_extensions is not None:
            result["include_extensions"] = sorted(self.include_extensions)
        if self.exclude_globs is not None:
            result["exclude_globs"] = sorted(self.exclude_globs)
        if self.max_file_bytes is not None:
            result["max_file_bytes"] = self.max_file_bytes
        if self.max_total_bytes is not None:
            result["max_total_bytes"] = self.max_total_bytes
        if self.follow_symlinks is not None:
            result["follow_symlinks"] = self.follow_symlinks
        if self.skip_minified is not None:
            result["skip_minified"] = self.skip_minified
        if self.max_tokens is not None:
            result["max_tokens"] = self.max_tokens
        if self.chunk_tokens is not None:
            result["chunk_tokens"] = self.chunk_tokens
        if self.chunk_overlap is not None:
            result["chunk_overlap"] = self.chunk_overlap
        if self.min_chunk_tokens is not None:
            result["min_chunk_tokens"] = self.min_chunk_tokens
        if self.output_dir is not None:
            result["output_dir"] = str(self.output_dir)
        if self.mode is not None:
            result["mode"] = self.mode
        if self.respect_gitignore is not None:
            result["respect_gitignore"] = self.respect_gitignore
        if self.redact_secrets is not None:
            result["redact_secrets"] = self.redact_secrets
        if self.tree_depth is not None:
            result["tree_depth"] = self.tree_depth

        # Only include weights if non-default
        weights_dict = self.ranking_weights.to_dict()
        default_weights = RankingWeights().to_dict()
        if weights_dict != default_weights:
            result["ranking_weights"] = dict(sorted(weights_dict.items()))

        # Include redaction config if set
        if self.redaction_config:
            result["redaction"] = self.redaction_config

        if self._config_file is not None:
            result["_loaded_from"] = str(self._config_file)

        return dict(sorted(result.items()))


def find_config_file(repo_root: Path) -> Path | None:
    """
    Find a configuration file in the repository root.

    Args:
        repo_root: Root directory of the repository

    Returns:
        Path to the config file, or None if not found
    """
    for name in CONFIG_FILE_NAMES:
        config_path = repo_root / name
        if config_path.exists() and config_path.is_file():
            return config_path
    return None


def _parse_toml(path: Path) -> dict[str, Any]:
    """Parse a TOML config file into a dict.

    Args:
        path: Path to the TOML file.

    Returns:
        Parsed TOML data as a dictionary, with support for nested sections.

    Raises:
        ImportError: If TOML parsing support is unavailable.
    """
    if tomllib is None:
        raise ImportError(
            "TOML support requires 'tomli' package (Python < 3.11) or Python 3.11+. "
            "Install with: pip install tomli"
        )

    with open(path, "rb") as f:
        data: dict[str, Any] = tomllib.load(f)

    # Support both flat and nested [repo-to-prompt] section
    if "repo-to-prompt" in data and isinstance(data["repo-to-prompt"], dict):
        return dict(data["repo-to-prompt"])
    if "r2p" in data and isinstance(data["r2p"], dict):
        return dict(data["r2p"])
    return data


def _parse_yaml(path: Path) -> dict[str, Any]:
    """Parse a YAML config file into a dict.

    Args:
        path: Path to the YAML file.

    Returns:
        Parsed YAML data as a dictionary, with support for nested sections.

    Raises:
        ImportError: If PyYAML is not installed.
    """
    if yaml is None:
        raise ImportError(
            "YAML support requires 'pyyaml' package. Install with: pip install pyyaml"
        )

    with open(path, encoding="utf-8") as f:
        raw_data = yaml.safe_load(f)

    if not isinstance(raw_data, dict):
        return {}

    data: dict[str, Any] = dict(raw_data)

    # Support both flat and nested section
    if "repo-to-prompt" in data and isinstance(data["repo-to-prompt"], dict):
        return dict(data["repo-to-prompt"])
    if "r2p" in data and isinstance(data["r2p"], dict):
        return dict(data["r2p"])
    return data


def _normalize_extensions(extensions: Any) -> set[str] | None:
    """Normalize extension input to a set of dot-prefixed extensions.

    Args:
        extensions: Extensions from config/CLI (string, list, set, or None).

    Returns:
        A set of normalized extensions (e.g., `{".py", ".ts"}`) or None if unset/invalid.
    """
    if extensions is None:
        return None

    if isinstance(extensions, str):
        extensions = [e.strip() for e in extensions.split(",")]

    if not isinstance(extensions, (list, set)):
        return None

    result = set()
    for ext in extensions:
        ext = str(ext).strip()
        if ext:
            if not ext.startswith("."):
                ext = f".{ext}"
            result.add(ext)

    return result if result else None


def _normalize_globs(globs: Any) -> set[str] | None:
    """Normalize glob input to a set of patterns.

    Args:
        globs: Globs from config/CLI (string, list, set, or None).

    Returns:
        A set of glob patterns or None if unset/invalid.
    """
    if globs is None:
        return None

    if isinstance(globs, str):
        globs = [g.strip() for g in globs.split(",")]

    if not isinstance(globs, (list, set)):
        return None

    result = {str(g).strip() for g in globs if g}
    return result if result else None


def load_config(repo_root: Path, config_path: Path | None = None) -> ProjectConfig:
    """
    Load configuration from a config file.

    Args:
        repo_root: Root directory of the repository
        config_path: Explicit path to config file (optional)

    Returns:
        ProjectConfig with loaded values (unset values remain None).
    """
    if config_path is None:
        config_path = find_config_file(repo_root)

    if config_path is None:
        return ProjectConfig()

    if not config_path.exists():
        return ProjectConfig()

    # Parse based on extension
    suffix = config_path.suffix.lower()
    try:
        if suffix == ".toml":
            data = _parse_toml(config_path)
        elif suffix in (".yml", ".yaml"):
            data = _parse_yaml(config_path)
        else:
            return ProjectConfig()
    except Exception:
        # Silently ignore parse errors - CLI will work without config
        return ProjectConfig()

    # Build config object
    config = ProjectConfig(_config_file=config_path)

    # File filtering
    config.include_extensions = _normalize_extensions(
        data.get("include_extensions") or data.get("include_ext")
    )
    config.exclude_globs = _normalize_globs(data.get("exclude_globs") or data.get("exclude_glob"))

    if "max_file_bytes" in data:
        config.max_file_bytes = int(data["max_file_bytes"])
    if "max_total_bytes" in data:
        config.max_total_bytes = int(data["max_total_bytes"])
    if "follow_symlinks" in data:
        config.follow_symlinks = bool(data["follow_symlinks"])
    if "skip_minified" in data:
        config.skip_minified = bool(data["skip_minified"])

    # Token budget
    if "max_tokens" in data:
        config.max_tokens = int(data["max_tokens"])

    # Chunking
    if "chunk_tokens" in data:
        config.chunk_tokens = int(data["chunk_tokens"])
    if "chunk_overlap" in data:
        config.chunk_overlap = int(data["chunk_overlap"])
    if "min_chunk_tokens" in data:
        config.min_chunk_tokens = int(data["min_chunk_tokens"])

    # Output
    if "output_dir" in data:
        config.output_dir = Path(data["output_dir"])
    if "mode" in data:
        config.mode = str(data["mode"]).lower()

    # Behavior
    if "respect_gitignore" in data:
        config.respect_gitignore = bool(data["respect_gitignore"])
    if "redact_secrets" in data:
        config.redact_secrets = bool(data["redact_secrets"])
    if "tree_depth" in data:
        config.tree_depth = int(data["tree_depth"])

    # Ranking weights
    weights_data = data.get("ranking_weights") or data.get("weights") or {}
    if weights_data:
        config.ranking_weights = RankingWeights.from_dict(weights_data)

    # Redaction configuration
    redaction_data = data.get("redaction") or data.get("redact") or {}
    if redaction_data:
        config.redaction_config = redaction_data

    return config


def merge_cli_with_config(
    config: ProjectConfig,
    *,
    # CLI arguments (None means not specified on CLI)
    include_ext: str | None = None,
    exclude_glob: str | None = None,
    max_file_bytes: int | None = None,
    max_total_bytes: int | None = None,
    max_tokens: int | None = None,
    follow_symlinks: bool | None = None,
    include_minified: bool | None = None,
    chunk_tokens: int | None = None,
    chunk_overlap: int | None = None,
    min_chunk_tokens: int | None = None,
    output_dir: Path | None = None,
    mode: str | None = None,
    no_gitignore: bool = False,
    no_redact: bool = False,
    tree_depth: int | None = None,
) -> dict[str, Any]:
    """Merge CLI arguments with config file values (CLI wins).

    Args:
        config: Config loaded from file (may have unset values).
        include_ext: Comma-separated extensions from CLI (optional).
        exclude_glob: Comma-separated exclude globs from CLI (optional).
        max_file_bytes: CLI override for max file size (optional).
        max_total_bytes: CLI override for max total included bytes (optional).
        max_tokens: CLI token budget (optional).
        follow_symlinks: CLI override for symlink traversal (optional).
        include_minified: CLI flag controlling minified inclusion (optional).
        chunk_tokens: CLI override for chunk size (optional).
        chunk_overlap: CLI override for chunk overlap (optional).
        min_chunk_tokens: CLI override for coalescing threshold (optional).
        output_dir: CLI override for output directory (optional).
        mode: CLI output mode override (optional).
        no_gitignore: CLI flag to disable `.gitignore` respect.
        no_redact: CLI flag to disable redaction.
        tree_depth: CLI override for rendered tree depth (optional).

    Returns:
        Dictionary of merged configuration values used by the export pipeline.
    """
    result: dict[str, Any] = {}

    # Include extensions: CLI overrides config
    if include_ext:
        result["include_extensions"] = _normalize_extensions(include_ext)
    elif config.include_extensions is not None:
        result["include_extensions"] = config.include_extensions
    else:
        result["include_extensions"] = None  # Use defaults

    # Exclude globs: CLI overrides config
    if exclude_glob:
        result["exclude_globs"] = _normalize_globs(exclude_glob)
    elif config.exclude_globs is not None:
        result["exclude_globs"] = config.exclude_globs
    else:
        result["exclude_globs"] = None  # Use defaults

    # Max file bytes
    if max_file_bytes is not None:
        result["max_file_bytes"] = max_file_bytes
    elif config.max_file_bytes is not None:
        result["max_file_bytes"] = config.max_file_bytes
    else:
        result["max_file_bytes"] = 1_048_576  # Default 1MB

    # Max total bytes
    if max_total_bytes is not None:
        result["max_total_bytes"] = max_total_bytes
    elif config.max_total_bytes is not None:
        result["max_total_bytes"] = config.max_total_bytes
    else:
        result["max_total_bytes"] = 20_000_000  # Default 20MB

    # Max tokens (token budget)
    if max_tokens is not None:
        result["max_tokens"] = max_tokens
    elif config.max_tokens is not None:
        result["max_tokens"] = config.max_tokens
    else:
        result["max_tokens"] = None  # No limit

    # Follow symlinks
    if follow_symlinks is not None:
        result["follow_symlinks"] = follow_symlinks
    elif config.follow_symlinks is not None:
        result["follow_symlinks"] = config.follow_symlinks
    else:
        result["follow_symlinks"] = False  # Default

    # Skip minified (inverse of include_minified)
    if include_minified is not None:
        result["skip_minified"] = not include_minified
    elif config.skip_minified is not None:
        result["skip_minified"] = config.skip_minified
    else:
        result["skip_minified"] = True  # Default

    # Chunk tokens
    if chunk_tokens is not None:
        result["chunk_tokens"] = chunk_tokens
    elif config.chunk_tokens is not None:
        result["chunk_tokens"] = config.chunk_tokens
    else:
        result["chunk_tokens"] = 800  # Default

    # Chunk overlap
    if chunk_overlap is not None:
        result["chunk_overlap"] = chunk_overlap
    elif config.chunk_overlap is not None:
        result["chunk_overlap"] = config.chunk_overlap
    else:
        result["chunk_overlap"] = 120  # Default

    # Min chunk tokens
    if min_chunk_tokens is not None:
        result["min_chunk_tokens"] = min_chunk_tokens
    elif config.min_chunk_tokens is not None:
        result["min_chunk_tokens"] = config.min_chunk_tokens
    else:
        result["min_chunk_tokens"] = 200  # Default

    # Output dir
    if output_dir is not None:
        result["output_dir"] = output_dir
    elif config.output_dir is not None:
        result["output_dir"] = config.output_dir
    else:
        result["output_dir"] = Path("./out")  # Default

    # Mode
    if mode is not None:
        result["mode"] = mode
    elif config.mode is not None:
        result["mode"] = config.mode
    else:
        result["mode"] = "both"  # Default

    # Respect gitignore (CLI --no-gitignore sets False)
    if no_gitignore:
        result["respect_gitignore"] = False
    elif config.respect_gitignore is not None:
        result["respect_gitignore"] = config.respect_gitignore
    else:
        result["respect_gitignore"] = True  # Default

    # Redact secrets (CLI --no-redact sets False)
    if no_redact:
        result["redact_secrets"] = False
    elif config.redact_secrets is not None:
        result["redact_secrets"] = config.redact_secrets
    else:
        result["redact_secrets"] = True  # Default

    # Tree depth
    if tree_depth is not None:
        result["tree_depth"] = tree_depth
    elif config.tree_depth is not None:
        result["tree_depth"] = config.tree_depth
    else:
        result["tree_depth"] = 4  # Default

    # Ranking weights (always from config, no CLI override)
    result["ranking_weights"] = config.ranking_weights

    # Redaction config (always from config, no CLI override)
    result["redaction_config"] = config.redaction_config

    return result
