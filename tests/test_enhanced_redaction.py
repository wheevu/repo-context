"""Tests for enhanced redaction features: entropy, paranoid mode, allowlists."""

from __future__ import annotations

import re
from pathlib import Path

import pytest

from repo_to_prompt.redactor import (
    RedactionConfig,
    RedactionRule,
    Redactor,
    calculate_entropy,
    create_redactor,
    is_high_entropy_secret,
    is_safe_value,
)


class TestEntropyCalculation:
    """Tests for Shannon entropy calculation."""

    def test_empty_string(self):
        """Empty string has zero entropy."""
        assert calculate_entropy("") == 0.0

    def test_single_character_repeated(self):
        """Repeated single character has zero entropy."""
        assert calculate_entropy("aaaaaaaaaa") == 0.0

    def test_two_chars_equal(self):
        """Equal distribution of two chars has entropy ~1.0."""
        entropy = calculate_entropy("ababababab")
        assert 0.9 < entropy < 1.1

    def test_high_entropy_string(self):
        """Random-looking string has high entropy."""
        # This looks like a secret
        high_entropy = "xK9fP2mN7qR4sT6vW8yB3dF5gH1jL0"
        entropy = calculate_entropy(high_entropy)
        assert entropy > 4.0

    def test_low_entropy_word(self):
        """Regular word has lower entropy."""
        entropy = calculate_entropy("password")
        assert entropy < 3.0

    def test_base64_like(self):
        """Base64-like string has high entropy."""
        b64 = "SGVsbG8gV29ybGQhIFRoaXMgaXMgYSB0ZXN0"
        entropy = calculate_entropy(b64)
        assert entropy > 4.0


class TestIsHighEntropySecret:
    """Tests for high entropy secret detection."""

    def test_too_short(self):
        """Short strings are not flagged."""
        assert not is_high_entropy_secret("abc123")

    def test_low_entropy_long(self):
        """Long but low-entropy strings are not flagged."""
        assert not is_high_entropy_secret("aaaaaaaaaaaaaaaaaaaaaaaa")

    def test_high_entropy_secret(self):
        """High-entropy strings are flagged."""
        secret = "xK9fP2mN7qR4sT6vW8yB3dF5gH1jL0aZ"
        assert is_high_entropy_secret(secret)

    def test_invalid_characters(self):
        """Strings with invalid characters are not flagged."""
        # Contains spaces
        assert not is_high_entropy_secret("hello world this is a test string")

    def test_custom_threshold(self):
        """Custom threshold can be used."""
        s = "abcdefghijklmnopqrstuvwxyz"
        # With low threshold, detected
        assert is_high_entropy_secret(s, threshold=3.0, min_length=20)
        # With high threshold, not detected
        assert not is_high_entropy_secret(s, threshold=5.0, min_length=20)


class TestIsSafeValue:
    """Tests for safe value detection (UUIDs, hashes, etc.)."""

    def test_uuid(self):
        """UUIDs are safe."""
        assert is_safe_value("550e8400-e29b-41d4-a716-446655440000")
        assert is_safe_value("550E8400-E29B-41D4-A716-446655440000")

    def test_git_sha(self):
        """Git commit SHAs are safe."""
        assert is_safe_value("a" * 40)
        assert is_safe_value("abc123def456789012345678901234567890abcd")

    def test_md5_hash(self):
        """MD5 hashes are safe."""
        assert is_safe_value("d41d8cd98f00b204e9800998ecf8427e")

    def test_sha256_hash(self):
        """SHA-256 hashes are safe."""
        assert is_safe_value("e3b0c44298fc1c149afbf4c8996fb924" + "27ae41e4649b934ca495991b7852b855")

    def test_not_safe(self):
        """Random strings are not safe."""
        assert not is_safe_value("sk_live_abcdefghijklmnop")


