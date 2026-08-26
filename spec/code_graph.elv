# Code graph — the typed entity/relationship graph everything else asks
# questions of.

import "concepts.elv"

c code_graph {
    d: "Typed entity/relationship graph built from heterogeneous sources — one parser per language, metrics engine, deterministic output."
    cr: "src/parser/", "src/analyzer/", "src/graph.rs"
    f model
    f pipeline
    f parsers
    f resolution
    f metrics
    f smells
    f parse_store
    f dep_paths
    f diff
    f renderers
    f recall_bench
}

f model {
    d: "The vocabulary every other area speaks: CodeEntity and its EntityKind/Visibility/EntityMetrics, Relationship with its RelationshipKind and Precision, FileInfo/Language/Span, plus the ScopeMetrics rollups and the Thresholds that turn raw numbers into warn/bad bands. Owned here rather than by the parsers, because build.rs hashes this directory into the parse-cache generation — a field added here re-parses the world."
    cr: "src/models/"
}

f pipeline {
    d: "Analyzer is the orchestration everything else calls: FileWalker selects files, each is parsed (cache first), the per-file results merge, then resolution, dep edges and metrics run in that fixed order. Returns one AnalysisResult carrying graph, files and the parser warnings. Long runs are interruptible — check_cancel yields Cancelled at the phase boundaries rather than mid-merge. discover_files walks one root, or two when spec_dir names a directory outside it: the second walk admits .elv alone, so pointing at a docs repo costs a traversal and not a second analysis. Set at all, spec_dir also decides which .elv files are the spec — outside it they stop counting, which is the half that lets a tree with .elv fixtures have a spec at all. A spec_dir that is not a directory says so and falls back to every .elv under the root, because a typo and a repo with no spec are otherwise the same empty picture."
    cr: "src/analyzer/mod.rs", "src/analyzer/file_walker.rs"
}

f dep_paths {
    d: "Reachability questions over the finished edge set: DependencyResolver answers direct dependencies/dependents and cycle detection, TransitiveDependencyResolver the closure behind `mezz deps --depth` and the MCP impact tool."
    cr: "src/analyzer/dependency_resolver.rs"
}

f diff {
    d: "Structural comparison of one tree against a git ref, the engine under `mezz diff`, the MCP assess_change tool and push mode. Resolves the ref, materialises it in a throwaway git worktree (create_worktree/remove_worktree), analyzes both sides and pairs entities by EntityKey — path plus qualified name, so a moved body is a change and not a delete-plus-add. Reports ChangeStatus per entity with a MetricDelta, and a source_hash distinguishes a real edit from a re-render. Any resolvable ref works, which is what already made a stash comparable with no engine work at all — a stash entry is a commit — except for the one thing a checkout of it silently drops: `git stash --include-untracked` stores those files in a THIRD PARENT rather than in the stash commit's own tree, so `git worktree add --detach` reproduces the stash minus exactly the new files a reader opened the diff to find, and the diff calls them absent rather than added. restore_stashed_untracked overlays that tree, in create_worktree rather than at any one call site so the CLI, the server and assess_change all get it; stashed_untracked_tree is the guard, and it needs both halves — a third parent alone is an octopus merge, whose sides are already in its own tree, so overlaying one would write files into a checkout that never held them. The subject git writes on the stash's untracked commit is the only thing that tells the two apart from outside. The property extends one step further than found refs: staged_commit MANUFACTURES a ref for the index, which is the one tree here that nothing resolves to, so f.diff_reading.staged costs the same nothing a stash did. write-tree over a copy of the index at GIT_INDEX_FILE — mezz reads the repository it watches and write-tree would update the real index's cache-tree — then commit-tree -p HEAD, over blobs git add already wrote. None when that tree is HEAD's tree, nothing staged being an answer and not an empty comparison; and the commit is unreferenced, so it is made and consumed inside one call rather than handed out."
    cr: "src/diff.rs"
}

