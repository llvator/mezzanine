# Elevator language guide

Elevator is a small spec language for describing a project's *user-facing capabilities* at the level above code. You write `.elv` files, Mezzanine parses them, and the analyzer turns them into a graph you can read top-down to onboard onto an unfamiliar project — categories at the ground floor, features one floor up, the verbs each feature supports as leaves.

This is the syntax reference. For domain terminology and design rationale, see [CONTEXT.md](../CONTEXT.md).

## Working method: sketch first, deepen where you work

Elevator is an abstraction layer, not a documentation language. Nothing
is required beyond an entity's name — no mandatory `d:`, `where:`, or
`cr:` — and that is deliberate: the spec is meant to grow along the
trail of actual work, not to be filled in upfront.

- **Sketch new areas as one line.** The moment you learn an area
  exists, add `e name { d: "one sentence" }` (or a bare `c`/`f`). A
  body-less definition is complete and valid; it gives the map breadth
  without claiming depth it doesn't have.
- **Deepen the branch you just worked on.** After implementing or
  changing something, add its Feature and `fu` verbs *then* — the
  knowledge is cheap the day you have it and expensive to reconstruct
  later. Per `fu`: one verb, one sentence of mechanism, one `cr:` to
  the exact file. Put concrete payload in `d:` (config keys, cron
  expressions, fallback behavior, the one trap) — pointers into config
  and data files (`impex`, YAML) are the ones code search can never
  reconstruct.
- **Don't deepen what you didn't touch**, and don't add
  sequence/state/actor pseudo-structure — detail at that level belongs
  in code or an ADR. If a feature would force authors to fill in more
  boxes per node, it doesn't belong in the spec.
- **Let the tools be the radar.** `--check` errors (broken references)
  must be fixed; its hints (orphan, empty, unused) are a *to-deepen
  list*, not a to-fix list — unfinished is a legitimate permanent
  state. `--code-map` flags the drift the abstraction is prone to:
  the same code claimed by two differently-named entities.
- **Consumers only need one branch deep.** `--focus` (and the mezz MCP
  `overview` tool) serve the entity being worked on; a stub extension
  elsewhere costs the reader nothing. Depth where work happens is also
  depth that stays fresh.
- **Split files when editing hurts, not before.** Start single-file.
  Move a category to its own file (`spec/<category>.elv` + a
  `concepts.elv`) when a *second* category gets deepened, or when a
  file passes ~100 lines — the points where concurrent editing and
  read-the-owning-file costs become real. Consumers never notice the
  layout (artifacts are rendered from the merged spec); only editors
  do. Structure follows work, like the spec itself.

## File basics

- File extension: `.elv`
- Whitespace and indentation are not significant
- `#` starts a line comment that runs to end-of-line
- Strings are double-quoted, single-line only
- Commas between siblings are optional

```
# This is a comment

c library {
    d: "Reusable building blocks."
    f protocol
    f template, f chain    # commas allowed
}
```

## Entity types

| Keyword | Type | Form | ID format |
|---------|------|------|-----------|
| `e` | **Extension** *(optional top tier)* | `e <name>` | `elevator::e.<name>` |
| `c` | **Category** | `c <name>` | `elevator::c.<name>` |
| `f` | **Feature** | `f <name>` | `elevator::f.<name>` |
| `fu` | **Functionality** | `fu <feature>.<verb>` (qualified) | `elevator::fu.<feature>.<verb>` |
| `concept` | **Concept** | `concept <name>` | `elevator::concept.<name>` |
| `ui` | **UI Page** | `ui <path>` | `elevator::ui.<path>` |

- **Extensions, Categories, Features, Concepts** have *flat* names (no dots).
- **Extensions are optional.** A project that doesn't need them keeps Categories at the top — that's the common case. Use Extensions when the project has clearly separable bundles (plugins, product variants, installable packs) each owning their own Categories.
- **Functionalities** must be *qualified* — they live under their parent Feature, and their names (like `creation`, `edition`, `run`) repeat across Features. `fu f.protocol.creation` and `fu f.template.creation` are different entities.
- **UI Pages** can use any dotted path; usually mirror your UI route structure (`ui.library.protocol`).

