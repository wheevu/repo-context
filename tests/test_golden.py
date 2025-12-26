"""
Golden file tests for deterministic output.

These tests verify that output is reproducible and matches expected "golden" outputs.
"""

import hashlib
import json
import tempfile
from pathlib import Path

import pytest

from repo_to_prompt.chunker import chunk_file, coalesce_small_chunks
from repo_to_prompt.config import OutputMode
from repo_to_prompt.ranker import FileRanker
from repo_to_prompt.redactor import create_redactor
from repo_to_prompt.renderer import render_context_pack, render_jsonl, write_outputs
from repo_to_prompt.scanner import scan_repository


def create_minimal_repo(root: Path) -> None:
    """Create a minimal, deterministic test repository."""
    # Single file to ensure deterministic output
    (root / "main.py").write_text('''"""Main module."""

def hello():
    """Say hello."""
    return "Hello, World!"


def add(a: int, b: int) -> int:
    """Add two numbers."""
    return a + b
''')
    
    (root / "README.md").write_text("""# Minimal Project

A minimal test project.
""")


@pytest.fixture
def minimal_repo():
    """Create a minimal fixture repository."""
    with tempfile.TemporaryDirectory() as tmpdir:
        root = Path(tmpdir)
        create_minimal_repo(root)
        yield root


class TestDeterministicOutput:
    """Tests for deterministic, reproducible output."""

    def test_context_pack_is_deterministic(self, minimal_repo):
        """Test that context pack output is identical across runs."""
        def generate_context_pack():
            files, stats = scan_repository(minimal_repo)
            scanned_paths = {f.relative_path for f in files}
            ranker = FileRanker(minimal_repo, scanned_files=scanned_paths)
            files = ranker.rank_files(files)
            
            all_chunks = []
            for f in files:
                chunks = chunk_file(f, max_tokens=500)
                all_chunks.extend(chunks)
            
            stats.chunks_created = len(all_chunks)
            
            return render_context_pack(
                root_path=minimal_repo,
                files=files,
                chunks=all_chunks,
                ranker=ranker,
                stats=stats,
                include_timestamp=False,  # Disable timestamps for reproducibility
            )
        
        output1 = generate_context_pack()
        output2 = generate_context_pack()
        
        assert output1 == output2

    def test_jsonl_is_deterministic(self, minimal_repo):
        """Test that JSONL output is identical across runs."""
        def generate_jsonl():
            files, _ = scan_repository(minimal_repo)
            scanned_paths = {f.relative_path for f in files}
            ranker = FileRanker(minimal_repo, scanned_files=scanned_paths)
            files = ranker.rank_files(files)
            
            all_chunks = []
            for f in files:
                chunks = chunk_file(f, max_tokens=500)
                all_chunks.extend(chunks)
            
            return render_jsonl(all_chunks)
        
        output1 = generate_jsonl()
        output2 = generate_jsonl()
        
        assert output1 == output2

    def test_report_json_is_deterministic(self, minimal_repo):
        """Test that report.json content (excluding paths) is identical across runs."""
        def generate_report():
            with tempfile.TemporaryDirectory() as outdir:
                output_dir = Path(outdir)
                files, stats = scan_repository(minimal_repo)
                scanned_paths = {f.relative_path for f in files}
                ranker = FileRanker(minimal_repo, scanned_files=scanned_paths)
                files = ranker.rank_files(files)
                
                all_chunks = []
                for f in files:
                    chunks = chunk_file(f, max_tokens=500)
                    all_chunks.extend(chunks)
                
                stats.chunks_created = len(all_chunks)
                # Set a fixed processing time for determinism
                stats.processing_time_seconds = 1.0
                
                context_pack = render_context_pack(
                    minimal_repo, files, all_chunks, ranker, stats,
                    include_timestamp=False,
                )
                
                write_outputs(
                    output_dir=output_dir,
                    mode=OutputMode.BOTH,
                    context_pack=context_pack,
                    chunks=all_chunks,
                    stats=stats,
                    config={"mode": "both", "chunk_tokens": 500},
                    include_timestamp=False,
                    files=files,
                )
                
                with open(output_dir / "report.json") as f:
                    report = json.load(f)
                
                # Remove non-deterministic fields (absolute paths)
                del report["output_files"]
                return json.dumps(report, sort_keys=True)
        
        output1 = generate_report()
        output2 = generate_report()
        
        assert output1 == output2

    def test_chunk_ids_are_stable(self, minimal_repo):
        """Test that chunk IDs are stable across runs."""
        def get_chunk_ids():
            files, _ = scan_repository(minimal_repo)
            scanned_paths = {f.relative_path for f in files}
            ranker = FileRanker(minimal_repo, scanned_files=scanned_paths)
            files = ranker.rank_files(files)
            
            all_chunks = []
            for f in files:
                chunks = chunk_file(f, max_tokens=500)
                all_chunks.extend(chunks)
            
            return [c.id for c in all_chunks]
        
        ids1 = get_chunk_ids()
        ids2 = get_chunk_ids()
        
        assert ids1 == ids2

    def test_file_ids_are_stable(self, minimal_repo):
        """Test that file IDs are stable across runs."""
        def get_file_ids():
            files, _ = scan_repository(minimal_repo)
            # Sort by path for comparison
            return sorted([(f.relative_path, f.id) for f in files])
        
        ids1 = get_file_ids()
        ids2 = get_file_ids()
        
        assert ids1 == ids2

    def test_files_sorted_by_path_in_scan(self, minimal_repo):
        """Test that scanned files are sorted deterministically."""
        # Add more files
        (minimal_repo / "aaa.py").write_text("# First alphabetically\n")
        (minimal_repo / "zzz.py").write_text("# Last alphabetically\n")
        (minimal_repo / "lib").mkdir()
        (minimal_repo / "lib" / "utils.py").write_text("# In lib\n")
        
        files1, _ = scan_repository(minimal_repo)
        files2, _ = scan_repository(minimal_repo)
        
        paths1 = [f.relative_path for f in files1]
        paths2 = [f.relative_path for f in files2]
        
        assert paths1 == paths2
        # Should be sorted
        assert paths1 == sorted(paths1)


