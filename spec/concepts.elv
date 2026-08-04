# Cross-cutting concepts. Consumers import this file (used_by edges
# are emitted consumer → concept, so the reference lives in their scope).

concept determinism {
    d: "Identical trees produce byte-identical graphs — every reported delta is real signal, and the MCP warm cache can be stale-old but never stale-wrong. Holds for the default path only: f.resolution's opt-in rust-analyzer pass resolves a different subset on every run, so anything measured through it needs one pinned oracle rather than two runs subtracted."
    used_by: f.overview, f.text_artifacts, f.resolution, f.parse_store, f.renderers, f.diff
}
