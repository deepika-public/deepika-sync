#!/usr/bin/env python3
"""Keep the documentation from drifting: no pinned version or download link outside
docs/reference/compatibility.md, and no relative link to a missing file."""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PAGES = [ROOT / "README.md", ROOT / "web" / "README.md", *(ROOT / "docs").rglob("*.md")]
FREE_TO_NAME_VERSIONS = {ROOT / "docs" / "reference" / "compatibility.md",
                         ROOT / "docs" / "explanation" / "decisions.md"}
# deepika-sync versions are 0.x.y; 127.0.0.1 and tool versions (1.95, 3.17, 0.28) do not match.
PINNED = re.compile(r"(?<![\d.])v?0\.\d+\.\d+(?![\d.])|package_files/\d+")
LINK = re.compile(r"\]\(([^)\s#]+\.md)(?:#[^)]*)?\)")

problems = []
for page in PAGES:
    text = page.read_text(encoding="utf-8")
    rel = page.relative_to(ROOT)
    if page not in FREE_TO_NAME_VERSIONS:
        for n, line in enumerate(text.splitlines(), 1):
            hit = PINNED.search(line)
            if hit:
                problems.append(f"{rel}:{n}: version ou lien figé « {hit.group(0)} »")
    for m in LINK.finditer(text):
        target = m.group(1)
        if target.startswith("http"):
            continue
        if not (page.parent / target).resolve().exists():
            problems.append(f"{rel}: lien vers un fichier absent « {target} »")

for p in problems:
    print(p)
print(f"{len(PAGES)} pages vérifiées, {len(problems)} problème(s)")
sys.exit(1 if problems else 0)
