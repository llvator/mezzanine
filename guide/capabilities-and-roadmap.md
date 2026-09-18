# Mezzanine: capabilities and roadmap

What Mezzanine does today, how its tools are organized, why they make human
coders and coding agents more effective, and where it could go next.

Mezzanine builds a single typed graph — Entities and Relationships with
per-entity metrics — from heterogeneous source files. Seventeen languages have
dedicated parsers (Rust, Python, TypeScript, JavaScript, Svelte, Java, Go, Kotlin,
Dart, Groovy, C++/C, Impex, Ansible, SQL, Docker, Markdown, Elevator specs);
everything else falls back to a generic parser with reduced fidelity. See the README for what each tier gives you.
Everything below is a different way of asking that graph a question.

## The altitude Mezzanine occupies

Existing tooling clusters at two extremes. LSP is **precise but
pointwise**: exact go-to-definition for one symbol at a time.
Grep/reading is **flexible but structureless**: text, not entities.
Documentation is **high-level but stale**. Mezzanine sits in the unoccupied
middle: **holistic, structural, always derived from the code as it is**.
That mid-altitude view is precisely what doesn't fit in an agent's
context window and what a newcomer lacks for their first weeks.

## What exists today, by role

Four roles cover the surface. Each role answers one kind of question.

### 1. Cartographer — "what is the shape of this code?"

| Surface | What it gives |
| --- | --- |
| MCP `map` | Files + entities with complexity/coupling annotations, 3 depth levels, single-file capable |
| MCP `trace` | Shortest dependency path(s) A→B with the relationship kind of every hop |
| UI visualizer + VS Code extension | Interactive force-directed graph, diff overlay, scope drill-down |
| Elevator (`.elv`) | Domain-level shape — Categories, Features, Functionalities — above the code entirely |
| MCP `overview` | The Elevator domain map (or a `focus` context bundle) as an agent tool call — the onboarding step before `map` |

### 2. Auditor — "where does this code hurt?"

| Surface | What it gives |
| --- | --- |
| MCP `quality` | Smells (God Class, Dispatcher, Feature Envy, Shotgun Surgery, Data Bag, Overfull Head), top offenders by composite refactor pressure, dependency cycles |
| MCP `hotspots` | Git churn × complexity — risk ranking that complexity alone cannot give (complex-but-stable ranks low) |
| Metrics engine | Cyclomatic, cognitive, nesting, fan-in/out, instability, WMC, chain depth, PageRank, composite score |
| Elevator spec health | `--check` (broken refs, orphans), `--code-map` (same code claimed twice), `--drift` (cr paths that no longer resolve, identifiers in `d:` no longer found in the claimed code), `--drift --fix` (rewrites the cr paths git recorded a rename for) |

### 3. Impact analyst — "what happens if I touch this?"

| Surface | What it gives |
| --- | --- |
| MCP `impact` | Blast radius of one entity: what it uses, direct dependents, transitive dependents level by level — exact at the type level via `UsesType` edges (Rust, TypeScript/Svelte, Java, Go, Kotlin, Dart, Python) |
| MCP `tests_for` | Which tests reach an entity (direct or transitive), including Rust inline `mod tests` |
| MCP `assess_change` | Working tree vs a git ref: per-entity metric deltas, added/removed entities, smell churn |
| `mezz diff` / UI diff overlay | The same change intelligence, visually |

### 4. Implementation support — "help me write this"

| Surface | What it gives |
| --- | --- |
| MCP `context` | Minimal edit pack: target's source + signatures of everything it uses / everything using it — one call instead of five file reads |
| MCP `similar` | Does-this-already-exist: similarity-ranked search before implementing something new |
| Educator | Language-specific good-practice rules and lessons at the point of authorship (VS Code hover + sidebar) |

## Why this makes coders more effective

- **Onboarding compresses.** Elevator gives the domain shape, `map` the
  code shape, the UI the connections — the "first two weeks of reading
  around" becomes hours. The Elevator layer is built *sketch-first*:
  breadth everywhere (every extension named in one line), depth only
  where work has actually happened — so it stays cheap to maintain and
  fresh where it matters.
- **Refactoring gets a target list.** `hotspots` says *where it pays*
  (churn × complexity), `quality` says *what's wrong there*, `impact`
  says *what the change will touch*, `tests_for` says *how to verify it*.
