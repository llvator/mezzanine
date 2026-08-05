# Nao

Interactive code visualizer that builds a single typed graph from heterogeneous source files. Per-language parsers are independent but feed a shared entity/relationship model.

## Language

### Code-graph terms

**Entity**:
A node in Nao's graph — a function, class, module, file, or any other source-level construct a parser surfaces. Identified by a stable string `id`; carries `kind`, `qualified_name`, `parent_id`, `span`, and per-entity `metrics`.
_Avoid_: Node, item, declaration.

**Relationship**:
A typed edge between two Entities (`source_id` → `target_id`). The `kind` (Contains, Calls, References, Imports, …) determines visual treatment in the renderer.
_Avoid_: Edge, link, dependency.

**LanguageParser**:
The trait every per-language parser implements. Returns a `ParseResult` of Entities, Relationships, Imports, and Warnings for one source file. Per ADR 0001, parsers stay independent — no shared scaffolding beyond the trait.

**Smell**:
A named anti-pattern signal (God Class, Dispatcher, Feature Envy, Shotgun Surgery, Data Bag) detected by combining metrics in a post-parse pass. Surfaced in the UI's Quality panel.

**Population**:
The set of Entities the Quality panel's numbers describe, chosen explicitly by the reader: the whole analysis scope, the scope tree selection, what the canvas is currently drawing (post-filter), the current selection, the file open in the editor, or the files a diff changed. One population governs the whole panel — summary, scatters, entity rows, and the file and module rollups — so two figures in it always count the same Entities. Distinct from **scope**, which decides what is loaded and analysed, and from the **Files** visual filter, which decides what the canvas draws and deliberately leaves the population alone (ADR 0010).
_Avoid_: Analysis scope (that is the tree), filter, selection.

### SQL schema

**Effective Schema**:
The tables and views a database actually has, as opposed to what any one file states. A migration repo never writes this down — it is the *result* of applying every change in order.
_Avoid_: The schema, final schema, DDL.

**Schema Source**:
Where a run's Effective Schema comes from — the repo's migration files, or a live database connection. Exactly one per run.
_Avoid_: Backend, provider, driver.

**Fold**:
The replay pass that applies each file's SchemaOps in filename order to produce the Effective Schema. The one stage in Nao where file order is load-bearing (ADR 0007).
_Avoid_: Merge, squash, compaction.

**Live Introspection**:
Reading the Effective Schema from a running database's catalog. Not a parser — it bypasses `LanguageParser`, the file walker and the parse store, and is a modifier on a repo analysis rather than a target of its own.
_Avoid_: DB parser, schema import, sync.

### Educator

**Educator**:
A Nao subsystem that surfaces language-specific good-practice guidance in the editor (VS Code hover on Java source, plus a sidebar mirror). Distinct from the **Smell** detector — the Educator teaches at the point of authorship; Smells flag patterns after the fact. Content lives in `content/<lang>/rules/` (in-repo for now, configurable path so it can be extracted later).
_Avoid_: Linter, doc tooltip, hint provider.

**Rule** (Educator Rule):
A single Educator content unit, one file per rule. Frontmatter declares `id`, `applies-to: [construct-kind …]`, optional `match:` predicate, severity, kind, sources. Body holds `Good` / `Bad` example pairs and the rationale. Same corpus is intended to feed the later analyzer.
_Avoid_: Check, lint rule, policy (these imply enforcement; Educator Rules teach first).

**Construct-kind**:
A per-language label a parser attaches to a span when it emits something rules can attach to. Includes syntactic kinds (`synchronized_statement`, `instanceof_expression`) and semantic-pattern kinds (`boxed-equality`, `closeable-without-try-with-resources`). Each construct instance carries an attribute map (e.g., `{ lock_expr_kind: this_expression }`) for predicates to query.
_Avoid_: Token, AST node, language feature.

**Match predicate**:
Optional declarative pattern on a Rule (`match:` field) that decides whether the rule fires *specifically* at a given construct instance. Expressed as structural attribute matching over the parser's attribute map (`eq`, `in`, `absent`, `present`). No Rust-handler escape hatch — adding a new primitive means extending the parser's emitted attributes.
_Avoid_: Detector, condition function, query.

