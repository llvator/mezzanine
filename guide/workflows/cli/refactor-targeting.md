# Refactor targeting

> Where does refactoring pay, what exactly is wrong there, what breaks if I
> touch it, and how do I verify the result?

Four questions, four tools, in that order. The chain matters more than any
single link: complexity alone picks the wrong targets, because the most
complex code in a repo is often the code nobody has needed to touch in two
years.

## The chain

| Step | Tool | Question |
| --- | --- | --- |
| 1 | `hotspots` | Where does it pay? |
| 2 | `quality` | What is wrong there? |
| 3 | `impact` | What breaks? |
| 4 | `tests_for` | How do I verify it? |

### 1. Where does it pay — `hotspots`

Ranks files by **git churn × complexity**. This is the step people skip, and
skipping it is why refactoring effort so often lands on code that was fine.

```
hotspots { "top": 8 }
```

```
- [risk 23.82] vscode-extension/src/extension.ts — 18 commits, avg pressure 1.32;
    worst: function `activate` (L44, loc 727, cx 65, cog 200, out 105)
- [risk 20.03] ui/src/components/GraphView.svelte — 21 commits, avg pressure 0.95;
    worst: function `applyDisplayPlan` (L518, loc 207, cx 75, cog 207, out 28)
```

Complex *and* changing every week is where bugs concentrate. Complex and
stable ranks low on purpose — it is load-bearing, it works, and touching it
buys you risk without buying anyone anything.

Scope it with `path` when you are working in one area, and widen `days` when
a repo has slow seasons.

### 2. What is wrong there — `quality`

Now that you have a file, ask what shape the problem takes.

```
quality { "path": "src/educator", "top": 5 }
```

```
## Smells (7)
- function `extract` — src/educator/java.rs:642 (loc 128, cx 32, out 34, ⚠ Dispatcher)
- struct `Rule` — src/educator/rules.rs:40 (in 13, ⚠ Data Bag)

Remediation hints:
- Dispatcher: Replace branching with polymorphism — Strategy, Command, or Chain of Responsibility.
```

The smell name is the useful part — it turns "this is messy" into a named
pattern with a known fix. The composite refactor-pressure ranking below it
catches the entities that are merely heavy without tripping a named smell.

> **Known bug:** pass a **directory**, not a single file. A file path
> currently returns empty sections rather than an error. An empty answer
> here means the tool failed, not that the file is clean.

### 3. What breaks — `impact`

Before you change a signature, find out who is downstream.

```
impact { "entity": "compute_diff" }
```

```
## Used by (4) — direct dependents, first to break on a contract change
- calls ·heuristic function `run_diff` — src/main.rs:1297
- calls ·heuristic function `compute_diff_blocking` — src/server/diff_handler.rs:85

## Blast radius to depth 2 (4 entities beyond direct dependents)
- [depth 2] function `pr_report` — src/mcp/push.rs:340
```

Read the `·exact` / `·heuristic` markers. Call resolution is name-based and
measured at **93.7% recall / 98.6% precision** against a rust-analyzer oracle
on this repo — so roughly one real dependent in sixteen is missing. A
`Used by (0)` on something you expected to be called is a reason to grep, not
an all-clear.

### 4. How to verify — `tests_for`

```
tests_for { "entity": "render_change_report" }
```

Run those tests, not the suite. When the answer is "no tests reach this,"
believe it enough to check: it is either genuinely untested — the useful
finding, and the thing to fix *before* refactoring — or the coverage flows
through call edges the graph could not resolve.

## Then close the loop

After the edit, `assess_change` tells you whether you actually improved
anything or just moved it:

```
assess_change { "base_ref": "HEAD" }
```

```
- function `forceFolderCohesion` — ui/src/utils/forceCohesion.ts:
    cyclomatic 19→27 (+8), max_nesting 3→4 (+1), loc 63→130 (+67)
```

"It feels cleaner" is not a review-grade claim. "Cognitive complexity 62→19,
Dispatcher smell resolved" is.

## Limits

- `hotspots` needs a git repository and reads `git log`; a shallow clone
  gives you a shallow ranking.
- Churn counts renames as fresh paths, so a big move looks like a spike for
  one window.
- Smell thresholds are absolute constants, not per-repo percentiles. A
  uniformly complex codebase will flag a lot; a uniformly simple one will
  flag little. Calibration is on the roadmap.
