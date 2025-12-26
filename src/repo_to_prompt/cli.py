"""
CLI entry point for repo-to-prompt.

Provides a command-line interface for converting repositories to LLM-friendly context packs.
"""

from __future__ import annotations

import sys
import time
from pathlib import Path
from typing import Optional

import typer
from rich.console import Console
from rich.progress import Progress, SpinnerColumn, TextColumn

from . import __version__
from .chunker import chunk_file, coalesce_small_chunks
from .config import Config, OutputMode
from .fetcher import FetchError, RepoContext
from .ranker import FileRanker
from .redactor import create_redactor
from .renderer import render_context_pack, write_outputs
from .scanner import scan_repository

# Initialize CLI app
app = typer.Typer(
    name="repo-to-prompt",
    help="Convert repositories into LLM-friendly context packs for prompting and RAG.",
    add_completion=False,
)

console = Console()


def version_callback(value: bool) -> None:
    """Print version and exit."""
    if value:
        console.print(f"repo-to-prompt version {__version__}")
        raise typer.Exit()


def parse_extensions(value: str) -> set[str]:
    """Parse comma-separated extensions."""
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
    """Parse comma-separated glob patterns."""
    if not value:
        return set()
    return {g.strip() for g in value.split(",") if g.strip()}


@app.command()
def export(
    # Input options
    path: Optional[Path] = typer.Option(
        None,
        "--path", "-p",
        help="Local path to the repository.",
        exists=True,
        file_okay=False,
        dir_okay=True,
        resolve_path=True,
    ),
    repo: Optional[str] = typer.Option(
        None,
        "--repo", "-r",
        help="GitHub repository URL (e.g., https://github.com/owner/name).",
    ),
    ref: Optional[str] = typer.Option(
        None,
        "--ref",
        help="Git ref (branch, tag, or commit SHA) for GitHub repos.",
    ),
    
    # Filter options
    include_ext: Optional[str] = typer.Option(
        None,
        "--include-ext", "-i",
        help="Comma-separated file extensions to include (e.g., '.py,.ts,.md').",
    ),
    exclude_glob: Optional[str] = typer.Option(
        None,
        "--exclude-glob", "-e",
        help="Comma-separated glob patterns to exclude (e.g., 'dist/**,build/**').",
    ),
    max_file_bytes: int = typer.Option(
        1_048_576,
        "--max-file-bytes",
        help="Maximum size in bytes for individual files (default: 1 MB).",
    ),
    max_total_bytes: int = typer.Option(
        20_000_000,
        "--max-total-bytes",
        help="Maximum total bytes to export (default: 20 MB).",
    ),
    no_gitignore: bool = typer.Option(
        False,
        "--no-gitignore",
        help="Don't respect .gitignore files.",
    ),
    
    # Chunking options
    chunk_tokens: int = typer.Option(
        800,
        "--chunk-tokens",
        help="Target tokens per chunk (approximate).",
    ),
    chunk_overlap: int = typer.Option(
        120,
        "--chunk-overlap",
        help="Token overlap between chunks.",
    ),
    min_chunk_tokens: int = typer.Option(
        200,
        "--min-chunk-tokens",
        help="Minimum tokens per chunk. Smaller chunks are coalesced. Set to 0 to disable.",
    ),
    
    # Output options
    mode: OutputMode = typer.Option(
        OutputMode.BOTH,
        "--mode", "-m",
        help="Output mode: 'prompt' (markdown only), 'rag' (JSONL only), or 'both'.",
    ),
    output_dir: Path = typer.Option(
        Path("./out"),
        "--output-dir", "-o",
        help="Output directory for generated files.",
    ),
    
    # Tree options
    tree_depth: int = typer.Option(
        4,
        "--tree-depth",
        help="Maximum depth for directory tree display.",
    ),
    
    # Redaction options
    no_redact: bool = typer.Option(
        False,
        "--no-redact",
        help="Disable secret redaction.",
    ),
    
    # Version
    version: bool = typer.Option(
        False,
        "--version", "-v",
        callback=version_callback,
        is_eager=True,
        help="Show version and exit.",
    ),
) -> None:
    """
    Export a repository as an LLM-friendly context pack.
    
    Examples:
    
        # Export a local repository
        repo-to-prompt export --path /path/to/repo
        
        # Export from GitHub
        repo-to-prompt export --repo https://github.com/owner/repo
        
        # Export specific branch with custom output
        repo-to-prompt export --repo https://github.com/owner/repo --ref develop -o ./output
        
        # Export only Python and Markdown files
        repo-to-prompt export -p ./repo --include-ext ".py,.md"
        
        # RAG mode only (JSONL chunks)
        repo-to-prompt export -p ./repo --mode rag
    """
    # Validate input
    if path is None and repo is None:
        console.print("[red]Error: Either --path or --repo must be specified.[/red]")
        raise typer.Exit(1)
    
    if path is not None and repo is not None:
        console.print("[red]Error: Cannot specify both --path and --repo.[/red]")
        raise typer.Exit(1)
    
    start_time = time.time()
    
    # Parse filter options
    include_extensions = parse_extensions(include_ext) if include_ext else None
    exclude_globs = parse_globs(exclude_glob) if exclude_glob else None
    
    try:
        # Fetch repository
        with Progress(
            SpinnerColumn(),
            TextColumn("[progress.description]{task.description}"),
            console=console,
        ) as progress:
            
            if repo:
                task = progress.add_task("Fetching repository...", total=None)
            
            with RepoContext(path=path, repo_url=repo, ref=ref) as repo_path:
                if repo:
                    progress.update(task, description="Repository fetched ✓")
                
                # Scan repository
                progress.add_task("Scanning files...", total=None)
                
                files, stats = scan_repository(
                    root_path=repo_path,
                    include_extensions=include_extensions,
                    exclude_globs=exclude_globs,
                    max_file_bytes=max_file_bytes,
                    respect_gitignore=not no_gitignore,
                )
                
                if not files:
                    console.print("[yellow]Warning: No files found matching criteria.[/yellow]")
                    raise typer.Exit(0)
                
                console.print(f"[green]Found {len(files)} files to process[/green]")
                
                # Rank files - pass scanned file paths to validate entrypoints
                progress.add_task("Ranking files...", total=None)
                scanned_paths = {f.relative_path for f in files}
                ranker = FileRanker(repo_path, scanned_files=scanned_paths)
                files = ranker.rank_files(files)
                
                # Create redactor
                redactor = create_redactor(enabled=not no_redact)
                
                # Chunk files
                progress.add_task("Chunking content...", total=None)
                all_chunks = []
                total_bytes = 0
                
                for file_info in files:
                    # Check total bytes limit
                    if total_bytes >= max_total_bytes:
                        console.print(
                            f"[yellow]Reached max total bytes limit ({max_total_bytes:,})[/yellow]"
                        )
                        break
                    
                    try:
                        file_chunks = chunk_file(
                            file_info=file_info,
                            max_tokens=chunk_tokens,
                            overlap_tokens=chunk_overlap,
                            redactor=redactor,
                        )
                        all_chunks.extend(file_chunks)
                        total_bytes += file_info.size_bytes
                    except Exception as e:
                        console.print(
                            f"[yellow]Warning: Failed to chunk {file_info.relative_path}: {e}[/yellow]"
                        )
                
                # Coalesce small chunks to reduce chunk explosion
                if min_chunk_tokens > 0:
                    chunks_before = len(all_chunks)
                    all_chunks = coalesce_small_chunks(
                        all_chunks,
                        min_tokens=min_chunk_tokens,
                        max_tokens=chunk_tokens,
                    )
                    if len(all_chunks) < chunks_before:
                        console.print(
                            f"[dim]Coalesced {chunks_before} → {len(all_chunks)} chunks[/dim]"
                        )
                
                stats.chunks_created = len(all_chunks)
                
                # Render context pack
                progress.add_task("Rendering output...", total=None)
                context_pack = render_context_pack(
                    root_path=repo_path,
                    files=files,
                    chunks=all_chunks,
                    ranker=ranker,
                    stats=stats,
                )
                
                # Prepare config for report
                config_dict = {
                    "path": str(path) if path else None,
                    "repo": repo,
                    "ref": ref,
                    "include_extensions": list(include_extensions) if include_extensions else None,
                    "exclude_globs": list(exclude_globs) if exclude_globs else None,
                    "max_file_bytes": max_file_bytes,
                    "max_total_bytes": max_total_bytes,
                    "chunk_tokens": chunk_tokens,
                    "chunk_overlap": chunk_overlap,
                    "mode": mode.value,
                    "redact_secrets": not no_redact,
                }
                
                # Calculate timing BEFORE writing outputs
                elapsed = time.time() - start_time
                stats.processing_time_seconds = elapsed
                
                # Write outputs
                output_files = write_outputs(
                    output_dir=output_dir,
                    mode=mode,
                    context_pack=context_pack,
                    chunks=all_chunks,
                    stats=stats,
                    config=config_dict,
                )
        
        # Print summary (elapsed was already calculated before write_outputs)
        console.print()
        console.print("[bold green]✓ Export complete![/bold green]")
        console.print()
        console.print(f"[cyan]Statistics:[/cyan]")
        console.print(f"  Files scanned: {stats.files_scanned}")
        console.print(f"  Files included: {stats.files_included}")
        console.print(f"  Chunks created: {stats.chunks_created}")
        console.print(f"  Total bytes: {stats.total_bytes_included:,}")
        console.print(f"  Processing time: {stats.processing_time_seconds:.2f}s")
        console.print()
        console.print(f"[cyan]Output files:[/cyan]")
        for f in output_files:
            console.print(f"  {f}")
        
        if redactor.get_stats():
            console.print()
            console.print(f"[cyan]Redactions applied:[/cyan]")
            for name, count in list(redactor.get_stats().items())[:5]:
                console.print(f"  {name}: {count}")
        
    except FetchError as e:
        console.print(f"[red]Error fetching repository: {e}[/red]")
        raise typer.Exit(1)
    except Exception as e:
        console.print(f"[red]Error: {e}[/red]")
        if "--verbose" in sys.argv:
            import traceback
            console.print(traceback.format_exc())
        raise typer.Exit(1)


