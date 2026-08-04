# CLI — the two binaries and the config object they build.

c cli {
    d: "Two binaries over the same library: `nao` for the code graph, `elevator` for the .elv spec layer. Both are thin — argument parsing and dispatch only, with the work living in the analyzer and output modules."
    cr: "src/main.rs", "src/bin/elevator.rs", "src/lib.rs"
    f nao_cli
    f elevator_cli
    f config
}

f nao_cli {
    d: "One Commands enum, four dispatchers so no single match grows without bound: dispatch_analysis (Analyze, Deps, Find, Cycles, Stats, Diff), dispatch_server (Watch, Serve), dispatch_educator (Educate, ConstructKinds, EducatorIndex) and dispatch_agent (Mcp, Hook, PrReport). The split is a complexity-gate constraint, not taste — CI fails on any metric increase to an existing function, which makes one growing dispatcher unextendable."
    cr: "src/main.rs"
}

f elevator_cli {
    d: "Spec-side binary: renders the map, --list, --stats and --focus artifacts, runs run_check and the drift and code-map reports, and run_extract for named slices. --docs prints LANGUAGE_GUIDE, the language reference embedded in the binary so an agent can learn the grammar without a second repo. write_out sends artifacts to a file or stdout."
    cr: "src/bin/elevator.rs"
}

f config {
    d: "The one knob object the CLI, the server and the MCP tools all build before analyzing: AnalysisConfig (roots, languages, allow_unsafe_passes), FilterConfig (include/exclude globs, test and vendor skipping) and DisplayConfig (LayoutDirection, ColorScheme, depth caps). Built per-invocation and passed down rather than read from a global, so two analyses in one process cannot contaminate each other."
    cr: "src/config.rs"
}