**Extension hierarchy:**

```
e my_extension {           # optional top tier
    c some_category {      # Categories inside the Extension
        f some_feature
    }
    concept some_cross_cut # Concepts can also live inside an Extension
}
```

An Extension's body can contain Categories and Concepts (the cross-cuts that are scoped to that Extension). Concepts can still be defined at top level when they cut across the whole project.

## Top-level definitions

Every entity is **defined exactly once at the top level**. Definitions never nest.

```
c library { ... }                 # Category
f protocol { ... }                # Feature
fu f.protocol.creation { ... }    # Functionality (qualified)
concept tax_calculation { ... }   # Concept
ui library.protocol               # UI Page (no body — name is enough)
```

Writing a body on a child reference is the one mistake this rule invites:

```
c library {
    f protocol {              # WRONG — `f protocol` here is a reference
        d: "Structured prompts."
    }
}
```

The parser warns and recovers by treating the inner block as the top-level `f protocol` it was meant to be, so the description lands on `protocol` and the `c library → f protocol` containment is still recorded. Move it out to silence the warning.

Bodies are optional. A body-less definition is the lightweight "this entity exists, no metadata" form:

```
f protocol            # valid — defines Feature `protocol` with no description, no children
f protocol {}         # also valid — empty body
f protocol {          # also valid — body with content
    d: "Structured prompts."
    fu creation
}
```

## Body contents

Inside a `{}` block, you can have:

### Description

```
f protocol {
    d: "Structured prompt for the LLM with enforced step sequence."
}
```

`d:` takes one double-quoted string. Maps to the entity's `documentation` field, shown in the UI's detail panel.

### Edge fields

```
fu f.protocol.creation {
    where: ui.library.protocol           # where in the UI this functionality lives
    references: f.conversation           # cross-link to another Feature
}

concept llm_invocation {
    used_by: f.protocol, f.template      # which Features pull this concept in
}
```

| Field | Edge direction | Target type | Allowed on |
|-------|----------------|-------------|------------|
| `where:` | source → UI Page | UI Page | Features, Functionalities |
| `references:` | source → Feature | Feature | Features, Functionalities |
| `used_by:` | each consumer → Concept | Feature | Concepts only |

Multiple targets are comma-separated: `where: ui.a, ui.b`.

The leading kind prefix on a target (`f.`, `ui.`, `concept.`) is optional. Without one, the field's default target type from the table above applies — `references: conversation` and `references: f.conversation` are equivalent. With one, it *overrides* the default, so `references: concept.tax_calculation` reaches the Concept rather than a Feature named `tax_calculation`.

### Code references (`cr:`)

A code-reference field — available on any entity kind — points at the file or folder in the codebase that implements the entity. The bridge between the abstraction and the concrete code:

```
f protocol {
    d: "Structured prompts."
    cr: "src/protocol/"
}

fu f.protocol.creation {
    cr: "src/protocol/builder.rs", "src/protocol/validator.rs"
}

c library {
    cr: "src/library/"
}
```

- **Quoted strings, mandatory.** Paths contain `/` and `.` which the qualname lexer treats as separators, so `cr` values must be quoted.
- **Comma-separated** for multiple paths (a Feature spanning several files).
- **Opaque to the parser.** No filesystem validation — specs often precede the code they describe, and projects use different conventions for what paths are relative to.
- **Allowed on every entity kind** (Category, Feature, Functionality, Concept, UI Page).

Surfaces in `elevator-text` artifacts as `[cr: path1, path2]`. Useful for an LLM working *on* a Functionality — it gets a direct pointer to the source file instead of having to grep.

#### Tagged code refs (`cr.<tag>:`)

For projects with multiple layers (frontend / backend / shared / mobile / infra / tests), use a `cr.<tag>:` prefix to partition refs:

```
fu f.protocol.creation {
    cr: "shared/protocol.proto"                  # generic
    cr.fe: "frontend/src/protocol.tsx"           # frontend layer
    cr.be: "backend/src/protocol/builder.rs"     # backend layer
    cr.test: "backend/tests/protocol_test.rs"    # custom tag
}
```