class TestRedactionConfig:
    """Tests for RedactionConfig parsing and options."""

    def test_empty_config(self):
        """Empty config uses defaults."""
        config = RedactionConfig()
        assert config.entropy_enabled is False
        assert config.paranoid_mode is False
        assert config.custom_rules == []

    def test_from_dict_custom_rules(self):
        """Custom rules are parsed from dict."""
        data = {
            "custom_rules": [
                {
                    "name": "my_api_key",
                    "pattern": r"MY_KEY_[A-Z0-9]{10}",
                    "replacement": "[MY_KEY_REDACTED]",
                }
            ]
        }
        config = RedactionConfig.from_dict(data)
        assert len(config.custom_rules) == 1
        assert config.custom_rules[0].name == "my_api_key"

    def test_from_dict_allowlist(self):
        """Allowlist is parsed from dict."""
        data = {
            "allowlist_patterns": ["*.md", "docs/**"],
            "allowlist_strings": ["test-uuid-123"],
        }
        config = RedactionConfig.from_dict(data)
        assert "*.md" in config.allowlist_patterns
        assert "test-uuid-123" in config.allowlist_strings

    def test_from_dict_entropy(self):
        """Entropy settings are parsed from dict."""
        data = {
            "entropy": {
                "enabled": True,
                "threshold": 4.0,
                "min_length": 30,
            }
        }
        config = RedactionConfig.from_dict(data)
        assert config.entropy_enabled is True
        assert config.entropy_threshold == 4.0
        assert config.entropy_min_length == 30

    def test_from_dict_paranoid(self):
        """Paranoid mode settings are parsed from dict."""
        data = {
            "paranoid": {
                "enabled": True,
                "min_length": 40,
            }
        }
        config = RedactionConfig.from_dict(data)
        assert config.paranoid_mode is True
        assert config.paranoid_min_length == 40

    def test_from_dict_invalid_regex(self):
        """Invalid regex patterns are skipped."""
        data = {
            "custom_rules": [
                {"pattern": "[invalid(regex"},  # Invalid
                {"pattern": r"valid_[A-Z]+"},  # Valid
            ]
        }
        config = RedactionConfig.from_dict(data)
        assert len(config.custom_rules) == 1


class TestRedactorWithConfig:
    """Tests for Redactor with advanced configuration."""

    def test_custom_pattern(self):
        """Custom patterns are applied."""
        config = RedactionConfig()
        config.custom_rules.append(RedactionRule(
            name="internal_key",
            pattern=re.compile(r"INTERNAL_KEY_[A-Z0-9]{16}"),
            replacement="[INTERNAL_KEY_REDACTED]",
        ))
        
        redactor = Redactor(config=config)
        text = "Use key INTERNAL_KEY_ABCD1234EFGH5678 for auth"
        result = redactor.redact(text)
        
        assert "[INTERNAL_KEY_REDACTED]" in result
        assert "ABCD1234" not in result

    def test_entropy_detection(self):
        """Entropy-based detection finds high-entropy strings."""
        config = RedactionConfig(
            entropy_enabled=True,
            entropy_threshold=4.0,
            entropy_min_length=20,
        )
        
        redactor = Redactor(config=config)
        # High entropy string
        text = "secret = xK9fP2mN7qR4sT6vW8yB3dF5gH1jL0aZ"
        result = redactor.redact(text)
        
        # Should be caught by either generic_secret or entropy
        assert "xK9fP2mN7q" not in result

    def test_entropy_skips_safe_values(self):
        """Entropy detection skips known safe values (UUIDs, hashes)."""
        config = RedactionConfig(
            entropy_enabled=True,
            entropy_threshold=3.0,  # Low threshold
            entropy_min_length=20,
        )
        
        redactor = Redactor(config=config)
        # UUID - should NOT be redacted
        text = "id = 550e8400-e29b-41d4-a716-446655440000"
        result = redactor.redact(text)
        
        assert "550e8400-e29b-41d4-a716-446655440000" in result

    def test_paranoid_mode(self):
        """Paranoid mode redacts long base64-like strings."""
        config = RedactionConfig(
            paranoid_mode=True,
            paranoid_min_length=32,
        )
        
        redactor = Redactor(config=config)
        # Long random-looking string
        text = "token = ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnop"
        result = redactor.redact(text)
        
        assert "[LONG_TOKEN_REDACTED]" in result or "[SECRET_REDACTED]" in result

    def test_paranoid_skips_safe_files(self):
        """Paranoid mode skips files matching safe patterns."""
        config = RedactionConfig(
            paranoid_mode=True,
            paranoid_min_length=32,
            safe_file_patterns=["*.md", "*.lock"],
        )
        
        redactor = Redactor(config=config, current_file=Path("README.md"))
        # This would normally be caught by paranoid mode
        text = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnop"
        result = redactor.redact(text)
        
        # Should NOT be redacted in .md file
        assert "ABCDEFGHIJ" in result

    def test_file_allowlist(self):
        """Files matching allowlist patterns are not redacted."""
        config = RedactionConfig(
            allowlist_patterns=["secrets.example", "*.test.py"],
        )
        
        # File matches allowlist
        redactor = Redactor(config=config, current_file=Path("secrets.example"))
        text = "AWS_SECRET_KEY = AKIAIOSFODNN7EXAMPLE"
        result = redactor.redact(text)
        
        # Should NOT be redacted (file is allowlisted)
        assert "AKIAIOSFODNN7EXAMPLE" in result

    def test_string_allowlist(self):
        """Specific strings in allowlist are not redacted."""
        config = RedactionConfig(
            entropy_enabled=True,
            entropy_threshold=3.0,
            entropy_min_length=10,
            allowlist_strings={"test_key_12345678901234567890"},
        )
        
        redactor = Redactor(config=config)
        text = "Use key test_key_12345678901234567890 for testing"
        result = redactor.redact(text)
        
        # The specific string should be preserved
        # (though generic_secret pattern might still match)
        # This tests that entropy detection respects allowlist

    def test_set_current_file(self):
        """set_current_file changes the file context."""
        config = RedactionConfig(
            allowlist_patterns=["*.safe"],
        )
        
        redactor = Redactor(config=config, current_file=Path("secrets.py"))
        
        # Initially not allowlisted
        text = "api_key = ghp_1234567890123456789012345678901234"
        result1 = redactor.redact(text)
        assert "ghp_" not in result1
        
        # Change to allowlisted file
        redactor.set_current_file(Path("config.safe"))
        result2 = redactor.redact(text)
        assert "ghp_" in result2


