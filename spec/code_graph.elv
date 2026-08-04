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
    d: "Analyzer is the orchestration everything else calls: FileWalker selects files, each is parsed (cache first), the per-file results merge, then resolution, dep edges and metrics run in that fixed order. Returns one AnalysisResult carrying graph, files and the parser warnings. Long runs are interruptible — check_cancel yields Cancelled at the phase boundaries rather than mid-merge."
    cr: "src/analyzer/mod.rs"
}

f dep_paths {
    d: "Reachability questions over the finished edge set: DependencyResolver answers direct dependencies/dependents and cycle detection, TransitiveDependencyResolver the closure behind `nao deps --depth` and the MCP impact tool."
    cr: "src/analyzer/dependency_resolver.rs"
}

f diff {
    d: "Structural comparison of one tree against a git ref, the engine under `nao diff`, the MCP assess_change tool and push mode. Resolves the ref, materialises it in a throwaway git worktree (create_worktree/remove_worktree), analyzes both sides and pairs entities by EntityKey — path plus qualified name, so a moved body is a change and not a delete-plus-add. Reports ChangeStatus per entity with a MetricDelta, and a source_hash distinguishes a real edit from a re-render."
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
    d: "Opt-in rust-analyzer pass (NAO_LSP_EXACT=1, off by default) that upgrades call edges it can resolve to Precision::Exact; everything else stays Heuristic and renders labelled. Best-effort — no server, no manifest, or a timeout degrades to heuristic edges rather than failing the analysis. Which edges upgrade is NOT reproducible: poll_until_ready fires the whole batch as soon as one probe resolves, so rust-analyzer answers from whatever it has finished indexing and nulls the rest. Same tree, same server, minutes apart: 3644 then 3676 exact edges. This is the one pass @determinism does not cover."
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
}

f smells {
    d: "God Class, Dispatcher, Feature Envy, Shotgun Surgery and Data Bag, derived from metric combinations after they settle. Data Bag is informational — a Kotlin data class is the same shape without the same problem."
    cr: "src/graph.rs"
}

f parse_store {
    d: "Content-hashed per-file parse cache on disk, and the whole of the incremental mechanism: there is no separate dirty tracking, unchanged files simply hit. One entry per path — the hash rides inside the entry, so a re-parse overwrites rather than accumulating a new entry per edit."
    cr: "src/analyzer/parse_store.rs"
    fu invalidate
}

fu f.parse_store.invalidate {
    d: "Entries are scoped by a generation key derived at build time: build.rs hashes src/parser/, src/models/ and the resolved tree-sitter versions, so anything that changes what unchanged source parses to mints a new generation on its own. PARSE_CACHE_SALT survives only as a manual escape hatch for what the fingerprint cannot see — it was load-bearing and was forgotten twice, serving pre-change parses from warm caches indefinitely (ADR 0004). Abandoned generations are reclaimed after STALE_GENERATION_DAYS, late enough that an older installed binary keeps its own cache warm."
    cr: "src/analyzer/parse_store.rs"
}
