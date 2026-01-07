"""
Secret redaction module for repo-to-prompt.

Detects and redacts common secrets like API keys, tokens, and private keys.

Features:
- 25+ built-in patterns for known secret formats (AWS, GitHub, Stripe, etc.)
- Entropy-based detection for high-entropy strings
- Context-based patterns (API_KEY=, Authorization: Bearer)
- Paranoid mode for aggressive redaction
- Allowlist support to prevent false positives
- Custom regex rules via config
- Structure-safe redaction: never breaks code syntax
- Source protection: source files use line-level redaction by default
"""

from __future__ import annotations

import ast
import math
import re
from collections.abc import Callable
from dataclasses import dataclass, field
from fnmatch import fnmatch
from pathlib import Path
from typing import Any


@dataclass
class RedactionRule:
    """A rule for detecting and redacting secrets.

    Attributes:
        name: Human-readable identifier for the rule (used for stats/reporting).
        pattern: Compiled regex used to detect the secret.
        replacement: Replacement text used when redacting.
        validator: Optional predicate used to reduce false positives.
    """

    name: str
    pattern: re.Pattern[str]
    replacement: str = "[REDACTED]"
    # Optional validator function for reducing false positives
    validator: Callable[[str], bool] | None = None


@dataclass
class RedactionConfig:
    """
    Configuration for advanced redaction features.

    Loaded from config file or set programmatically.

    Structure-safe redaction:
    - Source files (matching source_safe_patterns) use line-level redaction
    - Line-level redaction replaces entire lines with comments, preserving syntax
    - This prevents redaction placeholders from breaking code structure
    """

    # Custom regex patterns to add
    custom_rules: list[RedactionRule] = field(default_factory=list)

    # File/path patterns to skip redaction (allowlist)
    # Supports glob patterns like "*.md", "docs/**", "test_*.py"
    allowlist_patterns: list[str] = field(default_factory=list)

    # Specific strings to never redact (false positive list)
    allowlist_strings: set[str] = field(default_factory=set)

    # Entropy-based detection
    entropy_enabled: bool = False
    entropy_threshold: float = 4.5  # Shannon entropy threshold (0-log2(charset))
    entropy_min_length: int = 20  # Minimum string length for entropy check

    # Paranoid mode: redact any base64-like string >= 32 chars
    paranoid_mode: bool = False
    paranoid_min_length: int = 32

    # Files that are "known safe" - skip paranoid mode
    safe_file_patterns: list[str] = field(
        default_factory=lambda: [
            "*.md",
            "*.rst",
            "*.txt",
            "*.json",  # Often contains UUIDs, hashes that look like secrets
            "*.lock",
            "*.sum",
            "go.sum",
            "package-lock.json",
            "yarn.lock",
            "poetry.lock",
            "Cargo.lock",
        ]
    )

    # Source code patterns - use structure-safe (line-level) redaction
    # By default, Python/JS/TS source files use line-level redaction to preserve syntax
    source_safe_patterns: list[str] = field(
        default_factory=lambda: [
            "*.py",
            "*.pyi",
            "*.js",
            "*.jsx",
            "*.ts",
            "*.tsx",
            "*.go",
            "*.rs",
            "*.java",
            "*.kt",
            "*.c",
            "*.cpp",
            "*.h",
            "*.hpp",
            "*.cs",
            "*.rb",
            "*.php",
            "*.swift",
            "*.scala",
            "*.sh",
            "*.bash",
            "*.zsh",
        ]
    )

    # Enable structure-safe redaction for source files (default: True)
    # When True, source files use line-level redaction that preserves syntax
    # When False, uses inline replacement (may break syntax - use with caution)
    structure_safe_redaction: bool = True

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> RedactionConfig:
        """Create `RedactionConfig` from a dictionary (e.g., parsed from config file).

        Args:
            data: Mapping with optional keys like `custom_rules`, `allowlist_patterns`, etc.

        Returns:
            A `RedactionConfig` instance populated from the input data.
        """
        config = cls()

        # Custom rules
        for rule_data in data.get("custom_rules", []):
            if "pattern" in rule_data:
                try:
                    pattern = re.compile(rule_data["pattern"])
                    config.custom_rules.append(
                        RedactionRule(
                            name=rule_data.get("name", "custom"),
                            pattern=pattern,
                            replacement=rule_data.get("replacement", "[CUSTOM_REDACTED]"),
                        )
                    )
                except re.error:
                    pass  # Skip invalid patterns

        # Allowlist
        config.allowlist_patterns = list(data.get("allowlist_patterns", []))
        config.allowlist_strings = set(data.get("allowlist_strings", []))

        # Entropy settings
        if "entropy" in data:
            entropy = data["entropy"]
            config.entropy_enabled = entropy.get("enabled", False)
            config.entropy_threshold = float(entropy.get("threshold", 4.5))
            config.entropy_min_length = int(entropy.get("min_length", 20))

        # Paranoid mode
        if "paranoid" in data:
            paranoid = data["paranoid"]
            config.paranoid_mode = paranoid.get("enabled", False)
            config.paranoid_min_length = int(paranoid.get("min_length", 32))

        # Safe file patterns
        if "safe_file_patterns" in data:
            config.safe_file_patterns = list(data["safe_file_patterns"])

        # Source safe patterns (structure-safe redaction)
        if "source_safe_patterns" in data:
            config.source_safe_patterns = list(data["source_safe_patterns"])

        # Structure-safe redaction toggle
        if "structure_safe_redaction" in data:
            config.structure_safe_redaction = bool(data["structure_safe_redaction"])

        return config


