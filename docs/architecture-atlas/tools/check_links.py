#!/usr/bin/env python3
"""Check local Markdown links in the architecture atlas."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ATLAS = Path(__file__).resolve().parents[1]
SRC = ATLAS / "src"
LINK = re.compile(r"(?<![!\\])\[[^\]]+\]\(([^)]+)\)")


def main() -> int:
    failures: list[str] = []
    checked = 0
    for markdown in sorted(SRC.rglob("*.md")):
        text = markdown.read_text(encoding="utf-8", errors="replace")
        for raw in LINK.findall(text):
            target = raw.strip().split(maxsplit=1)[0].strip("<>")
            if (
                not target
                or target.startswith(("#", "http://", "https://", "mailto:"))
            ):
                continue
            path_text = target.split("#", 1)[0]
            if not path_text:
                continue
            checked += 1
            candidate = (markdown.parent / path_text).resolve()
            if not candidate.exists():
                failures.append(
                    f"{markdown.relative_to(ATLAS)} -> {target} "
                    f"(missing {candidate})"
                )
    if failures:
        print("Broken local links:", file=sys.stderr)
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(f"atlas link check: {checked} local targets OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
