# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "docs2markdown>=0.1.0",
#     "rich>=13.0.0",
#     "typer>=0.15.0",
# ]
# ///
"""
Fetch and convert Iced GUI framework documentation to LLM-friendly markdown.

This script downloads:
1. The Iced Book (tutorial/guide content)
2. API documentation built with cargo doc
3. Optionally: examples from the GitHub repository

Usage:
    uv run fetch_iced_docs.py [--version 0.13.1] [--output-dir ./docs/iced] [--include-examples]
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path
from typing import Annotated

import typer
from docs2markdown import DocType
from docs2markdown import Format
from docs2markdown import convert_directory
from rich.console import Console

console = Console()
app = typer.Typer(
    help="Fetch and convert Iced documentation to LLM-friendly markdown",
    rich_markup_mode="rich",
)


def clone_iced_book(output_dir: Path) -> bool:
    """Clone and build the Iced book."""
    book_dir = output_dir / "book-src"
    book_output = output_dir / "book"

    console.print("[cyan]Cloning Iced book repository...[/cyan]")

    try:
        # Clone the book repo
        subprocess.run(
            [
                "git",
                "clone",
                "--depth=1",
                "https://github.com/iced-rs/book.git",
                str(book_dir),
            ],
            check=True,
            capture_output=True,
        )

        # The book is already in markdown format (mdBook)
        # Just copy the src/ directory
        src_dir = book_dir / "src"
        if src_dir.exists():
            shutil.copytree(src_dir, book_output, dirs_exist_ok=True)
            console.print(f"[green]✓ Book copied to {book_output}[/green]")

            # Clean up
            shutil.rmtree(book_dir)
            return True
        else:
            console.print("[red]Book source not found in expected location[/red]")
            return False

    except subprocess.CalledProcessError as e:
        console.print(f"[red]Failed to clone book: {e}[/red]")
        return False
    except Exception as e:
        console.print(f"[red]Error processing book: {e}[/red]")
        return False


def build_cargo_docs(
    crates: list[str], version: str, layershell_version: str, output_dir: Path
) -> bool:
    """Build documentation using cargo doc."""
    temp_dir = output_dir / "cargo-temp"
    temp_dir.mkdir(parents=True, exist_ok=True)

    console.print(
        f"[cyan]Building documentation with cargo for {', '.join(crates)}...[/cyan]"
    )

    try:
        # Create minimal Cargo.toml
        cargo_toml = temp_dir / "Cargo.toml"

        # Build dependencies section dynamically
        deps = [f'iced = "{version}"']
        if "iced_layershell" in crates:
            deps.append(f'iced_layershell = "{layershell_version}"')

        cargo_toml.write_text(
            f"""[package]
name = "iced-docs"
version = "0.1.0"
edition = "2021"