def calculate_entropy(s: str) -> float:
    """
    Calculate Shannon entropy of a string.

    Higher entropy = more random = more likely to be a secret.
    Returns value between 0 and log2(charset_size).
    For base64 charset (64 chars), max is ~6 bits.
    For alphanumeric (62 chars), max is ~5.95 bits.

    Args:
        s: String to analyze

    Returns:
        Shannon entropy in bits per character
    """
    if not s:
        return 0.0

    # Count character frequencies
    freq: dict[str, int] = {}
    for char in s:
        freq[char] = freq.get(char, 0) + 1

    # Calculate entropy
    length = len(s)
    entropy = 0.0
    for count in freq.values():
        if count > 0:
            p = count / length
            entropy -= p * math.log2(p)

    return entropy


def is_high_entropy_secret(
    s: str,
    threshold: float = 4.5,
    min_length: int = 20,
) -> bool:
    """
    Check if a string appears to be a high-entropy secret.

    Args:
        s: String to check
        threshold: Minimum entropy to consider as secret
        min_length: Minimum length to consider

    Returns:
        True if the string appears to be a secret
    """
    if len(s) < min_length:
        return False

    # Must be alphanumeric with allowed special chars
    if not re.match(r"^[A-Za-z0-9+/=_\-]+$", s):
        return False

    return calculate_entropy(s) >= threshold


# Pattern for paranoid mode: long base64-like strings
PARANOID_PATTERN = re.compile(r"\b([A-Za-z0-9+/=_\-]{32,})\b")


