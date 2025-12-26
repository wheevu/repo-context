"""Tests for the redactor module."""


from repo_to_prompt.redactor import create_redactor


class TestRedactor:
    """Tests for Redactor."""

    def test_redacts_aws_access_key(self):
        """Test redaction of AWS access keys."""
        redactor = create_redactor(enabled=True)

        content = "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE"
        result = redactor.redact(content)

        assert "AKIAIOSFODNN7EXAMPLE" not in result
        assert "[AWS_ACCESS_KEY_REDACTED]" in result

    def test_redacts_github_token(self):
        """Test redaction of GitHub tokens."""
        redactor = create_redactor(enabled=True)

        content = "GITHUB_TOKEN=ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
        result = redactor.redact(content)

        assert "ghp_" not in result
        assert "[GITHUB_TOKEN_REDACTED]" in result

    def test_redacts_private_key(self):
        """Test redaction of private keys."""
        redactor = create_redactor(enabled=True)

        content = """-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEA0Z3VS5JJcds3xfn/ygWyF8PbnGy0AHB7WgGT3Lc0f1fX5K8v
anotherlineofkey
-----END RSA PRIVATE KEY-----"""

        result = redactor.redact(content)

        assert "MIIEpAIBAAKCAQEA0Z3VS5JJ" not in result
        assert "[PRIVATE_KEY_REDACTED]" in result

    def test_redacts_jwt_token(self):
        """Test redaction of JWT tokens."""
        redactor = create_redactor(enabled=True)

        # Example JWT structure (header.payload.signature)
        jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
        content = f"Authorization: Bearer {jwt}"
        result = redactor.redact(content)

        assert "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9" not in result
        assert "[JWT_TOKEN_REDACTED]" in result

    def test_redacts_generic_api_key(self):
        """Test redaction of generic API keys."""
        redactor = create_redactor(enabled=True)

        content = 'api_key = "abcdefghijklmnop1234567890abcdef"'
        result = redactor.redact(content)

        assert "abcdefghijklmnop1234567890abcdef" not in result
        assert "[SECRET_REDACTED]" in result

    def test_redacts_connection_string_password(self):
        """Test redaction of passwords in connection strings."""
        redactor = create_redactor(enabled=True)

        content = "DATABASE_URL=postgres://user:mysecretpassword@localhost:5432/db"
        result = redactor.redact(content)

        assert "mysecretpassword" not in result
        assert "[PASSWORD_REDACTED]" in result

    def test_redacts_slack_webhook(self):
        """Test redaction of Slack webhooks."""
        redactor = create_redactor(enabled=True)

        # Construct at runtime so GitHub push protection doesn't flag the literal.
        webhook = (
            "https://hooks.slack.com/services/"
            + "T" + "00000000"
            + "/B" + "00000000"
            + "/" + ("X" * 24)
        )
        content = "SLACK_WEBHOOK=" + webhook
        result = redactor.redact(content)

        assert "T00000000/B00000000" not in result
        assert "[SLACK_WEBHOOK_REDACTED]" in result

    def test_disabled_redactor_passes_through(self):
        """Test that disabled redactor doesn't modify content."""
        redactor = create_redactor(enabled=False)

        content = "GITHUB_TOKEN=ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
        result = redactor.redact(content)

        assert result == content

    def test_tracks_redaction_stats(self):
        """Test that redaction statistics are tracked."""
        redactor = create_redactor(enabled=True)

        content = """
        GITHUB_TOKEN=ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
        AWS_KEY=AKIAIOSFODNN7EXAMPLE
        """

        redactor.redact(content)
        stats = redactor.get_stats()

        assert len(stats) > 0

    def test_reset_stats(self):
        """Test resetting redaction statistics."""
        redactor = create_redactor(enabled=True)

        content = "GITHUB_TOKEN=ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
        redactor.redact(content)

        assert len(redactor.get_stats()) > 0

        redactor.reset_stats()

        assert len(redactor.get_stats()) == 0

    def test_preserves_non_secret_content(self):
        """Test that non-secret content is preserved."""
        redactor = create_redactor(enabled=True)

        content = """
def hello_world():
    print("Hello, World!")
    x = 42
    return x
"""
        result = redactor.redact(content)

        assert result == content

    def test_redacts_stripe_keys(self):
        """Test redaction of Stripe keys."""
        redactor = create_redactor(enabled=True)

        # Construct at runtime so GitHub push protection doesn't flag the literal.
        stripe_key = "sk_" + "live_" + "abcdefghijklmnopqrstuvwxyz"
        content = "STRIPE_KEY=" + stripe_key
        result = redactor.redact(content)

        assert "sk_live_" not in result
        assert "[STRIPE_SECRET_KEY_REDACTED]" in result

    def test_redact_line_method(self):
        """Test line-by-line redaction."""
        redactor = create_redactor(enabled=True)

        line = "export GITHUB_TOKEN=ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
        result = redactor.redact_line(line)

        assert "ghp_" not in result


class TestRedactionPrecedence:
    """Tests for pattern precedence in redaction."""

    def test_stripe_key_not_generic_redacted(self):
        """Test that Stripe key uses Stripe-specific replacement, not generic."""
        redactor = create_redactor(enabled=True)

        # Stripe live key in an api_key context (constructed at runtime to avoid scanners)
        stripe_key = "sk_" + "live_" + "ABCD1234567890abcdefghijklmnop"
        content = 'api_key = "' + stripe_key + '"'
        result = redactor.redact(content)

        # Should be Stripe-specific, NOT generic SECRET_REDACTED
        assert "[STRIPE_SECRET_KEY_REDACTED]" in result
        assert "[SECRET_REDACTED]" not in result

    def test_github_token_not_generic_redacted(self):
        """Test that GitHub token uses GitHub-specific replacement."""
        redactor = create_redactor(enabled=True)

        content = 'auth_token = "ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"'
        result = redactor.redact(content)

        # Should be GitHub-specific
        assert "[GITHUB_TOKEN_REDACTED]" in result
        assert "[SECRET_REDACTED]" not in result

    def test_aws_key_not_generic_redacted(self):
        """Test that AWS key uses AWS-specific replacement."""
        redactor = create_redactor(enabled=True)

        content = 'access_key = "AKIAIOSFODNN7EXAMPLE"'
        result = redactor.redact(content)

        # Should be AWS-specific
        assert "[AWS_ACCESS_KEY_REDACTED]" in result
        assert "[SECRET_REDACTED]" not in result

    def test_generic_fallback_for_unknown_secrets(self):
        """Test that unknown secrets still get generic redaction."""
        redactor = create_redactor(enabled=True)

        # Random long string that doesn't match specific patterns
        content = 'api_key = "randomsecretvalue12345678901234567890"'
        result = redactor.redact(content)

        # Should be generic redacted
        assert "[SECRET_REDACTED]" in result


class TestCreateRedactor:
    """Tests for create_redactor factory function."""

    def test_creates_enabled_redactor(self):
        """Test creating an enabled redactor."""
        redactor = create_redactor(enabled=True)

        assert redactor.enabled is True

    def test_creates_disabled_redactor(self):
        """Test creating a disabled redactor."""
        redactor = create_redactor(enabled=False)

        assert redactor.enabled is False