**Specific vs General Rule**:
A Rule is **Specific** when it has a `match:` predicate — it fires only when the predicate matches a construct instance, and renders in the prominent panel of the hover/sidebar. A Rule is **General** when it has no predicate — it fires on every instance of an attached construct-kind, and renders below the fold. The distinction is purely about hover prominence; both kinds share the same schema and storage.
_Avoid_: Strict vs relaxed, hard vs soft.

### Elevator (`.elv`) — the spec language

Domain-level language that sits **above** code: it describes a project's user-facing capabilities so a new engineer can see the high-level shape before drilling into source. Onboarding metaphor — read it ground-floor up.

**Extension**:
The widest grouping above Category (`e` keyword). Optional — most specs use Categories at the top. Used when a project has separable bundles (plugins, product variants, installable packs) each owning its own Categories and scoped Concepts.
_Avoid_: Pack, module, plugin (in spec context — these may refer to runtime code, not the spec layer).

**Category**:
A top-level grouping (`c` keyword) — or one tier below an Extension when used. Behaviourless container — name + description + a list of child Features. Categories are the onboarding "ground floor" a reader sees first.
_Avoid_: Area, pillar, module, domain.

**Feature**:
A cohesive user-facing capability of the project (`f` keyword). Defined flat at the top level; Features sit under Categories via cross-reference, never via nesting. Noun-shaped (Protocol, Workflow, Conversation).
_Avoid_: Module, capability, area.

**Functionality**:
A verb on a Feature (`fu` keyword). Leaf node — Functionalities never have children. Cohesion test: if a node has multiple operations of its own, it's a Feature; if it's a single operation, it's a Functionality. The relationship to Feature mirrors method to class.
_Avoid_: Action, operation, sub-feature.

**Concept**:
Cross-cutting domain logic that several Features reference (`concept` keyword). Flat namespace — Concepts are not hierarchical. Use case: tax calculation, LLM invocation, anything that legitimately spans 3+ unrelated points in the Feature tree.
_Avoid_: Aspect, policy, rule, shared service.

**UI Page**:
A page in the user-facing UI (`ui` keyword). Targeted by `where:` edges from Features and Functionalities. Auto-created on first reference if not declared explicitly.
_Avoid_: Screen, view, route.

**Code reference** (`cr:`):
An authored path on a spec entity naming the code that implements it (`cr: "src/parser/"`, or `cr.<tag>:` to partition by layer). A declaration, not a derivation — nothing infers one, and a `cr:` pointing at nothing is reported as drift rather than quietly matched approximately.
_Avoid_: Link, binding, mapping (all imply something computed).

**Claim**:
What a `cr:` asserts over a path — "this entity is about that code". Resolved through ancestor folders, so a ref naming a directory claims the files inside it. A path claimed twice under the same `cr` tag is a deduplication signal; under different tags it is a shared resource seen from two layers.

### Split view

**Spec pane**:
The second canvas, drawing the Elevator layer on its own (ADR 0011). Distinct from the **code canvas**: it renders no metrics and no aggregation levels, so it keeps an emphasis channel the code canvas has already spent. Absent, not empty, in a project with no `.elv` files.
_Avoid_: The Elevator view, the left pane.

**Cross-filter**:
The narrowing the **Spec selection** applies to the code canvas — the union of `cr:` **Claims** over each selected entity's whole `Contains`-subtree, which is what makes a behaviourless **Category** clickable. Driven from two surfaces, the **Spec pane** and the Filters pane's Spec section, and dependent on neither being visible. A layer *over* the visual scope, never a write to it: clearing it restores the reader's scope untouched, including exclusions.
_Avoid_: Spec scope (suggests it reaches the scope store).

**Spec selection**:
The set of spec entities the **Cross-filter** runs from. A set, not one entity: the Filters pane offers each tier as independent checkboxes, and the filter takes the *union* of what they claim. Outlives the **Spec pane** — collapsing that pane asks for canvas room, not for filtering to stop.
_Avoid_: Focus, spec scope (the first suggests one entity, the second suggests it reaches the scope store).

**Drill path**:
What the **Spec pane** has *opened*, root first — pure view state, distinct from the **Spec selection**. The pane shows its roots plus each open step's children and nothing else, so a level exists on screen only once its parent has been opened. Climbing the path changes what is visible and never what is filtered.

