"""
Secret redaction module for repo-to-prompt.

Detects and redacts common secrets like API keys, tokens, and private keys.
"""

from __future__ import annotations

import re
from collections.abc import Callable
from dataclasses import dataclass


@dataclass
class RedactionRule:
    """A rule for detecting and redacting secrets."""

    name: str
    pattern: re.Pattern[str]
    replacement: str = "[REDACTED]"
    # Optional validator function for reducing false positives
    validator: Callable[[str], bool] | None = None


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
]


class Redactor:
    """
    Redacts secrets from text content.

    Designed to be efficient for streaming large files.
    Specific patterns take precedence over generic ones.
    """

    def __init__(
        self,
        enabled: bool = True,
        custom_patterns: list[RedactionRule] | None = None,
    ):
        """
        Initialize the redactor.

        Args:
            enabled: Whether redaction is enabled
            custom_patterns: Additional patterns to use
        """
        self.enabled = enabled
        self.patterns = SECRET_PATTERNS.copy()

        if custom_patterns:
            self.patterns.extend(custom_patterns)

        # Track redaction stats
        self.redaction_counts: dict[str, int] = {}

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

        result = content

        # Track which regions have been redacted to prevent double-redaction
        # We'll use a simple approach: mark redacted regions with a unique placeholder,
        # process all patterns, then restore placeholders.
        # However, since replacements like [STRIPE_SECRET_KEY_REDACTED] are distinct
        # and won't match other patterns, we can simply ensure specific patterns
        # come first and their replacements don't match generic patterns.

        # The key insight: replacement strings like [STRIPE_SECRET_KEY_REDACTED]
        # won't match patterns looking for actual secrets. So order matters but
        # double-redaction is already prevented by the replacement format.

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

        return result

    def redact_line(self, line: str) -> str:
        """
        Redact secrets from a single line.

        More efficient for line-by-line processing.
        """
        if not self.enabled:
            return line

        result = line

        for rule in self.patterns:
            if rule.pattern.search(result):
                self.redaction_counts[rule.name] = (
                    self.redaction_counts.get(rule.name, 0) + 1
                )
                result = rule.pattern.sub(rule.replacement, result)

        return result

    def get_stats(self) -> dict[str, int]:
        """Get redaction statistics."""
        return dict(sorted(self.redaction_counts.items(), key=lambda x: -x[1]))

    def reset_stats(self) -> None:
        """Reset redaction statistics."""
        self.redaction_counts.clear()


def create_redactor(enabled: bool = True) -> Redactor:
    """Factory function to create a redactor instance."""
    return Redactor(enabled=enabled)
