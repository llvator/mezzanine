//! Parser-level tests for JavaScript (JS-001).
//!
//! Two things are pinned here that the TypeScript tests cannot pin: that a
//! JavaScript file is reported as JavaScript, and that JSX in a `.js` or
//! `.jsx` file gets the TSX grammar. The second is the whole reason grammar
//! selection is not simply "`.tsx` or not" — see `typescript::grammar_for_path`.

use super::super::language_parser::{LanguageParser, ParseResult};
use super::JavaScriptParser;
use crate::models::file_info::Language;
use crate::models::{EntityKind, RelationshipKind};
use std::path::PathBuf;

fn parse_as(file: &str, src: &str) -> ParseResult {
    JavaScriptParser::new()
        .parse(&PathBuf::from(file), src)
        .unwrap()
}

fn entity_names(result: &ParseResult) -> Vec<String> {
    result.entities.iter().map(|e| e.name.clone()).collect()
}

/// Targets of every `Calls` edge.
fn call_targets(result: &ParseResult) -> Vec<String> {
    result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .map(|r| r.target_id.clone())
        .collect()
}

#[test]
fn a_js_file_is_reported_as_javascript() {
    assert_eq!(JavaScriptParser::new().language(), Language::JavaScript);
}

#[test]
fn the_parser_claims_every_javascript_extension() {
    let parser = JavaScriptParser::new();
    for file in ["a.js", "a.mjs", "a.cjs", "a.jsx"] {
        assert!(parser.can_parse(&PathBuf::from(file)), "{file}");
    }
    assert!(!parser.can_parse(&PathBuf::from("a.ts")));
}

#[test]
fn class_members_and_their_calls_come_back_from_a_plain_js_file() {
    let res = parse_as(
        "svc.js",
        r#"
export class UserService {
  #cache = new Map();
  constructor(repo) { this.repo = repo; }
  async find(id) {
    const u = await this.repo.get(id);
    this.#cache.set(id, u);
    return u;
  }
  get size() { return this.#cache.size; }
}
"#,
    );

    let names = entity_names(&res);
    for expected in ["UserService", "find", "size"] {
        assert!(names.contains(&expected.to_string()), "{names:?}");
    }
    assert!(res
        .entities
        .iter()
        .any(|e| e.name == "UserService" && e.kind == EntityKind::Class));
    assert!(
        call_targets(&res).iter().any(|t| t.contains("set")),
        "{:?}",
        call_targets(&res)
    );
}

/// Parameters are carried on the declaring entity here — the standalone
/// `Parameter` entities are an analyzer post-pass, downstream of this.
#[test]
fn untyped_parameters_and_defaults_are_still_parameters() {
    let res = parse_as("h.js", "function helper(a, b = 2, ...rest) { return a; }");
    let helper = res
        .entities
        .iter()
        .find(|e| e.name == "helper")
        .expect("no helper entity");
    let names: Vec<_> = helper.parameters.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b", "...rest"]);
    assert!(
        helper.parameters.iter().all(|p| p.type_name.is_none()),
        "{:?}",
        helper.parameters
    );
}

/// The regression this grammar routing exists for. Under the TypeScript
/// grammar these three calls are inside an unparseable region and every
/// `Calls` edge is lost; under TSX they survive.
#[test]
fn calls_inside_jsx_survive_in_a_js_file() {
    let src = r#"
import { fmt } from './fmt';
export function Card({ n }) {
  return <div title={fmt(n)} onClick={() => save(n)}><Row v={compute(n)} /></div>;
}
export function save(n) { return n; }
export function compute(n) { return n * 2; }
"#;
    for file in ["Card.js", "Card.jsx"] {
        let targets = call_targets(&parse_as(file, src));
        for callee in ["fmt", "save", "compute"] {
            assert!(
                targets.iter().any(|t| t.ends_with(callee)),
                "{file}: {targets:?}"
            );
        }
    }
}

/// CommonJS states no `import`, so the module edge has to be recovered from
/// the use of the name. That is what carries a `require`-only repo today —
/// the import ledger itself does not (see this module's header).
#[test]
fn a_required_class_is_still_instantiated_by_name() {
    let res = parse_as(
        "svc.cjs",
        "const { Repo } = require('./repo');\nfunction make() { return new Repo(); }\n",
    );
    assert!(
        res.relationships
            .iter()
            .any(|r| r.kind == RelationshipKind::Instantiates && r.target_id.ends_with("Repo")),
        "{:?}",
        res.relationships
    );
}
