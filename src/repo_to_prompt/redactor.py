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
"""

from __future__ import annotations

import math
import re
from collections.abc import Callable
from dataclasses import dataclass, field
from fnmatch import fnmatch
from pathlib import Path


@dataclass
class RedactionRule:
    """A rule for detecting and redacting secrets."""

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
    safe_file_patterns: list[str] = field(default_factory=lambda: [
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
    ])
    
    @classmethod
    def from_dict(cls, data: dict) -> "RedactionConfig":
        """Create RedactionConfig from a dictionary (e.g., from config file)."""
        config = cls()
        
        # Custom rules
        for rule_data in data.get("custom_rules", []):
            if "pattern" in rule_data:
                try:
                    pattern = re.compile(rule_data["pattern"])
                    config.custom_rules.append(RedactionRule(
                        name=rule_data.get("name", "custom"),
                        pattern=pattern,
                        replacement=rule_data.get("replacement", "[CUSTOM_REDACTED]"),
                    ))
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
    if not re.match(r'^[A-Za-z0-9+/=_\-]+$', s):
        return False
    
    return calculate_entropy(s) >= threshold


# Pattern for paranoid mode: long base64-like strings
PARANOID_PATTERN = re.compile(r'\b([A-Za-z0-9+/=_\-]{32,})\b')


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
        pattern=re.compile(r"(-----BEGIN\s+(?:RSA\s+|DSA\s+|EC\s+|OPENSSH\s+)?PRIVATE\s+KEY-----[\s\S]*?-----END\s+(?:RSA\s+|DSA\s+|EC\s+|OPENSSH\s+)?PRIVATE\s+KEY-----)"),
        replacement="[PRIVATE_KEY_REDACTED]",
    ),
    RedactionRule(
        name="jwt_token",
        pattern=re.compile(r"\b(eyJ[A-Za-z0-9\-_]+\.eyJ[A-Za-z0-9\-_]+\.[A-Za-z0-9\-_]+)\b"),
        replacement="[JWT_TOKEN_REDACTED]",
    ),

    # Generic secret assignments (with common key names)
    RedactionRule(
        name="generic_secret",
        pattern=re.compile(
            r"(?i)((?:api[_\-]?key|apikey|secret[_\-]?key|secretkey|auth[_\-]?token|authtoken|access[_\-]?token|accesstoken|password|passwd|pwd|credentials?|bearer))['\"]?\s*[:=]\s*['\"]?([A-Za-z0-9\-_./+=]{16,})['\"]?",
        ),
        replacement=r"\1=[SECRET_REDACTED]",
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
        pattern=re.compile(
            r"((?:postgres|mysql|mongodb|redis|amqp)(?:ql)?://[^:]+:)([^@]+)(@)"
        ),
        replacement=r"\1[PASSWORD_REDACTED]\3",
    ),

    # Basic auth in URLs
    RedactionRule(
        name="url_auth",
        pattern=re.compile(
            r"(https?://[^:]+:)([^@]+)(@[^\s]+)"
        ),
        replacement=r"\1[PASSWORD_REDACTED]\3",
    ),
    
    # Context-based patterns (Authorization headers, env assignments)
    RedactionRule(
        name="auth_bearer",
        pattern=re.compile(
            r"(?i)(Authorization:\s*Bearer\s+)([A-Za-z0-9\-_./+=]{20,})"
        ),
        replacement=r"\1[BEARER_TOKEN_REDACTED]",
    ),
    RedactionRule(
        name="auth_basic",
        pattern=re.compile(
            r"(?i)(Authorization:\s*Basic\s+)([A-Za-z0-9+/=]{20,})"
        ),
        replacement=r"\1[BASIC_AUTH_REDACTED]",
    ),
    RedactionRule(
        name="x_api_key_header",
        pattern=re.compile(
            r"(?i)(X-API-Key:\s*)([A-Za-z0-9\-_./+=]{16,})"
        ),
        replacement=r"\1[API_KEY_REDACTED]",
    ),
]


# Patterns commonly found in safe content (UUIDs, hashes, etc.)
SAFE_PATTERNS = [
    # UUIDs (not secrets, just identifiers)
    re.compile(r'^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$', re.I),
    # Git commit SHAs
    re.compile(r'^[0-9a-f]{40}$'),
    # MD5 hashes (often used for checksums)
    re.compile(r'^[0-9a-f]{32}$'),
    # SHA-256 hashes
    re.compile(r'^[0-9a-f]{64}$'),
    # Package versions like 1.2.3-beta.4+build.567
    re.compile(r'^\d+\.\d+\.\d+[\w\-+.]*$'),
]


def is_safe_value(s: str) -> bool:
    """Check if a string matches known safe patterns (UUIDs, hashes, etc.)."""
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
        """Set the current file being processed."""
        self.current_file = Path(path) if path else None
    
    def _is_file_allowlisted(self) -> bool:
        """Check if current file matches allowlist patterns."""
        if not self.current_file:
            return False
        
        path_str = str(self.current_file)
        name = self.current_file.name
        
        for pattern in self.config.allowlist_patterns:
            if fnmatch(name, pattern) or fnmatch(path_str, pattern):
                return True
        
        return False
    
    def _is_file_safe(self) -> bool:
        """Check if current file is in the safe file list (for paranoid mode)."""
        if not self.current_file:
            return False
        
        name = self.current_file.name
        path_str = str(self.current_file)
        
        for pattern in self.config.safe_file_patterns:
            if fnmatch(name, pattern) or fnmatch(path_str, pattern):
                return True
        
        return False
    
    def _is_string_allowlisted(self, s: str) -> bool:
        """Check if a specific string is in the allowlist."""
        return s in self.config.allowlist_strings
    
    def _redact_entropy(self, content: str) -> str:
        """Apply entropy-based redaction."""
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
        pattern = re.compile(r'\b([A-Za-z0-9+/=_\-]{' + str(self.config.entropy_min_length) + r',})\b')
        return pattern.sub(replace_high_entropy, content)
    
    def _redact_paranoid(self, content: str) -> str:
        """Apply paranoid mode redaction."""
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
            r'\b([A-Za-z0-9+/=_\-]{' + str(self.config.paranoid_min_length) + r',})\b'
        )
        return pattern.sub(replace_long_token, content)

    def redact(self, content: str) -> str:
        """
        Redact secrets from content.

        Applies patterns in order. Specific patterns (earlier in list)
        take precedence - once text is redacted, later patterns won't
        re-match the replacement tokens.

        Args:
            content: Text content to redact

        Returns:
            Redacted content with same number of lines
        """
        if not self.enabled:
            return content
        
        # Skip entirely allowlisted files
        if self._is_file_allowlisted():
            return content

        result = content

        # Apply pattern-based redaction
        for rule in self.patterns:
            # Count matches before replacement
            matches = rule.pattern.findall(result)
            if matches:
                count = len(matches) if isinstance(matches[0], str) else len(matches)
                self.redaction_counts[rule.name] = (
                    self.redaction_counts.get(rule.name, 0) + count
                )

            # Perform replacement
            result = rule.pattern.sub(rule.replacement, result)
        
        # Apply entropy-based detection
        result = self._redact_entropy(result)
        
        # Apply paranoid mode
        result = self._redact_paranoid(result)

        return result

    def redact_line(self, line: str) -> str:
        """
        Redact secrets from a single line.

        More efficient for line-by-line processing.
        """
        if not self.enabled:
            return line
        
        if self._is_file_allowlisted():
            return line

        result = line

        for rule in self.patterns:
            if rule.pattern.search(result):
                self.redaction_counts[rule.name] = (
                    self.redaction_counts.get(rule.name, 0) + 1
                )
                result = rule.pattern.sub(rule.replacement, result)
        
        # Apply advanced detection
        result = self._redact_entropy(result)
        result = self._redact_paranoid(result)

        return result

    def get_stats(self) -> dict[str, int]:
        """Get redaction statistics."""
        return dict(sorted(self.redaction_counts.items(), key=lambda x: -x[1]))

    def reset_stats(self) -> None:
        """Reset redaction statistics."""
        self.redaction_counts.clear()


def create_redactor(
    enabled: bool = True,
    config: RedactionConfig | None = None,
    current_file: Path | str | None = None,
) -> Redactor:
    """Factory function to create a redactor instance."""
    return Redactor(enabled=enabled, config=config, current_file=current_file)