- The tag is **free-form**; the parser doesn't enforce a vocabulary. Conventional tags are `fe` (frontend), `be` (backend), `test`, `infra`, `mobile`, but any short identifier works.
- Each tagged variant is independent — `cr:`, `cr.fe:`, `cr.be:` can all coexist on the same entity.
- Surfaces in artifacts as separate bracket entries (`[cr.fe: ...; cr.be: ...]`), so an LLM consumer can pick out a single layer with a textual grep on the prefix instead of inferring from path conventions.

### Child references

A child reference inside a body declares **containment**: "this parent contains the named child." It does *not* introduce the child as an entity — that's the parent's body asserting structure, not defining its members.

```
c library {
    f protocol            # c library contains Feature protocol
    f template
    f chain
}

f protocol {
    fu creation           # f protocol contains Functionality creation
    fu edition
}
```

Children must be defined separately at the top level (in the same file or an imported one). If they're not, the analyzer creates an `unresolved`-tagged stub so the broken link is visible in the graph rather than silently dropped.

### Resolution rules for bare names

Inside a parent's body, bare references (no dots) are resolved using the parent's context:

| Inside parent | Bare child | Resolves to |
|---------------|-----------|-------------|
| `c X { ... }` | `f Y` | `elevator::f.Y` (flat — Features are always flat) |
| `f X { ... }` | `fu Y` | `elevator::fu.X.Y` (parent-qualified) |
| `f X { ... }` | `f Y` | `elevator::f.Y` (flat) |

Qualified references (with dots) are taken as-written:

```
f protocol {
    fu creation                           # → elevator::fu.protocol.creation
    fu f.template.creation                # → elevator::fu.template.creation (explicit)
}
```

## The use-vs-definition distinction

This is the most load-bearing concept in Elevator. **A name in a body is a *use*, not a definition.**

```
f protocol {
    fu creation     # USE — refers to fu.protocol.creation; doesn't define it
}

fu f.protocol.creation {     # DEFINITION — introduces the entity
    where: ui.library.protocol
}
```

Two consequences:

1. The same Feature can be referenced from multiple parents (e.g. one Feature listed in two Categories) without conflict — only the top-level statement is the definition.
2. If you write `f X` inside a body but never define `f X` at the top level (in any imported file), it stays as an `unresolved` stub.

The closest programming-language analogue is **struct field listing**: writing `struct Foo { bar: Bar }` doesn't define `Bar`, it just *names* `Bar` as a participant. `Bar` is defined elsewhere.

## Imports & scoping

Elevator uses **strict explicit imports**: a file can only reference entities defined in itself or in a file it explicitly imports (transitively).

```
import "features/protocol.elv"
import "features/template.elv"

c library {
    f protocol      # resolves only because protocol.elv was imported
    f template
}
```

- Imports are **path-based**, relative to the importing file.
- Imports must precede *every* definition in the file. The parser warns on misordering.
- Imports are **transitive**: importing a file pulls in its imports too.
- Cycles are allowed (no execution semantics).

### What happens when a reference is out-of-scope

Suppose `categories.elv` references `f protocol` but doesn't `import` the file that defines it:

- The Contains edge is still emitted, but tagged `metadata.unresolved=true, reason=out_of_scope`.
- The child's `parent_id` is *not* established (would silently re-enable ambient resolution).
- A warning is printed: `` `library` references `protocol` defined in foo.elv but bar.elv doesn't import it ``.

**Visual symptom:** the parent renders, the child renders, but they don't connect in the hierarchy. Fix it by adding the missing `import`.

### What happens when a reference has no definition anywhere

If `f never_defined` is referenced but no `.elv` file in the project defines it:

