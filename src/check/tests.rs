//! Tests for `nao check`.
//!
//! Three parts, and they fail for different reasons. The rules-file tests
//! are about *refusing to guess*: every one of them asserts an error naming
//! the key, because the failure this area exists to stop is a gate that
//! passed without understanding its configuration. The counting tests and
//! the tree tests grade real fixtures through the real analyzer, because
//! every rule is a count over resolved entities and edges — a hand-built
//! graph would prove nothing about whether the resolution is the part that
//! works, and resolution is the whole reason these rules belong in nao
//! rather than in a script beside it.

use std::path::{Path, PathBuf};

use super::rules;
use super::*;

/// A throwaway repo: `.nao/rules.json` plus whatever source the test writes.
struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("nao-check-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn write(&self, relative: &str, body: &str) -> PathBuf {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    fn rules(&self, body: &str) {
        self.write(".nao/rules.json", body);
    }

    /// The verdict, with a configuration built from the fixture alone — no
    /// settings file of the developer's reaches it.
    fn check(&self) -> Outcome {
        verdict(&self.0, Config::for_path(&self.0))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A file whose declarations a reader counts: two of them. Not two `const`s
/// — a variable is not a declaration [`is_listed`](crate::mcp) shows, and a
/// fixture that leaned on one would be testing the predicate rather than the
/// rule.
const TWO_DECLARATIONS: &str =
    "export function x(): number { return 1; }\nexport function y(): number { return 2; }\n";

fn violations(outcome: &Outcome) -> Vec<String> {
    match outcome {
        Outcome::Checked { violations, .. } => violations
            .iter()
            .map(|v| {
                format!(
                    "{}{} {} {}/{}",
                    v.path,
                    v.line.map(|n| format!(":{n}")).unwrap_or_default(),
                    v.rule.name(),
                    v.measured,
                    v.bar
                )
            })
            .collect(),
        _ => panic!("expected a graded tree, got: {}", report::human(outcome)),
    }
}

// ---------------------------------------------------------------- the file

#[test]
fn no_rules_file_passes_and_says_so() {
    let fixture = Fixture::new("absent");
    fixture.write("src/a.ts", "export const a = 1;\n");
    let outcome = fixture.check();
    assert_eq!(outcome.exit_code(), EXIT_PASS);
    assert!(
        report::human(&outcome).contains("no rules declared in"),
        "silence would read as a pass: {}",
        report::human(&outcome)
    );
}

#[test]
fn an_empty_rules_object_declares_nothing() {
    let fixture = Fixture::new("empty");
    fixture.rules(r#"{"rules": {}}"#);
    let outcome = fixture.check();
    assert_eq!(outcome.exit_code(), EXIT_PASS);
    assert!(matches!(outcome, Outcome::NoRules { .. }));
}

#[test]
fn the_rules_file_is_named_the_way_a_reader_would_type_it() {
    // `repo_dir(".")` answers `./.nao`, and a reader looking for that file
    // types `.nao/rules.json`.
    assert_eq!(tidy(Path::new("./.nao/rules.json")), ".nao/rules.json");
}

#[test]
fn a_path_that_is_not_there_yields_no_verdict() {
    let fixture = Fixture::new("missing-path");
    fixture.rules(r#"{"rules": {"max_entities_per_file": 7}}"#);
    let missing = fixture.0.join("src/nope");
    let outcome = verdict(&missing, Config::for_path(&missing));
    assert_eq!(outcome.exit_code(), EXIT_NO_VERDICT);
    assert!(report::human(&outcome).contains("no such path"));
}

#[test]
fn an_unknown_rule_is_an_error_naming_the_key() {
    let fixture = Fixture::new("unknown-rule");
    fixture.rules(r#"{"rules": {"min_shape": 3}}"#);
    let outcome = fixture.check();
    assert_eq!(outcome.exit_code(), EXIT_NO_VERDICT);
    let text = report::human(&outcome);
    assert!(text.contains("min_shape"), "key not named: {text}");
    assert!(text.contains("rules.json"), "file not named: {text}");
    assert!(
        text.contains("max_entities_per_file"),
        "what it could have meant is not offered: {text}"
    );
}

#[test]
fn an_unknown_top_level_key_is_an_error() {
    let fixture = Fixture::new("unknown-key");
    fixture.rules(r#"{"rules": {"max_entities_per_file": 7}, "exempts": []}"#);
    let outcome = fixture.check();
    assert_eq!(outcome.exit_code(), EXIT_NO_VERDICT);
    assert!(report::human(&outcome).contains("exempts"));
}

#[test]
fn a_bar_that_is_not_a_whole_number_is_an_error() {
    for body in [
        r#"{"rules": {"max_entities_per_file": "seven"}}"#,
        r#"{"rules": {"max_entities_per_file": 7.5}}"#,
        r#"{"rules": {"max_entities_per_file": -1}}"#,
    ] {
        let fixture = Fixture::new("bad-bar");
        fixture.rules(body);
        let outcome = fixture.check();
        assert_eq!(outcome.exit_code(), EXIT_NO_VERDICT, "accepted {body}");
        assert!(report::human(&outcome).contains("max_entities_per_file"));
    }
}

#[test]
fn an_uncompilable_glob_is_an_error_naming_it() {
    let fixture = Fixture::new("bad-glob");
    fixture.rules(r#"{"rules": {"max_entities_per_file": 7}, "exempt": ["src/**/[a"]}"#);
    let outcome = fixture.check();
    assert_eq!(outcome.exit_code(), EXIT_NO_VERDICT);
    assert!(report::human(&outcome).contains("src/**/[a"));
}

#[test]
fn malformed_json_is_an_error_not_an_empty_rule_set() {
    let fixture = Fixture::new("malformed");
    fixture.rules(r#"{"rules": {"max_entities_per_file": 7},}"#);
    assert_eq!(fixture.check().exit_code(), EXIT_NO_VERDICT);
}

#[test]
fn a_rules_file_that_is_not_an_object_is_an_error() {
    let fixture = Fixture::new("not-object");
    fixture.rules("[7]");
    assert_eq!(fixture.check().exit_code(), EXIT_NO_VERDICT);
}

// ------------------------------------------------------------- the counting

#[test]
fn a_file_over_the_bar_is_reported_once_at_line_one() {
    let fixture = Fixture::new("per-file");
    fixture.rules(r#"{"rules": {"max_entities_per_file": 3}}"#);
    let body: String = (0..5)
        .map(|i| format!("export function f{i}() {{ return {i}; }}\n"))
        .collect();
    fixture.write("src/wide.ts", &body);
    let outcome = fixture.check();
    assert_eq!(outcome.exit_code(), EXIT_VIOLATIONS);
    assert_eq!(
        violations(&outcome),
        vec!["src/wide.ts:1 max_entities_per_file 5/3"]
    );
}

#[test]
fn a_class_is_one_declaration_and_its_methods_are_its_elements() {
    let fixture = Fixture::new("partition");
    fixture.rules(r#"{"rules": {"max_entities_per_file": 1, "max_elements_per_entity": 2}}"#);
    let methods: String = (0..4)
        .map(|i| format!("  m{i}(): number {{ return {i}; }}\n"))
        .collect();
    fixture.write(
        "src/wide.ts",
        &format!("export class Wide {{\n{methods}}}\n"),
    );
    // One declaration in the file — the class — so only rule 4 fires, and it
    // fires on the class rather than on every method.
    assert_eq!(
        violations(&fixture.check()),
        vec!["src/wide.ts:1 max_elements_per_entity 4/2"]
    );
}

#[test]
fn a_callables_elements_are_its_parameters() {
    let fixture = Fixture::new("params");
    fixture.rules(r#"{"rules": {"max_elements_per_entity": 2}}"#);
    fixture.write(
        "src/many.ts",
        "export function many(a: number, b: number, c: number): number { return a + b + c; }\n",
    );
    assert_eq!(
        violations(&fixture.check()),
        vec!["src/many.ts:1 max_elements_per_entity 3/2"]
    );
}

#[test]
fn fields_are_elements_of_their_type_and_not_declarations_of_the_file() {
    let fixture = Fixture::new("fields");
    fixture.rules(r#"{"rules": {"max_entities_per_file": 1, "max_elements_per_entity": 2}}"#);
    fixture.write(
        "src/record.ts",
        "export interface Record {\n  a: string;\n  b: string;\n  c: string;\n}\n",
    );
    assert_eq!(
        violations(&fixture.check()),
        vec!["src/record.ts:1 max_elements_per_entity 3/2"]
    );
}

#[test]
fn violations_are_exhaustive_and_in_path_order() {
    let fixture = Fixture::new("order");
    fixture.rules(r#"{"rules": {"max_entities_per_file": 1}}"#);
    for name in ["src/c.ts", "src/a.ts", "src/b.ts"] {
        fixture.write(name, TWO_DECLARATIONS);
    }
    let found = violations(&fixture.check());
    assert_eq!(found.len(), 3, "not exhaustive: {found:?}");
    let paths: Vec<&str> = found.iter().map(|v| v.split(':').next().unwrap()).collect();
    assert_eq!(paths, vec!["src/a.ts", "src/b.ts", "src/c.ts"]);
}

#[test]
fn an_exempt_file_is_not_checked_and_is_counted_apart() {
    let fixture = Fixture::new("exempt");
    fixture.rules(
        r#"{"rules": {"max_entities_per_file": 1}, "exempt": ["src/generated/**", "**/*.d.ts"]}"#,
    );
    fixture.write("src/generated/api.ts", TWO_DECLARATIONS);
    fixture.write("src/contracts.d.ts", TWO_DECLARATIONS);
    fixture.write("src/kept.ts", TWO_DECLARATIONS);
    let outcome = fixture.check();
    assert_eq!(
        violations(&outcome),
        vec!["src/kept.ts:1 max_entities_per_file 2/1"]
    );
    match outcome {
        Outcome::Checked {
            files_checked,
            files_exempt,
            ..
        } => {
            assert_eq!((files_checked, files_exempt), (1, 2));
        }
        _ => panic!("expected a graded tree"),
    }
}

#[test]
fn a_repo_that_meets_its_rules_passes() {
    let fixture = Fixture::new("pass");
    fixture.rules(r#"{"rules": {"max_entities_per_file": 7, "max_elements_per_entity": 7}}"#);
    fixture.write(
        "src/ledger.ts",
        "export interface Row { label: string; cents: number; }\nexport function total(rows: Row[]): number { return rows.length; }\n",
    );
    let outcome = fixture.check();
    assert_eq!(
        outcome.exit_code(),
        EXIT_PASS,
        "{}",
        report::human(&outcome)
    );
    assert!(report::human(&outcome).contains("0 violations"));
}

/// The seven `shape/` examples are the reference for these two rules —
/// "at most seven entities per file, at most seven elements per entity" is
/// written at the top of [shape/README.md](../../shape/README.md) and every
/// example is built to hold it. If a language's counting drifts, it drifts
/// here first.
#[test]
fn the_shape_examples_meet_the_rules_they_were_built_to() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("shape");
    if !root.exists() {
        return; // the release snapshot ships without the examples
    }
    let rules_body = r#"{"rules": {
        "max_doors_per_folder": 1,
        "max_elements_per_entity": 7,
        "max_entities_per_file": 7,
        "max_importers_per_file": 1
    }}"#;
    for language in [
        "typescript",
        "rust",
        "python",
        "go",
        "java",
        "kotlin",
        "dart",
    ] {
        let example = root.join(language);
        let rules = rules::parse(&example.join(".nao/rules.json"), rules_body).unwrap();
        let config = Config::for_path(&example);
        let result = Analyzer::new(config).analyze().expect("analysis succeeds");
        let graph = DependencyGraph::from_analysis(&result);
        let found = super::violations(&graph, &rules, &example);
        let lines: Vec<String> = found
            .iter()
            .map(|v| {
                format!(
                    "{} {} {} ({})",
                    v.path,
                    v.rule.name(),
                    v.measured,
                    v.names
                        .iter()
                        .map(|cited| cited.path.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .collect();
        assert!(
            lines.is_empty(),
            "{language} breaks its own rules: {lines:?}"
        );
    }
}

// ------------------------------------------------------------ the tree

/// A leaf every fixture below shares: one function, imported by whoever
/// wants a second parent.
const LEAF: &str = "export function shared(): number { return 1; }\n";

/// A file that imports the leaf and *uses* it. The use is the point: a
/// specifier is a string, and what makes this an edge is the call resolving
/// to the entity `shared.ts` declares.
fn importer(from: &str) -> String {
    format!("import {{ shared }} from \"{from}\";\nexport function use(): number {{ return shared(); }}\n")
}

#[test]
fn a_file_with_two_importers_names_both() {
    let fixture = Fixture::new("importers");
    fixture.rules(r#"{"rules": {"max_importers_per_file": 1}}"#);
    fixture.write("src/shared.ts", LEAF);
    fixture.write("src/a.ts", &importer("./shared.ts"));
    fixture.write("src/b.ts", &importer("./shared.ts"));
    let outcome = fixture.check();
    assert_eq!(outcome.exit_code(), EXIT_VIOLATIONS);
    assert_eq!(
        violations(&outcome),
        vec!["src/shared.ts:1 max_importers_per_file 2/1"]
    );
    // A count would leave the author to find them; both are named, with the
    // line each was written on.
    assert!(
        report::human(&outcome).contains("(src/a.ts:1, src/b.ts:1)"),
        "importers not named: {}",
        report::human(&outcome)
    );
}

/// The capability the review's hand-written test lacked, and the reason this
/// rule belongs in nao at all: a file that never types the leaf's path still
/// depends on the leaf, because the shim forwards a name it does not own.
#[test]
fn an_importer_through_a_re_export_is_counted_against_the_declaring_file() {
    let fixture = Fixture::new("shim");
    fixture.rules(r#"{"rules": {"max_importers_per_file": 1}}"#);
    fixture.write("src/settings/settingsDefaults.ts", LEAF);
    fixture.write(
        "src/settings/index.ts",
        "export { shared } from \"./settingsDefaults.ts\";\n",
    );
    fixture.write(
        "src/settings/settings.ts",
        &importer("./settingsDefaults.ts"),
    );
    fixture.write("src/main.ts", &importer("./settings/index.ts"));
    let outcome = fixture.check();
    assert_eq!(
        violations(&outcome),
        vec!["src/settings/settingsDefaults.ts:1 max_importers_per_file 2/1"],
        "the shim laundered an importer: {}",
        report::human(&outcome)
    );
    // And the report says why the file main.ts never names is charged to it.
    assert!(
        report::human(&outcome).contains("src/main.ts:1 via src/settings/index.ts"),
        "the shim is not named: {}",
        report::human(&outcome)
    );
}

#[test]
fn a_file_nobody_imports_breaks_nothing() {
    let fixture = Fixture::new("orphan");
    fixture.rules(r#"{"rules": {"max_importers_per_file": 0}}"#);
    fixture.write("src/alone.ts", LEAF);
    let outcome = fixture.check();
    assert_eq!(
        outcome.exit_code(),
        EXIT_PASS,
        "zero importers is dead_code's question, not this one: {}",
        report::human(&outcome)
    );
}

#[test]
fn a_folder_entered_at_two_files_names_both_doors() {
    let fixture = Fixture::new("doors");
    fixture.rules(r#"{"rules": {"max_doors_per_folder": 1}}"#);
    fixture.write("src/settings/settings.ts", LEAF);
    fixture.write(
        "src/settings/settingsDefaults.ts",
        "export function defaults(): number { return 2; }\n",
    );
    fixture.write(
        "src/main.ts",
        "import { shared } from \"./settings/settings.ts\";\n         import { defaults } from \"./settings/settingsDefaults.ts\";\n         export function run(): number { return shared() + defaults(); }\n",
    );
    let outcome = fixture.check();
    assert_eq!(outcome.exit_code(), EXIT_VIOLATIONS);
    // Reported against the folder and without a line: a folder is not a
    // place in a file.
    assert_eq!(
        violations(&outcome),
        vec!["src/settings max_doors_per_folder 2/1"]
    );
    assert!(
        report::human(&outcome).contains("(settings.ts, settingsDefaults.ts)"),
        "doors not named: {}",
        report::human(&outcome)
    );
}

/// Both rules read edges rather than files, so `exempt` has to reach the
/// evidence and not only the subject — otherwise a file the project has
/// declared out of scope fails one that is in it, and the named fix is a
/// change nobody is grading.
#[test]
fn an_exempt_file_is_not_evidence_against_one_that_is_checked() {
    let fixture = Fixture::new("exempt-tree");
    fixture.rules(
        r#"{"rules": {"max_importers_per_file": 1, "max_doors_per_folder": 1}, "exempt": ["src/generated/**"]}"#,
    );
    fixture.write("src/settings/settings.ts", LEAF);
    fixture.write(
        "src/settings/settingsDefaults.ts",
        "export function defaults(): number { return 2; }\n",
    );
    fixture.write("src/main.ts", &importer("./settings/settings.ts"));
    fixture.write(
        "src/generated/api.ts",
        "import { shared } from \"../settings/settings.ts\";\n         import { defaults } from \"../settings/settingsDefaults.ts\";\n         export function api(): number { return shared() + defaults(); }\n",
    );
    let outcome = fixture.check();
    assert_eq!(
        outcome.exit_code(),
        EXIT_PASS,
        "an exempt file was counted: {}",
        report::human(&outcome)
    );
}

// ------------------------------------------------------------------- paths

#[test]
fn a_path_reads_the_same_however_the_command_was_pointed_at_it() {
    let cases = [
        (".", "./src/a.ts"),
        ("./", "src/a.ts"),
        ("/repo", "/repo/src/a.ts"),
    ];
    for (root, path) in cases {
        assert_eq!(
            repo_relative(Path::new(root), Path::new(path)),
            "src/a.ts",
            "root {root}, path {path}"
        );
    }
}

#[test]
fn a_path_outside_the_repo_root_keeps_its_own_spelling() {
    assert_eq!(
        repo_relative(Path::new("/repo"), Path::new("/elsewhere/a.ts")),
        "/elsewhere/a.ts"
    );
}

/// Over `KNOWN` rather than a list written here, so a rule added without a
/// phrase fails instead of being reported as a bare number.
#[test]
fn every_known_rule_has_a_name_and_a_phrase() {
    for rule in rules::KNOWN.iter().copied() {
        assert!(rule.name().starts_with("max_"));
        assert!(rule.phrase(9, Some("Thing")).contains('9'));
    }
}