f renderers {
    d: "Graph output formats, selected by OutputFormat: DotRenderer for Graphviz, MermaidRenderer, JsonRenderer for the UI and the server, AsciiRenderer for the terminal. Every renderer walks the graph in sorted order rather than map order, which is where @determinism is actually paid for."
    cr: "src/output/mod.rs", "src/output/dot_renderer.rs", "src/output/mermaid_renderer.rs", "src/output/json_renderer.rs", "src/output/ascii_renderer.rs"
}

f parsers {
    d: "One parser per language behind the LanguageParser trait, deliberately independent — no shared scaffolding beyond the trait (ADR 0001). Ten dedicated parsers; anything else falls back to GenericParser with entities but no reliable relationships."
    cr: "src/parser/"
    fu dispatch
    fu parse
    fu infer_types
    fu describe
}

fu f.parsers.dispatch {
    d: "The single language-detection point the walker, FileInfo and parser selection all share, so inclusion and dispatch can never disagree. detect_language prefers path-structural detection for languages that own no extension — is_deploy_file classifies ansible by repo layout — then falls back to Language::from_extension; get_parser maps the result to a LanguageParser, with GenericParser as the fallback."
    cr: "src/parser/mod.rs"
}

fu f.parsers.parse {
    d: "Returns a ParseResult of entities, relationships, imports and warnings. Three obligations a parser ships without easily, all silent when missed: warnings propagate into AnalysisResult rather than being dropped, so a partial parse reads as partial and not as clean; every entity carries its real span and source_code, because f.diff pairs entities by EntityKey and then compares source_hash — an entity whose source_code is None can never be reported as modified; and every callable carries the complexity triple, which is the parser's to compute in its own complexity.rs because f.metrics only ever consumes it. Miss the third and quality/hotspots return an empty ranking for that language, which reads as clean rather than unmeasured — the failure four parsers shipped with. The scoring convention has to be identical across languages or the numbers are not comparable: nesting structures add 1 + depth to cognitive, flat increments add 1, every branch adds 1 to cyclomatic, and a bodyless signature gets Some(1) rather than None because it has one straight-through path."
    cr: "src/parser/language_parser.rs", "src/parser/java/complexity.rs", "src/parser/groovy/complexity.rs"
    references: f.diff, f.metrics
}

fu f.parsers.describe {
    d: "Recovers what the code says about itself, and the host language decides which way a comment points. Rust has two directions that are not interchangeable: an outer doc (/// and /** */) describes the item below it, collected backwards through prev_sibling; an inner doc (//! and /*! */) describes the scope around it, collected forwards from the first child of a source_file or a mod body. Conflating them fails silently in both directions — a header with no use block under it is read as the description of whichever item happens to follow, and the file's own header lands nowhere. A file header belongs to no entity, because the module a .rs file defines is declared in a different file, so it travels beside them as file_documentation and the pipeline lifts it onto FileInfo for renderers to key by path. Two traps worth the words: extract_doc_comment must stop at an inner doc rather than absorb it, and the grammar's line_comment node includes its trailing newline, so a body that is not trimmed makes every multi-line description double-spaced."
    cr: "src/parser/rust/doc_comments.rs", "src/parser/rust/leaves.rs", "src/parser/rust/mod.rs"
    references: f.model, f.renderers
}

fu f.parsers.infer_types {
    d: "Reconstructs receiver types so a method call resolves to Type::method rather than the receiver's source text: parameter signatures, struct fields, and let bindings, widest scope first so each shadows the last (TypeEnv). Reads declared types only — a receiver it cannot account for yields no edge rather than a guess. Reaches only as far as one file: resolve_receiver returns a ReceiverPath whose unwalked tail rides the edge as recv_path/recv_member for f.resolution to finish, because a parse that read a second file would be cached wrong."
    cr: "src/parser/rust/inference.rs"
    references: f.resolution
}