- A synthetic stub entity is created post-merge.
- The stub inherits `file_path` from the entity that referenced it (so it appears in the same file's tree branch in the UI).
- The stub is tagged `unresolved` so the broken link is visible.

## Multi-file organisation

Three patterns work well:

**1. Single file** — small projects, everything in one `.elv`:

```
my-project.elv
```

**2. Categories in one file, features in another:**

```
categories.elv               # all c definitions + child references
features/protocol.elv        # f protocol + its functionalities
features/template.elv
```

`categories.elv` imports each feature file.

**3. One file per category area:**

```
library.elv                  # c library + f protocol/template/chain + their fu
orchestration.elv            # c orchestration + f workflows + its fu
concepts.elv                 # cross-cutting concepts
```

Each file is self-contained or imports only what it cross-references.

## Complete example

```
# library.elv

import "../concepts.elv"

c library {
    d: "Reusable building blocks an author calls during a conversation."
    f protocol
    f template
    f chain
}

f protocol {
    d: "Structured prompt with enforced step sequence and typed responses."
    fu creation
    fu edition
    fu call_during_conversation
}

f template {
    d: "Reusable parameterised prompt fragment."
    fu creation
}

f chain {
    d: "Composed sequence of protocols and templates."
    fu creation
}

fu f.protocol.creation {
    where: ui.library.protocol
}
fu f.protocol.edition {
    where: ui.library.protocol
}
fu f.protocol.call_during_conversation {
    references: f.conversation
}

fu f.template.creation {
    where: ui.library.template
}

fu f.chain.creation {
    where: ui.library.chain
}
```

```
# concepts.elv

concept llm_invocation {
    d: "Any path that calls the language model."
    used_by: f.protocol, f.template
}
```

## What the parser warns about

- `fu <name>` at top level without a dot — Functionalities must be qualified
- `e <name>.<sub>` — Extensions must be bare names
- `c <name>.<sub>` — Categories must be bare names
- `f <name>.<sub>` — Features must be bare names
- `import` after a definition — imports must precede definitions
- `used_by:` outside a `concept` body — only valid inside Concepts
- Out-of-scope cross-file reference — file references an entity defined in a non-imported file
- Unrecognised top-level keyword — anything other than `import`, `e`, `c`, `f`, `fu`, `concept`, `ui`
- `cr:` value not a quoted string — paths must be wrapped in `"..."`
- Unterminated string literal — strings are single-line; the string is closed at end-of-line and parsing continues
- Unexpected character — skipped, and parsing continues
- A definition nested inside a body — definitions never nest; the inner block is parsed as the top-level definition it was meant to be, so its `d:`/`cr:` land on the child rather than on the enclosing entity
- The same entity defined twice in one file — the first definition wins; both bodies' child references are kept
- Duplicate `d:` in one body, an empty `where:`/`references:`/`used_by:` list, or a bare `fu` child in a non-Feature body

Every warning carries `at line L column C`. **No warning is fatal to the file**: a typo costs one token or one definition, never the remaining entities. Run `--check` to surface them all at once.

## CLI

There are two binaries; pick whichever fits the workflow:

```bash
elevator ./my-spec                      # standalone, focused on `.elv` work
mezz analyze ./my-spec -l elevator …     # full Mezzanine surface; same engine
```

`elevator` is the recommended one for spec work — it implies `--language elevator` and the `elevator-text` output format, so the command line stays short. Internally it's the same parser + analyzer + renderer pipeline.

### Modes

```bash
elevator ./my-spec                              # full project map
elevator ./my-spec --root library               # subtree at the named entity
elevator ./my-spec --focus fu.protocol.creation # context bundle for one entity
elevator ./my-spec --check                      # run the spec checker (no artifact)
elevator ./my-spec --code-map                   # inverted index: code paths → entities
elevator ./my-spec --drift --code-root ../repo  # spec-vs-code drift: cr paths + identifier anchors
elevator ./my-spec --drift --fix                # rewrite the cr paths git can prove moved
elevator ./my-spec --list                       # enumerate every entity, grouped by kind
elevator ./my-spec --stats                      # compact count table per kind
elevator ./my-spec --extract f.protocol         # carve one slice back out as `.elv` source
elevator ./my-spec -o spec.txt                  # write to file instead of stdout
elevator ./my-spec --no-legend                  # suppress the format-key preamble
elevator --docs                                 # print this language guide and exit
```

The same flags work on `mezz analyze -f elevator-text`.

### `--code-map` (inverted index by code path)

Walks every entity's `cr:` / `cr.<tag>:` attributes and emits a *path → entities* listing. Designed to surface a specific class of mistake the abstraction layer is prone to: **the same physical code referenced by two differently-named spec entities**.

Because Elevator entity names are abstract (you pick them), nothing prevents `f protocol` and `f new_protocol` from both pointing at `src/protocol/builder.rs`. They're almost certainly the same thing named twice — but you'd never notice from reading the spec top-down.

The output groups every `cr` reference by code path:

```
# Code reference map (Elevator)
> 5 path(s), 7 reference(s), 1 potential duplicate(s)

frontend/protocol.tsx
  cr.fe ← f protocol

src/protocol/builder.rs   ★ DUPLICATE
  cr.be ← f new_protocol
  cr.be ← f protocol

src/shared/utils.rs
  cr ← f shared_helper
  cr.fe ← f another_helper

# Potential duplicates
src/protocol/builder.rs
  [cr.be]
    - f new_protocol
    - f protocol
```

**Duplicate criteria:**

- ★ flagged: same path, **same** `cr` kind, multiple entities. Strong signal — the spec almost certainly has a redundant entity.
- *not* flagged: same path, **different** `cr` kinds (e.g. one entity says `cr.fe`, another says `cr.be`). That's how shared resources legitimately get referenced from multiple layers; warning on it would be noise.

The artifact is also useful as LLM context — "here's how the spec maps to disk" is a precise, compact summary an agent can use to understand which spec entities cover which code areas.

### `--drift` (spec-vs-code anchor check)

The staleness radar. The spec's semantic payload (`d:` descriptions,
`cr:` paths) is authored, not derived — code can move under it. `--drift`
verifies the two anchors mechanically:

