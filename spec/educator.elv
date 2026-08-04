# Educator — good-practice content surfaced at authorship time.

c educator {
    d: "Language-specific good-practice rules and lessons surfaced at authorship time (VS Code hover + sidebar). Content is authored as markdown with frontmatter and loaded at runtime, so adding a rule is a file, not a release."
    cr: "src/educator/"
    f authored_content
    f constructs
    f matching
    f content_health
}

f authored_content {
    d: "Rules and lessons as markdown files with YAML frontmatter, loaded by load_all: split_frontmatter separates metadata from body, parse_one builds a Rule or Lesson. Parse problems become a LoadIssue with a LoadIssueSeverity rather than a hard failure — one malformed file must not blank the whole catalog. Educator is the loaded handle everything else queries."
    cr: "src/educator/rules.rs", "src/educator/lessons.rs", "src/educator/mod.rs"
}

f constructs {
    d: "The bridge from a tree-sitter node to something a rule can match on: JAVA_CONSTRUCT_KINDS declares the recognised kinds as ConstructKindSpec entries with their AttributeSpec list, and extract dispatches per node kind to an attribute extractor (class_attrs, method_attrs, if_statement_attrs, catch_attrs, …), each returning the Attrs map a predicate reads. Attributes are what the rule language sees, so a construct only becomes matchable once its extractor names the property; render_catalog publishes the declared set as the authoring reference."
    cr: "src/educator/java.rs", "src/educator/catalog.rs"
}

f matching {
    d: "Where content meets code. query answers the editor's cursor position: byte_offset_at locates the node, build_stack walks it to the root, and evaluate runs each Rule's MatchOp against the Attrs of every construct on that stack, so an enclosing class rule fires on a cursor inside a method. Returns RuleHit and LessonHit with the snippet and Attachment. scan.rs is the same evaluation over a whole file for the diagnostics list."
    cr: "src/educator/position.rs", "src/educator/predicate.rs", "src/educator/scan.rs"
}

f content_health {
    d: "Authoring guardrails, because the content is not compiled: validate checks severity, kind and applies_to against ALLOWED_SEVERITIES / ALLOWED_KINDS / ALLOWED_LESSON_LEVELS and the declared construct kinds, and a wrong value is answered with nearest — a levenshtein suggestion rather than a bare rejection. render_index regenerates the browsable content map with relative_link so the index survives being moved."
    cr: "src/educator/validator.rs", "src/educator/index.rs"
}
