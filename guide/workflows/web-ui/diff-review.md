# Diff review

> Did this change the shape of the system, not just its text?

Line diffs are excellent at "what text changed" and structurally blind. They
show a function moved and a function rewritten identically. They cannot show
you that a clean boundary sprouted three new crossings, because nothing on
either side of that boundary changed textually — only the edge between them
did.

The diff overlay is a **structural** comparison, computed by analyzing both
trees in git worktrees.

## Using it

Open the **Diff** panel and pick your two commits with the CommitPicker. The
overlay highlights what changed structurally: entities added, removed, and
those whose metrics or coupling moved.

Two comparison modes behave differently on purpose:

| Comparison | Behaviour |
| --- | --- |
| Working tree vs a ref | **Follows the watcher** — every re-analysis recomputes it against the same base, so the overlay describes the tree you are editing now |
| Commit vs commit | A fixed comparison; never follows |

Pass `--pin-diff` to `mezz watch` if you want a working-tree diff frozen at
the moment you computed it instead of tracking your edits.

## Comparing two branches

**Branches** in the picker compares one branch against another without
checking either one out. Pick the base — usually `main` — and the branch under
review; the panel says in one sentence what it is about to compare, then runs
it.

It compares **from where the two branches diverged**, not from the base
branch's tip, and that difference is the whole point. A branch cut a week ago
differs from `main` by its own work *and* by everything that has landed on
main since, so a tip-to-tip comparison reports main's own recent commits as
things the branch deleted. Measuring from the fork point is the same
comparison a pull request shows you.

Untick **Measure from where the two branches diverged** for the tip-to-tip
reading. That one answers a different question — "how do these two trees
differ right now", which is what you want before a merge rather than during a
review.

Commits from other branches are reachable in the **Compare Commits** tab too:
the list has a branch selector above it, and `From` and `To` hold the commits
you picked, so a base on one branch and a target on another is two clicks.
Either side also still takes a typed ref — a branch name, a tag, `HEAD~3`.

The canvas keeps drawing your working tree throughout. Only the overlay
describes the branch, because the engine analyses the checkout it was pointed
at and a comparison does not re-point it.

## The workflow that pays: review before reading

Run the overlay on someone else's branch **before** you read their diff.

You are not looking for correctness — you cannot see correctness on a canvas.
You are looking for *where to read carefully*:

- **New nodes in an unexpected neighbourhood.** A change described as "fix
  the parser" that added entities under `server/` is a change whose
  description is incomplete.
- **A node that grew.** Fan-out ballooning on one function means it took on
  coordination it did not have before — often the real cost of a feature that
  looked additive.
- **New edges crossing a boundary.** Two subsystems that did not touch now
  do. Sometimes correct, always worth a sentence in the PR.
- **Nodes that disappeared.** Deletions are the least reviewed part of any
  diff and the most likely to be load-bearing.

Then read the textual diff with those three or four places already flagged.
You spend your attention where the structure says it matters instead of
uniformly from top to bottom.

## Pair it with the text report

The canvas shows you *where*; `assess_change` gives you the numbers to quote
in the review:

```
assess_change { "base_ref": "main" }
```

```
- function `forceFolderCohesion` — ui/src/utils/forceCohesion.ts:
    cyclomatic 19→27 (+8), max_nesting 3→4 (+1), loc 63→130 (+67)
```

"This feels more complex" is not actionable feedback and invites an argument
about taste. "Cyclomatic 19→27" is a fact, and the conversation moves
directly to whether that increase is justified — which is the conversation
worth having.

For a team, [push review](../cli/push-review.md) puts exactly this in the PR
automatically.

## Reading the noise

`assess_change` reports modified entities split into **source** and
**coupling-only** changes, and the ratio is often lopsided:

```
28 added, 6 removed, 1662 modified (91 source, 1571 coupling-only)
```

Coupling-only means the entity's own code did not change — its neighbourhood
did. That is real information when you are asking "what did this ripple
into," and pure noise when you are asking "what did the author write." Read
the source count first.

## Limits

- The comparison needs a git repository and computes the base in a temporary
  worktree, so the base ref must be checkoutable.
- Renames read as a delete plus an add. A large refactor that mostly moves
  code produces a dramatic overlay describing very little semantic change.
- Structural equivalence is not behavioural equivalence: a one-character
  change inside a function that alters everything shows as "source changed,
  metrics stable." The overlay tells you where to look, never whether the
  code is right.