1. **`cr:` resolution** — every referenced path must exist under the
   code root. Missing path = the code moved or died; fix the `cr:` or
   retire the entity. *Error, exit 1.*
2. **Identifier grounding** — identifier-shaped tokens in `d:`
   descriptions (CamelCase names, `Class.member` forms, dotted config
   keys, `UPPER_SNAKE` job codes) are searched in the text of the files
   under the spec's resolved `cr:` paths. A token found nowhere is a
   rename/delete suspect. *Hint, exit 0.*

```bash
elevator ./my-spec --drift                        # specs co-located with code
elevator ./my-spec --drift --code-root ~/repo     # specs in a separate docs repo
elevator ./my-spec --drift --fix                  # apply the moves git recorded
```

#### `--fix` — apply the moves the repository already recorded

Most dead `cr:` paths died the same way: someone moved the file. Git
recorded that, so re-typing the new path by hand is work the repository
can do. `--drift --fix` rewrites those paths in place.

A path is rewritten only when **both** gates pass:

1. `git log -M --diff-filter=R` names the commit in which the old path
   became a new one, and
2. the new path exists on disk — the same check `--drift` runs, so a
   repair can never introduce the drift it was meant to remove.

Chains are followed (`A → B → C` rewrites straight to `C`). Directory
refs — the usual shape — are fixed only when every file that left the
old directory in that commit landed under one common new directory; a
directory whose contents scattered is reported, not rewritten. The edit
is textual and bounded by the entity's own definition, so comments,
blank lines and the other paths on a `cr: "a", "b"` line are untouched.

Everything else is left for a human, and that is deliberate. A `cr:`
is a declaration, not a derivation; approximate matching would make the
anchor a guess and there would be nothing left to trust. Where a dead
file path has exactly one same-named file left in the tree, the report
names it as a **candidate** — never applied, never counted as repaired,
and not enough to change the exit code. The identifier hints have no
mechanical fix at all: rewriting a `d:` means rewriting prose.

Without `--fix`, the report names each rename it found and the commit it
came from, so you can see exactly what would change before it does.
`--fix` writes `.elv` files under the spec path only; the code root is
read for evidence and never touched. Outside a git repository it
degrades to the plain report with a note.

`--code-root` is the directory `cr:` paths are relative to; it defaults
to the spec path. Grounding scans file *text*, not the parsed graph, so
XML/properties/impex targets anchor identifiers just as well as source
files.

