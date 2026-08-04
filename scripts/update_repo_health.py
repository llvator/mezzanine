#!/usr/bin/env python3
"""Regenerate the Repo Health table in README.md.

Runs `nao analyze` on `src/`, computes a small set of summary metrics,
and rewrites the sentinel-bracketed block in `README.md`.

Idempotent: re-running on an unchanged tree produces a byte-identical
README.md, so the pre-commit hook only stages a change when the metrics
actually moved.

Run manually:
    python3 scripts/update_repo_health.py

Run automatically: install the pre-commit hook with
    bash scripts/install_hooks.sh
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

START_SENTINEL = "<!-- repo-health:start -->"
END_SENTINEL = "<!-- repo-health:end -->"

# Same thresholds as the complexity gate in CONTRIBUTING.md and the
# diff-style comparator in .github/scripts/check_complexity.py.
CEILING = {"cyclo": 15, "cog": 22, "nest": 4}


def repo_root() -> Path:
    out = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        capture_output=True,
        text=True,
        check=False,
    )
    if out.returncode != 0:
        sys.exit("not in a git repo")
    return Path(out.stdout.strip())


def percentile(sorted_values: list[int], p: float) -> int:
    if not sorted_values:
        return 0
    idx = min(int(len(sorted_values) * p), len(sorted_values) - 1)
    return sorted_values[idx]


def collect_metrics(json_path: Path) -> dict[str, object]:
    data = json.loads(json_path.read_text())
    cyclos: list[int] = []
    cogs: list[int] = []
    nests: list[int] = []
    files: set[str] = set()
    over_ceiling = 0

    for entity in data.get("entities", []):
        m = entity.get("metrics") or {}
        if m.get("cyclomatic") is None:
            # No body to measure (signatures, abstract methods, etc.).
            continue
        files.add(entity["file_path"])
        c = int(m["cyclomatic"])
        g = int(m.get("cognitive_complexity") or 0)
        n = int(m.get("max_nesting") or 0)
        cyclos.append(c)
        cogs.append(g)
        nests.append(n)
        if c > CEILING["cyclo"] or g > CEILING["cog"] or n > CEILING["nest"]:
            over_ceiling += 1

    cyclos.sort()
    cogs.sort()
    nests.sort()

    return {
        "files": len(files),
        "functions": len(cyclos),
        "over_ceiling": over_ceiling,
        "cyclo": (percentile(cyclos, 0.5), percentile(cyclos, 0.9), max(cyclos) if cyclos else 0),
        "cog":   (percentile(cogs,   0.5), percentile(cogs,   0.9), max(cogs)   if cogs   else 0),
        "nest":  (percentile(nests,  0.5), percentile(nests,  0.9), max(nests)  if nests  else 0),
    }


def render(m: dict[str, object]) -> str:
    cyclo = m["cyclo"]      # type: ignore[assignment]
    cog = m["cog"]          # type: ignore[assignment]
    nest = m["nest"]        # type: ignore[assignment]
    return "\n".join([
        "| Metric | Value |",
        "|---|---|",
        f"| Source files (Rust) | {m['files']} |",
        f"| Functions analyzed | {m['functions']} |",
        f"| Functions above ceiling (grandfathered) | {m['over_ceiling']} |",
        f"| Cyclomatic complexity (p50 / p90 / max) | {cyclo[0]} / {cyclo[1]} / {cyclo[2]} |",
        f"| Cognitive complexity (p50 / p90 / max) | {cog[0]} / {cog[1]} / {cog[2]} |",
        f"| Max nesting depth (p50 / p90 / max) | {nest[0]} / {nest[1]} / {nest[2]} |",
    ])


def update_readme(readme_path: Path, new_block: str) -> bool:
    """Replace the content between sentinels. Returns True if the file changed."""
    text = readme_path.read_text()
    if START_SENTINEL not in text or END_SENTINEL not in text:
        sys.exit(
            f"Sentinels {START_SENTINEL} / {END_SENTINEL} not found in {readme_path}.\n"
            "Add them around an empty placeholder block before running this script."
        )

    pattern = re.compile(
        rf"({re.escape(START_SENTINEL)})(.*?)({re.escape(END_SENTINEL)})",
        re.DOTALL,
    )
    new_text = pattern.sub(f"\\1\n{new_block}\n\\3", text)

    if new_text == text:
        return False
    readme_path.write_text(new_text)
    return True


def run_nao(root: Path, output: Path) -> None:
    """Run nao via cargo so cargo decides whether to rebuild."""
    with output.open("w") as f:
        subprocess.run(
            [
                "cargo", "run", "--release", "--quiet", "--bin", "nao", "--",
                "analyze", "-f", "json", "-l", "rust", "src/",
            ],
            cwd=root,
            stdout=f,
            stderr=subprocess.DEVNULL,
            check=True,
        )


def main() -> int:
    root = repo_root()
    readme = root / "README.md"
    out_dir = root / "target"
    out_dir.mkdir(parents=True, exist_ok=True)
    tmp = out_dir / "repo_health.json"

    run_nao(root, tmp)
    metrics = collect_metrics(tmp)
    block = render(metrics)
    changed = update_readme(readme, block)

    rel = readme.relative_to(root)
    if changed:
        print(f"Updated {rel}")
    else:
        print(f"{rel} already up to date.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
