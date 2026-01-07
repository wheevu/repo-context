"""Command-line interface for repo-to-prompt.

Provides commands for converting repositories into LLM-friendly context packs
suitable for prompting and RAG (Retrieval-Augmented Generation) workflows.

Commands:
    export  Convert a repository to context pack format
    info    Display repository analysis without exporting

Configuration:
    Supports config files: repo-to-prompt.toml, .r2p.yml, etc.
    CLI flags override config file values.
"""

from __future__ import annotations

import sys
import time
from pathlib import Path

import typer
from rich.console import Console
from rich.progress import (
    BarColumn,
    MofNCompleteColumn,
    Progress,
    SpinnerColumn,
    TextColumn,
    TimeElapsedColumn,
)

from . import __version__
from .chunker import chunk_file, coalesce_small_chunks
from .config import OutputMode
from .config_loader import load_config, merge_cli_with_config
from .fetcher import FetchError, RepoContext
from .ranker import FileRanker
from .redactor import RedactionConfig, create_redactor
from .renderer import render_context_pack, write_outputs
from .scanner import FileScanner, scan_repository

# Initialize CLI app
app = typer.Typer(
    name="repo-to-prompt",
    help="""Convert repositories into LLM-friendly context packs.

Generates Markdown context packs for prompting and JSONL chunks for RAG workflows.
Supports local directories and GitHub repositories.

Examples:
    repo-to-prompt export --path ./my-project
    repo-to-prompt export --repo https://github.com/owner/repo
    repo-to-prompt info ./my-project
""",
    add_completion=False,
    no_args_is_help=True,
)

console = Console()


def create_progress() -> Progress:
    """Create a Rich progress bar used for file scanning/chunking phases.

    Returns:
        A configured `rich.progress.Progress` instance.
    """
    return Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        BarColumn(bar_width=30),
        MofNCompleteColumn(),
        TimeElapsedColumn(),
        console=console,
        transient=True,
    )


def create_spinner_progress() -> Progress:
    """Create a simple spinner progress UI for indeterminate tasks.

    Returns:
        A configured `rich.progress.Progress` instance.
    """
    return Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        console=console,
        transient=True,
    )


def get_repo_output_dir(base_output_dir: Path, repo_root: Path) -> Path:
    """Return an output directory namespaced by repository name.

    If the provided base directory already ends with the repo name, it is returned
    unchanged to avoid double-nesting.

    Args:
        base_output_dir: Base output directory (may or may not already include repo name).
        repo_root: Repository root path.

    Returns:
        Output directory path to use for this repository.
    """
    repo_dir_name = repo_root.name
    if base_output_dir.name == repo_dir_name:
        return base_output_dir
    return base_output_dir / repo_dir_name


def version_callback(value: bool) -> None:
    """Typer callback to print version and exit.

    Args:
        value: Whether the `--version` flag was provided.

    Raises:
        typer.Exit: Always exits when `value` is True.
    """
    if value:
        console.print(f"repo-to-prompt version {__version__}")
        raise typer.Exit()


def parse_extensions(value: str) -> set[str]:
    """Parse a comma-separated extension list into a normalized set.

    Args:
        value: Comma-separated extension string (e.g., ".py,.ts" or "py,ts").

    Returns:
        Set of normalized extensions with leading dots.
    """
    if not value:
        return set()
    extensions = set()
    for ext in value.split(","):
        ext = ext.strip()
        if ext:
            if not ext.startswith("."):
                ext = f".{ext}"
            extensions.add(ext)
    return extensions


def parse_globs(value: str) -> set[str]:
    """Parse a comma-separated glob list into a normalized set.

    Args:
        value: Comma-separated glob patterns (e.g., "dist/**,build/**").

    Returns:
        Set of stripped glob patterns.
    """
    if not value:
        return set()
    return {g.strip() for g in value.split(",") if g.strip()}


