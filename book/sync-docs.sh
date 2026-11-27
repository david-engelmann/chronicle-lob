#!/usr/bin/env bash
# Stage curated docs/ pages and media into book/src/docs/ for mdBook, and
# rewrite relative links that point outside the published book tree to
# canonical GitHub URLs so mdbook-linkcheck passes with zero warnings.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
src_docs="$repo_root/book/src/docs"

rm -rf "$src_docs"
mkdir -p "$src_docs/media"

published_pages=(
  "ARCHITECTURE"
  "INVARIANTS"
  "PERFORMANCE"
  "FAQ"
  "COMPARISON"
)

for page in "${published_pages[@]}"; do
  cp "$repo_root/docs/$page.md" "$src_docs/$page.md"
done

for ext in svg png gif; do
  for asset in "$repo_root"/docs/media/*."$ext"; do
    if [ -f "$asset" ]; then
      cp "$asset" "$src_docs/media/"
    fi
  done
done

python3 - "$src_docs" <<'PY'
import pathlib
import re
import sys

src_docs = pathlib.Path(sys.argv[1])
gh_blob = "https://github.com/david-engelmann/chronicle-lob/blob/main"
gh_tree = "https://github.com/david-engelmann/chronicle-lob/tree/main"

root_files = (
    "AGENTS.md",
    "CHANGELOG.md",
    "CITATION.cff",
    "CONTRIBUTING.md",
    "Cargo.toml",
    "DECISIONS.md",
    "LICENSE-APACHE",
    "LICENSE-MIT",
    "Makefile",
    "README.md",
    "SECURITY.md",
    "deny.toml",
    "pyproject.toml",
    "rust-toolchain.toml",
)
root_alt = "|".join(re.escape(name) for name in root_files)

# 1) Relative links to repo-root files (with optional ../ prefix and #anchor).
root_re = re.compile(
    rf"\]\((?:\.\./)+({root_alt})(#[^)]*)?\)"
)
# Bare repo-root maintainer files written without ../ inside docs/ prose.
bare_root_alt = "|".join(
    re.escape(name)
    for name in (
        "AGENTS.md",
        "CHANGELOG.md",
        "CONTRIBUTING.md",
        "DECISIONS.md",
        "SECURITY.md",
    )
)
bare_root_re = re.compile(rf"\]\(({bare_root_alt})(#[^)]*)?\)")

# 2) Unpublished maintainer docs/ files (ROADMAP.md, docs/README.md).
maintainer_docs_re = re.compile(
    r"\]\((?:\.\./docs/|\.\./)?(ROADMAP\.md|README\.md)(#[^)]*)?\)"
)

# 3) Relative links to source directories outside book/src/.
repo_path_re = re.compile(
    r"\]\((?:\.\./)+(crates|examples|fuzz|python|\.github)/([^)]*)\)"
)

fence_re = re.compile(r"^ {0,3}(`{3,}|~{3,})")
code_span_re = re.compile(r"(`+[^`]*`+)")
bare_bracket_re = re.compile(r"(?<!\\)\[([^\[\]\n]+)\](?![(\[:])")


def escape_non_link_brackets(text: str) -> str:
    ref_defs = {
        m.group(1).strip().lower()
        for m in re.finditer(r"(?m)^ {0,3}\[([^\]]+)\]:\s*\S+", text)
    }
    out_lines: list[str] = []
    active_fence: str | None = None
    for line in text.splitlines(keepends=True):
        fence_match = fence_re.match(line)
        if fence_match:
            marker = fence_match.group(1)
            if active_fence is None:
                active_fence = marker
            elif line.lstrip().startswith(active_fence):
                active_fence = None
            out_lines.append(line)
            continue
        if active_fence is not None:
            out_lines.append(line)
            continue

        parts = code_span_re.split(line)
        for idx in range(0, len(parts), 2):
            parts[idx] = bare_bracket_re.sub(
                lambda m: (
                    m.group(0)
                    if m.group(1).strip().lower() in ref_defs
                    else rf"\[{m.group(1)}\]"
                ),
                parts[idx],
            )
        out_lines.append("".join(parts))
    return "".join(out_lines)


for md_path in sorted(src_docs.glob("*.md")):
    text = md_path.read_text(encoding="utf-8")
    text = root_re.sub(
        lambda m: f"]({gh_blob}/{m.group(1)}{m.group(2) or ''})", text
    )
    text = bare_root_re.sub(
        lambda m: f"]({gh_blob}/{m.group(1)}{m.group(2) or ''})", text
    )
    text = maintainer_docs_re.sub(
        lambda m: f"]({gh_blob}/docs/{m.group(1)}{m.group(2) or ''})", text
    )

    def _rewrite_repo_path(match: re.Match[str]) -> str:
        top, rest = match.group(1), match.group(2)
        base = gh_tree if (not rest or rest.endswith("/")) else gh_blob
        suffix = f"/{rest}" if rest else ""
        return f"]({base}/{top}{suffix})"

    text = repo_path_re.sub(_rewrite_repo_path, text)
    text = escape_non_link_brackets(text)
    md_path.write_text(text, encoding="utf-8")
PY

echo "staged $(find "$src_docs" -maxdepth 1 -name '*.md' | wc -l | tr -d ' ') docs pages into book/src/docs/"
