# Spec health

> Is the spec still telling the truth about the code?

A domain spec that has quietly drifted is worse than no spec, because people
still trust it. These four checks are cheap enough to run in CI and pointed
enough to be worth reading when they fire.

All of them take the spec directory; `-q` suppresses the analyzer's progress
lines on stderr.

## `--check` — is the spec internally coherent?

```bash
elevator spec --check -q
```

```
✓ No issues.
```

Five rules. Parse/lex errors and unresolved references are **errors** (exit
1); orphan Features, empty Categories and unused Concepts are **hints** (exit
0).

An unresolved reference means the spec names an entity that no file defines —
a broken link in the domain map:

```
error: unresolved: fu entities.creation is referenced but never defined
```

This is the one to gate CI on, because it is unambiguous and always the
author's to fix.

## `--drift` — do the code anchors still resolve?

```bash
elevator spec --drift --code-root . -q
```

```
> 94 cr reference(s): 80 resolved, 0 missing — 93 identifier(s) checked in 390 file(s), 1 unanchored

## Unanchored identifiers (hint)
f node_encoding
  `isElevatorGraph`
```

Two verifications, deliberately at different severities:

| Check | Severity | Meaning |
| --- | --- | --- |
| Every `cr:` path resolves under the code root | **error**, exit 1 | The spec points at a file that no longer exists |
| Identifier-shaped names in `d:` still appear in the claimed files | **hint**, exit 0 | Something was renamed or removed — the description may now be describing nothing |

The hint level is right for the second: a `d:` might legitimately name a
concept that is not a symbol. But an identifier that used to exist and now
does not is the strongest cheap signal of drift there is, and it costs one
scan of the claimed files to get.

Use `--code-root` when the spec lives in a separate docs repository; it
defaults to the spec path itself for co-located specs.

## `--code-map` — is any code claimed twice?

```bash
elevator spec --code-map -q
```

An inverted index: each unique path referenced via `cr:`, listed with the
entities pointing at it. Paths claimed by two or more entities under the same
`cr` kind are marked `★ DUPLICATE`.

That mark usually means the spec has grown a duplicate — two differently
named entities describing the same physical code, which is how a domain map
starts lying about how many things exist. It is occasionally legitimate (a
Category and a Feature both claiming a folder is normal containment), so read
it rather than gating on it.

## `--stats` — one-glance size and health

```bash
elevator spec --stats -q
```

```
# Elevator stats
> 64 entities, 72 relationships

e   Extensions         0
c   Categories         7
f   Features          38
fu  Functionalities   18
@   Concepts           1
ui  UI Pages           0
```

Suitable for a CI status line. The shape tells you things: zero
Functionalities under a large Feature count means the spec is broad and
shallow — fine if deliberate (sketch-first), a gap if that Feature is where
all the work is happening.

## Reading the whole spec

```bash
elevator spec --list features        # flat, alphabetical, stable diffs
elevator spec --list --grouped       # under the full parent chain
elevator spec --root c.agent_tools   # one branch only
```

`--list` sorts by qualified name so consecutive runs produce stable diffs —
useful for committing a rendered snapshot and reviewing what a change did to
the map.

## Wiring it into CI

```bash
elevator spec --check -q || exit 1
elevator spec --drift --code-root . -q || exit 1
```

Both exit non-zero only on errors, so hints — orphans, unanchored identifiers
— stay visible in the log without failing the build. That is the right
default: a hint is a prompt to a human, and a build that fails on prompts
gets its check deleted.

## Limits

`--drift` verifies **anchors**, not meaning. A `d:` description that is
entirely wrong but names no identifier and points at a path that still exists
passes every check here. Only a reader comparing the description against the
`cr:` target catches that — which is what the **Spec claims** section of
`assess_change` prompts, at the moment the relevant change lands. See
[spec-first tasks](spec-first-tasks.md).
