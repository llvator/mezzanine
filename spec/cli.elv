# CLI — the two binaries and the config object they build.

# The settings report and its save path are served by the HTTP layer and
# drawn by the browser UI, so f.config's verbs reach into both.
import "server.elv"
import "visualizer.elv"

c cli {
    d: "Two binaries over the same library: `mezz` for the code graph, `elevator` for the .elv spec layer. Both are thin — argument parsing and dispatch only, with the work living in the analyzer and output modules."
    cr: "src/main.rs", "src/bin/elevator.rs", "src/lib.rs"
    f mezz_cli
    f elevator_cli
    f config
    f init
}

f mezz_cli {
    d: "One Commands enum, four dispatchers so no single match grows without bound: dispatch_analysis (Analyze, Deps, Find, Cycles, Stats, Diff), dispatch_server (Watch, Serve), dispatch_educator (Educate, ConstructKinds, EducatorIndex) and dispatch_agent (Mcp, Hook, PrReport). The split is a complexity-gate constraint, not taste — CI fails on any metric increase to an existing function, which makes one growing dispatcher unextendable. Init sits outside the four: it writes the repo's configuration instead of reading its code."
    cr: "src/main.rs"
}

f init {
    d: "`mezz init` scaffolds the two files a repo would otherwise hand-write. The settings file carries only what the tree proves — the languages holding at least a twentieth of the walked files, Elevator exempt because a spec is outnumbered by design, Markdown excluded because a pinned list is where opt-in docs would stop being opt-in, plus spec_dir when every .elv sits in one directory. Never a defaulted key: spelling out today's defaults would freeze them into every repo that ran it. --vscode adds the start/open/stop tasks for the browser UI, merging by label into an existing tasks.json and refusing outright to rewrite one it cannot parse, since a tasks.json with comments is valid to VS Code and rewriting it would delete them."
    cr: "src/init.rs"
}

f elevator_cli {
    d: "Spec-side binary: renders the map, --list, --stats and --focus artifacts, runs run_check and the drift and code-map reports, and run_extract for named slices. --docs prints LANGUAGE_GUIDE, the language reference embedded in the binary so an agent can learn the grammar without a second repo. write_out sends artifacts to a file or stdout."
    cr: "src/bin/elevator.rs"
}

f config {
    d: "The one knob object the CLI, the server and the MCP tools all build before analyzing: AnalysisConfig (roots, languages, spec_dir, allow_unsafe_passes), FilterConfig (include/exclude globs, test and vendor skipping) and DisplayConfig (LayoutDirection, ColorScheme, depth caps). Built per-invocation and passed down rather than read from a global, so two analyses in one process cannot contaminate each other. Settings fills what the flags left alone, from two files scoped by what they describe — the installation (ui_dir, content_fallback) or the repo (output_dir, spec_dir) — and names back every key it refuses rather than dropping it. Refusal is the point of the module: allow_agent_spawn, no_token, allow_origin and allow_unsafe_passes are not fields at either scope, and a repo-scope spec_dir may not be absolute or climb out with .., since a cloned file does not get to choose which directories mezz reads. Naming one outside the repo takes an operator — --spec-dir, or the browser UI's scope panel. The module is a folder: mod.rs loads and refuses, report.rs answers which link of the chain won."
    cr: "src/config.rs", "src/settings/"
    fu resolve
    fu report
    fu refuse
    fu save
}

fu f.config.resolve {
    d: "Flag beats env beats repo file beats user file beats default, first hit wins — and the two keys that could not express that. apply_to_config guards spec_dir on is_none and languages on is_empty, because an unset value has a spelling there; max_depth and min_weight have none, a 3 the caller typed and a 3 Config::default chose being the same usize. Assigning them unconditionally therefore ran the chain backwards: `mezz analyze --depth 7` against a file saying 3 traversed 3. The flag is now passed in rather than inferred — Flags carries max_depth and min_weight, apply_with takes it, apply_scalars resolves flag.or(file) in one expression, and apply_to_config is the no-flag caller's door. `mezz deps --depth` carried clap's default_value = 2, which hid the same distinction inside the flag itself; it is Option<usize> now with DEPS_DEFAULT_DEPTH applied underneath both links. min_weight has no flag at all — its slot in Flags exists so that adding one stays a change to main.rs. The four include_* switches are deliberately outside this rule: they merge with |=, so the file can turn one on and never off."
    cr: "src/settings/mod.rs", "src/main.rs"
}

