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

## Relationships

- An **Educator Rule** attaches to one or more **Construct-kinds** (via `applies-to`); the parser emits Construct-kinds with attribute maps that **Match predicates** query.
- A **Category** contains one or more **Features** (via child references in its body)
- A **Feature** contains zero or more **Functionalities** and is contained by zero or more **Categories**
- A **Functionality** belongs to exactly one **Feature** and has no children; defined flat at the top level with a qualified name (`fu f.<feat>.<verb>`)
- A **Concept** is referenced by one or more **Features** or **Functionalities** (via `used_by:`)
- A **Feature** or **Functionality** appears on zero or more **UI Pages** (via `where:`)
- A **Feature** or **Functionality** may reference another **Feature** (via `references:`) for cross-links the hierarchy doesn't capture

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

- **"Category"** was initially collapsed into **Feature** (depth in the tree was meant to encode abstraction). Reversed after looking at the rendered graph: top-level groupings benefit from a distinct visual treatment. **Category** is now its own entity kind — behaviourless, sits above Features in the hierarchy.
- **"Sub-feature"** was used to refer to a Feature nested under another. Resolved: there is no nesting in Elevator. Sub-features are just Features cross-referenced from a parent's body.
- **Top-level qualified names**: Functionalities are defined as `fu f.<feature>.<verb>` (qualified) because verbs like `creation` and `edition` repeat across Features. Categories, Features, Concepts are flat names — collisions there would be a real authoring problem and worth surfacing.
