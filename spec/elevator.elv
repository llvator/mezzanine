# Elevator — the .elv spec language and its tooling.

import "concepts.elv"

c elevator {
    d: "The .elv spec language: a domain abstraction above the code, with its own parser, text artifacts, and health checks."
    f language
    f text_artifacts
    f spec_health
}

f language {
    d: "Lexer/parser for .elv with strict explicit imports; unresolved references become visible stubs rather than silent drops. Two phases behind one LanguageParser::parse, sharing the DefStmt vocabulary in ast.rs. Neither phase can fail the file: a stray character costs one token, a malformed definition costs one definition, and every warning carries `at line L column C`."
    cr: "src/parser/elevator/"
    fu tokenize
    fu parse_defs
    fu emit
}

fu f.language.tokenize {
    d: "Hand-written lexer over braces, colons, commas, dots, identifiers and double-quoted strings, with `#` to end-of-line. Every SpannedToken carries a half-open byte range plus line/column — no token spans a newline, so end_column is derived rather than stored, and spans can slice source_code straight back out. tokenize returns (tokens, Vec<LexDiag>) rather than a Result: an unexpected character is skipped and an unterminated string is closed at end-of-line, because the fail-fast version let one stray quote zero out a whole file's entities."
    cr: "src/parser/elevator/lexer.rs"
}

fu f.language.parse_defs {
    d: "Recursive descent into DefStmt that resolves nothing — bodies are captured verbatim so forward and cross-file references still work. Also the author's entire feedback loop: warn_at stamps every diagnostic with its Loc, an unclosed body is reported at its `{` rather than at EOF, and a definition nested inside a body (which the language forbids) is re-parsed via recover_nested_def as the top-level definition it was meant to be — bounded by MAX_NESTING — so its `d:` lands on the child instead of silently on the enclosing entity."
    cr: "src/parser/elevator/grammar.rs"
}

fu f.language.emit {
    d: "DefStmt to CodeEntity plus the Contains/References edges each body declares. build_entity sets the statement's real span, its source_code slice and metrics.loc; without the slice source_hash reads None on both sides of a diff, so no spec edit is ever reported as a change. An id defined twice in one file warns and the first wins, but both bodies' children still emit; ensure_stub auto-creates UiPage targets only, leaving every other missing target to the analyzer's post-merge stub pass. An explicit kind prefix on a where:/references:/used_by: value overrides the field's default target kind."
    cr: "src/parser/elevator/emit.rs"
}

f text_artifacts {
    d: "Low-token text renderings for LLM consumption: full map, focus bundles, list, stats. The legend carries the trust contract (structure checked, descriptions authored)."
    cr: "src/output/elevator_text_renderer.rs", "src/output/elevator_list.rs"
    fu focus
    fu extract
}

f spec_health {
    d: "Correctness signals for a spec: checker, code-map, drift — and the one repair that needs no judgement, applying the moves git already recorded."
    fu check
    fu code_map
    fu drift
    fu fix_drift
}

fu f.text_artifacts.focus {
    d: "Context bundle for one entity: ancestors, target subtree, siblings, cross-cutting concepts. Focus renders descriptions unclipped; tree views clip at DESC_TRUNCATE."
    cr: "src/output/elevator_text_renderer.rs"
}

fu f.text_artifacts.extract {
    d: "The one renderer that emits .elv rather than a reading artifact: a named slice, closed over descendants and used Concepts, with ancestors pruned to position it. Agent snapshots — what a run claimed to touch, kept as history after the shared spec moves on."
    cr: "src/output/elevator_extract.rs"
}

fu f.spec_health.check {
    d: "Five rules — parse failures and unresolved refs are errors (exit 1); orphan, empty, unused are hints (exit 0). Deliberately never enforces required fields."
    cr: "src/output/elevator_check.rs"
}

fu f.spec_health.code_map {
    d: "Inverted index path → entities. Same path claimed twice under the same cr kind is flagged as a duplicate — one thing named twice in the spec."
    cr: "src/output/elevator_code_map.rs"
}

fu f.spec_health.drift {
    d: "Anchor verification against --code-root: cr paths must resolve (error); identifier-shaped tokens from d: are grounded by text search in the claimed files (hint). No shape comparison — the abstraction is not a projection of the code tree."
    cr: "src/output/elevator_drift.rs"
}

fu f.spec_health.fix_drift {
    d: "--drift --fix rewrites the dead cr paths git recorded a move for. Two gates: rename_log finds the commit that deleted the path then re-diffs it unrestricted, because a pathspec narrows the diff before rename detection and hides the pair; and the new path must exist. dir_rename demands unanimity across the files that left a directory. Anything unproven is reported, never applied — a same-basename candidate is offered for a human to confirm."
    cr: "src/output/elevator_fix.rs"
}