fu f.config.report {
    d: "What the settings resolved to and which link decided it, served at GET /api/settings for a reader who would otherwise have to know the file existed. Origin is unrecoverable after the merge — over() collapses four sources into one value — so the scopes stop merging at load time: load_scoped returns a Loaded holding repo and user apart, merged() is called only where a merged view is wanted, and user_scoped is serve's variant. Flags reach the report as a set of key names (named), built in main.rs where the CLI values are still in scope; Effective carries port, debounce_ms and output_dir, which were consumed at startup and live nowhere else; SettingsView on AppState holds all three behind a lock so a save can refresh them. Every Row carries a Tier, and the tiers are what gate editing rather than decoration: Analysis is editable by re-parsing, View must not be because the filter panel and saved views already set min_weight and kind, Process cannot be because an editable port field on a page served over that port is a trap. Two keys refuse the one-source-won story and say so instead of inventing a winner — widened lists every contributor to an include_* switch, patterns gives each glob its own Origin because the lists concatenate. serve gets the read route with repo_scope_read false, so an empty repo scope reads as 'never looked' rather than 'no file'."
    references: f.local_serve
    cr: "src/settings/report.rs", "src/server/settings_handler.rs"
    cr.fe: "ui/src/stores/settingsReport.ts", "ui/src/components/SettingsReport.svelte"
}

fu f.config.refuse {
    d: "The refusals became data without ceasing to be stderr. Every diagnostic was an eprintln! and nothing else, which is right for a terminal and useless for a browser: a typo'd exclude_pattern did nothing visible, and neither did a cloned repo asking for allow_agent_spawn — the case the loudness was for. read now returns (Settings, Vec<Warning>) and load_scoped prints them, so the two surfaces cannot drift and a terminal user's output is unchanged. Each Warning carries the file, the key and a Severity: Rejected for a REJECTED key named with the capability it would have granted, Ignored for a typo or a wrong-scope key or a spec_dir that climbs out, Malformed for a file that would not parse — the last being the one most easily missed, since every value the reader believes they set is then quietly a default. misplaced folds report_repo_only and clear_user_only into one loop over data. unknown_languages is recomputed rather than captured, apply_filters running once per command and long after the file was read. Nothing here may fail a command that would otherwise work."
    cr: "src/settings/mod.rs"
}

fu f.config.save {
    d: "Promoting a scope the reader arrived at by experiment to the repo's default, at POST /api/settings/analysis. It saves what is APPLIED, not what is staged — the button sits beside Apply and is disabled while the panel is dirty, since saving an unseen scope would be a second, quieter Apply — and it never re-analyzes, there being nothing to recompute. Nine keys, SAVED_KEYS, all analysis-shaping; min_weight and kind are deliberately absent because one value with two controls is how 'why does the graph look like this?' stops having an answer. merge_analysis_keys lays them over the parsed file so keys this endpoint does not understand survive, and read_raw is deliberately stricter than read: absent is fine, unparseable is a 500 and a refusal, because answering 'you had nothing' for a file we failed to parse is how the next save destroys it. write renames a sibling temp file, pretty-printed, the discipline write_views already uses next door. Settings serializes with skip_serializing_if on every field, which is what keeps the capability-granting keys unwritable — they are not fields, so nothing can emit them — and reject_escaping_spec_dir returns 400 rather than writing a path the loader would refuse on the next start. Neither route nor button exists under serve."
    references: f.saved_views, f.graph_api
    cr: "src/server/settings_handler.rs"
    cr.fe: "ui/src/stores/analysisScope.ts", "ui/src/components/AnalysisScopePanel.svelte"
}
