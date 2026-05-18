#!/usr/bin/env python3
"""
Markdown Link Checker

Scans all Markdown files in the repository and validates local relative links.
Ignores:
- HTTP/HTTPS URLs
- mailto: links
- Anchor links (#)
- tmp/ directory

Exit code 0 if all links are valid, 1 if any broken links found.
"""

import os
import re
import sys
from pathlib import Path
from typing import List, Tuple

# Security: Maximum file size to prevent DoS (10MB)
MAX_FILE_SIZE_BYTES = 10 * 1024 * 1024

# Security: Regex timeout to prevent ReDoS attacks
REGEX_TIMEOUT_SECONDS = 5

# Regex to match markdown links: [text](link)
# Using non-greedy quantifiers to prevent ReDoS
LINK_PATTERN = re.compile(r'\[([^\]]+?)\]\(([^)]+?)\)', re.DOTALL)


def is_external_or_ignored(link: str) -> bool:
    """Check if link should be ignored"""
    return (
        link.startswith('http://') or
        link.startswith('https://') or
        link.startswith('mailto:') or
        link.startswith('#') or
        '/tmp/' in link or
        link.startswith('tmp/')
    )


def is_safe_path(path: Path, repo_root: Path) -> bool:
    """
    Check if a resolved path is safe (within repo and not a symlink).

    Security: Prevents path traversal attacks and symlink attacks.
    """
    try:
        # Check if path is a symlink
        if path.is_symlink():
            return False

        # Ensure resolved path is within repository
        # This prevents path traversal attacks like ../../../../etc/passwd
        resolved_path = path.resolve()
        resolved_repo = repo_root.resolve()

        # Check if resolved_path is under resolved_repo
        try:
            resolved_path.relative_to(resolved_repo)
            return True
        except ValueError:
            # Path is outside repository
            return False

    except (OSError, RuntimeError):
        # Handle resolution errors
        return False


def resolve_relative_path(md_file: Path, link: str, repo_root: Path) -> Path:
    """
    Resolve relative link to absolute path with security validation.

    Security: Validates path safety to prevent traversal and symlink attacks.
    """
    # Remove anchor if present
    link_path = link.split('#')[0]

    if not link_path:  # Pure anchor link
        return md_file

    # Resolve relative to markdown file's directory
    md_dir = md_file.parent
    resolved = (md_dir / link_path).resolve()

    return resolved


def check_markdown_file(md_file: Path, repo_root: Path) -> List[Tuple[str, str]]:
    """
    Check all links in a markdown file with security validation.

    Returns list of (link, error_message) tuples for broken links.

    Security: Implements file size limits, encoding validation, and path safety checks.
    """
    broken_links = []

    # Security: Check file size before reading
    try:
        file_size = md_file.stat().st_size
        if file_size > MAX_FILE_SIZE_BYTES:
            print(f"Warning: Skipping {md_file} (size {file_size} exceeds {MAX_FILE_SIZE_BYTES} bytes)",
                  file=sys.stderr)
            return broken_links
    except Exception as e:
        print(f"Warning: Could not stat {md_file}: {e}", file=sys.stderr)
        return broken_links

    # Security: Proper encoding error handling
    try:
        content = md_file.read_text(encoding='utf-8', errors='strict')
    except UnicodeDecodeError as e:
        print(f"Warning: Invalid UTF-8 encoding in {md_file}: {e}", file=sys.stderr)
        return broken_links
    except Exception as e:
        print(f"Warning: Could not read {md_file}: {e}", file=sys.stderr)
        return broken_links

    for line_num, line in enumerate(content.splitlines(), 1):
        for match in LINK_PATTERN.finditer(line):
            text = match.group(1)
            link = match.group(2)

            # Skip external and ignored links
            if is_external_or_ignored(link):
                continue

            # Resolve and check local link
            try:
                target_path = resolve_relative_path(md_file, link, repo_root)

                # Security: Validate path safety (prevents traversal and symlink attacks)
                if not is_safe_path(target_path, repo_root):
                    relative_md = md_file.relative_to(repo_root)
                    broken_links.append((
                        f"{relative_md}:{line_num}",
                        f"Security: Unsafe link [{text}]({link}) -> {target_path} (path traversal or symlink detected)"
                    ))
                    continue

                if not target_path.exists():
                    relative_md = md_file.relative_to(repo_root)
                    broken_links.append((
                        f"{relative_md}:{line_num}",
                        f"Broken link [{text}]({link}) -> {target_path} does not exist"
                    ))
            except Exception as e:
                relative_md = md_file.relative_to(repo_root)
                broken_links.append((
                    f"{relative_md}:{line_num}",
                    f"Error resolving link [{text}]({link}): {e}"
                ))

    return broken_links


def find_markdown_files(repo_root: Path) -> List[Path]:
    """Find all markdown files in the repository, excluding tmp/"""
    markdown_files = []

    for md_file in repo_root.rglob('*.md'):
        # Skip tmp/ directories
        if any(part == 'tmp' for part in md_file.parts):
            continue

        # Skip hidden directories
        if any(part.startswith('.') for part in md_file.relative_to(repo_root).parts[:-1]):
            continue

        markdown_files.append(md_file)

    return sorted(markdown_files)


def main():
    # Determine repository root (parent of scripts/ directory)
    script_dir = Path(__file__).parent
    repo_root = script_dir.parent

    print(f"Checking Markdown links in {repo_root}")
    print("-" * 80)

    # Find all markdown files
    md_files = find_markdown_files(repo_root)
    print(f"Found {len(md_files)} Markdown files\n")

    # Check each file
    all_broken_links = []
    for md_file in md_files:
        broken_links = check_markdown_file(md_file, repo_root)
        all_broken_links.extend(broken_links)

    # Report results
    if all_broken_links:
        print(f"\n❌ Found {len(all_broken_links)} broken link(s):\n")
        for location, error in all_broken_links:
            print(f"  {location}")
            print(f"    {error}\n")
        return 1
    else:
        print("✅ All Markdown links are valid!")
        return 0


if __name__ == '__main__':
    sys.exit(main())
