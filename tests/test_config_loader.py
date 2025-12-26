"""Tests for configuration file loading and merging."""

import tempfile
from pathlib import Path

import pytest

from repo_to_prompt.config_loader import (
    CONFIG_FILE_NAMES,
    ProjectConfig,
    RankingWeights,
    find_config_file,
    load_config,
    merge_cli_with_config,
)


class TestConfigFileFinding:
    """Tests for finding config files."""

    def test_find_toml_config(self):
        """Test finding repo-to-prompt.toml config file."""
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            config_file = root / "repo-to-prompt.toml"
            config_file.write_text('[repo-to-prompt]\nmax_tokens = 50000\n')
            
            found = find_config_file(root)
            assert found == config_file

    def test_find_hidden_toml_config(self):
        """Test finding .repo-to-prompt.toml config file."""
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            config_file = root / ".repo-to-prompt.toml"
            config_file.write_text('[repo-to-prompt]\nmax_tokens = 50000\n')
            
            found = find_config_file(root)
            assert found == config_file

    def test_find_r2p_yaml_config(self):
        """Test finding .r2p.yml config file."""
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            config_file = root / ".r2p.yml"
            config_file.write_text('max_tokens: 50000\n')
            
            found = find_config_file(root)
            assert found == config_file

    def test_no_config_file(self):
        """Test when no config file exists."""
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            found = find_config_file(root)
            assert found is None

    def test_config_file_priority(self):
        """Test that first matching file in priority order is found."""
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            # Create multiple config files
            (root / "repo-to-prompt.toml").write_text('[repo-to-prompt]\nmax_tokens = 1\n')
            (root / ".r2p.yml").write_text('max_tokens: 2\n')
            
            found = find_config_file(root)
            # repo-to-prompt.toml should be found first
            assert found.name == "repo-to-prompt.toml"


class TestConfigLoading:
    """Tests for loading config from files."""

    def test_load_toml_config(self):
        """Test loading configuration from TOML file."""
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            config_file = root / "repo-to-prompt.toml"
            config_file.write_text('''
[repo-to-prompt]
max_tokens = 50000
chunk_tokens = 1000
include_extensions = [".py", ".ts"]
exclude_globs = ["dist/**", "build/**"]
follow_symlinks = true
''')
            
            config = load_config(root)
            
            assert config.max_tokens == 50000
            assert config.chunk_tokens == 1000
            assert config.include_extensions == {".py", ".ts"}
            assert config.exclude_globs == {"dist/**", "build/**"}
            assert config.follow_symlinks is True
            assert config._config_file == config_file

    def test_load_yaml_config(self):
        """Test loading configuration from YAML file."""
        pytest.importorskip("yaml")
        
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            config_file = root / ".r2p.yml"
            config_file.write_text('''
max_tokens: 100000
chunk_tokens: 500
include_extensions:
  - .py
  - .js
mode: rag
redact_secrets: false
''')
            
            config = load_config(root)
            
            assert config.max_tokens == 100000
            assert config.chunk_tokens == 500
            assert config.include_extensions == {".py", ".js"}
            assert config.mode == "rag"
            assert config.redact_secrets is False

    def test_load_nested_toml_section(self):
        """Test loading config from nested [repo-to-prompt] section."""
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            config_file = root / "repo-to-prompt.toml"
            config_file.write_text('''
# Other config
[something_else]
foo = "bar"

[repo-to-prompt]
max_tokens = 75000
''')
            
            config = load_config(root)
            assert config.max_tokens == 75000

    def test_load_ranking_weights(self):
        """Test loading custom ranking weights."""
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            config_file = root / "repo-to-prompt.toml"
            config_file.write_text('''
[repo-to-prompt]
max_tokens = 50000

[repo-to-prompt.ranking_weights]
readme = 1.0
test = 0.3
generated = 0.1
''')
            
            config = load_config(root)
            
            assert config.ranking_weights.readme == 1.0
            assert config.ranking_weights.test == 0.3
            assert config.ranking_weights.generated == 0.1
            # Unspecified weights should be default
            assert config.ranking_weights.config == 0.90

    def test_load_extensions_as_string(self):
        """Test loading extensions from comma-separated string."""
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            config_file = root / "repo-to-prompt.toml"
            config_file.write_text('''
[repo-to-prompt]
include_extensions = ".py,.ts,.js"
''')
            
            config = load_config(root)
            assert config.include_extensions == {".py", ".ts", ".js"}

    def test_load_nonexistent_config(self):
        """Test loading returns empty config when file doesn't exist."""
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            config = load_config(root)
            
            assert config.max_tokens is None
            assert config.include_extensions is None
            assert config._config_file is None

    def test_load_invalid_config_gracefully(self):
        """Test that invalid config files are handled gracefully."""
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            config_file = root / "repo-to-prompt.toml"
            config_file.write_text('this is not valid toml {{{{')
            
            # Should not raise, returns empty config
            config = load_config(root)
            assert config.max_tokens is None


