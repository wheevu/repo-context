"""
Repo-to-Prompt: Convert repositories into LLM-friendly context packs.

This tool produces high-signal text bundles suitable for LLM prompting and RAG:
- A compact "repo context pack" (Markdown) for direct prompting
- Optional JSONL chunk files for embedding/vector search
"""

__version__ = "0.1.0"
__all__ = ["__version__"]