**Scope state** (spec):
Why clicking an entity would show nothing, when it would. Four values, kept apart because they need four different fixes: `in-scope` (it works), `out-of-scope` (real code the analysis scope excludes — widen it), `unanalyzed` (nothing analysed at those paths), `unanchored` (no `cr:` in the subtree — nobody wrote it down). Telling the first two apart needs two path universes; against the scope alone, a dead path and a narrow scope are the same picture. `unanalyzed` deliberately stops short of claiming drift: the UI sees the parsed graph, not the filesystem, so a `cr:` at a file type outside the analysed languages is indistinguishable from one that died. `elevator --drift` is the check that can separate them.
_Avoid_: Broken, missing, invalid (all three flatten the distinction the states exist to make).

### Saved views

**Saved view**:
A named record of *what the canvas is drawing*, restorable in one click and stored per repo. Holds everything that decides which **Entities** and **Relationships** reach the canvas — the scope rules, the aggregation level, the kind / direction / language / file filters, the **Spec selection**, a committed search — and nothing that decides where the reader is standing: no camera, no selection, no open panes, no **Population**. Restoring writes the same stores the controls write, so a restored view is indistinguishable from having set it by hand and stays editable from there.
_Avoid_: Bookmark, preset, snapshot (the first two suggest defaults shipped with the tool; the third suggests it captures the data, which it never does — a view re-reads the current analysis and may find parts of itself gone).

### Canvas viewport

**Overview panel**:
The whole drawn graph, unzoomed, in a corner of the code canvas, with a **viewport box** marking the part on screen. It draws one circle per drawn **Entity** and nothing else — no labels, no **Relationships**, no folder outlines — and takes position, size and fill from the canvas's live encoding rather than a palette of its own, so it is a picture of that graph and not a second opinion about it. A control as well as a readout: clicking or dragging pans the canvas, and never zooms it.
_Avoid_: Minimap, thumbnail, bird's-eye (all three suggest a scaled screenshot; this is a separate drawing off the same encoding, and it deliberately shows less).

**Viewport box**:
The rectangle inside the **Overview panel** enclosing what the canvas is currently showing. Its size *is* the zoom level made visible, which is why the panel is fitted to the union of the graph and the viewport rather than to the graph alone — panning clear of the graph is when a reader is most lost, and a panel fitted to the nodes puts the answer off its own edge at exactly that moment.
_Avoid_: Camera, frame, window.

## Relationships

- An **Educator Rule** attaches to one or more **Construct-kinds** (via `applies-to`); the parser emits Construct-kinds with attribute maps that **Match predicates** query.
- A **Schema Source** yields exactly one **Effective Schema** per run. The two sources are mutually exclusive and never merge — comparing them is a diff between two runs, not two subgraphs in one.
- An **Effective Schema** holds tables and views, which enter the graph as **Entities** keyed `sql::table.<schema>.<name>` whichever **Schema Source** produced them.
- A **Category** contains one or more **Features** (via child references in its body)
- A **Feature** contains zero or more **Functionalities** and is contained by zero or more **Categories**
- A **Functionality** belongs to exactly one **Feature** and has no children; defined flat at the top level with a qualified name (`fu f.<feat>.<verb>`)
- A **Concept** is referenced by one or more **Features** or **Functionalities** (via `used_by:`)
- A **Feature** or **Functionality** appears on zero or more **UI Pages** (via `where:`)
- A **Feature** or **Functionality** may reference another **Feature** (via `references:`) for cross-links the hierarchy doesn't capture
- A **Saved view** holds one scope, one **Spec selection** and one set of visual filters; it never holds a **Population**, which the Quality panel chooses independently and which no view restores
- The **Overview panel** reports the camera a **Saved view** deliberately omits: the view decides *what* the canvas draws, the panel says *where in it* the reader is standing, and neither can answer the other's question
- Any spec entity declares zero or more **Code references**; the **Cross-filter** reads them rolled up over the `Contains`-subtree, while the reverse lookup ("who claims this file") reads only an entity's own — rolled up, every **Category** would claim most of the repo

## Syntax shape

