# Spec-first tasks

> Which part of the domain does this task belong to, and what did it claim
> before I started?

When a project has an Elevator (`.elv`) layer, it sits *above* the code: a
description of user-facing capabilities that a newcomer can read before
touching a file. This workflow uses it as the entry point to a task and the
exit point of one, so the spec deepens as work happens instead of rotting.

## 1. Ground yourself in the domain

```bash
elevator spec                        # the whole map
elevator spec --focus f.parsers      # one branch, with context
```

Or as a tool call, which is the same renderer:

```
overview {}
overview { "focus": "f.parsers" }
```

The full map on this repo is 7 Categories, 38 Features, 18 Functionalities and
1 Concept — the whole system in about sixty lines, each entity carrying a
`cr:` path into the code that implements it.

`--focus` is the right artifact when you are about to work *on* something: it
returns the ancestor chain, the target's subtree, its siblings, and the
cross-cutting Concepts touching the path. Siblings matter more than they
sound — they are how you learn that the thing you are about to add already
has three peers with an established shape.

## 2. Capture the slice you are working on

```
spec_slice { "path": "src/mcp", "out": ".mezz/spec-slice.elv" }
```

This extracts every spec entity whose `cr:` claims a path inside the folder,
plus its subtree, the Concepts it uses, and its ancestors as pruned context —
re-emitted as **standalone, valid `.elv`** that re-parses and passes `--check`
on its own.

Dropped into a ticket folder, it becomes a small diffable record of what the
domain layer said about this area when the work began. Six months later that
answers "was this always the design, or did we drift into it?"

The CLI equivalent selects by name rather than by path:

```bash
elevator spec --extract f.spec_health -o work/slice.elv
```

The difference is the question in front of it. `--extract` asks "give me this
entity's branch"; `spec_slice` asks "what does the spec claim about this
folder", answered from the `cr:` index. When nothing claims a path inside the
folder, it falls back to the narrowest claim *above* it and says so — and a
folder no `cr:` mentions at all is an error, because an empty slice reads as
"the spec says nothing here" while looking like a successful extraction.

`spec_slice` is the only MCP tool that writes. `out` is fenced to the project
root and never replaces an existing file unless you pass `overwrite`.

## 3. Do the work

Nothing spec-specific here — see [the agent loop](agent-loop.md) or
[refactor targeting](refactor-targeting.md).

## 4. Let the change tell you what to deepen

```
assess_change { "base_ref": "HEAD" }
```

The report ends with a **Spec claims** section:

```
## Spec claims (2)
- c visualizer claims ui/ — is its description still true?
- fu node_encoding.legend claims ui/scripts/ux-probe.mjs — is its description still true?
```

That is the prompt to update the `d:` description while the change is fresh.
It is the step that keeps the layer honest, because it arrives attached to the
diff rather than as a periodic documentation chore.

## Why this stays cheap

The Elevator layer is built **sketch-first**: breadth everywhere — every area
named in one line — and depth only where work has actually happened. You are
never asked to fill in detail for a corner nobody has touched. That is a
standing project principle: no feature may force authors to annotate by hand;
the value has to come from what the code already says, or from a description
someone wrote *because* they just did the work.

## Limits

Elevator's semantic payload — the `d:` descriptions and `cr:` paths — is
**authored, not derived**. It is the one layer that can lag the code, and the
artifact legend says so to its consumers ("strong leads, not ground truth").

[Spec health](spec-health.md) is how you verify the anchors. But note what
even that cannot catch: a description that is wrong *without naming anything
checkable* can only be caught by a human or agent reading the `cr:` target.