class TestGoldenFileComparison:
    """Tests comparing output against golden files."""

    def test_context_pack_matches_expected_structure(self, minimal_repo):
        """Test that context pack matches expected structure."""
        files, stats = scan_repository(minimal_repo)
        scanned_paths = {f.relative_path for f in files}
        ranker = FileRanker(minimal_repo, scanned_files=scanned_paths)
        files = ranker.rank_files(files)
        
        all_chunks = []
        for f in files:
            chunks = chunk_file(f, max_tokens=500)
            all_chunks.extend(chunks)
        
        stats.chunks_created = len(all_chunks)
        
        context_pack = render_context_pack(
            root_path=minimal_repo,
            files=files,
            chunks=all_chunks,
            ranker=ranker,
            stats=stats,
            include_timestamp=False,
        )
        
        # Verify expected sections exist
        expected_sections = [
            "# Repository Context Pack:",
            "## 📋 Repository Overview",
            "## 📁 Directory Structure",
            "## 🔑 Key Files",
            "## 📄 File Contents",
        ]
        
        for section in expected_sections:
            assert section in context_pack, f"Missing section: {section}"
        
        # Verify README is mentioned
        assert "README.md" in context_pack
        assert "main.py" in context_pack

    def test_report_json_matches_expected_schema(self, minimal_repo):
        """Test that report.json matches expected schema."""
        with tempfile.TemporaryDirectory() as outdir:
            output_dir = Path(outdir)
            files, stats = scan_repository(minimal_repo)
            scanned_paths = {f.relative_path for f in files}
            ranker = FileRanker(minimal_repo, scanned_files=scanned_paths)
            files = ranker.rank_files(files)
            
            all_chunks = []
            for f in files:
                chunks = chunk_file(f, max_tokens=500)
                all_chunks.extend(chunks)
            
            stats.chunks_created = len(all_chunks)
            stats.processing_time_seconds = 1.0
            
            context_pack = render_context_pack(
                minimal_repo, files, all_chunks, ranker, stats,
                include_timestamp=False,
            )
            
            write_outputs(
                output_dir=output_dir,
                mode=OutputMode.BOTH,
                context_pack=context_pack,
                chunks=all_chunks,
                stats=stats,
                config={"mode": "both"},
                include_timestamp=False,
                files=files,
            )
            
            with open(output_dir / "report.json") as f:
                report = json.load(f)
        
        # Verify schema version
        assert "schema_version" in report
        assert report["schema_version"] == "1.0.0"
        
        # Verify required top-level keys
        required_keys = ["schema_version", "stats", "config", "output_files", "files"]
        for key in required_keys:
            assert key in report, f"Missing required key: {key}"
        
        # Verify stats structure
        stats_keys = [
            "files_scanned", "files_included", "chunks_created",
            "total_bytes_included", "files_skipped",
        ]
        for key in stats_keys:
            assert key in report["stats"], f"Missing stats key: {key}"
        
        # Verify files have IDs
        for file_entry in report["files"]:
            assert "id" in file_entry
            assert "path" in file_entry
            assert len(file_entry["id"]) == 16

    def test_jsonl_lines_are_valid_json(self, minimal_repo):
        """Test that each line in JSONL is valid JSON."""
        files, _ = scan_repository(minimal_repo)
        scanned_paths = {f.relative_path for f in files}
        ranker = FileRanker(minimal_repo, scanned_files=scanned_paths)
        files = ranker.rank_files(files)
        
        all_chunks = []
        for f in files:
            chunks = chunk_file(f, max_tokens=500)
            all_chunks.extend(chunks)
        
        jsonl_output = render_jsonl(all_chunks)
        
        for line in jsonl_output.strip().split("\n"):
            if line:
                # Should not raise
                chunk_data = json.loads(line)
                
                # Verify chunk structure
                assert "id" in chunk_data
                assert "path" in chunk_data
                assert "content" in chunk_data
                assert "start_line" in chunk_data
                assert "end_line" in chunk_data


