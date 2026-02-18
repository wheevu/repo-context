//! Redaction rules
//!
//! ORDER MATTERS: Specific patterns (AWS, GitHub, Stripe, etc.) must come BEFORE
//! generic patterns (generic_secret, env_secret). This ensures that a Stripe key
//! gets redacted as [STRIPE_SECRET_KEY_REDACTED] instead of generic [SECRET_REDACTED].

use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Clone)]
pub struct RedactionRule {
    pub name: &'static str,
    pub pattern: Regex,
    pub replacement: &'static str,
}

pub static DEFAULT_RULES: Lazy<Vec<RedactionRule>> = Lazy::new(|| {
    vec![
        // ── AWS ──────────────────────────────────────────────────────────────────
        RedactionRule {
            name: "aws_access_key",
            pattern: Regex::new(r"\bAKIA[0-9A-Z]{16}\b").expect("valid regex"),
            replacement: "[AWS_ACCESS_KEY_REDACTED]",
        },
        RedactionRule {
            name: "aws_secret_key",
            pattern: Regex::new(
                "(?i)(aws[_\\-]?secret[_\\-]?(?:access[_\\-]?)?key)['\"]?\\s*[:=]\\s*['\"]?([A-Za-z0-9/+=]{40})['\"]?",
            )
            .expect("valid regex"),
            replacement: "${1}=[AWS_SECRET_REDACTED]",
        },
        // ── GitHub ───────────────────────────────────────────────────────────────
        RedactionRule {
            name: "github_token",
            pattern: Regex::new(r"\bghp_[A-Za-z0-9]{36}\b").expect("valid regex"),
            replacement: "[GITHUB_TOKEN_REDACTED]",
        },
        RedactionRule {
            name: "github_oauth",
            pattern: Regex::new(r"\bgho_[A-Za-z0-9]{36}\b").expect("valid regex"),
            replacement: "[GITHUB_OAUTH_REDACTED]",
        },
        RedactionRule {
            name: "github_app_token",
            pattern: Regex::new(r"\bghu_[A-Za-z0-9]{36}\b").expect("valid regex"),
            replacement: "[GITHUB_APP_TOKEN_REDACTED]",
        },
        RedactionRule {
            name: "github_refresh_token",
            pattern: Regex::new(r"\bghr_[A-Za-z0-9]{36}\b").expect("valid regex"),
            replacement: "[GITHUB_REFRESH_TOKEN_REDACTED]",
        },
        // ── GitLab ───────────────────────────────────────────────────────────────
        RedactionRule {
            name: "gitlab_token",
            pattern: Regex::new(r"\bglpat-[A-Za-z0-9\-_]{20,}\b").expect("valid regex"),
            replacement: "[GITLAB_TOKEN_REDACTED]",
        },
        // ── Slack ────────────────────────────────────────────────────────────────
        RedactionRule {
            name: "slack_token",
            pattern: Regex::new(r"\bxox[baprs]-[0-9A-Za-z\-]{10,}\b").expect("valid regex"),
            replacement: "[SLACK_TOKEN_REDACTED]",
        },
        RedactionRule {
            name: "slack_webhook",
            pattern: Regex::new(
                r"https://hooks\.slack\.com/services/T[A-Z0-9]+/B[A-Z0-9]+/[A-Za-z0-9]+",
            )
            .expect("valid regex"),
            replacement: "[SLACK_WEBHOOK_REDACTED]",
        },
        // ── Stripe ───────────────────────────────────────────────────────────────
        RedactionRule {
            name: "stripe_key",
            pattern: Regex::new(r"\bsk_live_[A-Za-z0-9]{24,}\b").expect("valid regex"),
            replacement: "[STRIPE_SECRET_KEY_REDACTED]",
        },
        RedactionRule {
            name: "stripe_test_key",
            pattern: Regex::new(r"\bsk_test_[A-Za-z0-9]{24,}\b").expect("valid regex"),
            replacement: "[STRIPE_TEST_KEY_REDACTED]",
        },
        // ── Twilio ───────────────────────────────────────────────────────────────
        RedactionRule {
            name: "twilio_api_key",
            pattern: Regex::new(r"\bSK[0-9a-fA-F]{32}\b").expect("valid regex"),
            replacement: "[TWILIO_KEY_REDACTED]",
        },
        // ── SendGrid ─────────────────────────────────────────────────────────────
        RedactionRule {
            name: "sendgrid_key",
            pattern: Regex::new(r"\bSG\.[A-Za-z0-9\-_]{22,}\.[A-Za-z0-9\-_]{22,}\b")
                .expect("valid regex"),
            replacement: "[SENDGRID_KEY_REDACTED]",
        },
        // ── Mailchimp ────────────────────────────────────────────────────────────
        RedactionRule {
            name: "mailchimp_key",
            pattern: Regex::new(r"\b[a-f0-9]{32}-us[0-9]{1,2}\b").expect("valid regex"),
            replacement: "[MAILCHIMP_KEY_REDACTED]",
        },
        // ── Google ───────────────────────────────────────────────────────────────
        RedactionRule {
            name: "google_api_key",
            pattern: Regex::new(r"\bAIza[0-9A-Za-z\-_]{35}\b").expect("valid regex"),
            replacement: "[GOOGLE_API_KEY_REDACTED]",
        },
        RedactionRule {
            name: "google_oauth",
            pattern: Regex::new(
                r"\b[0-9]+-[a-z0-9_]{32}\.apps\.googleusercontent\.com\b",
            )
            .expect("valid regex"),
            replacement: "[GOOGLE_OAUTH_REDACTED]",
        },
        // ── Firebase ─────────────────────────────────────────────────────────────
        RedactionRule {
            name: "firebase_key",
            pattern: Regex::new(r"\bAAAA[A-Za-z0-9_-]{7,}:[A-Za-z0-9_-]{140,}\b")
                .expect("valid regex"),
            replacement: "[FIREBASE_KEY_REDACTED]",
        },
        // ── Heroku ───────────────────────────────────────────────────────────────
        RedactionRule {
            name: "heroku_api_key",
            pattern: Regex::new(
                "(?i)(heroku[_\\-]?api[_\\-]?key)['\"]?\\s*[:=]\\s*['\"]?([a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})['\"]?",
            )
            .expect("valid regex"),
            replacement: "${1}=[HEROKU_KEY_REDACTED]",
        },
        // ── npm ──────────────────────────────────────────────────────────────────
        RedactionRule {
            name: "npm_token",
            pattern: Regex::new(r"\bnpm_[A-Za-z0-9]{36}\b").expect("valid regex"),
            replacement: "[NPM_TOKEN_REDACTED]",
        },
        // ── PyPI ─────────────────────────────────────────────────────────────────
        RedactionRule {
            name: "pypi_token",
            pattern: Regex::new(r"\bpypi-[A-Za-z0-9\-_]{50,}\b").expect("valid regex"),
            replacement: "[PYPI_TOKEN_REDACTED]",
        },
        // ── OpenAI ───────────────────────────────────────────────────────────────
        RedactionRule {
            name: "openai_key",
            pattern: Regex::new(r"\bsk-[A-Za-z0-9]{20,}\b").expect("valid regex"),
            replacement: "[REDACTED_OPENAI_KEY]",
        },
        // ── Private keys (PEM blocks) ─────────────────────────────────────────────
        RedactionRule {
            name: "private_key_header",
            pattern: Regex::new(
                r"-----BEGIN\s+(?:RSA\s+|DSA\s+|EC\s+|OPENSSH\s+)?PRIVATE\s+KEY-----[\s\S]*?-----END\s+(?:RSA\s+|DSA\s+|EC\s+|OPENSSH\s+)?PRIVATE\s+KEY-----",
            )
            .expect("valid regex"),
            replacement: "[PRIVATE_KEY_REDACTED]",
        },
        // ── JWT ──────────────────────────────────────────────────────────────────
        RedactionRule {
            name: "jwt_token",
            pattern: Regex::new(
                r"\beyJ[A-Za-z0-9\-_]+\.eyJ[A-Za-z0-9\-_]+\.[A-Za-z0-9\-_]+\b",
            )
            .expect("valid regex"),
            replacement: "[JWT_TOKEN_REDACTED]",
        },
        // ── Connection strings ────────────────────────────────────────────────────
        RedactionRule {
            name: "connection_string",
            pattern: Regex::new(
                r"((?:postgres|mysql|mongodb|redis|amqp)(?:ql)?://[^:]+:)([^@]+)(@)",
            )
            .expect("valid regex"),
            replacement: "${1}[PASSWORD_REDACTED]${3}",
        },
        // ── Basic auth in URLs ────────────────────────────────────────────────────
        RedactionRule {
            name: "url_auth",
            pattern: Regex::new(r"(https?://[^:]+:)([^@]+)(@[^\s]+)").expect("valid regex"),
            replacement: "${1}[PASSWORD_REDACTED]${3}",
        },
        // ── HTTP Authorization headers ────────────────────────────────────────────
        RedactionRule {
            name: "auth_bearer",
            pattern: Regex::new(
                r"(?i)(Authorization:\s*Bearer\s+)([A-Za-z0-9\-_./+=]{20,})",
            )
            .expect("valid regex"),
            replacement: "${1}[BEARER_TOKEN_REDACTED]",
        },
        RedactionRule {
            name: "auth_basic",
            pattern: Regex::new(
                r"(?i)(Authorization:\s*Basic\s+)([A-Za-z0-9+/=]{20,})",
            )
            .expect("valid regex"),
            replacement: "${1}[BASIC_AUTH_REDACTED]",
        },
        RedactionRule {
            name: "x_api_key_header",
            pattern: Regex::new(r"(?i)(X-API-Key:\s*)([A-Za-z0-9\-_./+=]{16,})")
                .expect("valid regex"),
            replacement: "${1}[API_KEY_REDACTED]",
        },
        // ── Generic secret assignments ────────────────────────────────────────────
        // Must come AFTER all specific rules so specific replacements win.
        RedactionRule {
            name: "generic_secret",
            pattern: Regex::new(
                "(?i)((?:api[_\\-]?key|apikey|secret[_\\-]?key|secretkey|auth[_\\-]?token|authtoken|access[_\\-]?token|accesstoken|password|passwd|pwd|credentials?|bearer))(['\"]?\\s*[:=]\\s*['\"]?)([A-Za-z0-9\\-_./+=]{16,})(['\"]?)",
            )
            .expect("valid regex"),
            replacement: "${1}${2}[SECRET_REDACTED]${4}",
        },
        // ── Environment variable exports ──────────────────────────────────────────
        RedactionRule {
            name: "env_secret",
            pattern: Regex::new(
                r"(?i)(export\s+(?:API_KEY|SECRET_KEY|AUTH_TOKEN|ACCESS_TOKEN|PASSWORD|DATABASE_URL|PRIVATE_KEY)[=])([^\s\n]+)",
            )
            .expect("valid regex"),
            replacement: "${1}[SECRET_REDACTED]",
        },
    ]
});

