# Agent tools — the MCP server surface.

import "concepts.elv"
import "code_graph.elv"
import "elevator.elv"

c agent_tools {
    d: "The agent-facing surface over the graph: read-only MCP tools pulled on demand, plus push mode that surfaces structural regressions without being asked."
    cr: "src/mcp/"
    f protocol
    f overview
    f spec_slice
    f shape_tools
    f navigation_tools
    f dead_code
    f assess_change
    f push_review
}

f protocol {
    d: "The stdio JSON-RPC layer: McpServer speaks the versions in SUPPORTED_VERSIONS and dispatches through the TOOLS table — name → fn, a table rather than a match so a new tool adds no branch to the dispatcher. tool_definitions advertises the same names, and every_advertised_tool_is_dispatchable is the test that keeps the two in step. The graph is analyzed once into a CachedGraph and reused; spawn_invalidation_watcher drops it when a source file the analysis covers changes, so a warm answer can be stale-old but never stale-wrong."
    cr: "src/mcp/mod.rs"
}

f shape_tools {
    d: "The orientation half of the surface — what is here and what is wrong with it. map renders a folder's files and entities with loc/complexity/coupling; quality ranks smells and complexity hotspots; hotspots crosses metrics with churn; context bundles one entity with its signature, callers and callees. All read-only, all capped (MAX_BODY_LINES, cap_lines, cap_chars) because the budget being protected is the agent's context window, not the terminal."
    cr: "src/mcp/tools.rs"
}

f navigation_tools {
    d: "The follow-the-edges half: impact walks transitive_dependents for a blast radius, trace finds shortest_paths between two entities, tests_for maps an entity to the tests exercising it via is_test_entity, similar scores lexical overlap of entity vocabularies (tokenize, STOPWORDS, SIMILARITY_FLOOR) for the near-duplicate question the graph has no edge for."
    cr: "src/mcp/tools.rs"
    references: f.dep_paths
}

f assess_change {
    d: "Self-review as a tool call: the f.diff engine against a git ref, rendered by render_change_report as prose an agent can act on rather than a metric table. spec_claims cross-references the touched paths against the Elevator cr: index, so a change landing in an area the spec claims says which entity now needs deepening."
    cr: "src/mcp/tools.rs"
    references: f.diff
}

f overview {
    d: "Domain-level Elevator map (or focus bundle) as an MCP tool call — the onboarding step before map. Bails with guidance when the project has no .elv layer."
    cr: "src/mcp/tools.rs"
}

f spec_slice {
    d: "The spec narrowed to a folder and emitted back as .elv, so a task can keep the branch it works on as a file. select seeds from the cr: index by path rather than by name: claims inside the folder win and bring their subtrees, and only when there are none does the narrowest enclosing claim stand in — a wide claim seeding a narrow question would return most of the spec. A folder no cr: mentions is an error, since an empty slice reads as a successful extraction of nothing. The only tool that writes: resolve_out fences out to the project root and never replaces a file unasked."
    cr: "src/mcp/slice.rs"
    references: f.text_artifacts
}

f dead_code {
    d: "Removal candidates as an MCP tool call: fan-in 0, minus the populations for which no dependent is not evidence of death. is_entry_point forgives program entry points, runtime hooks and members satisfying a declared contract; is_public_api forgives declared-visible entities, but only in languages records_visibility covers — a TypeScript export is never excluded this way. The last gate is names_mentioned_elsewhere: a name the source mentions anywhere the graph cannot see is referenced, not dead, and on this repo that one scan suppresses more candidates than the other three together. Grouped by file, densest first, ties by path so the report is stable across runs."
    cr: "src/mcp/tools.rs"
}

f push_review {
    d: "Push mode: nao's structural signal arrives without being asked. Fresh-process commands over the assess_change diff, advisory only — signals, never gates."
    cr: "src/mcp/push.rs"
    fu self_review
    fu pr_report
}

fu f.push_review.self_review {
    d: "Quiet-when-clean Stop-hook review: only new smells, cycles, and above-floor complexity jumps of the working tree, each surfaced once per session via stable fingerprints, under a hard line cap. The LSP-diagnostics token contract — silent steady state costs the agent zero context."
}

fu f.push_review.pr_report {
    d: "The CI leg: assess_change rendered as one PR comment, edited in place by marker, a one-liner when nothing structural changed. Always exits zero, so wiring it into a pipeline can never fail the build."
}