What `--drift` deliberately does **not** do: compare the spec's shape to
the code's shape. The abstraction is not a projection of the code tree —
a `fu` whose `cr:` points into a different module is the value the spec
adds, not drift. Hints are a review queue, not errors: most code changes
don't move the abstraction, and an unanchored token just means "verify
this description against its `cr:` target."

Consequence for authors: identifiers you name in `d:` become verifiable
anchors. Naming the real class, method, and config key (rather than a
paraphrase) is what makes your description checkable next year.

### `--list [KIND]` (enumerate entities by kind)

Lists Elevator entities grouped by kind (Categories, Features, Functionalities, Concepts, UI Pages), sorted alphabetically within each group. The optional `KIND` argument restricts the output to one kind.

```bash
elevator . --list                  # every entity, grouped
elevator . --list features         # Features only
elevator . --list f                # short form, same result
elevator . --list fu               # Functionalities only
elevator . --list concept          # Concepts only
```

**Accepted KIND values** (case-insensitive, multiple aliases per kind):

| Kind | Aliases |
|------|---------|
| (all) | `all` (default if no argument) |
| Extension | `e`, `extension`, `extensions` |
| Category | `c`, `category`, `categories` |
| Feature | `f`, `feature`, `features` |
| Functionality | `fu`, `functionality`, `functionalities` |
| Concept | `concept`, `concepts` |
| UI Page | `ui`, `ui-page`, `ui-pages`, `pages` |

An unrecognised filter is a hard error with a list of accepted aliases (exit code 2) — useful in CI so a typo fails loudly.

Designed for two use cases:

1. **Tracking what exists** — quick "what features do we have?" answer without scanning the spec.
2. **Grepping further** — the per-line format is `  <marker> <qualified_name> [tag]`:
   ```bash
   elevator . --list f | grep UNRESOLVED        # broken-link Features only
   diff <(elevator . --list) <(elevator . --list)   # spec drift across runs
   ```

Stable sort within each kind means consecutive runs produce diff-friendly output.

**`--grouped`** rearranges the listing so entities are nested under their full parent chain — Features under their Category, Functionalities under `Category / Feature`:

```bash
elevator . --list f --grouped
```

```
## Features grouped by Category (12)
c conversation
  f conversation_thread
  f skills
c library
  f chain
  f protocol
  f template
c orchestration
  f workflows
...
```

```bash
elevator . --list fu --grouped
```

```
## Functionalities grouped by parent path (19)
c library / f protocol
  fu call_during_conversation
  fu creation
  fu edition
c library / f template
  fu call_during_conversation
  fu creation
c orchestration / f workflows
  fu creation
  fu run
...
```

Grouping is a no-op for kinds without parents — Categories, Concepts, and UI Pages render the same with or without `--grouped`. Entities whose `parent_id` chain is empty (e.g. unresolved-stub Features) appear in an `(orphan / unparented)` bucket at the bottom; usually a sign that a Feature was referenced but never listed under any Category.

### `--stats` (count table)

Compact one-glance summary: total entities, total relationships, and per-kind counts with unresolved entities flagged. Suitable for CI status checks or for piping into a status dashboard.

```
# Elevator stats
> 42 entities, 60 relationships (11 unresolved)

e   Extensions         2
c   Categories         5
f   Features          12  (1 unresolved)
fu  Functionalities   19  (10 unresolved)
@   Concepts           2
ui  UI Pages           4
```

The `e Extensions` row only appears when at least one Extension is defined.

### `--extract` (carve a slice back into `.elv` source)