- **Review gets objective ground.** `assess_change` turns "this feels
  more complex" into "cognitive complexity 8→19, new Dispatcher smell."
- **Teaching happens in the editor.** The Educator surfaces the rule at
  the moment the construct is written, not in a wiki nobody reads.

## Why this makes coding agents more effective

Agents have a specific economics: every token of context costs, and the
global view never fits. Mezzanine's MCP tools are built for that economics —
compact ranked text, capped with explicit truncation, filtered of noise
(ghosts, parameters, imports), every tool `readOnlyHint: true`.

A typical agent loop, each step one tool call:

1. **Ground** — `overview` for the domain shape when the project has an
   Elevator spec: which Feature the task belongs to, and the `cr:`
   pointers into the code (skipped when no `.elv` layer exists).
2. **Orient** — `map` the folder instead of reading ten files.
3. **Locate** — `similar` before writing a helper that already exists.
4. **Prepare** — `context` for the edit target: source + neighbor
   signatures, no file-hopping.
5. **Check reach** — `impact` for who breaks; `trace` for how the flow
   arrives here.
6. **Edit** — with LSP or plain edits; Mezzanine doesn't replace the precise
   pointwise layer.
7. **Verify** — `tests_for` to run the relevant tests, not the suite.
8. **Self-review** — `assess_change` before presenting: metric deltas
   and smell churn on the actual diff.

Two properties make the loop trustworthy: analysis is **deterministic**
(identical trees → byte-identical graphs, so every reported delta is
real signal — AN-002), and type usage is **exact** where parsers emit
`UsesType` edges from signatures and fields — Rust (RS-001),
TypeScript/Svelte (TS-001), Java (JV-001), Go, Kotlin (KT-001), Dart and
Python (PY-025).

Call edges are the other half of that loop, and they are *measured* rather
than assumed: 93.7% recall at 98.6% precision against a rust-analyzer oracle
on this repo (AN-005). Good enough to act on, not good enough to treat an
empty result as proof — see the limitations below.

## Honest limitations

- Call resolution is **name-based and locality-ranked, not type-resolved**.
  Measured against rust-analyzer as an oracle (AN-005) on this repo — Rust,
  tests included, 3649 call edges the oracle resolves exactly:

  | | |
  |---|---|
  | Recall (real dependents found) | **93.7%** |
  | Precision (of those found, correct) | **98.6%** |

  Read that as: **an empty `impact` result is strong evidence, not proof.**
  Roughly one real dependent in sixteen is still missing, so `Used by (0)` on
  an entity you expect to be called is a reason to check, not an all-clear
  for a breaking change. `impact` now says as much in its own output rather
  than printing a bare zero, and lists the unresolved references that share
  the entity's name as *possible* dependents (MCP-013). Re-measure with
  [`scripts/call_edge_recall.py`](../scripts/call_edge_recall.py).
- **That measurement is one corpus in one language.** The figures above are
  Rust-only, on mezz itself, because rust-analyzer is the only oracle wired up.
  The other parsers are unmeasured — assume they are worse, not equal.