class TestOutputHashStability:
    """Tests for output hash stability (byte-identical output)."""

    def test_output_hash_is_stable(self, minimal_repo):
        """Test that deterministic output files have stable hashes."""
        def generate_and_hash():
            with tempfile.TemporaryDirectory() as outdir:
                output_dir = Path(outdir)
                files, stats = scan_repository(minimal_repo)
                scanned_paths = {f.relative_path for f in files}
                ranker = FileRanker(minimal_repo, scanned_files=scanned_paths)
                files = ranker.rank_files(files)
                
                all_chunks = []
                for f in files:
                    chunks = chunk_file(f, max_tokens=500)
                    all_chunks.extend(chunks)
                
                stats.chunks_created = len(all_chunks)
                stats.processing_time_seconds = 1.0
                
                context_pack = render_context_pack(
                    minimal_repo, files, all_chunks, ranker, stats,
                    include_timestamp=False,
                )
                
                write_outputs(
                    output_dir=output_dir,
                    mode=OutputMode.BOTH,
                    context_pack=context_pack,
                    chunks=all_chunks,
                    stats=stats,
                    config={"mode": "both"},
                    include_timestamp=False,
                    files=files,
                )
                
                # Hash only deterministic output files (not report.json which has paths)
                hashes = {}
                for name in ["context_pack.md", "chunks.jsonl"]:
                    output_file = output_dir / name
                    if output_file.exists():
                        content = output_file.read_bytes()
                        hashes[name] = hashlib.sha256(content).hexdigest()
                
                return hashes
        
        hashes1 = generate_and_hash()
        hashes2 = generate_and_hash()
        
        assert hashes1 == hashes2