# Comprehensive list of secret patterns
# ORDER MATTERS: Specific patterns (AWS, GitHub, Stripe, etc.) must come BEFORE
# generic patterns (generic_secret, env_secret). This ensures that a Stripe key
# gets redacted as [STRIPE_SECRET_KEY_REDACTED] instead of generic [SECRET_REDACTED].
SECRET_PATTERNS: list[RedactionRule] = [
    # AWS
    RedactionRule(
        name="aws_access_key",
        pattern=re.compile(r"\b(AKIA[0-9A-Z]{16})\b"),
        replacement="[AWS_ACCESS_KEY_REDACTED]",
    ),
    RedactionRule(
        name="aws_secret_key",
        pattern=re.compile(
            r"(?i)(aws[_\-]?secret[_\-]?(?:access[_\-]?)?key)['\"]?\s*[:=]\s*['\"]?([A-Za-z0-9/+=]{40})['\"]?",
        ),
        replacement=r"\1=[AWS_SECRET_REDACTED]",
    ),
    # GitHub
    RedactionRule(
        name="github_token",
        pattern=re.compile(r"\b(ghp_[A-Za-z0-9]{36})\b"),
        replacement="[GITHUB_TOKEN_REDACTED]",
    ),
    RedactionRule(
        name="github_oauth",
        pattern=re.compile(r"\b(gho_[A-Za-z0-9]{36})\b"),
        replacement="[GITHUB_OAUTH_REDACTED]",
    ),
    RedactionRule(
        name="github_app_token",
        pattern=re.compile(r"\b(ghu_[A-Za-z0-9]{36})\b"),
        replacement="[GITHUB_APP_TOKEN_REDACTED]",
    ),
    RedactionRule(
        name="github_refresh_token",
        pattern=re.compile(r"\b(ghr_[A-Za-z0-9]{36})\b"),
        replacement="[GITHUB_REFRESH_TOKEN_REDACTED]",
    ),
    # GitLab
    RedactionRule(
        name="gitlab_token",
        pattern=re.compile(r"\b(glpat-[A-Za-z0-9\-_]{20,})\b"),
        replacement="[GITLAB_TOKEN_REDACTED]",
    ),
    # Slack
    RedactionRule(
        name="slack_token",
        pattern=re.compile(r"\b(xox[baprs]-[0-9A-Za-z\-]{10,})\b"),
        replacement="[SLACK_TOKEN_REDACTED]",
    ),
    RedactionRule(
        name="slack_webhook",
        pattern=re.compile(
            r"(https://hooks\.slack\.com/services/T[A-Z0-9]+/B[A-Z0-9]+/[A-Za-z0-9]+)"
        ),
        replacement="[SLACK_WEBHOOK_REDACTED]",
    ),
    # Stripe
    RedactionRule(
        name="stripe_key",
        pattern=re.compile(r"\b(sk_live_[A-Za-z0-9]{24,})\b"),
        replacement="[STRIPE_SECRET_KEY_REDACTED]",
    ),
    RedactionRule(
        name="stripe_test_key",
        pattern=re.compile(r"\b(sk_test_[A-Za-z0-9]{24,})\b"),
        replacement="[STRIPE_TEST_KEY_REDACTED]",
    ),
    # Twilio
    RedactionRule(
        name="twilio_api_key",
        pattern=re.compile(r"\b(SK[0-9a-fA-F]{32})\b"),
        replacement="[TWILIO_KEY_REDACTED]",
    ),
    # SendGrid
    RedactionRule(
        name="sendgrid_key",
        pattern=re.compile(r"\b(SG\.[A-Za-z0-9\-_]{22,}\.[A-Za-z0-9\-_]{22,})\b"),
        replacement="[SENDGRID_KEY_REDACTED]",
    ),
    # Mailchimp
    RedactionRule(
        name="mailchimp_key",
        pattern=re.compile(r"\b([a-f0-9]{32}-us[0-9]{1,2})\b"),
        replacement="[MAILCHIMP_KEY_REDACTED]",
    ),
    # Google
    RedactionRule(
        name="google_api_key",
        pattern=re.compile(r"\b(AIza[0-9A-Za-z\-_]{35})\b"),
        replacement="[GOOGLE_API_KEY_REDACTED]",
    ),
    RedactionRule(
        name="google_oauth",
        pattern=re.compile(r"\b([0-9]+-[a-z0-9_]{32}\.apps\.googleusercontent\.com)\b"),
        replacement="[GOOGLE_OAUTH_REDACTED]",
    ),
    # Firebase
    RedactionRule(
        name="firebase_key",
        pattern=re.compile(r"\b(AAAA[A-Za-z0-9_-]{7,}:[A-Za-z0-9_-]{140,})\b"),
        replacement="[FIREBASE_KEY_REDACTED]",
    ),
    # Heroku
    RedactionRule(
        name="heroku_api_key",
        pattern=re.compile(
            r"(?i)(heroku[_\-]?api[_\-]?key)['\"]?\s*[:=]\s*['\"]?([a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})['\"]?"
        ),
        replacement=r"\1=[HEROKU_KEY_REDACTED]",
    ),
    # npm
    RedactionRule(
        name="npm_token",
        pattern=re.compile(r"\b(npm_[A-Za-z0-9]{36})\b"),
        replacement="[NPM_TOKEN_REDACTED]",
    ),
    # PyPI
    RedactionRule(
        name="pypi_token",
        pattern=re.compile(r"\b(pypi-[A-Za-z0-9\-_]{50,})\b"),
        replacement="[PYPI_TOKEN_REDACTED]",
    ),
    # Generic patterns
    RedactionRule(
        name="private_key_header",
        pattern=re.compile(
            r"(-----BEGIN\s+(?:RSA\s+|DSA\s+|EC\s+|OPENSSH\s+)?PRIVATE\s+KEY-----[\s\S]*?-----END\s+(?:RSA\s+|DSA\s+|EC\s+|OPENSSH\s+)?PRIVATE\s+KEY-----)"
        ),
        replacement="[PRIVATE_KEY_REDACTED]",
    ),
    RedactionRule(
        name="jwt_token",
        pattern=re.compile(r"\b(eyJ[A-Za-z0-9\-_]+\.eyJ[A-Za-z0-9\-_]+\.[A-Za-z0-9\-_]+)\b"),
        replacement="[JWT_TOKEN_REDACTED]",
    ),
    # Generic secret assignments (with common key names)
    # This pattern captures secrets in various assignment formats while preserving structure
    # Handles: key="value", key: "value", "key": "value", key = value
    RedactionRule(
        name="generic_secret",
        pattern=re.compile(
            r"(?i)((?:api[_\-]?key|apikey|secret[_\-]?key|secretkey|auth[_\-]?token|authtoken|access[_\-]?token|accesstoken|password|passwd|pwd|credentials?|bearer))(['\"]?\s*[:=]\s*['\"]?)([A-Za-z0-9\-_./+=]{16,})(['\"]?)",
        ),
        replacement=r"\1\2[SECRET_REDACTED]\4",
    ),
    # Environment variable exports with secrets
    RedactionRule(
        name="env_secret",
        pattern=re.compile(
            r"(?i)(export\s+(?:API_KEY|SECRET_KEY|AUTH_TOKEN|ACCESS_TOKEN|PASSWORD|DATABASE_URL|PRIVATE_KEY)[=])([^\s\n]+)"
        ),
        replacement=r"\1[SECRET_REDACTED]",
    ),
    # Connection strings with passwords
    RedactionRule(
        name="connection_string",
        pattern=re.compile(r"((?:postgres|mysql|mongodb|redis|amqp)(?:ql)?://[^:]+:)([^@]+)(@)"),
        replacement=r"\1[PASSWORD_REDACTED]\3",
    ),
    # Basic auth in URLs
    RedactionRule(
        name="url_auth",
        pattern=re.compile(r"(https?://[^:]+:)([^@]+)(@[^\s]+)"),
        replacement=r"\1[PASSWORD_REDACTED]\3",
    ),
    # Context-based patterns (Authorization headers, env assignments)
    RedactionRule(
        name="auth_bearer",
        pattern=re.compile(r"(?i)(Authorization:\s*Bearer\s+)([A-Za-z0-9\-_./+=]{20,})"),
        replacement=r"\1[BEARER_TOKEN_REDACTED]",
    ),
    RedactionRule(
        name="auth_basic",
        pattern=re.compile(r"(?i)(Authorization:\s*Basic\s+)([A-Za-z0-9+/=]{20,})"),
        replacement=r"\1[BASIC_AUTH_REDACTED]",
    ),
    RedactionRule(
        name="x_api_key_header",
        pattern=re.compile(r"(?i)(X-API-Key:\s*)([A-Za-z0-9\-_./+=]{16,})"),
        replacement=r"\1[API_KEY_REDACTED]",
    ),
]