@app.command()
def export(
    # === Input Source (mutually exclusive) ===
    path: Path | None = typer.Option(
        None,
        "--path",
        "-p",
        help="Local directory path to export.",
        exists=True,
        file_okay=False,
        dir_okay=True,
        resolve_path=True,
    ),
    repo: str | None = typer.Option(
        None,
        "--repo",
        "-r",
        help="GitHub repository URL to clone and export.",
    ),
    ref: str | None = typer.Option(
        None,
        "--ref",
        help="Git ref (branch/tag/SHA) when using --repo.",
    ),
    # === Configuration File ===
    config_file: Path | None = typer.Option(
        None,
        "--config",
        "-c",
        help="Path to config file (repo-to-prompt.toml or .r2p.yml).",
        exists=True,
        file_okay=True,
        dir_okay=False,
    ),
    # === File Filtering ===
    include_ext: str | None = typer.Option(
        None,
        "--include-ext",
        "-i",
        help="Include only these extensions (comma-separated, e.g., '.py,.ts').",
    ),
    exclude_glob: str | None = typer.Option(
        None,
        "--exclude-glob",
        "-e",
        help="Exclude paths matching these globs (comma-separated, e.g., 'dist/**').",
    ),
    max_file_bytes: int | None = typer.Option(
        None,
        "--max-file-bytes",
        help="Skip files larger than this (bytes). [default: 1MB]",
    ),
    max_total_bytes: int | None = typer.Option(
        None,
        "--max-total-bytes",
        help="Stop after exporting this many bytes total. [default: 20MB]",
    ),
    no_gitignore: bool = typer.Option(
        False,
        "--no-gitignore",
        help="Ignore .gitignore rules (include all matching files).",
    ),
    follow_symlinks: bool = typer.Option(
        False,
        "--follow-symlinks",
        help="Follow symbolic links when scanning. [default: skip symlinks]",
    ),
    include_minified: bool = typer.Option(
        False,
        "--include-minified",
        help="Include minified/bundled files. [default: skip]",
    ),
    # === Token Budget ===
    max_tokens: int | None = typer.Option(
        None,
        "--max-tokens",
        "-t",
        help="Maximum tokens in output. Packs best content under this budget.",
    ),
    # === Chunking Options ===
    chunk_tokens: int | None = typer.Option(
        None,
        "--chunk-tokens",
        help="Target tokens per chunk. [default: 800]",
    ),
    chunk_overlap: int | None = typer.Option(
        None,
        "--chunk-overlap",
        help="Overlap tokens between adjacent chunks. [default: 120]",
    ),
    min_chunk_tokens: int | None = typer.Option(
        None,
        "--min-chunk-tokens",
        help="Coalesce chunks smaller than this. Set 0 to disable. [default: 200]",
    ),
    # === Output Options ===
    mode: OutputMode | None = typer.Option(
        None,
        "--mode",
        "-m",
        help="Output format: 'prompt' (Markdown), 'rag' (JSONL), or 'both'. [default: both]",
    ),
    output_dir: Path | None = typer.Option(
        None,
        "--output-dir",
        "-o",
        help="Directory for output files. [default: ./out]",
    ),
    no_timestamp: bool = typer.Option(
        False,
        "--no-timestamp",
        help="Omit timestamps from output for reproducible diffs.",
    ),
    # === Display Options ===
    tree_depth: int | None = typer.Option(
        None,
        "--tree-depth",
        help="Max depth for directory tree in output. [default: 4]",
    ),
    # === Security Options ===
    no_redact: bool = typer.Option(
        False,
        "--no-redact",
        help="Disable automatic secret/credential redaction.",
    ),
    # === Misc ===
    version: bool = typer.Option(
        False,
        "--version",
        "-v",
        callback=version_callback,
        is_eager=True,
        help="Show version and exit.",
    ),
) -> None:
    """Export a repository as an LLM-friendly context pack.

    Scans a local directory or GitHub repository, chunks the code files,
    and generates output suitable for LLM prompting or RAG indexing.

    \b
    CONFIGURATION:
      Supports config files: repo-to-prompt.toml, .r2p.yml
      CLI flags always override config file values.

    \b
    OUTPUT FILES:
      context_pack.md  - Markdown file with full repository context
      chunks.jsonl     - JSONL file with chunked content for embeddings
      report.json      - Processing statistics and configuration (versioned schema)

    \b
    EXAMPLES:
      # Export a local project
      repo-to-prompt export -p ./my-project

      # Export from GitHub
      repo-to-prompt export -r https://github.com/owner/repo

      # Export specific branch, Python files only
      repo-to-prompt export -r https://github.com/owner/repo --ref main -i .py

      # RAG mode only (for embedding pipelines)
      repo-to-prompt export -p ./repo --mode rag

      # Limit output to 50K tokens (best content under budget)
      repo-to-prompt export -p ./repo --max-tokens 50000

      # Use custom config file
      repo-to-prompt export -p ./repo --config ./r2p.toml

      # Reproducible output (no timestamps)
      repo-to-prompt export -p ./repo --no-timestamp

    Args:
        path: Local directory path to export (mutually exclusive with `repo`).
        repo: GitHub repository URL to clone and export (mutually exclusive with `path`).
        ref: Optional git ref (branch/tag/SHA) when using `repo`.
        config_file: Optional explicit path to a config file.
        include_ext: Optional comma-separated list of extensions to include.
        exclude_glob: Optional comma-separated list of glob patterns to exclude.
        max_file_bytes: Optional max file size (bytes).
        max_total_bytes: Optional max total included bytes across all files.
        no_gitignore: Whether to ignore `.gitignore` rules.
        follow_symlinks: Whether to follow symlinks during scanning.
        include_minified: Whether to include minified/bundled files.
        max_tokens: Optional maximum token budget for the output.
        chunk_tokens: Optional target tokens per chunk.
        chunk_overlap: Optional token overlap between adjacent chunks.
        min_chunk_tokens: Optional threshold below which to coalesce chunks.
        mode: Optional output mode override (`prompt`, `rag`, `both`).
        output_dir: Optional output directory.
        no_timestamp: Whether to omit timestamps for reproducible output.
        tree_depth: Optional maximum directory tree depth in rendered output.
        no_redact: Whether to disable secret redaction.
        version: Whether to print version and exit (handled by Typer callback).

    Returns:
        None. Output is written to files on disk.

    Raises:
        typer.Exit: On invalid arguments, empty scan results, fetch failures, or any
            unexpected processing errors.
    """
    # NOTE: Signature intentionally mirrors CLI flags. Keep defaults/types stable to avoid
    # breaking Typer's option parsing and downstream keyword callers.

    # Validate input
    if path is None and repo is None:
        console.print("[red]Error: Either --path or --repo must be specified.[/red]")
        raise typer.Exit(1)

    if path is not None and repo is not None:
        console.print("[red]Error: Cannot specify both --path and --repo.[/red]")
        raise typer.Exit(1)

    start_time = time.time()

    try:
        # Fetch repository and keep it alive for the entire export.
        # NOTE: For --repo, RepoContext clones into a temporary directory and cleans it up
        # on exit. The full export pipeline must run inside this context.
        with create_spinner_progress() as progress:
            task = None
            if repo:
                task = progress.add_task("Fetching repository...", total=None)

            with RepoContext(path=path, repo_url=repo, ref=ref) as repo_path:
                if task is not None:
                    progress.update(task, description="[green]✓[/green] Repository fetched")

                # Load config file (searches repo root for config if not specified)
                project_config = load_config(repo_path, config_file)
                if project_config._config_file:
                    console.print(f"[dim]Using config: {project_config._config_file.name}[/dim]")

                # Merge CLI args with config (CLI takes precedence)
                merged = merge_cli_with_config(
                    project_config,
                    include_ext=include_ext,
                    exclude_glob=exclude_glob,
                    max_file_bytes=max_file_bytes,
                    max_total_bytes=max_total_bytes,
                    max_tokens=max_tokens,
                    follow_symlinks=follow_symlinks if follow_symlinks else None,
                    include_minified=include_minified if include_minified else None,
                    chunk_tokens=chunk_tokens,
                    chunk_overlap=chunk_overlap,
                    min_chunk_tokens=min_chunk_tokens,
                    output_dir=output_dir,
                    mode=mode.value if mode else None,
                    no_gitignore=no_gitignore,
                    no_redact=no_redact,
                    tree_depth=tree_depth,
                )

                # Extract merged values
                m_include_extensions = merged["include_extensions"]
                m_exclude_globs = merged["exclude_globs"]
                m_max_file_bytes = merged["max_file_bytes"]
                m_max_total_bytes = merged["max_total_bytes"]
                m_max_tokens = merged["max_tokens"]
                m_follow_symlinks = merged["follow_symlinks"]
                m_skip_minified = merged["skip_minified"]
                m_chunk_tokens = merged["chunk_tokens"]
                m_chunk_overlap = merged["chunk_overlap"]
                m_min_chunk_tokens = merged["min_chunk_tokens"]
                m_output_dir = merged["output_dir"]
                m_mode = OutputMode(merged["mode"])
                m_respect_gitignore = merged["respect_gitignore"]
                m_redact_secrets = merged["redact_secrets"]
                _ = merged["tree_depth"]  # Used in merged config, not needed here
                m_ranking_weights = merged["ranking_weights"]
                m_redaction_config = merged.get("redaction_config", {})

                # Phase 1: Scan repository (with progress bar)
                console.print("[cyan]Scanning repository...[/cyan]")

                scanner = FileScanner(
                    root_path=repo_path,
                    include_extensions=m_include_extensions,
                    exclude_globs=m_exclude_globs,
                    max_file_bytes=m_max_file_bytes,
                    respect_gitignore=m_respect_gitignore,
                    follow_symlinks=m_follow_symlinks,
                    skip_minified=m_skip_minified,
                )

                files = list(scanner.scan())
                stats = scanner.stats

                if not files:
                    console.print("[yellow]Warning: No files found matching criteria.[/yellow]")
                    raise typer.Exit(0)

                console.print(
                    f"[green]✓[/green] Found {len(files)} files ({stats.files_scanned} scanned)"
                )

                # Phase 2: Rank files (with progress)
                with create_spinner_progress() as progress:
                    task = progress.add_task("Ranking files by importance...", total=None)
                    scanned_paths = {f.relative_path for f in files}
                    ranker = FileRanker(
                        repo_path,
                        scanned_files=scanned_paths,
                        weights=m_ranking_weights.to_dict()
                        if hasattr(m_ranking_weights, "to_dict")
                        else None,
                    )
                    files = ranker.rank_files(files)
                    progress.update(task, description="[green]✓[/green] Files ranked")

                # Create redactor with advanced config
                redaction_config = (
                    RedactionConfig.from_dict(m_redaction_config) if m_redaction_config else None
                )
                redactor = create_redactor(enabled=m_redact_secrets, config=redaction_config)

                # Phase 3: Chunk files (with detailed progress bar)
                console.print("[cyan]Chunking content...[/cyan]")
                all_chunks = []
                total_bytes = 0
                total_tokens = 0
                files_processed = []
                dropped_files = []

                with create_progress() as progress:
                    chunk_task = progress.add_task(
                        "Processing files",
                        total=len(files),
                    )

                    for idx, file_info in enumerate(files):
                        progress.update(
                            chunk_task,
                            completed=idx,
                            description=f"Chunking {file_info.relative_path[:40]}...",
                        )

                        # Check total bytes limit
                        if total_bytes >= m_max_total_bytes:
                            console.print(
                                f"[yellow]Reached max total bytes limit ({m_max_total_bytes:,})[/yellow]"
                            )
                            for remaining in files[idx:]:
                                dropped_files.append(
                                    {
                                        "path": remaining.relative_path,
                                        "reason": "bytes_limit",
                                        "priority": round(remaining.priority, 3),
                                    }
                                )
                            stats.files_dropped_budget = len(dropped_files)
                            break

                        try:
                            redactor.set_current_file(file_info.path)

                            file_chunks = chunk_file(
                                file_info=file_info,
                                max_tokens=m_chunk_tokens,
                                overlap_tokens=m_chunk_overlap,
                                redactor=redactor,
                            )

                            file_tokens = sum(c.token_estimate for c in file_chunks)
                            file_info.token_estimate = file_tokens

                            if m_max_tokens and total_tokens + file_tokens > m_max_tokens:
                                dropped_files.append(
                                    {
                                        "path": file_info.relative_path,
                                        "reason": "token_budget",
                                        "priority": round(file_info.priority, 3),
                                        "tokens": file_tokens,
                                    }
                                )
                                continue

                            all_chunks.extend(file_chunks)
                            total_bytes += file_info.size_bytes
                            total_tokens += file_tokens
                            files_processed.append(file_info)
                        except Exception as e:
                            console.print(
                                f"[yellow]Warning: Failed to chunk {file_info.relative_path}: {e}[/yellow]"
                            )

                    progress.update(
                        chunk_task,
                        completed=len(files),
                        description="[green]✓[/green] Chunking complete",
                    )

                console.print(
                    f"[green]✓[/green] Created {len(all_chunks)} chunks from {len(files_processed)} files"
                )

                stats.total_tokens_estimated = total_tokens
                stats.files_dropped_budget = len(dropped_files)
                stats.dropped_files = dropped_files
                stats.top_ranked_files = [
                    {"path": f.relative_path, "priority": round(f.priority, 3)}
                    for f in files_processed[:20]
                ]

                if m_min_chunk_tokens > 0:
                    chunks_before = len(all_chunks)
                    all_chunks = coalesce_small_chunks(
                        all_chunks,
                        min_tokens=m_min_chunk_tokens,
                        max_tokens=m_chunk_tokens,
                    )
                    if len(all_chunks) < chunks_before:
                        console.print(
                            f"[dim]Coalesced {chunks_before} → {len(all_chunks)} chunks[/dim]"
                        )

                stats.chunks_created = len(all_chunks)
                stats.files_included = len(files_processed)

                # Phase 4: Render output (with spinner)
                with create_spinner_progress() as progress:
                    render_task = progress.add_task("Rendering output...", total=None)
                    context_pack = render_context_pack(
                        root_path=repo_path,
                        files=files_processed,
                        chunks=all_chunks,
                        ranker=ranker,
                        stats=stats,
                        include_timestamp=not no_timestamp,
                    )
                    progress.update(render_task, description="[green]✓[/green] Output rendered")

                # Prepare config for report (sorted keys for determinism)
                config_dict = {
                    "chunk_overlap": m_chunk_overlap,
                    "chunk_tokens": m_chunk_tokens,
                    "exclude_globs": sorted(m_exclude_globs) if m_exclude_globs else None,
                    "follow_symlinks": m_follow_symlinks,
                    "include_extensions": sorted(m_include_extensions)
                    if m_include_extensions
                    else None,
                    "max_file_bytes": m_max_file_bytes,
                    "max_tokens": m_max_tokens,
                    "max_total_bytes": m_max_total_bytes,
                    "mode": m_mode.value,
                    "path": str(path) if path else None,
                    "redact_secrets": m_redact_secrets,
                    "ref": ref,
                    "repo": repo,
                    "skip_minified": m_skip_minified,
                }

                elapsed = time.time() - start_time
                stats.processing_time_seconds = elapsed
                stats.redaction_counts = redactor.get_stats()

                # Write outputs
                repo_output_dir = get_repo_output_dir(m_output_dir, repo_path)
                with create_spinner_progress() as progress:
                    write_task = progress.add_task("Writing output files...", total=None)
                    output_files = write_outputs(
                        output_dir=repo_output_dir,
                        mode=m_mode,
                        context_pack=context_pack,
                        chunks=all_chunks,
                        stats=stats,
                        config=config_dict,
                        include_timestamp=not no_timestamp,
                        files=files_processed,
                    )
                    progress.update(write_task, description="[green]✓[/green] Output written")

                # Print summary
                console.print()
                console.print("[bold green]✓ Export complete![/bold green]")
                console.print()
                console.print("[cyan]Statistics:[/cyan]")
                console.print(f"  Files scanned: {stats.files_scanned}")
                console.print(f"  Files included: {stats.files_included}")
                if stats.files_dropped_budget > 0:
                    console.print(f"  Files dropped (budget): {stats.files_dropped_budget}")
                console.print(f"  Chunks created: {stats.chunks_created}")
                console.print(f"  Total bytes: {stats.total_bytes_included:,}")
                console.print(f"  Total tokens: ~{stats.total_tokens_estimated:,}")
                console.print(f"  Processing time: {stats.processing_time_seconds:.2f}s")
                console.print()
                console.print("[cyan]Output files:[/cyan]")
                for f in output_files:
                    console.print(f"  {f}")

                if redactor.get_stats():
                    console.print()
                    console.print("[cyan]Redactions applied:[/cyan]")
                    for name, count in list(redactor.get_stats().items())[:5]:
                        console.print(f"  {name}: {count}")

                if dropped_files:
                    console.print()
                    console.print(
                        f"[yellow]Dropped {len(dropped_files)} files due to budget constraints:[/yellow]"
                    )
                    for df in dropped_files[:5]:
                        console.print(f"  {df['path']} ({df['reason']})")
                    if len(dropped_files) > 5:
                        console.print(f"  ... and {len(dropped_files) - 5} more (see report.json)")

    except FetchError as e:
        console.print(f"[red]Error fetching repository: {e}[/red]")
        raise typer.Exit(1) from None
    except Exception as e:
        console.print(f"[red]Error: {e}[/red]")
        if "--verbose" in sys.argv:
            import traceback

            console.print(traceback.format_exc())
        raise typer.Exit(1) from None