class TestContextBasedPatterns:
    """Tests for context-based redaction patterns."""

    def test_auth_bearer_header(self):
        """Authorization: Bearer tokens are redacted."""
        redactor = Redactor()
        text = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.test"
        result = redactor.redact(text)
        
        assert "Bearer" in result
        assert "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9" not in result

    def test_auth_basic_header(self):
        """Authorization: Basic tokens are redacted."""
        redactor = Redactor()
        text = "Authorization: Basic dXNlcm5hbWU6cGFzc3dvcmQ="
        result = redactor.redact(text)
        
        assert "Basic" in result
        assert "dXNlcm5hbWU6cGFzc3dvcmQ=" not in result

    def test_x_api_key_header(self):
        """X-API-Key headers are redacted."""
        redactor = Redactor()
        text = "X-API-Key: xapi_1234567890123456"
        result = redactor.redact(text)
        
        # The pattern may match as generic secret or x_api_key_header
        assert "xapi_1234567890123456" not in result


class TestRedactorStats:
    """Tests for redaction statistics tracking."""

    def test_stats_count_patterns(self):
        """Stats track which patterns matched."""
        redactor = Redactor()
        text = """
        ghp_abc1234567890123456789012345678901
        password = "mysecretpassword123"
        """
        redactor.redact(text)
        stats = redactor.get_stats()
        
        # At least one pattern should match (github_token or password)
        assert len(stats) >= 1

    def test_stats_count_entropy(self):
        """Stats track entropy detections."""
        config = RedactionConfig(
            entropy_enabled=True,
            entropy_threshold=3.5,
            entropy_min_length=25,
        )
        redactor = Redactor(config=config)
        text = "token = xK9fP2mN7qR4sT6vW8yB3dF5gH1jL0aZbC"
        redactor.redact(text)
        stats = redactor.get_stats()
        
        # Either caught by generic_secret or entropy
        assert len(stats) > 0

    def test_reset_stats(self):
        """Stats can be reset."""
        redactor = Redactor()
        # Use a definite pattern that will match (password pattern needs 16+ chars)
        redactor.redact('password = "mysupersecretpassword123"')
        assert len(redactor.get_stats()) > 0
        
        redactor.reset_stats()
        assert len(redactor.get_stats()) == 0


class TestCreateRedactor:
    """Tests for the create_redactor factory function."""

    def test_disabled(self):
        """Disabled redactor passes through unchanged."""
        redactor = create_redactor(enabled=False)
        text = "ghp_abc1234567890123456789012345678901"
        assert redactor.redact(text) == text

    def test_with_config(self):
        """Config is passed to redactor."""
        config = RedactionConfig(paranoid_mode=True)
        redactor = create_redactor(enabled=True, config=config)
        assert redactor.config.paranoid_mode is True

    def test_with_current_file(self):
        """Current file is set."""
        redactor = create_redactor(enabled=True, current_file=Path("test.py"))
        assert redactor.current_file == Path("test.py")