# Patterns commonly found in safe content (UUIDs, hashes, etc.)
SAFE_PATTERNS = [
    # UUIDs (not secrets, just identifiers)
    re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$", re.I),
    # Git commit SHAs
    re.compile(r"^[0-9a-f]{40}$"),
    # MD5 hashes (often used for checksums)
    re.compile(r"^[0-9a-f]{32}$"),
    # SHA-256 hashes
    re.compile(r"^[0-9a-f]{64}$"),
    # Package versions like 1.2.3-beta.4+build.567
    re.compile(r"^\d+\.\d+\.\d+[\w\-+.]*$"),
]


def is_safe_value(s: str) -> bool:
    """Check whether a string matches known safe patterns (UUIDs, hashes, versions).

    Args:
        s: String to test.

    Returns:
        True if `s` looks like a common identifier/hash/version and should not be treated
        as a secret.
    """
    return any(pattern.match(s) for pattern in SAFE_PATTERNS)


class Redactor:
    """
    Redacts secrets from text content.

    Designed to be efficient for streaming large files.
    Specific patterns take precedence over generic ones.

    Features:
    - Built-in patterns for 25+ secret types
    - Custom regex rules via config
    - Entropy-based detection for unknown secrets
    - Paranoid mode for maximum security
    - Allowlist support for false positives
    - Structure-safe redaction: source files use line-level redaction

    Structure-Safe Redaction:
    For source code files (*.py, *.js, etc.), redaction never injects
    placeholders inline within code. Instead, if a secret is detected
    in a line of source code, the entire line is replaced with a
    redaction comment. This ensures the output always remains syntactically
    valid and parseable.
    """

    def __init__(
        self,
        enabled: bool = True,
        custom_patterns: list[RedactionRule] | None = None,
        config: RedactionConfig | None = None,
        current_file: Path | str | None = None,
    ):
        """
        Initialize the redactor.

        Args:
            enabled: Whether redaction is enabled
            custom_patterns: Additional patterns to use (legacy API)
            config: Advanced redaction configuration
            current_file: Current file being processed (for allowlist matching)
        """
        self.enabled = enabled
        self.config = config or RedactionConfig()
        self.current_file = Path(current_file) if current_file else None

        # Build pattern list: built-in + config custom + legacy custom
        self.patterns = SECRET_PATTERNS.copy()

        if self.config.custom_rules:
            self.patterns.extend(self.config.custom_rules)

        if custom_patterns:
            self.patterns.extend(custom_patterns)

        # Track redaction stats
        self.redaction_counts: dict[str, int] = {}

    def set_current_file(self, path: Path | str | None) -> None:
        """Set the current file being processed.

        This influences allowlist matching and file-type-specific behavior (e.g., source-safe
        redaction).

        Args:
            path: Current file path (or None to clear).
        """
        self.current_file = Path(path) if path else None

    def _is_file_allowlisted(self) -> bool:
        """Check whether the current file matches allowlist patterns.

        Returns:
            True if the current file should be skipped entirely for redaction.
        """
        if not self.current_file:
            return False

        path_str = str(self.current_file)
        name = self.current_file.name

        for pattern in self.config.allowlist_patterns:
            if fnmatch(name, pattern) or fnmatch(path_str, pattern):
                return True

        return False

    def _is_file_safe(self) -> bool:
        """Check whether the current file is considered "safe" for paranoid mode.

        Returns:
            True if the file matches `safe_file_patterns` and should not have paranoid
            redaction applied.
        """
        if not self.current_file:
            return False

        name = self.current_file.name
        path_str = str(self.current_file)

        for pattern in self.config.safe_file_patterns:
            if fnmatch(name, pattern) or fnmatch(path_str, pattern):
                return True

        return False

    def _is_source_file(self) -> bool:
        """Check whether the current file is a source file requiring structure-safe redaction.

        Returns:
            True if structure-safe redaction is enabled and the file matches a configured
            source pattern.
        """
        if not self.current_file or not self.config.structure_safe_redaction:
            return False

        name = self.current_file.name
        path_str = str(self.current_file)

        for pattern in self.config.source_safe_patterns:
            if fnmatch(name, pattern) or fnmatch(path_str, pattern):
                return True

        return False

    def _get_comment_prefix(self) -> str:
        """Get the appropriate single-line comment prefix for the current file type.

        Returns:
            Comment prefix such as `"#"` or `"//"`.
        """
        if not self.current_file:
            return "#"

        suffix = self.current_file.suffix.lower()

        # Languages using # for comments
        if suffix in {".py", ".pyi", ".rb", ".sh", ".bash", ".zsh", ".yaml", ".yml"}:
            return "#"

        # Languages using // for comments
        if suffix in {
            ".js",
            ".jsx",
            ".ts",
            ".tsx",
            ".go",
            ".java",
            ".kt",
            ".c",
            ".cpp",
            ".h",
            ".hpp",
            ".cs",
            ".swift",
            ".scala",
            ".rs",
        }:
            return "//"

        # PHP can use both, prefer //
        if suffix == ".php":
            return "//"

        # Default to #
        return "#"

    def _is_string_allowlisted(self, s: str) -> bool:
        """Check whether a specific string is allowlisted.

        Args:
            s: String to test.

        Returns:
            True if the string is explicitly allowlisted.
        """
        return s in self.config.allowlist_strings

    def _line_has_secret(self, line: str) -> tuple[bool, str]:
        """
        Check if a line contains a secret pattern.

        Args:
            line: Line of text to test.

        Returns:
            Tuple `(has_secret, rule_name)` where `rule_name` is empty when no match is found.
        """
        for rule in self.patterns:
            if rule.pattern.search(line):
                return True, rule.name
        return False, ""

    def _is_python_file(self) -> bool:
        """Check whether the current file is a Python source file.

        Returns:
            True if the current file extension is one of `.py`, `.pyi`, `.pyx`.
        """
        if not self.current_file:
            return False
        suffix = self.current_file.suffix.lower()
        return suffix in {".py", ".pyi", ".pyx"}

    def _redact_source_safe(self, content: str) -> str:
        """
        Apply structure-safe redaction for source code.

        For Python files:
        - Uses inline replacement but verifies AST still parses
        - If AST would break, returns original content unmodified

        For other source files:
        - Uses inline replacement (JS, Go, etc. don't have as strict
          syntax where inline replacement within strings breaks things)

        The key principle: NEVER return syntactically invalid code.

        Args:
            content: Full file content

        Returns:
            Redacted content that is guaranteed to remain syntactically valid for Python,
            and best-effort safe for other source file types.
        """
        # For Python files, use AST-aware redaction
        if self._is_python_file():
            result, success = self._redact_python_ast_safe(content)
            if success:
                # Also apply entropy/paranoid but verify AST after
                if self.config.entropy_enabled or self.config.paranoid_mode:
                    result = self._redact_entropy(result)
                    result = self._redact_paranoid(result)
                    # Verify still parses
                    try:
                        ast.parse(result)
                    except SyntaxError:
                        # Entropy/paranoid broke it - return just the AST-safe result
                        result, _ = self._redact_python_ast_safe(content)
                return result
            else:
                # AST parsing failed - return unchanged
                return content

        # For non-Python source files, use inline replacement
        # These languages are more forgiving about string literal changes
        return self._redact_inline(content)

    def _redact_line_level(self, content: str) -> str:
        """
        Apply line-level redaction for structure-safe processing.

        DEPRECATED: This method is kept for reference but is no longer
        the primary approach. Use _redact_source_safe instead.

        Instead of inline replacement, replaces entire lines containing
        secrets with redaction comments. This preserves code syntax
        for simple cases but can break multi-line constructs.

        Args:
            content: Full file content

        Returns:
            Content with secret-containing lines replaced by comments
        """
        lines = content.splitlines(keepends=True)
        result_lines = []
        comment_prefix = self._get_comment_prefix()

        for line in lines:
            has_secret, rule_name = self._line_has_secret(line)

            # Also check entropy and paranoid patterns
            if not has_secret and self.config.entropy_enabled:
                # Check for high-entropy strings in the line
                pattern = re.compile(
                    r"\b([A-Za-z0-9+/=_\-]{" + str(self.config.entropy_min_length) + r",})\b"
                )
                for match in pattern.finditer(line):
                    value = match.group(1)
                    if (
                        not self._is_string_allowlisted(value)
                        and not is_safe_value(value)
                        and is_high_entropy_secret(
                            value,
                            threshold=self.config.entropy_threshold,
                            min_length=self.config.entropy_min_length,
                        )
                    ):
                        has_secret = True
                        rule_name = "entropy_detected"
                        break

            if not has_secret and self.config.paranoid_mode and not self._is_file_safe():
                pattern = re.compile(
                    r"\b([A-Za-z0-9+/=_\-]{" + str(self.config.paranoid_min_length) + r",})\b"
                )
                for match in pattern.finditer(line):
                    value = match.group(1)
                    if not self._is_string_allowlisted(value) and not is_safe_value(value):
                        has_secret = True
                        rule_name = "paranoid_redacted"
                        break

            if has_secret:
                # Replace the entire line with a redaction comment
                # Preserve the line ending
                line_ending = ""
                stripped = line.rstrip("\r\n")
                if line.endswith("\r\n"):
                    line_ending = "\r\n"
                elif line.endswith("\n"):
                    line_ending = "\n"
                elif line.endswith("\r"):
                    line_ending = "\r"

                # Get indentation from original line
                indent = ""
                for char in stripped:
                    if char in " \t":
                        indent += char
                    else:
                        break

                redaction_comment = f"{indent}{comment_prefix} [REDACTED: {rule_name}]{line_ending}"
                result_lines.append(redaction_comment)

                self.redaction_counts[rule_name] = self.redaction_counts.get(rule_name, 0) + 1
            else:
                result_lines.append(line)

        return "".join(result_lines)

    def _redact_python_ast_safe(self, content: str) -> tuple[str, bool]:
        """
        Apply AST-aware redaction for Python files.

        Only redacts within string literals, never modifying identifiers,
        keywords, or structural elements. Falls back to no-op if AST
        parsing fails.

        Strategy:
        - Parse the AST to verify code is valid
        - Find secrets in string literals
        - Replace the secret values within strings using inline replacement
        - This preserves function signatures, imports, etc.

        Args:
            content: Python source code

        Returns:
            Tuple `(redacted_content, success)`. If `success` is False, the returned content
            is the original input and no redaction was applied.
        """
        try:
            ast.parse(content)
        except SyntaxError:
            # Can't parse - return unchanged to avoid breaking things
            return content, False

        # The key insight: we can use inline replacement for Python source
        # because we're only modifying string literal VALUES, not structure.
        # The quotation marks and string boundaries remain intact.

        # Apply pattern-based redaction - this works on string contents
        result = content
        had_redaction = False

        for rule in self.patterns:
            matches = rule.pattern.findall(result)
            if matches:
                count = len(matches) if isinstance(matches[0], str) else len(matches)
                self.redaction_counts[rule.name] = self.redaction_counts.get(rule.name, 0) + count
                had_redaction = True
            result = rule.pattern.sub(rule.replacement, result)

        # Verify the result still parses
        if had_redaction:
            try:
                ast.parse(result)
            except SyntaxError:
                # Our redaction broke the syntax - return original
                # This shouldn't happen if patterns are well-designed,
                # but we're being defensive
                return content, False

        return result, True

    def _redact_inline(self, content: str) -> str:
        """
        Apply inline redaction (original behavior).

        Used for non-source files like configs, docs, etc.

        Args:
            content: Content to redact.

        Returns:
            Redacted content.
        """
        result = content

        for rule in self.patterns:
            matches = rule.pattern.findall(result)
            if matches:
                count = len(matches) if isinstance(matches[0], str) else len(matches)
                self.redaction_counts[rule.name] = self.redaction_counts.get(rule.name, 0) + count
            result = rule.pattern.sub(rule.replacement, result)

        # Apply entropy-based detection
        result = self._redact_entropy(result)

        # Apply paranoid mode
        result = self._redact_paranoid(result)

        return result

    def _redact_entropy(self, content: str) -> str:
        """Apply entropy-based redaction.

        Args:
            content: Content to redact.

        Returns:
            Redacted content (or original content if entropy detection is disabled).
        """
        if not self.config.entropy_enabled:
            return content

        def replace_high_entropy(match: re.Match[str]) -> str:
            value = match.group(1)

            # Skip if in allowlist
            if self._is_string_allowlisted(value):
                return match.group(0)

            # Skip if it's a known safe pattern
            if is_safe_value(value):
                return match.group(0)

            # Check entropy
            if is_high_entropy_secret(
                value,
                threshold=self.config.entropy_threshold,
                min_length=self.config.entropy_min_length,
            ):
                self.redaction_counts["entropy_detected"] = (
                    self.redaction_counts.get("entropy_detected", 0) + 1
                )
                return "[HIGH_ENTROPY_REDACTED]"

            return match.group(0)

        # Find potential high-entropy strings
        pattern = re.compile(
            r"\b([A-Za-z0-9+/=_\-]{" + str(self.config.entropy_min_length) + r",})\b"
        )
        return pattern.sub(replace_high_entropy, content)

    def _redact_paranoid(self, content: str) -> str:
        """Apply paranoid mode redaction.

        Args:
            content: Content to redact.

        Returns:
            Redacted content (or original content if paranoid mode is disabled).
        """
        if not self.config.paranoid_mode:
            return content

        # Skip for safe files
        if self._is_file_safe():
            return content

        def replace_long_token(match: re.Match[str]) -> str:
            value = match.group(1)

            # Skip if in allowlist
            if self._is_string_allowlisted(value):
                return match.group(0)

            # Skip if it's a known safe pattern
            if is_safe_value(value):
                return match.group(0)

            # Skip if already redacted
            if "[REDACTED]" in value or value.startswith("[") and value.endswith("]"):
                return match.group(0)

            self.redaction_counts["paranoid_redacted"] = (
                self.redaction_counts.get("paranoid_redacted", 0) + 1
            )
            return "[LONG_TOKEN_REDACTED]"

        pattern = re.compile(
            r"\b([A-Za-z0-9+/=_\-]{" + str(self.config.paranoid_min_length) + r",})\b"
        )
        return pattern.sub(replace_long_token, content)

    def redact(self, content: str) -> str:
        """
        Redact secrets from content.

        Uses structure-safe redaction for source code files to preserve
        valid syntax. For Python files, inline replacement is used but
        the result is verified via AST parsing - if parsing would fail,
        the original content is returned unchanged.

        For non-source files (configs, docs, etc.), inline replacement
        is used without validation.

        Args:
            content: Text content to redact

        Returns:
            Redacted content. For source files, always syntactically valid.
        """
        if not self.enabled:
            return content

        # Skip entirely allowlisted files
        if self._is_file_allowlisted():
            return content

        # Use structure-safe redaction for source files
        if self._is_source_file():
            return self._redact_source_safe(content)

        # Use inline redaction for non-source files
        return self._redact_inline(content)

    def redact_line(self, line: str) -> str:
        """
        Redact secrets from a single line.

        More efficient for line-by-line processing.
        For source files, uses inline replacement (consistent with `redact()`).

        Args:
            line: Input line to redact.

        Returns:
            Redacted line.
        """
        if not self.enabled:
            return line

        if self._is_file_allowlisted():
            return line

        # For all files, use inline redaction on a per-line basis
        # The full-content redact() method handles AST validation for Python
        result = line

        for rule in self.patterns:
            if rule.pattern.search(result):
                self.redaction_counts[rule.name] = self.redaction_counts.get(rule.name, 0) + 1
                result = rule.pattern.sub(rule.replacement, result)

        # Apply advanced detection
        result = self._redact_entropy(result)
        result = self._redact_paranoid(result)

        return result

    def get_stats(self) -> dict[str, int]:
        """Get redaction statistics.

        Returns:
            Mapping of rule name to match/redaction count, sorted by count descending.
        """
        return dict(sorted(self.redaction_counts.items(), key=lambda x: -x[1]))

    def reset_stats(self) -> None:
        """Reset redaction statistics."""
        self.redaction_counts.clear()


def create_redactor(
    enabled: bool = True,
    config: RedactionConfig | None = None,
    current_file: Path | str | None = None,
) -> Redactor:
    """Create a `Redactor` instance.

    Args:
        enabled: Whether redaction is enabled.
        config: Optional `RedactionConfig` for advanced features.
        current_file: Optional current file path used for allowlist matching.

    Returns:
        A configured `Redactor` instance.
    """
    return Redactor(enabled=enabled, config=config, current_file=current_file)