class TestConfigMerging:
    """Tests for merging CLI args with config file values."""

    def test_cli_overrides_config(self):
        """Test that CLI values override config file values."""
        config = ProjectConfig(
            max_tokens=50000,
            chunk_tokens=800,
            mode="both",
        )
        
        merged = merge_cli_with_config(
            config,
            max_tokens=100000,  # CLI override
            chunk_tokens=None,  # Not specified, use config
            mode="rag",  # CLI override
        )
        
        assert merged["max_tokens"] == 100000  # CLI value
        assert merged["chunk_tokens"] == 800  # Config value
        assert merged["mode"] == "rag"  # CLI value

    def test_config_values_used_when_cli_not_specified(self):
        """Test that config values are used when CLI args are None."""
        config = ProjectConfig(
            include_extensions={".py", ".ts"},
            exclude_globs={"node_modules/**"},
            max_file_bytes=2_000_000,
            follow_symlinks=True,
        )
        
        merged = merge_cli_with_config(config)
        
        assert merged["include_extensions"] == {".py", ".ts"}
        assert merged["exclude_globs"] == {"node_modules/**"}
        assert merged["max_file_bytes"] == 2_000_000
        assert merged["follow_symlinks"] is True

    def test_defaults_used_when_neither_specified(self):
        """Test that defaults are used when neither CLI nor config specify a value."""
        config = ProjectConfig()  # Empty config
        
        merged = merge_cli_with_config(config)
        
        assert merged["max_file_bytes"] == 1_048_576  # Default 1MB
        assert merged["max_total_bytes"] == 20_000_000  # Default 20MB
        assert merged["chunk_tokens"] == 800  # Default
        assert merged["mode"] == "both"  # Default
        assert merged["max_tokens"] is None  # No limit by default

    def test_no_gitignore_flag(self):
        """Test that --no-gitignore overrides config."""
        config = ProjectConfig(respect_gitignore=True)
        
        merged = merge_cli_with_config(config, no_gitignore=True)
        
        assert merged["respect_gitignore"] is False

    def test_no_redact_flag(self):
        """Test that --no-redact overrides config."""
        config = ProjectConfig(redact_secrets=True)
        
        merged = merge_cli_with_config(config, no_redact=True)
        
        assert merged["redact_secrets"] is False

    def test_include_minified_flag(self):
        """Test that --include-minified sets skip_minified=False."""
        config = ProjectConfig(skip_minified=True)
        
        merged = merge_cli_with_config(config, include_minified=True)
        
        assert merged["skip_minified"] is False

    def test_ranking_weights_passed_through(self):
        """Test that ranking weights from config are passed through."""
        config = ProjectConfig()
        config.ranking_weights = RankingWeights(readme=0.9, test=0.2)
        
        merged = merge_cli_with_config(config)
        
        assert merged["ranking_weights"].readme == 0.9
        assert merged["ranking_weights"].test == 0.2


class TestRankingWeights:
    """Tests for ranking weights configuration."""

    def test_default_weights(self):
        """Test default ranking weights."""
        weights = RankingWeights()
        
        assert weights.readme == 1.0
        assert weights.config == 0.90
        assert weights.test == 0.50
        assert weights.generated == 0.20

    def test_custom_weights(self):
        """Test creating custom ranking weights."""
        weights = RankingWeights(
            readme=0.95,
            test=0.3,
            generated=0.05,
        )
        
        assert weights.readme == 0.95
        assert weights.test == 0.3
        assert weights.generated == 0.05
        # Unspecified should be default
        assert weights.config == 0.90

    def test_weights_from_dict(self):
        """Test creating weights from dictionary."""
        weights = RankingWeights.from_dict({
            "readme": 0.8,
            "test": 0.4,
            "unknown_key": 0.5,  # Should be ignored
        })
        
        assert weights.readme == 0.8
        assert weights.test == 0.4
        assert weights.config == 0.90  # Default

    def test_weights_to_dict(self):
        """Test converting weights to dictionary."""
        weights = RankingWeights(readme=0.9, test=0.3)
        weights_dict = weights.to_dict()
        
        assert weights_dict["readme"] == 0.9
        assert weights_dict["test"] == 0.3
        assert "config" in weights_dict


class TestProjectConfigSerialization:
    """Tests for ProjectConfig serialization."""

    def test_to_dict_sorted_keys(self):
        """Test that to_dict returns sorted keys."""
        config = ProjectConfig(
            max_tokens=50000,
            include_extensions={".ts", ".py", ".js"},
            mode="rag",
        )
        
        result = config.to_dict()
        keys = list(result.keys())
        
        assert keys == sorted(keys)

    def test_to_dict_sorted_extensions(self):
        """Test that extensions are sorted in to_dict."""
        config = ProjectConfig(
            include_extensions={".ts", ".py", ".js"},
        )
        
        result = config.to_dict()
        
        assert result["include_extensions"] == [".js", ".py", ".ts"]

    def test_to_dict_excludes_none_values(self):
        """Test that None values are excluded from to_dict."""
        config = ProjectConfig(max_tokens=50000)
        
        result = config.to_dict()
        
        assert "max_tokens" in result
        assert "chunk_tokens" not in result  # None, should be excluded

    def test_to_dict_includes_config_file_path(self):
        """Test that _loaded_from is included when config file is set."""
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            config = ProjectConfig(
                max_tokens=50000,
                _config_file=root / "repo-to-prompt.toml",
            )
            
            result = config.to_dict()
            
            assert "_loaded_from" in result