[dependencies]
{chr(10).join(deps)}
"""
        )

        # Create dummy src/lib.rs
        src_dir = temp_dir / "src"
        src_dir.mkdir(exist_ok=True)
        (src_dir / "lib.rs").write_text("")

        # Build docs
        console.print("[cyan]Running cargo doc (this may take a few minutes)...[/cyan]")
        result = subprocess.run(
            ["cargo", "doc", "--no-deps"] + [f"--package={crate}" for crate in crates],
            cwd=temp_dir,
            capture_output=True,
            text=True,
        )

        if result.returncode != 0:
            console.print(f"[red]Cargo doc failed: {result.stderr}[/red]")
            return False

        doc_dir = temp_dir / "target" / "doc"
        if not doc_dir.exists():
            console.print(
                "[red]Documentation directory not found after cargo doc[/red]"
            )
            return False

        console.print(f"[green]✓ Documentation built at {doc_dir}[/green]")
        return True

    except Exception as e:
        console.print(f"[red]Failed to build cargo docs: {e}[/red]")
        return False


def convert_cargo_docs(crate: str, output_dir: Path) -> bool:
    """Convert cargo-generated HTML documentation to markdown."""
    temp_dir = output_dir / "cargo-temp"
    doc_dir = temp_dir / "target" / "doc" / crate
    markdown_dir = output_dir / f"{crate}-api"

    if not doc_dir.exists():
        console.print(f"[yellow]Warning: No docs found for {crate}[/yellow]")
        return False

    console.print(f"[cyan]Converting {crate} documentation to markdown...[/cyan]")

    try:
        success_count = 0
        error_count = 0

        # Use convert_directory from docs2markdown - it handles everything
        for input_file, result in convert_directory(
            doc_dir,
            markdown_dir,
            doc_type=DocType.DEFAULT,
            format=Format.LLMSTXT,
        ):
            if isinstance(result, Exception):
                console.print(
                    f"[yellow]Warning: Failed to convert {input_file.name}: {result}[/yellow]"
                )
                error_count += 1
            else:
                success_count += 1

        if success_count == 0:
            console.print(f"[red]No files converted for {crate}[/red]")
            return False

        console.print(
            f"[green]✓ Converted {success_count} pages for {crate} to {markdown_dir}[/green]"
        )
        if error_count > 0:
            console.print(f"[yellow]  ({error_count} files failed)[/yellow]")

        return True

    except Exception as e:
        console.print(f"[red]Failed to convert {crate} docs: {e}[/red]")
        return False


def fetch_examples(version: str, output_dir: Path) -> bool:
    """Fetch example code from the Iced repository."""
    examples_dir = output_dir / "examples"
    repo_dir = output_dir / "iced-repo-temp"

    console.print("[cyan]Cloning Iced repository for examples...[/cyan]")

    try:
        # Try with version tag first, fall back to main if it doesn't exist
        tag = f"v{version}" if not version.startswith("v") else version

        # Clone just the examples directory
        result = subprocess.run(
            [
                "git",
                "clone",
                "--depth=1",
                "--filter=blob:none",
                "--sparse",
                f"--branch={tag}",
                "https://github.com/iced-rs/iced.git",
                str(repo_dir),
            ],
            capture_output=True,
            text=True,
        )

        # If tag doesn't exist, try main branch
        if result.returncode != 0:
            console.print(
                f"[yellow]Tag {tag} not found, trying main branch...[/yellow]"
            )
            subprocess.run(
                [
                    "git",
                    "clone",
                    "--depth=1",
                    "--filter=blob:none",
                    "--sparse",
                    "https://github.com/iced-rs/iced.git",
                    str(repo_dir),
                ],
                check=True,
                capture_output=True,
            )

        subprocess.run(
            ["git", "-C", str(repo_dir), "sparse-checkout", "set", "examples"],
            check=True,
            capture_output=True,
        )

        # Copy examples
        src_examples = repo_dir / "examples"
        if src_examples.exists():
            shutil.copytree(src_examples, examples_dir, dirs_exist_ok=True)
            console.print(f"[green]✓ Examples copied to {examples_dir}[/green]")

            # Clean up
            shutil.rmtree(repo_dir)
            return True
        else:
            console.print("[yellow]No examples found[/yellow]")
            shutil.rmtree(repo_dir, ignore_errors=True)
            return False

    except subprocess.CalledProcessError as e:
        console.print(f"[red]Failed to fetch examples: {e}[/red]")
        if repo_dir.exists():
            shutil.rmtree(repo_dir, ignore_errors=True)
        return False


def create_index(output_dir: Path, crates: list[str], has_examples: bool):
    """Create an index file to help navigate the documentation."""
    index_content = """# Iced Framework Documentation

This directory contains comprehensive documentation for the Iced GUI framework, converted to LLM-friendly markdown format.

## Contents

### 📚 Book
- `book/` - The official Iced book with tutorials and guides
  - Start here for conceptual understanding and examples

### 📖 API Documentation
"""

    for crate in crates:
        index_content += f"- `{crate}-api/` - API reference for the `{crate}` crate\n"

    if has_examples:
        index_content += """
### 💡 Examples
- `examples/` - Real-world example code from the Iced repository
  - See practical implementations of Iced patterns
"""

    index_content += """
## Using with LLMs

When prompting an LLM about Iced, you can:

1. Reference specific sections: "See book/overview.md for architecture"
2. Include API docs: "Check iced-api/widget/ for available widgets"
3. Point to examples: "Look at examples/counter.rs for a simple app"

The documentation is in `llmstxt` format, optimized for LLM comprehension.

## Tips for Conversion

