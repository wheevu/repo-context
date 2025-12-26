"""
File ranking module for repo-to-prompt.

Assigns priority scores to files based on their importance to understanding the codebase.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

from .config import (
    IMPORTANT_CONFIG_FILES,
    IMPORTANT_DOC_FILES,
    FileInfo,
)
from .utils import (
    is_likely_generated,
    is_lock_file,
    is_vendored,
    normalize_path,
    read_file_safe,
)


class FileRanker:
    """
    Ranks files by their importance for understanding a codebase.

    Priority scoring factors:
    - File type (README, config, entrypoint, test, etc.)
    - Directory location (src vs test vs docs)
    - Content indicators (exports, main, public APIs)
    - Generated/vendored status (deprioritized)
    """

    # Default priority weights for different categories
    DEFAULT_WEIGHTS = {
        "readme": 1.0,
        "main_doc": 0.95,
        "config": 0.90,
        "entrypoint": 0.85,
        "core_source": 0.75,
        "api_definition": 0.80,
        "test": 0.50,
        "example": 0.60,
        "generated": 0.20,
        "lock_file": 0.15,
        "vendored": 0.10,
        "default": 0.50,
    }

    # Patterns for core source directories
    CORE_DIR_PATTERNS = [
        re.compile(r"^src/"),
        re.compile(r"^lib/"),
        re.compile(r"^pkg/"),
        re.compile(r"^app/"),
        re.compile(r"^core/"),
        re.compile(r"^internal/"),
        re.compile(r"^cmd/"),
    ]

    # Patterns for test directories
    TEST_DIR_PATTERNS = [
        re.compile(r"^tests?/"),
        re.compile(r"^__tests__/"),
        re.compile(r"^spec/"),
        re.compile(r"test_"),
        re.compile(r"_test\."),
        re.compile(r"\.test\."),
        re.compile(r"\.spec\."),
    ]

    # Patterns for example directories
    EXAMPLE_DIR_PATTERNS = [
        re.compile(r"^examples?/"),
        re.compile(r"^samples?/"),
        re.compile(r"^demo/"),
    ]

    def __init__(
        self,
        root_path: Path,
        scanned_files: set[str] | None = None,
        weights: dict[str, float] | None = None,
    ):
        """
        Initialize the ranker.

        Args:
            root_path: Root path of the repository
            scanned_files: Optional set of relative file paths that were scanned.
                           Used to validate entrypoints.
            weights: Optional custom weights for ranking categories.
                     Overrides default weights for specified keys.
        """
        self.root_path = root_path.resolve()
        self._entrypoint_candidates: set[str] = set()  # Raw candidates from manifests
        self._entrypoints: set[str] = set()  # Validated entrypoints
        self._detected_languages: set[str] = set()
        self._manifest_info: dict = {}
        self._scanned_files: set[str] = scanned_files or set()

        # Merge custom weights with defaults
        self.WEIGHTS = self.DEFAULT_WEIGHTS.copy()
        if weights:
            for key, value in weights.items():
                if key in self.WEIGHTS and isinstance(value, (int, float)):
                    self.WEIGHTS[key] = float(value)

        # Load manifest information
        self._load_manifests()

        # Validate entrypoints against scanned files
        self._validate_entrypoints()

    def _load_manifests(self) -> None:
        """Load information from package manifests."""
        # Python manifests
        pyproject = self.root_path / "pyproject.toml"
        if pyproject.exists():
            self._parse_pyproject(pyproject)

        setup_py = self.root_path / "setup.py"
        if setup_py.exists():
            self._detected_languages.add("python")

        # JavaScript/TypeScript manifests
        package_json = self.root_path / "package.json"
        if package_json.exists():
            self._parse_package_json(package_json)

        # Go manifests
        go_mod = self.root_path / "go.mod"
        if go_mod.exists():
            self._detected_languages.add("go")
            self._parse_go_mod(go_mod)

        # Rust manifests
        cargo_toml = self.root_path / "Cargo.toml"
        if cargo_toml.exists():
            self._detected_languages.add("rust")

    def _parse_pyproject(self, path: Path) -> None:
        """Parse pyproject.toml for entrypoints."""
        try:
            content, _ = read_file_safe(path)
            self._detected_languages.add("python")

            # Look for scripts section - parse only lines after [project.scripts]
            if "[project.scripts]" in content:
                in_scripts = False
                for line in content.splitlines():
                    stripped = line.strip()
                    if stripped == "[project.scripts]":
                        in_scripts = True
                        continue
                    if in_scripts:
                        if stripped.startswith("["):  # New section started
                            break
                        # Match script entrypoints like: name = "module.path:function"
                        match = re.match(r'^(\w[\w-]*)\s*=\s*"([\w._]+)(?::\w+)?"', stripped)
                        if match:
                            module_path = match.group(2)
                            # Only add if it looks like a valid module path (contains dot, no spaces)
                            if "." in module_path and " " not in module_path:
                                # Convert module path to file path
                                file_path = module_path.replace(".", "/") + ".py"
                                # Also add the __init__.py variant
                                init_path = module_path.replace(".", "/") + "/__init__.py"
                                self._entrypoint_candidates.add(normalize_path(file_path))
                                self._entrypoint_candidates.add(normalize_path(init_path))

        except Exception:
            pass

    def _parse_package_json(self, path: Path) -> None:
        """Parse package.json for entrypoints."""
        try:
            content, _ = read_file_safe(path)
            data = json.loads(content)
            self._detected_languages.add("javascript")

            self._manifest_info["name"] = data.get("name", "")
            self._manifest_info["description"] = data.get("description", "")

            # Main entrypoint
            if "main" in data:
                self._entrypoint_candidates.add(normalize_path(data["main"]))

            # Module entrypoint (ESM)
            if "module" in data:
                self._entrypoint_candidates.add(normalize_path(data["module"]))

            # TypeScript entrypoint
            if "types" in data:
                self._entrypoint_candidates.add(normalize_path(data["types"]))

            # Binary/CLI entrypoints
            if "bin" in data:
                bins = data["bin"]
                if isinstance(bins, str):
                    self._entrypoint_candidates.add(normalize_path(bins))
                elif isinstance(bins, dict):
                    for bin_path in bins.values():
                        self._entrypoint_candidates.add(normalize_path(bin_path))

            # Scripts (for detecting test/build commands)
            if "scripts" in data:
                self._manifest_info["scripts"] = data["scripts"]

        except Exception:
            pass

    def _parse_go_mod(self, path: Path) -> None:
        """Parse go.mod for module info."""
        try:
            content, _ = read_file_safe(path)

            # Get module name
            match = re.search(r"^module\s+(.+)$", content, re.MULTILINE)
            if match:
                self._manifest_info["go_module"] = match.group(1).strip()

            # Look for cmd directories as entrypoints
            cmd_dir = self.root_path / "cmd"
            if cmd_dir.exists():
                for entry in cmd_dir.iterdir():
                    if entry.is_dir():
                        main_go = entry / "main.go"
                        if main_go.exists():
                            rel_path = main_go.relative_to(self.root_path)
                            # Go files exist on disk, add directly to entrypoints
                            self._entrypoints.add(normalize_path(str(rel_path)))

        except Exception:
            pass

    def _validate_entrypoints(self) -> None:
        """
        Validate entrypoint candidates against scanned files.

        Only entrypoints that exist in scanned_files are kept.
        If scanned_files is not provided, validate against filesystem.
        """
        for candidate in self._entrypoint_candidates:
            if self._scanned_files:
                # Validate against scanned files
                if candidate in self._scanned_files:
                    self._entrypoints.add(candidate)
            else:
                # Fallback: validate against filesystem
                if (self.root_path / candidate).exists():
                    self._entrypoints.add(candidate)

    def set_scanned_files(self, scanned_files: set[str]) -> None:
        """
        Update scanned files and re-validate entrypoints.

        Useful when ranker is created before scanning completes.

        Args:
            scanned_files: Set of relative file paths that were scanned
        """
        self._scanned_files = scanned_files
        self._entrypoints.clear()
        self._validate_entrypoints()

    def rank_file(self, file_info: FileInfo, content_sample: str = "") -> float:
        """
        Calculate priority score for a file.

        Args:
            file_info: Information about the file
            content_sample: Optional content sample for analysis

        Returns:
            Priority score between 0.0 and 1.0
        """
        rel_path = normalize_path(file_info.relative_path)
        name_lower = file_info.path.name.lower()

        # Check for README (highest priority)
        if name_lower.startswith("readme"):
            return self.WEIGHTS["readme"]

        # Check for important docs
        if rel_path in IMPORTANT_DOC_FILES or file_info.path.name in IMPORTANT_DOC_FILES:
            return self.WEIGHTS["main_doc"]

        # Check for vendored/generated (lowest priority)
        if is_vendored(file_info.path):
            return self.WEIGHTS["vendored"]

        if is_lock_file(file_info.path):
            return self.WEIGHTS["lock_file"]

        if is_likely_generated(file_info.path, content_sample):
            return self.WEIGHTS["generated"]

        # Check for config files
        if rel_path in IMPORTANT_CONFIG_FILES or file_info.path.name in IMPORTANT_CONFIG_FILES:
            return self.WEIGHTS["config"]

        # Check for entrypoints
        if rel_path in self._entrypoints:
            return self.WEIGHTS["entrypoint"]

        # Check common entrypoint names
        if name_lower in {"main.py", "main.go", "main.rs", "index.js", "index.ts", "app.py", "cli.py"}:
            return self.WEIGHTS["entrypoint"]

        # Check for tests
        for pattern in self.TEST_DIR_PATTERNS:
            if pattern.search(rel_path):
                return self.WEIGHTS["test"]

        # Check for examples
        for pattern in self.EXAMPLE_DIR_PATTERNS:
            if pattern.search(rel_path):
                return self.WEIGHTS["example"]

        # Check for core source
        for pattern in self.CORE_DIR_PATTERNS:
            if pattern.search(rel_path):
                return self.WEIGHTS["core_source"]

        # Check for API definitions
        if any(x in name_lower for x in ["api", "interface", "types", "models", "schema"]):
            return self.WEIGHTS["api_definition"]

        return self.WEIGHTS["default"]

    def rank_files(self, files: list[FileInfo]) -> list[FileInfo]:
        """
        Rank a list of files by priority.

        Modifies files in place and returns sorted list.
        """
        for file_info in files:
            # Read a small sample for analysis
            try:
                content_sample, _ = read_file_safe(file_info.path, max_bytes=2000)
            except Exception:
                content_sample = ""

            file_info.priority = self.rank_file(file_info, content_sample)

            # Add tags based on ranking
            if file_info.is_readme:
                file_info.tags.append("readme")
            if file_info.is_config:
                file_info.tags.append("config")
            if file_info.relative_path in self._entrypoints:
                file_info.tags.append("entrypoint")
            if is_lock_file(file_info.path):
                file_info.tags.append("lock-file")

        # Sort by priority (descending)
        files.sort(key=lambda f: (-f.priority, f.relative_path))

        return files

    def get_detected_languages(self) -> set[str]:
        """Get detected programming languages."""
        return self._detected_languages.copy()

    def get_manifest_info(self) -> dict:
        """Get information extracted from manifests."""
        return self._manifest_info.copy()

    def get_entrypoints(self) -> set[str]:
        """Get detected entrypoint files."""
        return self._entrypoints.copy()


def rank_files(root_path: Path, files: list[FileInfo]) -> list[FileInfo]:
    """
    Convenience function to rank files.

    Args:
        root_path: Repository root path
        files: List of files to rank

    Returns:
        Sorted list of files by priority
    """
    ranker = FileRanker(root_path)
    return ranker.rank_files(files)