f resolution {
    d: "Turns the callee strings parsers emit into entity ids. A target that matches nothing becomes a tagged ghost entity, so the broken link stays visible instead of vanishing."
    cr: "src/graph.rs"
    fu rank_by_locality
    fu walk_fields
    fu trace_exact
}

fu f.resolution.rank_by_locality {
    d: "Names collide constantly, so every candidate is kept and pick_nearest ranks them: same file, then same directory, then deepest shared path prefix, then lowest entity id. The final tie-break is by id, never by map iteration order, because the ranking has to be a function of the tree."
    cr: "src/graph.rs"
}

fu f.resolution.walk_fields {
    d: "Finishes the receiver paths a parser could only type halfway, where the field's struct is declared in another file — entity.kind.is_callable() stalls because CodeEntity's fields live in models/entity.rs. FieldIndex folds every Rust struct entity into struct → (field → declared type) and retargets the edge at Type::member; it runs after merge, when every struct is known, and before trace_exact, so an exact resolution still wins. Two same-named structs disagreeing on a field map it to None and the lookup declines, which is also what makes the pass independent of hash order."
    cr: "src/analyzer/receiver_index.rs"
}

fu f.resolution.trace_exact {
    d: "Opt-in rust-analyzer pass (MEZZ_LSP_EXACT=1, off by default) that upgrades call edges it can resolve to Precision::Exact; everything else stays Heuristic and renders labelled. Best-effort — no server, no manifest, or a timeout degrades to heuristic edges rather than failing the analysis. Which edges upgrade is NOT reproducible: poll_until_ready fires the whole batch as soon as one probe resolves, so rust-analyzer answers from whatever it has finished indexing and nulls the rest. Same tree, same server, minutes apart: 3644 then 3676 exact edges. This is the one pass @determinism does not cover."
    cr: "src/analyzer/lsp_tracer.rs"
    references: f.recall_bench
}

f recall_bench {
    d: "Scores heuristic call edges against a trace_exact run of the same tree, pairing by (source_id, order): agree / mistargeted / missed, with edges the oracle could not resolve excluded rather than counted as agreement. Maintainer-run, never a CI gate — rust-analyzer costs ~300s per oracle and does not amortise. Because the oracle is not reproducible, it is treated as a fixture: --save-oracle once, --oracle it for every binary compared, and vary nothing else. Two headlines scored against two oracle runs are not subtractable — the scored set moves by more edges than most single fixes recover, which is how a real +24 read as no change at all."
    cr: "scripts/call_edge_recall.py"
    references: f.resolution
}

f metrics {
    d: "Coupling and the numbers derived from it — fan-in/out, instability, WMC, chain depth, PageRank, composite_score — plus per-file and per-module rollups, computed after the edge set is final. The complexity triple (cyclomatic, cognitive_complexity, max_nesting) is not computed here: each parser measures its own bodies and this layer only reads them, through unwrap_or(0) in populate_wmc and populate_composite_scores. That default is the silent part — a language whose parser ships no complexity pass scores 0, and the quality/hotspots ranking filters on composite_score > 0.0, so it disappears from the ranking entirely rather than appearing as unmeasured."
    cr: "src/graph.rs"
    references: f.parsers
    fu roll_up
    fu shape
    fu picture
}

fu f.metrics.roll_up {
    d: "Rolling per-entity numbers up to files and directories, and what that rollup is allowed to count. populate_scope_metrics runs four phases in order — scan_entity_files, scan_file_edges, compute_file_cycles, then assemble_file_metrics and compute_module_metrics — and a module is nothing but a directory prefix, so the same classify_module_edges answers for every ancestor of a file. scan_file_edges is where the second silent part lives: it admits only RelationshipKind::is_dependency, so a graph whose relationships are all References — every Markdown link, Elevator, Impex and folded SQL edge — produced fan_in 0, fan_out 0, cohesion None on every scope while the canvas drew arrows between them. cohesion is Option and reads as unmeasured; the counts had no such escape hatch, and 0 is the flattering end of that scale, so a folder nobody measured reported as a folder with nothing to answer for. The scan therefore fills two EdgeBuckets of the same shape, deps and refs, which is what lets the module rollup run the existing classification over either instead of keeping a second copy: ref_fan_in and ref_fan_out ride beside the coupling pair and feed no ratio, no cycle and no composite_score. References was deliberately not added to is_dependency — that predicate is global, so admitting it would move cohesion, instability and every composite_score for four languages that never asked. The counts travel to the browser through JsonScopeMetrics, not ScopeMetrics, and were correct in the graph and absent from the payload for one round of the work; they are skipped when zero, so a code graph's JSON is byte-identical to what it was."
    cr: "src/graph.rs", "src/output/json_renderer.rs"
    references: f.model
}

