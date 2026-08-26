# The docs graph

> What does the documentation cover, contradict, and no longer reach?

Mezzanine read every language in this repo except the one most of it was written
in: 368 markdown files against 175 Rust ones, carrying 670 links to each
other and 1,549 to source files. None of those were in the graph, so none of
the questions they answer were askable.

Markdown now joins as a **topology language** — one Note per file, no
tree-sitter, no complexity metrics. Which turns your documentation into
something you can look at.

## Turning it on

Opt-in, by design:

```bash
mezz watch . -l markdown -l rust
```

Every other extension Mezzanine claims belongs to source code, so finding one is
evidence you want it read. `.md` is not — claiming it by default would
rewrite every existing graph.

## How links resolve

Three forms, three mechanisms — worth knowing because they fail differently:

| Link form | Resolves | Failure mode |
| --- | --- | --- |
| Relative link (`[x](../adr/0001.md)`) | In the parser — both ends compute the same `md::note.<path>` and meet post-merge | Path typo → unresolved Note |
| Wikilink (`[[title]]`) | Post-merge, matched by title or filename stem once the corpus is known | Ambiguous or missing title → unresolved Note |
| Link to source (`[y](../../src/graph.rs)`) | Becomes a **`cr:` code ref**, not an edge | Path no longer exists → shows in drift |

A link to a note that does not exist stays pointing at its placeholder and
becomes an **unresolved Note** — Obsidian's ghost node, through the same stub
pass that already served Elevator.

Doc-to-code pairing is expressed as *scope* rather than as an edge (ADR 0005),
so notes ride the panel listing, the re-scope, the reverse lookup and the
drift check unchanged — the same machinery `.elv` already used.

## The four workflows

### 1. Broken links, enumerated

On this repo: **898 links, 28 broken** — mostly tickets that moved to `Done/`
and left their inbound links behind. That is a known documentation problem
with, for the first time, a way to list it exhaustively rather than
discovering entries one dead click at a time.

Run this after any bulk move or rename.

### 2. Staleness clusters

`GraphView.svelte` is described by **30 documents**.

That is not thorough documentation. That is thirty places to update when the
component changes, twenty-nine of which will not be updated. Finding the code
with the most inbound notes finds your highest-risk documentation debt — the
files where the docs are most likely to be quietly wrong.

### 3. Undocumented code

Code nodes with **zero** inbound `cr:` from any note. On this repo the docs
reach 931 code refs across 257 source paths — so the complement is the part
of the system nobody has written a word about.

Read it against `hotspots`: undocumented *and* churning is where a newcomer
will get hurt. Undocumented and stable is usually fine.

### 4. Orphan notes

Notes nothing links to. Either the document is unreachable — nobody will ever
find it — or it is genuinely a root, like a README. The unreachable ones are
either worth linking or worth deleting; leaving them is how a docs folder
becomes a place people stop looking.

## Reading it on the canvas

Notes carry no complexity metrics, so they render like Elevator entities do —
by kind rather than by magnitude. A dense cluster of notes around one source
path is workflow 2 in visual form; a note sitting alone at the edge of the
graph is workflow 4.

## Limits

- **One Note per file.** Headings and sections are not entities, so "which
  section of this doc describes that function" is not askable.
- Only links are read — prose that names a file without linking it does not
  become a ref.
- The 28 broken links on this repo are a real finding, tracked as `DOC-003`.
  Your own count is likely worse; documentation link rot is the default state.