Every other mode renders *away* from the language — the output is for
reading, not re-parsing. `--extract` renders *back into* it: it emits a
standalone `.elv` file containing only the entities you name, and writes
it wherever you point `-o` (creating the folder if it doesn't exist).

```bash
elevator . --extract f.spec_health -o work/2026-07-29/slice.elv
elevator . --extract fu.f.spec_health.drift,f.text_artifacts   # to stdout
```

The motivating use is **agent observability**. An agent that finishes a
feature has already deepened the shared spec; extracting the branch it
touched into its own folder leaves a small, diffable record of what it
claimed to work on — one that survives after the shared spec has moved
on. Snapshots accumulate as history; `diff` between two of them shows
how one area's abstraction changed over time.

What lands in a slice:

| | |
|---|---|
| **Members** | The selected entities plus every `Contains` descendant. Selecting a Feature brings its Functionalities — they're part of that Feature's scope. |
| **Concepts** | Any Concept a member uses, emitted whole, with `used_by:` pruned to the members that pulled it in. |
| **Ancestors** | Emitted with `d:` / `cr:` and a child list **pruned to the slice**, trailed by a `# context` comment. Enough to show where the slice sits; not a claim about their own full contents. |
| **Name-only** | The far end of a `references:` / `where:` edge that leaves the slice — emitted bodyless so the edge still resolves. The definition stays in the source spec. |

Inbound edges from non-members are dropped: a slice records what was
touched, not everything pointing at it. Containment ancestors are the
deliberate exception, because a Feature with no Category above it has
lost the only thing saying where it lives.

An edge whose target the source spec never defines is **dropped and
counted in the header**, not emitted as a name-only entry. Emitting one
would repair the dangling reference inside the slice — the snapshot
would look healthier than the spec it came from.

Every slice is a real `.elv` file: it re-parses, and `--check` passes on
it standalone. The header records where it came from and what was asked
for, so a snapshot is self-describing:

```
# Elevator slice — a generated subset of a spec.
#
# Source:    spec/
# Selection: f.spec_health
# Slice:     1 feature, 3 functionalities
# Context:   1 ancestor(s), child lists pruned to this slice
#
# Entities trailed by `context` are ancestors: they place the slice in
# the hierarchy and do not describe their own full contents.

c elevator {    # context
    d: "The .elv spec language: a domain abstraction above the code…"
    f spec_health
}

f spec_health {
    d: "Correctness signals for a spec: checker, code-map, drift."
    fu f.spec_health.check
    fu f.spec_health.code_map
    fu f.spec_health.drift
}

fu f.spec_health.drift {
    d: "Anchor verification against --code-root…"
    cr: "src/output/elevator_drift.rs"
}
```

Selectors accept the same three forms as `--root` / `--focus` (see
below), comma- or space-separated. A selector that matches nothing is an
error with did-you-mean suggestions, not an empty slice.

Agents reach the same machinery through the `spec_slice` MCP tool, which
selects by **code path** instead of by name — "what does the spec claim
about `src/mcp`?" — using the `cr:` index, and can write the result
straight into a ticket folder. See
[docs/agents/mcp-server.md](agents/mcp-server.md).

### `--docs` (language reference)

Prints the full Elevator language guide (this document) and exits. The text is **baked into the binary** at compile time — no separate docs install, no version skew between the language and the reference.

Two main use cases:

1. **Quick human reference** — `elevator --docs | less`, or pipe to your editor.
2. **Bootstrap context for an LLM** that's going to write `.elv` files for an existing codebase:

   ```bash
   {
     echo "Map this codebase as Elevator (.elv) files. Reference:"
     elevator --docs
     echo "---"
     echo "Codebase tree:"
     find ./src -type f | head -50
   } | claude   # or paste into your LLM session
   ```

   The LLM gets the full grammar, semantics, examples, and conventions in one shot. It can then emit `.elv` files matching your codebase structure — `cr.fe:` and `cr.be:` paths included.

### Entity resolution for `--root` and `--focus`

Both flags accept three forms — pick whichever is shortest:

| Form | Example | Notes |
|------|---------|-------|
| Bare name | `protocol` | Searched against `qualified_name`/`name`; ambiguity broken Category > Feature > Concept > Functionality > UI Page |
| Kind-prefixed qualname | `c.library`, `f.protocol`, `fu.protocol.creation` | Direct lookup |
| Full ID | `elevator::c.library` | What's stored internally; useful for scripting |

### `--root` (subtree)

Renders only the named entity and its descendants. Concepts that touch the subtree are still included; everything else is omitted.

### `--focus` (LLM context bundle)

The artifact a *coding-LLM session* needs when working on a specific entity. Contains:

1. **Path** — ancestor chain from a root Category down to the target, target marked `← target`
2. **Target subtree** — full descendant tree of the target (so focusing a Feature shows all its Functionalities)
3. **Siblings** — other entities at the target's level (so the LLM knows what else exists, doesn't duplicate work)
4. **Cross-cutting concepts** — any Concept whose `used_by:` reaches into the path or subtree