#[cfg(test)]
mod tests {
    use super::DEFAULT_RULES;

    fn redact(input: &str) -> String {
        let mut output = input.to_string();
        for rule in DEFAULT_RULES.iter() {
            output = rule
                .pattern
                .replace_all(&output, |caps: &regex::Captures<'_>| {
                    let mut expanded = String::new();
                    caps.expand(rule.replacement, &mut expanded);
                    expanded
                })
                .into_owned();
        }
        output
    }

    #[test]
    fn redacts_aws_access_key() {
        let out = redact("key=AKIAIOSFODNN7EXAMPLE");
        assert!(out.contains("[AWS_ACCESS_KEY_REDACTED]"), "got: {out}");
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn redacts_github_token() {
        // ghp_ + exactly 36 alphanumeric chars
        let out = redact("GITHUB_TOKEN=ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
        assert!(out.contains("[GITHUB_TOKEN_REDACTED]"), "got: {out}");
        assert!(!out.contains("ghp_"));
    }

    #[test]
    fn redacts_github_oauth() {
        let out = redact("token=gho_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
        assert!(out.contains("[GITHUB_OAUTH_REDACTED]"), "got: {out}");
    }

    #[test]
    fn redacts_gitlab_token() {
        let out = redact("token=glpat-abcdefghijklmnopqrst");
        assert!(out.contains("[GITLAB_TOKEN_REDACTED]"), "got: {out}");
    }

    #[test]
    fn redacts_slack_token() {
        let out = redact("token=xoxb-1234567890-abcdefghij");
        assert!(out.contains("[SLACK_TOKEN_REDACTED]"), "got: {out}");
    }

    #[test]
    fn redacts_slack_webhook() {
        let url = "https://hooks.slack.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXXXXXX";
        let out = redact(&format!("WEBHOOK={url}"));
        assert!(out.contains("[SLACK_WEBHOOK_REDACTED]"), "got: {out}");
        assert!(!out.contains("T00000000"));
    }

    #[test]
    fn redacts_stripe_live_key() {
        let key = "sk_live_abcdefghijklmnopqrstuvwxyz";
        let out = redact(&format!("STRIPE_KEY={key}"));
        assert!(out.contains("[STRIPE_SECRET_KEY_REDACTED]"), "got: {out}");
        assert!(!out.contains("sk_live_"));
    }

    #[test]
    fn redacts_stripe_test_key() {
        let key = "sk_test_abcdefghijklmnopqrstuvwxyz";
        let out = redact(&format!("STRIPE_TEST_KEY={key}"));
        assert!(out.contains("[STRIPE_TEST_KEY_REDACTED]"), "got: {out}");
    }

    #[test]
    fn redacts_sendgrid_key() {
        let key = "SG.abcdefghijklmnopqrstuvwx.abcdefghijklmnopqrstuvwx";
        let out = redact(&format!("SG_KEY={key}"));
        assert!(out.contains("[SENDGRID_KEY_REDACTED]"), "got: {out}");
    }

    #[test]
    fn redacts_google_api_key() {
        // AIza + exactly 35 alphanumeric/dash/underscore chars
        let key = "AIzaSyD-abcdefghijklmnopqrstuvwxyz12345";
        let out = redact(&format!("KEY={key}"));
        assert!(out.contains("[GOOGLE_API_KEY_REDACTED]"), "got: {out}");
    }

    #[test]
    fn redacts_npm_token() {
        let key = "npm_abcdefghijklmnopqrstuvwxyz1234567890";
        let out = redact(&format!("NPM_TOKEN={key}"));
        assert!(out.contains("[NPM_TOKEN_REDACTED]"), "got: {out}");
    }

    #[test]
    fn redacts_pypi_token() {
        let key = "pypi-AgEIcHlwaS5vcmcCJDEyMzQ1Njc4LTEyMzQtMTIzNC0xMjM0LTEyMzQ1Njc4OTAxMg";
        let out = redact(&format!("PYPI_TOKEN={key}"));
        assert!(out.contains("[PYPI_TOKEN_REDACTED]"), "got: {out}");
    }

    #[test]
    fn redacts_openai_key() {
        let out = redact("key=sk-abcdefghijklmnopqrstuvwxyz12345");
        assert!(out.contains("[REDACTED_OPENAI_KEY]"), "got: {out}");
    }

    #[test]
    fn redacts_private_key_block() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAK\n-----END RSA PRIVATE KEY-----";
        let out = redact(pem);
        assert!(out.contains("[PRIVATE_KEY_REDACTED]"), "got: {out}");
        assert!(!out.contains("MIIEpAIBAAK"));
    }

    #[test]
    fn redacts_jwt_token() {
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0In0.SflKxwRJSMeKKF2QT4fw";
        let out = redact(&format!("Authorization: Bearer {jwt}"));
        assert!(
            out.contains("[JWT_TOKEN_REDACTED]") || out.contains("[BEARER_TOKEN_REDACTED]"),
            "got: {out}"
        );
        assert!(!out.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"));
    }

    #[test]
    fn redacts_connection_string_password() {
        let out = redact("postgres://user:mysecretpassword@localhost:5432/db");
        assert!(out.contains("[PASSWORD_REDACTED]"), "got: {out}");
        assert!(!out.contains("mysecretpassword"));
    }

    #[test]
    fn redacts_url_basic_auth() {
        let out = redact("https://user:secret123@example.com/api");
        assert!(out.contains("[PASSWORD_REDACTED]"), "got: {out}");
        assert!(!out.contains("secret123"));
    }

    #[test]
    fn redacts_auth_bearer_header() {
        let out = redact("Authorization: Bearer abcdefghijklmnopqrstuvwxyz");
        assert!(out.contains("[BEARER_TOKEN_REDACTED]"), "got: {out}");
    }

    #[test]
    fn redacts_x_api_key_header() {
        let out = redact("X-API-Key: abcdefghijklmnopqrs");
        assert!(out.contains("[API_KEY_REDACTED]"), "got: {out}");
    }

    #[test]
    fn redacts_generic_api_key_assignment() {
        let out = redact(r#"api_key = "abcdefghijklmnop1234567890abcdef""#);
        assert!(out.contains("[SECRET_REDACTED]"), "got: {out}");
        assert!(!out.contains("abcdefghijklmnop1234567890abcdef"));
    }

    #[test]
    fn redacts_env_export_secret() {
        let out = redact("export API_KEY=supersecretvalue123");
        assert!(out.contains("[SECRET_REDACTED]"), "got: {out}");
    }

    #[test]
    fn specific_pattern_wins_over_generic() {
        // Stripe key should use Stripe-specific replacement, not generic SECRET_REDACTED
        let key = "sk_live_ABCD1234567890abcdefghijklmnop";
        let out = redact(&format!(r#"api_key = "{key}""#));
        assert!(out.contains("[STRIPE_SECRET_KEY_REDACTED]"), "got: {out}");
        assert!(!out.contains("[SECRET_REDACTED]"));
    }

    #[test]
    fn github_token_wins_over_generic() {
        let out = redact(r#"auth_token = "ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx""#);
        assert!(out.contains("[GITHUB_TOKEN_REDACTED]"), "got: {out}");
        assert!(!out.contains("[SECRET_REDACTED]"));
    }
}
