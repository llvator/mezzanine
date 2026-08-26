#!/usr/bin/env python3
"""Compare mezz analysis output between the PR's base and head.

Fails (exit 1) if either:
  - A new function (not in baseline) exceeds the complexity ceiling.
  - An existing function's metrics regressed (any of cyclo / cognitive / nesting
    increased compared to baseline).

Usage:
    check_complexity.py BASELINE.json CURRENT.json

Both inputs are expected to be the JSON output of `mezz analyze -f json`.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

# Ceiling: p90 of the codebase at the time this gate was introduced.
# See CONTRIBUTING.md ("Complexity ceiling") for rationale.
CEILING = {
    "cyclo": 15,
    "cog": 22,
    "nest": 4,
}

# Strip any prefix before "src/" so paths from different working dirs
# (e.g., a worktree at /tmp/base/src/...) compare equal to local paths.
SRC_RE = re.compile(r"(src/.*)")


def normalize_path(p: str) -> str:
    m = SRC_RE.search(p)
    return m.group(1) if m else p


def load(path: str) -> dict[str, dict[str, int]]:
    """Read a mezz JSON dump → {key: {cyclo, cog, nest}}."""
    data = json.loads(Path(path).read_text())
    out: dict[str, dict[str, int]] = {}
    for entity in data.get("entities", []):
        m = entity.get("metrics") or {}
        if m.get("cyclomatic") is None:
            # No body to measure (signatures, abstract methods, etc.) — skip.
            continue
        key = f"{normalize_path(entity['file_path'])}::{entity['name']}"
        out[key] = {
            "cyclo": int(m["cyclomatic"]),
            "cog": int(m.get("cognitive_complexity") or 0),
            "nest": int(m.get("max_nesting") or 0),
        }
    return out


def above_ceiling(m: dict[str, int]) -> bool:
    return any(m[k] > CEILING[k] for k in CEILING)


def diagnose(m: dict[str, int]) -> str:
    parts = [f"{k}={m[k]}" + ("!" if m[k] > CEILING[k] else "") for k in CEILING]
    return " ".join(parts)


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print(f"usage: {argv[0]} BASELINE.json CURRENT.json", file=sys.stderr)
        return 2

    baseline = load(argv[1])
    current = load(argv[2])

    new_violations: list[str] = []
    regressions: list[str] = []

    for key, m in current.items():
        old = baseline.get(key)
        if old is None:
            # New function — must be at or below the ceiling.
            if above_ceiling(m):
                new_violations.append(f"  {key}\n      {diagnose(m)}")
        else:
            # Existing function — must not get worse on ANY metric.
            worse = [k for k in CEILING if m[k] > old[k]]
            if worse:
                changes = ", ".join(
                    f"{k}: {old[k]} → {m[k]}" for k in worse
                )
                regressions.append(f"  {key}\n      {changes}")

    if not new_violations and not regressions:
        print(
            f"Complexity gate passed (ceiling: cyclo<={CEILING['cyclo']}, "
            f"cognitive<={CEILING['cog']}, nesting<={CEILING['nest']})."
        )
        return 0

    print("Complexity gate failed.\n", file=sys.stderr)

    if new_violations:
        print(
            f"New functions exceeding the ceiling "
            f"(cyclo<={CEILING['cyclo']}, cognitive<={CEILING['cog']}, "
            f"nesting<={CEILING['nest']}):",
            file=sys.stderr,
        )
        for v in new_violations:
            print(v, file=sys.stderr)
        print("", file=sys.stderr)

    if regressions:
        print("Existing functions regressed:", file=sys.stderr)
        for r in regressions:
            print(r, file=sys.stderr)
        print("", file=sys.stderr)

    print(
        'See CONTRIBUTING.md ("Complexity ceiling") for the contract and for\n'
        "how to reproduce this locally.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