Example for `--focus fu.protocol.creation` on a small spec — under 100 tokens, fits comfortably at the top of an LLM prompt:

```
# Focus: fu protocol.creation
> ancestors, siblings, target subtree, and cross-cutting concepts

c library — Reusable building blocks an author calls during a conversation.
  f protocol — Structured prompt for the LLM with enforced step sequence.
    fu creation [where: ui.library.protocol; cr.be: backend/protocol/builder.rs]   ← target

# Siblings (other children of f.protocol)
    fu edition [where: ui.library.protocol; cr.be: backend/protocol/editor.rs]

# Cross-cutting concepts touching this path
@ llm_invocation — Any path that calls the language model.
  used by: f.protocol, f.template
```

### `--check` (spec checker)

Runs five rules against the merged spec and reports findings. Errors exit `1`; hints exit `0`. Use it in a pre-commit hook or CI:

| Rule | Severity | Catches |
|------|----------|---------|
| `parse:` | error | Lex / parse failures, out-of-scope cross-file references |
| `unresolved:` | error | An entity is referenced by name but never defined anywhere |
| `orphan:` | hint | A Feature is defined but no Category lists it |
| `empty:` | hint | A Category has no Feature children |
| `unused:` | hint | A Concept has no `used_by:` consumers |

Deliberately *not* checked (would force documentation overhead): required `d:`, required `where:`, required `cr:`, prescriptive tag vocabularies. The checker catches mistakes; it doesn't enforce style.

```bash
$ elevator ./my-spec --check
error: parse: ./my-spec/categories.elv: elevator: unterminated string at line 6 column 12
hint: orphan: f templates is defined but not listed under any Category
1 error(s), 1 hint(s).
$ echo $?
1
```

### Artifact format

Every text artifact starts with a six-line legend (suppressible with `--no-legend`) so a fresh-context LLM can interpret the markers without prior exposure to the language. Its final line is a trust calibration for consumers: structure (hierarchy, references) is analyzer-checked, but descriptions and `cr:` paths are authored, not derived from code — an LLM consumer is told to treat them as strong leads, not ground truth, and to verify mechanism details against the `cr:` target before relying on them. The body is one entity per line, indentation = containment, with bracketed metadata.

**Markers:**

| Marker | Kind |
|--------|------|
| `e` | Extension |
| `c` | Category |
| `f` | Feature |
| `fu` | Functionality |
| `@` | Concept |
| `ui` | UI Page |

**Bracketed metadata (between `[` and `]`, semi-colon separated):**

| Bracket | Meaning |
|---------|---------|
| `[where: ui.X]` | Entity surfaces at UI page `X` |
| `[refs: f.X]` | Entity cross-references Feature `X` |
| `[cr: PATH]` | Generic code reference |
| `[cr.<tag>: PATH]` | Tagged code reference (e.g. `cr.fe`, `cr.be`) |
| `[UNRESOLVED]` | Entity has no definition (broken link) |
| `[UNRESOLVED — out-of-scope: missing import]` | Reference target lives in a file the source didn't import |

**Tail markers (focus mode only):**

| Marker | Meaning |
|--------|---------|
| `← target` | The entity the artifact is centred on |

### Watch mode + VS Code

```bash
mezz watch ./my-spec --port 3200
```

The VS Code extension picks `.elv` files up automatically and re-analyzes on save. Multi-file specs render whole: selecting any `.elv` file in the Visual Scopes panel pulls in the transitive containment descendants of its entities (and any attached Concepts) even when they are defined in other files — a Category never renders without the Features it declares elsewhere. Ancestors are not pulled in, so scoping to one feature file still narrows the view to that branch.

### Installation

```bash
# From the repo root — installs both `mezz` and `elevator` to ~/.cargo/bin/
cargo install --path . --force
```

`--force` here is `cargo install`'s "replace existing" flag — nothing destructive.