- **Elm Architecture**: Iced follows The Elm Architecture (TEA)
- **State Management**: Application state in a single struct
- **Messages**: User interactions trigger messages
- **Update Function**: Messages update state
- **View Function**: State renders to widgets
- **Subscriptions**: Async events (timers, streams)

## Key Modules

- `iced::widget` - Built-in widgets (button, text, container, etc.)
- `iced::application` - Main application trait
- `iced::executor` - Async runtime
- `iced::Theme` - Styling system
"""

    (output_dir / "README.md").write_text(index_content)
    console.print(f"[green]✓ Created index at {output_dir / 'README.md'}[/green]")


@app.command()
def main(
    version: Annotated[
        str,
        typer.Option(help="Iced version to fetch"),
    ] = "0.13.1",
    layershell_version: Annotated[
        str,
        typer.Option(help="iced_layershell version to fetch"),
    ] = "0.13.7",
    output_dir: Annotated[
        Path,
        typer.Option(help="Output directory for documentation"),
    ] = Path("./docs/iced"),
    include_examples: Annotated[
        bool,
        typer.Option(help="Also fetch example code from GitHub"),
    ] = False,
    crates: Annotated[
        list[str],
        typer.Option(help="Crates to fetch docs for"),
    ] = ["iced", "iced_core", "iced_widget", "iced_runtime", "iced_layershell"],
):
    """
    Fetch and convert Iced GUI framework documentation to LLM-friendly markdown.

    This script:

    1. Clones the Iced Book (tutorial/guide content)
    2. Builds API documentation using cargo doc
    3. Optionally fetches examples from the GitHub repository

    [bold cyan]Examples:[/bold cyan]

      [dim]# Basic usage - fetch book + core API docs[/dim]
      uv run fetch_iced_docs.py

      [dim]# Include examples from GitHub[/dim]
      uv run fetch_iced_docs.py --include-examples

      [dim]# Specific version and output location[/dim]
      uv run fetch_iced_docs.py --version 0.13.1 --output-dir ./iced-docs

      [dim]# Custom set of crates[/dim]
      uv run fetch_iced_docs.py --crates iced --crates iced_core
    """
    output_dir.mkdir(parents=True, exist_ok=True)

    console.print(f"\n[bold cyan]Fetching Iced {version} documentation[/bold cyan]")
    console.print(f"Output directory: {output_dir.absolute()}\n")

    success_count = 0
    total_tasks = 2 + (
        1 if include_examples else 0
    )  # book + cargo docs + optional examples

    # 1. Fetch the Iced book
    if clone_iced_book(output_dir):
        success_count += 1

    # 2. Build API docs with cargo
    if build_cargo_docs(crates, version, layershell_version, output_dir):
        # Convert each crate's docs to markdown
        for crate in crates:
            if not convert_cargo_docs(crate, output_dir):
                console.print(
                    f"[yellow]Warning: Failed to convert {crate} docs[/yellow]"
                )
        success_count += 1

        # Clean up cargo temp directory
        temp_dir = output_dir / "cargo-temp"
        if temp_dir.exists():
            console.print("[cyan]Cleaning up temporary cargo files...[/cyan]")
            shutil.rmtree(temp_dir)

    # 3. Optionally fetch examples
    if include_examples:
        if fetch_examples(version, output_dir):
            success_count += 1

    # 4. Create index
    create_index(output_dir, crates, include_examples)

    # Summary
    console.print("\n[bold]Summary:[/bold]")
    console.print(f"  Completed: {success_count}/{total_tasks} tasks")
    console.print(f"  Output: {output_dir.absolute()}")

    if success_count == total_tasks:
        console.print(
            "\n[bold green]✓ All documentation fetched successfully![/bold green]"
        )
        console.print(
            "\n[cyan]You can now reference these docs when prompting your LLM about Iced.[/cyan]"
        )
        console.print(
            f"[cyan]See {output_dir / 'README.md'} for navigation tips.[/cyan]"
        )
        raise typer.Exit(0)
    else:
        console.print(
            f"\n[yellow]⚠ {total_tasks - success_count} task(s) failed - check the logs above[/yellow]"
        )
        raise typer.Exit(1)


if __name__ == "__main__":
    app()