- What is still missed, and why: closure parameters (`|e| e.kind.is_callable()`,
  whose type is the iterator's item type), locals bound to non-constructor
  expressions (`let c = spawn()?`), and chained calls (`a().b()`) — all three
  need return-type inference, which nothing currently does. Field types for
  structs declared in *other* files used to lead this list; AN-012 resolves
  those against the whole tree's structs. What is still *mis*targeted is a long tail of same-directory
  same-name ambiguity — locality prefers the nearest candidate, which is not
  the same as the one actually in scope. One class of mistarget is gone:
  names no longer bind across language families, so a TypeScript type cannot
  resolve to a Rust struct that happens to share its name (AN-014). Families
  that *do* share an entity space — Java/Groovy/Kotlin/Impex,
  TypeScript/JavaScript/Svelte — still resolve into each other.
- Type usage is the exact layer, and only where `UsesType` is emitted: Rust,
  TypeScript/Svelte, Java, Go, Kotlin, Dart, Python, Groovy. Impex, Ansible and the
  generic fallback have no type edges, so `impact` there is the "used via
  members" approximation. Groovy's edges only reach what the code declares —
  a `def` is the absence of a type, not a type, so dynamically-typed members
  contribute nothing (GR-015).
- A Svelte component's **markup** resolves at component granularity. An
  imported name used inside `{…}` produces an `Interpolates` edge from the
  Component (SV-002) — enough for `impact` to find the consumer, not enough
  to say which member of the component wrote it. Names the script declares
  itself are left to the existing `Contains` edge, and references in markup
  *text* are not read at all: only brace expressions are scanned.
- First MCP call after a source change pays a full re-analysis (seconds,
  growing with repo size); subsequent calls are served from the warm
  cache in milliseconds.
- `similar` v1 is identifier/signature-based — it finds *namable*
  duplication, not structural clones.
- Elevator's semantic payload (`d:` descriptions, `cr:` paths) is
  authored, not derived — the one layer that can lag the code. The
  artifact legend says so to consumers ("strong leads, not ground
  truth"), and `--drift` verifies the anchors (paths resolve,
  named identifiers still exist in the claimed files) — but a
  description that is wrong without naming anything checkable can
  only be caught by a human or agent reading the `cr:` target.

## Potential enhancements

Roughly ordered by leverage-to-effort within each group.

**Deepen what exists**
- ~~`UsesType` for TypeScript, Java, Python, Kotlin, Groovy~~ — shipped
  (TS-001, JV-001, KT-001, PY-025, GR-014's sibling pass): type-level impact
  is exact in every language with a typed parser. Go has no dedicated parser
  at all — `tree-sitter-go` is an unused dependency.
- ~~Call-edge recall and precision~~ — measured and raised (AN-005 built the
  benchmark; AN-006/009/010/011/012 took recall 66.5% → 93.7% and precision
  80.7% → 98.6%). Remaining: return-type inference (closure parameters,
  non-constructor bindings, chained-call receivers — one gap wearing three
  hats), and import-scope resolution for the residual same-name ambiguity.
- ~~Warm MCP server~~ — shipped (MCP-006): graphs cached per path with
  watcher-driven invalidation; repeat calls are milliseconds. The
  remaining step-up is *incremental* re-analysis (re-parse only changed
  files instead of the whole tree on invalidation).
- `similar` v2: structural/body similarity (token shingles or AST
  shapes) to catch clones that share no identifier vocabulary.
- Smell-threshold calibration per repo (percentile-based rather than
  absolute constants).

**New questions to the graph**
- Dead-code tool: fan-in 0 entities that are neither entry points nor
  public API.
- Ownership overlay on `hotspots`: churn × complexity × bus-factor
  (git blame concentration).
- Real coverage joins: ingest `llvm-cov`/`coverage.py` reports so
  `tests_for` reports measured, not inferred, coverage.
- Layering/dependency-rule checks: declare intended layer order, flag
  edges that violate it (the graph already has every edge).

**New surfaces**
- Educator over MCP: an `educate` tool so agents get rule feedback on
  freshly written code, closing the authorship loop the hover provides
  for humans.
- ~~Elevator over MCP~~ — shipped: the `overview` tool renders the
  domain map (or a `focus` context bundle) from the project's `.elv`
  specs, giving agents the onboarding ground floor before the code map.
  Next step-up: joining `cr:` paths into the code graph so the bundle
  links spec entities to the code entities that implement them.
- ~~Elevator drift detection~~ — shipped: `elevator --drift
  [--code-root]` verifies the spec's anchors against the code — every
  `cr:` path resolves (error), every identifier named in `d:` still
  appears in the claimed files (hint; the rename/delete signal).
  `--drift --fix` then repairs the subset the repository can prove:
  a `cr:` path whose move git recorded, and whose new path exists, is
  rewritten in place. Deliberately not extended to guesses — a dead
  path with a single same-named file elsewhere is offered as a
  candidate to confirm, never applied, because a `cr:` that can be
  inferred is no longer an anchor worth trusting.
  Next step-up: surfacing the same claimed-by join inside
  `assess_change`, so an agent self-reviewing a diff is told which
  spec entities its change touches.
- PR mode: `assess_change` rendered as a review comment (CI posts the
  metric deltas + smell churn on every PR).
- Service level: cross-repo graphs for multi-service systems — the
  `Service` entity kind exists; nothing populates it yet.

The bar for any of these, per the project's standing principle: the
value must come from what the code already says — no feature may force
authors to annotate or fill in detail by hand.