fu f.metrics.shape {
    d: "The one number here that asks whether a folder can be READ rather than whether its code is hard to change, and the only one whose sub-scores run higher-is-better (ADR 0012). folder_shape::compute is called once for the whole tree from compute_module_metrics, over edges.deps.pairs — the same dependency bucket the coupling numbers use, so References edges are outside it for the reason roll_up gives and a spec-heavy folder reports an unmeasured shape rather than a flattering one. It scores the graph the CANVAS draws when collapsed to a folder: its immediate children, each subfolder one node. Flattening every descendant would see more and score a picture nobody looks at. accumulate is what makes that one walk instead of a scan of every folder against every edge — each edge lands in the folder where its two paths part (the lowest common ancestor), because above that folder both endpoints collapse into the same child and below it only one is present. Folders are scored deepest-first, since child_compliance needs its subfolders already done. ShapePattern is a ladder of four — Cyclic, Tangled, Hierarchical, Fractal — and its DECLARATION order is the ladder, which is what lets a parent ask through Ord whether its worst child clears a bar. Legibility is layering, not tree-ness: layering_of takes the share of edges advancing exactly one level, over the cycle-condensed graph (petgraph condensation, so a loop is charged to acyclicity once and does not also erase the layering), which scores a diamond perfectly and a shortcut past the middle layer poorly — the healthy reuse shape is several files leaning on one helper, and the costly shape is the edge a reader has to hold in their head. compliance_of is a weighted mean (W_ACYCLICITY 0.40, W_LAYERING 0.30, W_ENTRY 0.15, W_CHILD 0.15) in which an absent term drops out of the DENOMINATOR rather than scoring zero. classify returns the tier and the ShapeBlocker together from one walk, short_of_fractal naming the binding gate against shape_layering / shape_entry / shape_child / shape_compliance in Thresholds: a verdict and a reason derived separately are two claims free to disagree, and compliance cannot stand in for the reason because two folders at 0.72 can need opposite fixes. The gates are an AND, so their order changes nothing about the tier and only decides which one is reported. Nothing here feeds composite_score — organisation and code quality have different remedies, and one number moving for two unrelated reasons can be acted on for neither. The trap is the single-child waiver: child_count <= 1 waves the structured gate through, so a folder holding one file scores Fractal at compliance 1.0 with layering and entry unmeasured, which reads as praise for an accident and inflates the fractal count in any tally — 9 of the 11 folders this repo calls fractal are waived that way. arborescence is the second reading of the same edges and the one layering deliberately refuses (ADR 0013): the share that would survive in a spanning forest, one arriving per child, so two files leaning on one helper score layering 1.00 and branching 0.50 and BOTH are right — nothing skips a level, and the drawing still converges instead of branching. It is an edge ratio over layering's own denominator so the two read on one scale and are unmeasured together, it is condensed for the reason layering is, and counting nodes-with-one-parent instead would score the commonest real shape at zero and rank nothing. It gates Fractal via ShapeBlocker::Merges — asked FIRST among those gates, a merge being the most local thing a reader can go and look at — and stays OUT of compliance_of, so no existing number moved when it landed and shape_arborescence took shape_layering's own 0.70. Like ref_fan_in it reaches the browser through JsonScopeMetrics, not ScopeMetrics."
    cr: "src/analyzer/folder_shape.rs", "src/models/scope_metrics.rs", "src/graph.rs", "src/output/json_renderer.rs"
    references: f.model
}