Definitions are **flat** — every entity is defined at the top level. Bodies (`{}`) hold metadata fields and child references *by name*; they never contain nested definitions. The hierarchy is built entirely by cross-reference.

## Multi-file organisation — strict explicit imports

Elevator uses **scoped resolution with explicit imports**. A file can only reference entities defined in itself or in a file it explicitly `import`s (transitively). Ambient cross-file resolution is *not* supported — every cross-file dependency is a written contract.

### Import syntax

```
import "categories.elv"
import "library/protocol.elv"

c library { ... }
```

- Imports are path-based, relative to the importing file.
- Imports must precede every definition; the parser warns on misordering.
- Imports are **transitive**: importing a file pulls in everything *it* imports too. A "table of contents" file can re-export the rest of the project.
- Cycles are allowed (no execution semantics; the analyzer deduplicates).

### Resolution rules

- Same-file references always resolve.
- Cross-file references resolve only if the target's defining file is in the source file's transitive import scope.
- Out-of-scope references stay in the graph but are **tagged**: the relationship gets `metadata.unresolved=true, reason=out_of_scope`, the target's `parent_id` is *not* established (would silently re-enable ambient resolution), and a warning is emitted naming the missing import.
- Truly-unresolved references (entity not defined in any parsed file) get a synthetic stub entity tagged `unresolved` so the broken link is visible to the reader.

### Use vs definition

Definitions live at the top level. Bodies (`{}`) hold metadata fields and **uses** of other entities by name. A child reference inside a body (`f protocol { fu creation }`) is a *use*, not a declaration — it doesn't introduce the entity, it asserts containment between two already-defined endpoints. There is no separate declaration syntax; a top-level statement with no body (`fu f.protocol.creation`) is a minimal definition that serves the same role authors might call a "forward declaration."

### What the analyzer does post-merge

1. `validate_elevator_imports` — builds per-file scope = own definitions ∪ transitive imports; tags out-of-scope `Contains`/`References` edges.
2. `synthesise_unresolved_stubs` — creates a stub entity for any relationship target with no definition anywhere; tags `unresolved`.
3. `derive_parent_from_contains` — fills cross-file `parent_id` from `Contains` edges, *skipping* any tagged out-of-scope.

## Example dialogue

> **Engineer (new):** "I see `f workflows` at the top. What does `fu run` underneath it actually do?"
> **Engineer (owner):** "`fu run` is a **Functionality** — it's the verb 'execute the workflow'. The `where:` line tells you which **UI Page** triggers it. The `references: f.conversation` says workflow runs can be initiated from a conversation, which is the cross-link you'd never see in the hierarchy."
> **Engineer (new):** "And `concept llm_invocation` — why isn't that a sub-feature of workflows?"
> **Engineer (owner):** "Because it's pulled in by Library, Conversation, AND Workflows. Anything touching the model lives there. If we made it a sub-feature of one, the cross-cut would be invisible from the others."

## Flagged ambiguities

- **"Schema"** was used for two different things: the SQL namespace qualifier (`public` in `public.users`, what `split_name` returns) and the whole set of tables. Resolved: the namespace keeps the bare word **schema**, the set is the **Effective Schema**.
- **"Source"** for SQL meant both source *files* and the origin of a schema. Resolved: **Schema Source** always means the latter — migration files or a live database.
- **"Table"** as an **Entity** has no file, when it comes from **Live Introspection**. Resolved: it carries the `<database>` sentinel path, following the `<unresolved>` convention already used for synthesised stubs. `file_path` stays non-optional.
- **"Category"** was initially collapsed into **Feature** (depth in the tree was meant to encode abstraction). Reversed after looking at the rendered graph: top-level groupings benefit from a distinct visual treatment. **Category** is now its own entity kind — behaviourless, sits above Features in the hierarchy.
- **"Sub-feature"** was used to refer to a Feature nested under another. Resolved: there is no nesting in Elevator. Sub-features are just Features cross-referenced from a parent's body.
- **Top-level qualified names**: Functionalities are defined as `fu f.<feature>.<verb>` (qualified) because verbs like `creation` and `edition` repeat across Features. Categories, Features, Concepts are flat names — collisions there would be a real authoring problem and worth surfacing.