@app.command()
def info(
    path: Path = typer.Argument(
        ...,
        help="Path to the repository.",
        exists=True,
        file_okay=False,
        dir_okay=True,
        resolve_path=True,
    ),
    include_ext: Optional[str] = typer.Option(
        None,
        "--include-ext", "-i",
        help="Comma-separated file extensions to include (e.g., '.py,.ts,.md').",
    ),
    exclude_glob: Optional[str] = typer.Option(
        None,
        "--exclude-glob", "-e",
        help="Comma-separated glob patterns to exclude (e.g., 'dist/**,build/**').",
    ),
    max_file_bytes: int = typer.Option(
        1_048_576,
        "--max-file-bytes",
        help="Maximum size in bytes for individual files (default: 1 MB).",
    ),
    no_gitignore: bool = typer.Option(
        False,
        "--no-gitignore",
        help="Don't respect .gitignore files.",
    ),
) -> None:
    """
    Show information about a repository without exporting.
    
    Displays detected languages, entrypoints, and file statistics.
    Uses the same scanning logic as 'export' for consistent results.
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
        
        # Languages
        console.print("[cyan]Languages detected:[/cyan]")
        for lang, count in sorted(stats.languages_detected.items(), key=lambda x: -x[1]):
            console.print(f"  {lang}: {count} files")
        
        # Entrypoints
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
        raise typer.Exit(1)


def main() -> None:
    """Entry point for the CLI."""
    app()


if __name__ == "__main__":
    main()