fu f.metrics.picture {
    d: "The evidence behind the verdict, which the scoring pass used to compute and throw away. folder_shape::picture takes the same three inputs as compute and derives from them the same way — one derivation, so the drawing and the number cannot disagree, which is the whole reason it lives in that module rather than beside it. It returns the folder's immediate children with the level each draws on, every child-to-child edge marked step / skip / back, the doors, and the one-hop traffic across the boundary marked entry / breach / exit. Levels come from the same longest-path walk layering_of measures against, so a node cannot sit on one row for the picture and another for the score; loop membership is an IDENTITY and not a flag, because two children in two different cycles have an ordinary edge between them and a boolean would call it a back edge. Doors are every file tied at the busiest, since picking one of a tie would paint an honest tie as a breach — and a breach names the FILE to change alongside the CHILD circle the line attaches to, which for a nested file are not the same thing. Boundary traffic is one hop and never transitive, the restraint f.visual_scopes.neighbourhood keeps and for the same reason. Computed for one folder rather than all of them: four floats a folder is free to carry on every graph load, a picture is the edge list again. Served at GET /api/shape with the verdict in the same response, so no reader can pair one analysis's drawing with another's number, and every path relativized through relative_to because the graph stores absolute ones. The invariant worth keeping: the share of `step` edges IS the layering score, asserted rather than assumed."
    cr: "src/analyzer/folder_shape.rs", "src/models/folder_picture.rs", "src/server/shape_handler.rs"
}

f smells {
    d: "God Class, Dispatcher, Feature Envy, Shotgun Surgery and Data Bag, derived from metric combinations after they settle. Data Bag is informational — a Kotlin data class is the same shape without the same problem."
    cr: "src/graph.rs"
}

f parse_store {
    d: "Content-hashed per-file parse cache on disk, and the whole of the incremental mechanism: there is no separate dirty tracking, unchanged files simply hit. One entry per canonical path — key_stem hashes the path, the content hash rides inside the entry, so a re-parse overwrites rather than accumulating a new entry per edit. Canonical is what makes the key an identity; it is not what makes a hit applicable, which is why reuse is a step of its own."
    cr: "src/analyzer/parse_store.rs"
    fu invalidate
    fu reuse
}

fu f.parse_store.invalidate {
    d: "Entries are scoped by a generation key derived at build time: build.rs hashes src/parser/, src/models/ and the resolved tree-sitter versions, so anything that changes what unchanged source parses to mints a new generation on its own. PARSE_CACHE_SALT survives only as a manual escape hatch for what the fingerprint cannot see — it was load-bearing and was forgotten twice, serving pre-change parses from warm caches indefinitely (ADR 0004). Abandoned generations are reclaimed after STALE_GENERATION_DAYS, late enough that an older installed binary keeps its own cache warm."
    cr: "src/analyzer/parse_store.rs"
}

fu f.parse_store.reuse {
    d: "Content decides whether an entry is stale; it does not decide whether the entry applies. usable_hit filters ParseStore::get on ParsedFile.file_path and parse_file_standalone takes the hit only if it equals the path being walked now. The two differ because the key is canonical while everything the entry records — file_path, FileInfo::path, every entity's file_path — is the path as walked, and one file has as many walked paths as there are ways to reach it: a symlinked spec directory, `mezz analyze .` against an absolute root, one repo cloned twice. Serving the entry regardless put the other spelling on every entity, so click-to-open and cr: anchors named a path the run never visited, and since content decides a hit it stayed wrong until someone edited the file. Keying on the walked path instead is the obvious simplification and is worse: the store is machine-wide, so `./src/main.rs` is not a unique name and two repos with an identical file would trade entries."
    cr: "src/analyzer/mod.rs"
}