@app.command()
def info(
    path: Path = typer.Argument(
        ...,
        help="Local directory path to analyze.",
        exists=True,
        file_okay=False,
        dir_okay=True,
        resolve_path=True,
    ),
    include_ext: str | None = typer.Option(
        None,
        "--include-ext",
        "-i",
        help="Include only these extensions (comma-separated).",
    ),
    exclude_glob: str | None = typer.Option(
        None,
        "--exclude-glob",
        "-e",
        help="Exclude paths matching these globs (comma-separated).",
    ),
    max_file_bytes: int = typer.Option(
        1_048_576,
        "--max-file-bytes",
        help="Skip files larger than this (bytes). [default: 1MB]",
    ),
    no_gitignore: bool = typer.Option(
        False,
        "--no-gitignore",
        help="Ignore .gitignore rules.",
    ),
) -> None:
    """Analyze a repository and display summary statistics.

    Shows detected languages, entrypoints, top files by priority,
    and scanning statistics. Useful for previewing what 'export' will process.

    \b
    EXAMPLES:
      repo-to-prompt info ./my-project
      repo-to-prompt info ./my-project --include-ext .py,.ts

    Args:
        path: Local directory path to analyze.
        include_ext: Optional comma-separated list of extensions to include.
        exclude_glob: Optional comma-separated list of glob patterns to exclude.
        max_file_bytes: Maximum file size to include (bytes).
        no_gitignore: Whether to ignore `.gitignore` rules.

    Returns:
        None. Prints summary information to stdout.

    Raises:
        typer.Exit: On scanning/ranking failures.
    """
    # Parse filter options (same as export)
    include_extensions = parse_extensions(include_ext) if include_ext else None
    exclude_globs = parse_globs(exclude_glob) if exclude_glob else None

    try:
        # Scan repository with same options as export
        files, stats = scan_repository(
            root_path=path,
            include_extensions=include_extensions,
            exclude_globs=exclude_globs,
            max_file_bytes=max_file_bytes,
            respect_gitignore=not no_gitignore,
        )

        # Rank files - pass scanned file paths to validate entrypoints
        scanned_paths = {f.relative_path for f in files}
        ranker = FileRanker(path, scanned_files=scanned_paths)
        files = ranker.rank_files(files)

        # Print info
        console.print(f"\n[bold]Repository: {path.name}[/bold]\n")

        # Languages (sorted for determinism)
        console.print("[cyan]Languages detected:[/cyan]")
        for lang, count in sorted(stats.languages_detected.items(), key=lambda x: (-x[1], x[0])):
            console.print(f"  {lang}: {count} files")

        # Entrypoints (sorted for determinism)
        entrypoints = ranker.get_entrypoints()
        if entrypoints:
            console.print("\n[cyan]Entrypoints:[/cyan]")
            for ep in sorted(entrypoints):
                console.print(f"  {ep}")

        # Top files
        console.print("\n[cyan]Top priority files:[/cyan]")
        for f in files[:10]:
            console.print(f"  {f.relative_path} ({f.priority:.0%})")

        # Stats
        console.print("\n[cyan]Statistics:[/cyan]")
        console.print(f"  Total files scanned: {stats.files_scanned}")
        console.print(f"  Files included: {stats.files_included}")
        console.print(f"  Files skipped (size): {stats.files_skipped_size}")
        console.print(f"  Files skipped (binary): {stats.files_skipped_binary}")
        console.print(f"  Files skipped (extension): {stats.files_skipped_extension}")
        console.print(f"  Files skipped (gitignore): {stats.files_skipped_gitignore}")
        console.print(f"  Total bytes: {stats.total_bytes_included:,}")

    except Exception as e:
        console.print(f"[red]Error: {e}[/red]")
        raise typer.Exit(1) from None


def main() -> None:
    """CLI entry point used by console scripts.

    Returns:
        None.
    """
    app()


if __name__ == "__main__":
    main()
